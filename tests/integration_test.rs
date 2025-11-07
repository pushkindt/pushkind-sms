use std::sync::{Mutex, OnceLock};

use pushkind_sms::ServiceConfig;
use serde_json;
use zmq;

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn set_env_var(key: &str, value: &str) {
    // SAFETY: guarded by `env_guard` to avoid concurrent environment access.
    unsafe {
        std::env::set_var(key, value);
    }
}

fn remove_env_var(key: &str) {
    // SAFETY: guarded by `env_guard` to avoid concurrent environment access.
    unsafe {
        std::env::remove_var(key);
    }
}

#[test]
fn zmq_message_serialization() {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct TestMessage {
        sender_id: String,
        phone_number: String,
        message: String,
    }

    let msg = TestMessage {
        sender_id: "TEST".into(),
        phone_number: "+1234567890".into(),
        message: "Integration test".into(),
    };

    let serialized = serde_json::to_vec(&msg).unwrap();
    let deserialized: TestMessage = serde_json::from_slice(&serialized).unwrap();

    assert_eq!(msg, deserialized);
}

#[test]
fn zmq_context_creation() {
    let context = zmq::Context::new();
    let socket = context.socket(zmq::PUB);
    assert!(socket.is_ok());
}

#[test]
fn service_config_reads_env_overrides() {
    let _guard = env_guard();
    set_env_var("ZMQ_SMS_SUB", "tcp://example:9999");
    set_env_var("MAX_CONCURRENT_SMS", "42");

    let config = ServiceConfig::from_env();

    assert_eq!(config.zmq_endpoint, "tcp://example:9999");
    assert_eq!(config.max_concurrent_sms, 42);

    remove_env_var("ZMQ_SMS_SUB");
    remove_env_var("MAX_CONCURRENT_SMS");
}

#[test]
fn service_config_uses_defaults_when_missing() {
    let _guard = env_guard();
    remove_env_var("ZMQ_SMS_SUB");
    remove_env_var("MAX_CONCURRENT_SMS");

    let config = ServiceConfig::from_env();

    assert_eq!(config.zmq_endpoint, "tcp://127.0.0.1:5562");
    assert_eq!(config.max_concurrent_sms, 100);
}
