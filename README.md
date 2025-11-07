# Pushkind SMS Service

Pushkind SMS bridges ZeroMQ-based ingestion with AWS SNS so upstream systems can fire-and-forget SMS payloads while the service validates, masks, and publishes them in a controlled, asynchronous pipeline.

## Overview

- **Intent**: provide a small, deployable Rust binary that listens on a ZeroMQ `SUB` socket, validates phone/message input, applies concurrency limits, and publishes the SMS through AWS SNS.
- **Code layout**: `src/lib.rs` hosts the full runtime (configuration, ZeroMQ source, SNS publisher, orchestration); `src/main.rs` is a thin Tokio entry point calling `pushkind_sms::run()`.
- **Tech**: Rust 2024, Tokio async runtime, ZeroMQ for ingress, AWS SDK for SNS, `dotenvy` + `env_logger` for configuration & logging.

## Architecture

1. **ZeroMQ subscriber** (`ZmqMessageSource`) listens on `ZMQ_SMS_SUB` and yields JSON payloads without blocking the runtime.
2. Payloads are deserialized into `ZMQSendSmsMessage`, validated (sender, E.164 phone, message body), and masked before logging.
3. `SnsPublisher` converts the request into SNS attributes (`SenderID`, etc.) and publishes using the AWS SDK client.
4. A Tokio semaphore (`MAX_CONCURRENT_SMS`) bounds concurrent publishes while the service awaits a shutdown broadcast triggered by SIGINT/SIGTERM.

## Requirements

- Rust toolchain with edition 2024 support (`rustup toolchain install stable`)
- Native ZeroMQ library headers (e.g., `libzmq-dev` on Debian/Ubuntu, `zeromq` via Homebrew)
- AWS credentials with permission to publish to SNS (env vars, profile, or IAM role)
- Optional: Python 3 + `pyzmq` for the provided `test_sms.py` publisher

## Configuration

Create a `.env` alongside the binary:

```env
AWS_ACCESS_KEY_ID=your_access_key
AWS_SECRET_ACCESS_KEY=your_secret_key
AWS_REGION=us-east-1
# Optional override for alternative clouds/localstack:
AWS_ENDPOINT_URL=https://notifications.yandexcloud.net/
ZMQ_SMS_SUB=tcp://127.0.0.1:5562
MAX_CONCURRENT_SMS=100
```

Additional knobs:

- `RUST_LOG=debug` to surface verbose diagnostics.
- `SENDER_ID` can be specified in upstream payloads; keep values <= 11 chars per carrier rules.

## Running the Service

```bash
cargo run --bin pushkind-sms
```

The binary loads `.env`, initializes logging, connects to ZeroMQ, and runs until interrupted (`Ctrl+C` or SIGTERM). For deployment use the optimized build:

```bash
cargo build --release
./target/release/pushkind-sms
```

## Development Workflow

```bash
cargo check                # fast type-check
cargo fmt && cargo clippy --all-targets --all-features
cargo test --all           # unit + integration tests
```

## Message Format

Send JSON payloads to the ZeroMQ publisher socket that feeds this service:

```json
{
  "sender_id": "SENDER",
  "phone_number": "+1234567890",
  "message": "Your SMS text"
}
```

Invalid payloads are rejected with descriptive logs; valid requests log a masked phone number (first 4 digits + 6 asterisks).

## Testing

Rust tests:

```bash
cargo test --all
```

Python smoke publisher (optional):

```bash
pip install pyzmq
python test_sms.py  # sends a sample payload to ZMQ
```

## Observability

- Logging defaults to `info` via `env_logger`; set `RUST_LOG=pushkind_sms=debug` for per-module output.
- Phone numbers are masked before logging to avoid leaking PII.
- Health can be inferred from log lines: startup banner, ZeroMQ connection errors, publish success/failure, and shutdown notices.
