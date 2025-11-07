use std::{env, sync::Arc};

use aws_config::BehaviorVersion;
use aws_sdk_sns::{Client, types::MessageAttributeValue};
use dotenvy::dotenv;
use phonenumber::parse;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

#[derive(Error, Debug)]
enum ServiceError {
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

#[derive(Serialize, Deserialize, Debug)]
struct ZMQSendSmsMessage {
    sender_id: String,
    phone_number: String,
    message: String,
}

impl ZMQSendSmsMessage {
    fn validate(&self) -> Result<(), ServiceError> {
        if self.sender_id.is_empty() {
            return Err(ServiceError::InvalidMessage(
                "sender_id is empty".to_string(),
            ));
        }
        let phone = parse(None, &self.phone_number)?;
        if !phone.is_valid() {
            return Err(ServiceError::InvalidPhoneNumber(
                phonenumber::ParseError::NoNumber,
            ));
        }
        if self.message.is_empty() {
            return Err(ServiceError::InvalidMessage("message is empty".to_string()));
        }
        Ok(())
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
    log::info!(
        "Published SMS to {}...{}",
        &msg.phone_number[..4],
        &msg.phone_number[msg.phone_number.len() - 2..]
    );
    Ok(())
}

async fn run_service(mut shutdown: broadcast::Receiver<()>) -> Result<(), ServiceError> {
    let zmq_address = env::var("ZMQ_SMS_SUB").unwrap_or("tcp://127.0.0.1:5562".to_string());

    let context = zmq::Context::new();
    let responder = context.socket(zmq::SUB)?;
    responder.connect(&zmq_address)?;
    responder.set_subscribe(b"")?;

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Arc::new(Client::new(&config));

    log::info!("Starting sms sending worker at {zmq_address}");

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
                log::info!(
                    "Received SMS request for {}...{}",
                    &msg.phone_number[..std::cmp::min(4, msg.phone_number.len())],
                    if msg.phone_number.len() > 2 {
                        &msg.phone_number[msg.phone_number.len() - 2..]
                    } else {
                        ""
                    }
                );

                let client = client.clone();
                tokio::spawn(async move {
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

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            log::info!("Received SIGINT, shutting down");
        }
        _ = async {
            let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to setup SIGTERM handler")
                .recv()
                .await;
        } => {
            log::info!("Received SIGTERM, shutting down");
        }
    }

    let _ = shutdown_tx.send(());
    let _ = service_handle.await;
    log::info!("Service stopped");
}
