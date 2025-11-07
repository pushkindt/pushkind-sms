# Pushkind SMS Service

A lightweight SMS gateway service that receives messages via ZeroMQ and sends them through AWS SNS.

## Architecture

The service acts as a ZeroMQ subscriber that listens for SMS requests and forwards them to AWS SNS for delivery. Each message is processed asynchronously using Tokio.

## Requirements

- Rust 2024 edition
- ZeroMQ
- AWS credentials with SNS permissions

## Configuration

Create a `.env` file with the following variables:

```env
AWS_ACCESS_KEY_ID=your_access_key
AWS_SECRET_ACCESS_KEY=your_secret_key
AWS_REGION=your_region
AWS_ENDPOINT_URL=https://notifications.yandexcloud.net/
ZMQ_SMS_SUB=tcp://127.0.0.1:5562
MAX_CONCURRENT_SMS=100
```

## Message Format

Send JSON messages to the ZeroMQ endpoint:

```json
{
  "sender_id": "SENDER",
  "phone_number": "+1234567890",
  "message": "Your SMS text"
}
```

## Build & Run

```bash
cargo build --release
cargo run
```

## Testing

Install Python dependencies:
```bash
pip install pyzmq
```

Run the test script:
```bash
python test_sms.py
```

## Logging

Set log level via `RUST_LOG` environment variable (default: `info`).
