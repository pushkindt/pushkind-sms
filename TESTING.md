# Testing Documentation

The `pushkind-sms` crate relies on a mix of library unit tests and async integration tests. Run `cargo test --all` in the repo root to execute everything.

## Current Test Suites

### Library unit tests (`src/lib.rs`)

These tests live inside the crate and use `#[tokio::test]` or `#[test]`.

- `send_sms_builds_sender_id_attribute` & `send_sms_propagates_publisher_errors` assert the SNS publisher wiring: message attributes are created correctly and upstream errors bubble back.
- Validation tests (`valid_message_passes_validation`, `empty_sender_id_fails_validation`, `empty_message_fails_validation`, `invalid_phone_number_fails_validation`) cover the `ZMQSendSmsMessage::validate` rules, including phonenumber parsing.
- Masking tests (`phone_masking_works`, `phone_masking_short_number`) ensure we never log raw PII regardless of number length.
- Async service loop tests (`run_service_processes_messages_with_concurrency_limit`, `run_service_handles_publisher_errors`, `run_service_stops_when_shutdown_is_triggered`) spin up fake sources/publishers to exercise semaphore limits, retry/error logging, and graceful shutdown signaling.

### Integration tests (`tests/`)

- `tests/integration_test.rs`
  - `zmq_message_serialization` confirms JSON payloads survive round-tripping exactly as expected by the ZeroMQ producers.
  - `zmq_context_creation` ensures the native `zmq` bindings can create sockets in the target environment.
- `tests/service_harness.rs`
  - Re-runs the async service loop scenarios (`run_service_processes_messages_with_concurrency_limit`, `run_service_handles_publisher_errors`, `run_service_stops_on_shutdown_signal`) but from the integration harness, proving the public API (`run_service`) behaves correctly when linked as an external crate.

## Running Tests

```bash
cargo test --all                     # run every test (default)
cargo test --lib                     # only unit tests in src/lib.rs
cargo test --test integration_test   # ZeroMQ serialization/context checks
cargo test --test service_harness    # async service harness scenarios
cargo test send_sms_builds_sender_id_attribute  # single test
```

Add `-- --nocapture` to any of the above when you need to stream log output from async tests.

## Coverage Summary

- ✅ Message validation and masking
- ✅ SNS attribute construction and error propagation
- ✅ Tokio semaphore/backpressure behavior and shutdown signaling
- ✅ ZeroMQ serialization basics and socket initialization

## Future Improvements

- Stub AWS SNS via the SDK’s `mockall` support to validate request bodies end-to-end.
- Add integration coverage for real ZeroMQ pub/sub sockets (e.g., using `zmq::Socket::bind` + `tokio::task::spawn` publisher).
- Extend tests to cover `.env`/configuration parsing edge cases (missing `MAX_CONCURRENT_SMS`, invalid integers, etc.).
