use serde_json;
use zmq;

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
fn parse_max_concurrent_from_env() {
    unsafe {
        std::env::set_var("MAX_CONCURRENT_SMS", "50");
    }
    let max_concurrent = std::env::var("MAX_CONCURRENT_SMS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    assert_eq!(max_concurrent, 50);
    unsafe {
        std::env::remove_var("MAX_CONCURRENT_SMS");
    }
}

#[test]
fn parse_max_concurrent_default() {
    unsafe {
        std::env::remove_var("MAX_CONCURRENT_SMS");
    }
    let max_concurrent = std::env::var("MAX_CONCURRENT_SMS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    assert_eq!(max_concurrent, 100);
}
