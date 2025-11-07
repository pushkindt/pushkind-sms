use aws_config::BehaviorVersion;
use aws_sdk_sns::{Client, types::MessageAttributeValue};
use dotenvy::dotenv;

#[tokio::main]
async fn main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    dotenv().ok(); // Load .env file

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;

    let client = Client::new(&config);

    let sender_id = MessageAttributeValue::builder()
        .data_type("String")
        .string_value("cns.shared")
        .build()
        .unwrap();

    let result = client
        .publish()
        .message_attributes("SenderID", sender_id)
        .phone_number("+7xxxxxxxxxx")
        .message("12345")
        .send()
        .await;
    log::info!("{result:?}");
}
