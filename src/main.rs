use std::{env, sync::Arc};

use aws_config::BehaviorVersion;
use aws_sdk_sns::{Client, types::MessageAttributeValue};
use dotenvy::dotenv;
use phonenumber::parse;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Semaphore, broadcast};

#[derive(Error, Debug)]
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
        self.phone_number
            .chars()
            .take(4)
            .chain(
                self.phone_number
                    .chars()
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev(),
            )
            .collect()
    }
}

async fn send_sms(msg: ZMQSendSmsMessage, client: &Client) -> Result<(), ServiceError> {
    msg.validate()?;

    let sender_id = MessageAttributeValue::builder()
        .data_type("String")
        .string_value(&msg.sender_id)
        .build()
        .map_err(|e| ServiceError::Build(Box::new(e)))?;
    let _result = client
        .publish()
        .message_attributes("SenderID", sender_id)
        .phone_number(&msg.phone_number)
        .message(&msg.message)
        .send()
        .await
        .map_err(|e| ServiceError::Publish(Box::new(e)))?;
    
    log::info!("Published SMS to {}", msg.mask_phone());
    Ok(())
}

async fn run_service(mut shutdown: broadcast::Receiver<()>) -> Result<(), ServiceError> {
    let zmq_address = env::var("ZMQ_SMS_SUB").unwrap_or_else(|_| "tcp://127.0.0.1:5562".into());
    let max_concurrent = env::var("MAX_CONCURRENT_SMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);

    let context = zmq::Context::new();
    let responder = context.socket(zmq::SUB)?;
    responder.connect(&zmq_address)?;
    responder.set_subscribe(b"")?;

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Arc::new(Client::new(&config));
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    log::info!("Starting sms sending worker at {zmq_address} (max concurrent: {max_concurrent})");

    loop {
        if shutdown.try_recv().is_ok() {
            log::info!("Shutdown signal received, stopping service");
            break;
        }

        let msg = match responder.recv_bytes(zmq::DONTWAIT) {
            Ok(msg) => msg,
            Err(zmq::Error::EAGAIN) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        match serde_json::from_slice::<ZMQSendSmsMessage>(&msg) {
            Ok(msg) => {
                log::info!("Received SMS request for {}", msg.mask_phone());

                let client = client.clone();
                let semaphore = semaphore.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = semaphore.acquire().await else {
                        log::error!("Semaphore closed, cannot send SMS");
                        return;
                    };
                    if let Err(e) = send_sms(msg, &client).await {
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

#[tokio::main]
async fn main() {
    dotenv().ok();
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let service_handle = tokio::spawn(async move {
        if let Err(e) = run_service(shutdown_rx).await {
            log::error!("Error running service: {e}");
        }
    });

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
        assert_eq!(msg.mask_phone(), "+12301");
    }

    #[test]
    fn phone_masking_short_number() {
        let msg = ZMQSendSmsMessage {
            sender_id: "TEST".into(),
            phone_number: "+123".into(),
            message: "Test".into(),
        };
        assert_eq!(msg.mask_phone(), "+12323");
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
}
