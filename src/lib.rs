//! Library that bridges ZeroMQ SMS ingestion with AWS SNS publishing.
//!
//! The service consumes JSON-encoded SMS payloads from a ZeroMQ `SUB` socket,
//! validates each request, and forwards it to Amazon SNS for delivery. It runs
//! on Tokio with asynchronous concurrency limits, relies on ZeroMQ and AWS
//! credentials provided through environment variables, and gracefully shuts
//! down in response to `SIGINT`/`SIGTERM`.

use std::{env, future::Future, pin::Pin, sync::Arc, time::Duration};

use aws_config::BehaviorVersion;
use aws_sdk_sns::{Client, types::MessageAttributeValue};
use dotenvy::dotenv;
use phonenumber::parse;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Semaphore, broadcast};

/// Environment variable defining the ZeroMQ endpoint used for SMS intake.
/// Defaults to `tcp://127.0.0.1:5562` when unset, matching the local dev
/// configuration in docker-compose.
const ENV_ZMQ_SMS_SUB: &str = "ZMQ_SMS_SUB";

/// Environment variable limiting how many SMS publish tasks can run at once.
/// Accepts a positive integer and defaults to `100` to provide ample
/// concurrency without overwhelming the downstream SNS service.
const ENV_MAX_CONCURRENT_SMS: &str = "MAX_CONCURRENT_SMS";

