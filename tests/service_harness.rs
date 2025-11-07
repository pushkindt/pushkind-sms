use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use pushkind_sms::{
    MessageSource, ServiceConfig, ServiceError, SmsPublisher, ZMQSendSmsMessage, run_service,
};
use tokio::sync::{Mutex, broadcast};

struct StubSource {
    messages: Vec<Vec<u8>>,
    index: usize,
}

impl StubSource {
    fn new(messages: Vec<Vec<u8>>) -> Self {
        Self { messages, index: 0 }
    }
}

impl MessageSource for StubSource {
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
            if self.attempts().await >= expected {
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

    let source = StubSource::new(messages);
    let publisher = Arc::new(RecordingPublisher::new(Duration::from_millis(20)));
    let config = ServiceConfig {
        zmq_endpoint: "inproc://test".into(),
        max_concurrent_sms: 2,
    };

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let task = tokio::spawn(run_service(
        source,
        Arc::clone(&publisher),
        config,
        shutdown_rx,
    ));

    publisher.wait_for_messages(3).await;
    let _ = shutdown_tx.send(());
    task.await.unwrap().unwrap();

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

    let source = StubSource::new(messages);
    let publisher = Arc::new(FlakyPublisher::new(1));
    let config = ServiceConfig {
        zmq_endpoint: "inproc://test".into(),
        max_concurrent_sms: 1,
    };

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let task = tokio::spawn(run_service(
        source,
        Arc::clone(&publisher),
        config,
        shutdown_rx,
    ));

    publisher.wait_for_attempts(2).await;
    let _ = shutdown_tx.send(());
    task.await.unwrap().unwrap();

    assert_eq!(publisher.attempts().await, 2);
    assert_eq!(publisher.successes().await, 1);
}

#[tokio::test]
async fn run_service_stops_on_shutdown_signal() {
    let source = StubSource::new(vec![]);
    let publisher = Arc::new(RecordingPublisher::new(Duration::from_millis(5)));
    let config = ServiceConfig {
        zmq_endpoint: "inproc://test".into(),
        max_concurrent_sms: 1,
    };

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let task = tokio::spawn(run_service(
        source,
        Arc::clone(&publisher),
        config,
        shutdown_rx,
    ));

    tokio::time::sleep(Duration::from_millis(30)).await;
    let _ = shutdown_tx.send(());
    task.await.unwrap().unwrap();

    assert!(publisher.messages().await.is_empty());
}
