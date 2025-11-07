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