#[derive(Error, Debug)]
/// Errors that can occur while accepting, validating, or publishing SMS jobs.
///
/// Each variant wraps the underlying library error when available so callers
/// receive structured context suitable for logging or retry decisions.
pub enum ServiceError {
    #[error("ZeroMQ communication error")]
    Zmq(#[from] zmq::Error),
    #[error("SNS Publish error")]
    Publish(
        #[from] Box<aws_sdk_sns::error::SdkError<aws_sdk_sns::operation::publish::PublishError>>,
    ),
    #[error("Build error")]
    Build(#[from] Box<aws_sdk_sns::error::BuildError>),
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),
    #[error("Invalid phone number")]
    InvalidPhoneNumber(#[from] phonenumber::ParseError),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
/// JSON payload representing a request to send a single SMS message.
///
/// The payload is provided by the ZeroMQ producer and must contain a
/// non-empty sender ID, a valid E.164-formatted phone number, and a message
/// body. Validation ensures these requirements before any publish occurs.
pub struct ZMQSendSmsMessage {
    pub sender_id: String,
    pub phone_number: String,
    pub message: String,
}

impl ZMQSendSmsMessage {
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.sender_id.is_empty() {
            return Err(ServiceError::InvalidMessage("sender_id is empty".into()));
        }
        let phone = parse(None, &self.phone_number)?;
        if !phone.is_valid() {
            return Err(ServiceError::InvalidPhoneNumber(
                phonenumber::ParseError::NoNumber,
            ));
        }
        if self.message.is_empty() {
            return Err(ServiceError::InvalidMessage("message is empty".into()));
        }
        Ok(())
    }

    pub fn mask_phone(&self) -> String {
        let mut s = self.phone_number.chars();

        let keep_start = 4;

        let mut out = String::new();
        out.extend(s.by_ref().take(keep_start));
        out.extend(std::iter::repeat_n('*', 6));
        out.extend(s.skip(6));
        out
    }
}

pub trait PublishClient: Send + Sync + 'static {
    fn publish(
        &self,
        msg: ZMQSendSmsMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>>;
}

impl PublishClient for Client {
    fn publish(
        &self,
        msg: ZMQSendSmsMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>> {
        let client = self.clone();
        Box::pin(async move {
            let ZMQSendSmsMessage {
                sender_id,
                phone_number,
                message,
            } = msg;
            let sender_attr = MessageAttributeValue::builder()
                .data_type("String")
                .string_value(sender_id)
                .build()
                .map_err(|e| ServiceError::Build(Box::new(e)))?;

            let mut request = client.publish().phone_number(phone_number).message(message);
            request = request.message_attributes("SenderID", sender_attr);

            request
                .send()
                .await
                .map_err(|e| ServiceError::Publish(Box::new(e)))?;
            Ok(())
        })
    }
}

/// Publishes a validated SMS payload to Amazon SNS.
///
/// * `msg` - The SMS request received from ZeroMQ. It is validated prior to
///   publishing; validation errors are surfaced to the caller as
///   [`ServiceError`].
/// * `client` - The SNS client used to send the message. Any publish or
///   request-building issues are propagated via the corresponding
///   [`ServiceError`] variants.
///
/// Returns `Ok(())` after the SNS publish completes successfully.
async fn send_sms<C: PublishClient + ?Sized>(
    msg: ZMQSendSmsMessage,
    client: &C,
) -> Result<(), ServiceError> {
    msg.validate()?;

    let masked_phone = msg.mask_phone();
    client.publish(msg).await?;

    log::info!("Published SMS to {}", masked_phone);
    Ok(())
}

#[derive(Clone, Debug)]
/// Configuration values that govern runtime connectivity and concurrency.
pub struct ServiceConfig {
    pub zmq_endpoint: String,
    pub max_concurrent_sms: usize,
}

impl ServiceConfig {
    /// Loads configuration using environment variables documented in
    /// [`ENV_ZMQ_SMS_SUB`] and [`ENV_MAX_CONCURRENT_SMS`], falling back to
    /// sensible local defaults when keys are missing or malformed.
    pub fn from_env() -> Self {
        let zmq_endpoint =
            env::var(ENV_ZMQ_SMS_SUB).unwrap_or_else(|_| "tcp://127.0.0.1:5562".into());
        let max_concurrent_sms = env::var(ENV_MAX_CONCURRENT_SMS)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        Self {
            zmq_endpoint,
            max_concurrent_sms,
        }
    }
}

pub trait SmsPublisher: Send + Sync + 'static {
    fn publish(
        &self,
        msg: ZMQSendSmsMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>>;
}

pub trait MessageSource: Send {
    fn try_next(&mut self) -> Result<Option<Vec<u8>>, ServiceError>;
}

pub struct SnsPublisher {
    client: Client,
}

impl SnsPublisher {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl SmsPublisher for SnsPublisher {
    fn publish(
        &self,
        msg: ZMQSendSmsMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>> {
        let client = self.client.clone();
        Box::pin(async move { send_sms(msg, &client).await })
    }
}

pub struct ZmqMessageSource {
    socket: zmq::Socket,
}

impl ZmqMessageSource {
    pub fn connect(context: &zmq::Context, endpoint: &str) -> Result<Self, ServiceError> {
        let socket = context.socket(zmq::SUB)?;
        socket.connect(endpoint)?;
        socket.set_subscribe(b"")?;
        Ok(Self { socket })
    }
}

impl MessageSource for ZmqMessageSource {
    fn try_next(&mut self) -> Result<Option<Vec<u8>>, ServiceError> {
        match self.socket.recv_bytes(zmq::DONTWAIT) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(zmq::Error::EAGAIN) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Main event loop that converts ZeroMQ messages into SNS publish tasks.
///
/// * `source` - Non-blocking message source used to poll ZeroMQ. Any transport
///   errors are surfaced through [`ServiceError::Zmq`].
/// * `publisher` - SMS publisher implementation (typically SNS) used to send
///   validated requests. Publish and validation failures are logged and
///   propagated to the task.
/// * `config` - Runtime configuration controlling the ZeroMQ endpoint and
///   concurrency limits.
/// * `shutdown` - Broadcast channel that signals a graceful stop when a value
///   arrives.
///
/// Returns `Ok(())` when the shutdown signal is observed and the loop exits
/// cleanly. Upstream errors are propagated so callers can perform additional
/// recovery or restart logic.
pub async fn run_service<S, P>(
    mut source: S,
    publisher: Arc<P>,
    config: ServiceConfig,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), ServiceError>
where
    S: MessageSource,
    P: SmsPublisher,
{
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_sms));

    log::info!(
        "Starting sms sending worker at {} (max concurrent: {})",
        config.zmq_endpoint,
        config.max_concurrent_sms
    );

    loop {
        if shutdown.try_recv().is_ok() {
            log::info!("Shutdown signal received, stopping service");
            break;
        }

        let msg = match source.try_next()? {
            Some(msg) => msg,
            None => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };

        match serde_json::from_slice::<ZMQSendSmsMessage>(&msg) {
            Ok(msg) => {
                log::info!("Received SMS request for {}", msg.mask_phone());

                let publisher = Arc::clone(&publisher);
                let semaphore = semaphore.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = semaphore.acquire().await else {
                        log::error!("Semaphore closed, cannot send SMS");
                        return;
                    };
                    if let Err(e) = publisher.publish(msg).await {
                        log::error!("Error sending sms message: {e}");
                    }
                });
            }
            Err(e) => {
                log::error!("Error deserializing message: {e}");
            }
        }
    }
    Ok(())
}

pub async fn run() {
    dotenv().ok();
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let config = ServiceConfig::from_env();
    let aws_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let publisher = Arc::new(SnsPublisher::new(Client::new(&aws_config)));

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let service_handle = {
        let publisher = Arc::clone(&publisher);
        let config = config.clone();
        tokio::spawn(async move {
            let context = zmq::Context::new();
            let source = match ZmqMessageSource::connect(&context, &config.zmq_endpoint) {
                Ok(source) => source,
                Err(e) => {
                    log::error!("Failed to connect to ZeroMQ source: {e}");
                    return;
                }
            };

            if let Err(e) = run_service(source, publisher, config, shutdown_rx).await {
                log::error!("Error running service: {e}");
            }
        })
    };

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to setup SIGTERM handler");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            log::info!("Received SIGTERM, shutting down");
        }
    }

    let _ = shutdown_tx.send(());
    let _ = service_handle.await;
    log::info!("Service stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct PublishCall {
        message: ZMQSendSmsMessage,
    }

    #[derive(Clone, Default)]
    struct MockPublishClient {
        calls: Arc<Mutex<Vec<PublishCall>>>,
        error: Arc<Mutex<Option<ServiceError>>>,
    }

    impl MockPublishClient {
        fn new() -> Self {
            Self::default()
        }

        async fn set_error(&self, error: ServiceError) {
            *self.error.lock().await = Some(error);
        }

        async fn calls(&self) -> Vec<PublishCall> {
            self.calls.lock().await.clone()
        }
    }

    impl PublishClient for MockPublishClient {
        fn publish(
            &self,
            msg: ZMQSendSmsMessage,
        ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>> {
            let calls = Arc::clone(&self.calls);
            let error = Arc::clone(&self.error);
            Box::pin(async move {
                calls.lock().await.push(PublishCall { message: msg });
                if let Some(err) = error.lock().await.take() {
                    return Err(err);
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn send_sms_invokes_publish_with_message() {
        let client = MockPublishClient::new();
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "Hello".into(),
        };

        send_sms(msg.clone(), &client).await.unwrap();

        let calls = client.calls().await;
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.message, msg);
    }

    #[tokio::test]
    async fn send_sms_propagates_publisher_errors() {
        let client = MockPublishClient::new();
        client
            .set_error(ServiceError::InvalidMessage("publisher error".into()))
            .await;
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "Hello".into(),
        };

        let err = send_sms(msg.clone(), &client)
            .await
            .expect_err("expected error");
        assert!(matches!(err, ServiceError::InvalidMessage(ref m) if m == "publisher error"));

        let calls = client.calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn valid_message_passes_validation() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "Test message".into(),
        };
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn empty_sender_id_fails_validation() {
        let msg = ZMQSendSmsMessage {
            sender_id: "".into(),
            phone_number: "+1234567890".into(),
            message: "Test".into(),
        };
        assert!(matches!(
            msg.validate(),
            Err(ServiceError::InvalidMessage(_))
        ));
    }

    #[test]
    fn empty_message_fails_validation() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "".into(),
        };
        assert!(matches!(
            msg.validate(),
            Err(ServiceError::InvalidMessage(_))
        ));
    }

    #[test]
    fn invalid_phone_number_fails_validation() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "invalid".into(),
            message: "Test".into(),
        };
        assert!(matches!(
            msg.validate(),
            Err(ServiceError::InvalidPhoneNumber(_))
        ));
    }

