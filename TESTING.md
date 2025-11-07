# Testing Documentation

## Test Coverage

### Unit Tests (src/main.rs)

**Message Validation Tests:**
- `valid_message_passes_validation` - Validates correct message structure
- `empty_sender_id_fails_validation` - Ensures sender_id is required
- `empty_message_fails_validation` - Ensures message content is required
- `invalid_phone_number_fails_validation` - Validates E.164 phone format

**Phone Masking Tests:**
- `phone_masking_works` - Tests masking of standard phone numbers
- `phone_masking_short_number` - Tests masking edge case with short numbers

**Serialization Tests:**
- `deserialize_valid_json` - Tests JSON to struct deserialization
- `deserialize_invalid_json_fails` - Tests error handling for malformed JSON
- `serialize_message` - Tests struct to JSON serialization

### Integration Tests (tests/integration_test.rs)

**ZeroMQ Tests:**
- `zmq_message_serialization` - End-to-end message serialization
- `zmq_context_creation` - ZeroMQ socket creation

**Configuration Tests:**
- `parse_max_concurrent_from_env` - Environment variable parsing
- `parse_max_concurrent_default` - Default value fallback

## Running Tests

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test valid_message_passes_validation

# Run only unit tests
cargo test --bin pushkind-sms

# Run only integration tests
cargo test --test integration_test
```

## Test Results

```
running 9 tests (unit tests)
test result: ok. 9 passed; 0 failed

running 4 tests (integration tests)
test result: ok. 4 passed; 0 failed
```

## Coverage Areas

✅ Message validation (sender_id, phone_number, message)
✅ Phone number E.164 format validation
✅ Phone number masking for privacy
✅ JSON serialization/deserialization
✅ ZeroMQ context and socket creation
✅ Environment variable configuration
✅ Error handling and propagation

## Future Test Additions

- Mock AWS SNS client for send_sms testing
- Graceful shutdown signal handling
- Semaphore concurrency limiting
- End-to-end ZeroMQ pub/sub flow