    #[test]
    fn phone_masking_works() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "Test".into(),
        };
        assert_eq!(msg.mask_phone(), "+123******01");
    }

    #[test]
    fn phone_masking_short_number() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+123".into(),
            message: "Test".into(),
        };
        assert_eq!(msg.mask_phone(), "+123******");
    }

    #[test]
    fn deserialize_valid_json() {
        let json = r#"{"sender_id":"TEST","phone_number":"+12345678901","message":"Hello"}"#;
        let msg: Result<ZMQSendSmsMessage, _> = serde_json::from_str(json);
        assert!(msg.is_ok());
        let msg = msg.unwrap();
        assert_eq!(msg.sender_id, "TEST");
        assert_eq!(msg.phone_number, "+12345678901");
        assert_eq!(msg.message, "Hello");
    }

    #[test]
    fn deserialize_invalid_json_fails() {
        let json = r#"{"sender_id":"TEST"}"#;
        let msg: Result<ZMQSendSmsMessage, _> = serde_json::from_str(json);
        assert!(msg.is_err());
    }

    #[test]
    fn serialize_message() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+12345678901".into(),
            message: "Hello".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("TEST"));
        assert!(json.contains("+12345678901"));
        assert!(json.contains("Hello"));
    }

    struct FakeSource {
        messages: Vec<Vec<u8>>,
        index: usize,
    }

    impl FakeSource {
        fn new(messages: Vec<Vec<u8>>) -> Self {
            Self { messages, index: 0 }
        }
    }

    impl MessageSource for FakeSource {
        fn try_next(&mut self) -> Result<Option<Vec<u8>>, ServiceError> {
            if let Some(msg) = self.messages.get(self.index).cloned() {
                self.index += 1;
                Ok(Some(msg))
            } else {
                Ok(None)
            }
        }
    }

    #[derive(Default)]
    struct RecordingState {
        active: usize,
        max_active: usize,
        messages: Vec<ZMQSendSmsMessage>,
    }

    struct RecordingPublisher {
        state: Arc<Mutex<RecordingState>>,
        delay: Duration,
    }

    impl RecordingPublisher {
        fn new(delay: Duration) -> Self {
            Self {
                state: Arc::new(Mutex::new(RecordingState::default())),
                delay,
            }
        }

        async fn wait_for_messages(&self, expected: usize) {
            for _ in 0..100 {
                if self.messages().await.len() >= expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("Timed out waiting for messages");
        }

        async fn messages(&self) -> Vec<ZMQSendSmsMessage> {
            self.state.lock().await.messages.clone()
        }

        async fn max_active(&self) -> usize {
            self.state.lock().await.max_active
        }
    }

    impl SmsPublisher for RecordingPublisher {
        fn publish(
            &self,
            msg: ZMQSendSmsMessage,
        ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>> {
            let state = Arc::clone(&self.state);
            let delay = self.delay;
            Box::pin(async move {
                {
                    let mut guard = state.lock().await;
                    guard.active += 1;
                    guard.max_active = guard.max_active.max(guard.active);
                }

                tokio::time::sleep(delay).await;

                let mut guard = state.lock().await;
                guard.active = guard.active.saturating_sub(1);
                guard.messages.push(msg);
                Ok(())
            })
        }
    }

    struct FlakyState {
        failures_remaining: usize,
        attempts: usize,
        successes: usize,
    }

    struct FlakyPublisher {
        state: Arc<Mutex<FlakyState>>,
    }

    impl FlakyPublisher {
        fn new(failures: usize) -> Self {
            Self {
                state: Arc::new(Mutex::new(FlakyState {
                    failures_remaining: failures,
                    attempts: 0,
                    successes: 0,
                })),
            }
        }

        async fn wait_for_attempts(&self, expected: usize) {
            for _ in 0..100 {
                if self.state.lock().await.attempts >= expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("Timed out waiting for attempts");
        }

        async fn attempts(&self) -> usize {
            self.state.lock().await.attempts
        }

        async fn successes(&self) -> usize {
            self.state.lock().await.successes
        }
    }

    impl SmsPublisher for FlakyPublisher {
        fn publish(
            &self,
            _msg: ZMQSendSmsMessage,
        ) -> Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + '_>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut guard = state.lock().await;
                guard.attempts += 1;
                if guard.failures_remaining > 0 {
                    guard.failures_remaining -= 1;
                    return Err(ServiceError::InvalidMessage("forced failure".into()));
                }
                guard.successes += 1;
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn run_service_processes_messages_with_concurrency_limit() {
        let messages = vec![
            serde_json::to_vec(&ZMQSendSmsMessage {
                sender_id: "TEST1".into(),
                phone_number: "+12345678901".into(),
                message: "Hello".into(),
            })
            .unwrap(),
            serde_json::to_vec(&ZMQSendSmsMessage {
                sender_id: "TEST2".into(),
                phone_number: "+12345678902".into(),
                message: "World".into(),
            })
            .unwrap(),
            serde_json::to_vec(&ZMQSendSmsMessage {
                sender_id: "TEST3".into(),
                phone_number: "+12345678903".into(),
                message: "Again".into(),
            })
            .unwrap(),
        ];

        let source = FakeSource::new(messages);
        let publisher = Arc::new(RecordingPublisher::new(Duration::from_millis(20)));
        let config = ServiceConfig {
            zmq_endpoint: "inproc://test".into(),
            max_concurrent_sms: 2,
        };

        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let service = tokio::spawn(run_service(
            source,
            Arc::clone(&publisher),
            config,
            shutdown_rx,
        ));

        publisher.wait_for_messages(3).await;
        let _ = shutdown_tx.send(());

        service.await.unwrap().unwrap();

        assert_eq!(publisher.messages().await.len(), 3);
        assert!(publisher.max_active().await <= 2);
    }

    #[tokio::test]
    async fn run_service_handles_publisher_errors() {
        let messages = vec![
            serde_json::to_vec(&ZMQSendSmsMessage {
                sender_id: "TEST1".into(),
                phone_number: "+12345678901".into(),
                message: "Hello".into(),
            })
            .unwrap(),
            serde_json::to_vec(&ZMQSendSmsMessage {
                sender_id: "TEST2".into(),
                phone_number: "+12345678902".into(),
                message: "World".into(),
            })
            .unwrap(),
        ];

        let source = FakeSource::new(messages);
        let publisher = Arc::new(FlakyPublisher::new(1));
        let config = ServiceConfig {
            zmq_endpoint: "inproc://test".into(),
            max_concurrent_sms: 1,
        };

        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let service = tokio::spawn(run_service(
            source,
            Arc::clone(&publisher),
            config,
            shutdown_rx,
        ));

        publisher.wait_for_attempts(2).await;
        let _ = shutdown_tx.send(());

        service.await.unwrap().unwrap();

        assert_eq!(publisher.attempts().await, 2);
        assert_eq!(publisher.successes().await, 1);
    }

    #[tokio::test]
    async fn run_service_stops_when_shutdown_is_triggered() {
        let source = FakeSource::new(vec![]);
        let publisher = Arc::new(RecordingPublisher::new(Duration::from_millis(5)));
        let config = ServiceConfig {
            zmq_endpoint: "inproc://test".into(),
            max_concurrent_sms: 1,
        };

        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let service = tokio::spawn(run_service(
            source,
            Arc::clone(&publisher),
            config,
            shutdown_rx,
        ));

        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = shutdown_tx.send(());

        service.await.unwrap().unwrap();

        assert!(publisher.messages().await.is_empty());
    }
}
