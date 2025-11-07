# Repository Guidelines

## Project Structure & Module Organization
This workspace is a single Rust crate (`Cargo.toml`, edition 2024). Core logic lives in `src/lib.rs` (see `run()`), which wires ZeroMQ ingestion, AWS SNS publishing, and logging; `src/main.rs` is a thin Tokio entry point that delegates to the library. Add shared helpers under `src/` using modules (e.g., `src/publisher.rs`) and keep binary-specific wiring in `main.rs`. Build artifacts land in `target/`; place integration tests in `tests/` when functionality grows, and sample payloads or fixtures under `fixtures/`.

## Rust Quality Expectations
- Every change should raise the quality bar: prioritize readability, maintainability, and predictable error handling.
- Leave the codebase more idiomatic than you found it—refactor opportunistically, remove `unwrap()`s, and keep APIs narrow with focused traits/structs.
- Thread safety and async correctness are table stakes. Use `Send + Sync` bounds consciously and exercise backpressure-friendly patterns rather than spawning unchecked tasks.
- Always pair logic changes with tests that prove behavior and guard against regressions; use property tests or fixtures when serialization/deserialization is involved.

## Build, Test, and Development Commands
- `cargo run --bin pushkind-sms` loads `.env`, subscribes to `ZMQ_SMS_SUB`, and publishes a sample SMS via SNS.
- `cargo check` performs fast type-checking and ensures dependencies resolve.
- `cargo test --all` executes unit and integration tests; add `-- --nocapture` when debugging async logs.
- `cargo fmt && cargo clippy --all-targets --all-features` enforces formatting and linting before pushing.
- `cargo build --release` produces the optimized binary for deployment targets.

## Coding Style & Naming Conventions
Follow `rustfmt` defaults (4-space indent, trailing commas, grouped `use`). Prefer small modules with explicit `pub(crate)` visibility. Async functions should read as verbs (`send_batch`, `poll_queue`). Constants use SCREAMING_SNAKE_CASE, structs and enums use PascalCase, and feature flags or env keys (e.g., `ZMQ_SMS_SUB`) stay uppercase. Keep logs structured via `log` macros and gate noisy traces behind `debug!`.

**Write idiomatic Rust at all times:**
- Use `?` operator for error propagation instead of explicit `match` or `unwrap()`
- Prefer `let-else` patterns over `unwrap()` when handling `Result` or `Option`
- Use `if let` or `match` for pattern matching rather than boolean checks
- Leverage iterator methods (`.map()`, `.filter()`, `.collect()`) over explicit loops
- Use `thiserror` for custom error types with `#[from]` for automatic conversions
- Avoid `.clone()` unless necessary; prefer borrowing with `&` or `Arc` for shared ownership
- Use `_` prefix for intentionally unused variables (e.g., `_permit`, `_result`)
- Prefer explicit error messages with context over generic `expect()` or `unwrap()`

## Testing Guidelines
Write unit tests alongside implementation blocks using `#[cfg(test)] mod tests` and name cases `fn sends_minimal_payload()`. For end-to-end flows, add integration tests in `tests/` that stub ZeroMQ input and assert SNS payload construction. Target coverage on parsing, attribute mapping, and error propagation; mock AWS clients via dependency injection when possible. Run `cargo test --features mock` if you introduce mock-only code paths.

## Commit & Pull Request Guidelines
History favors concise, sentence-cased summaries (e.g., “Minimal working prototype”). Keep commits scoped to one concern, include rustfmt/clippy results, and mention related issues in the body (`Refs #123`). Pull requests should describe the motivation, include test evidence (command + result), call out config changes, and attach screenshots for user-visible behavior. Request review once CI (fmt, clippy, tests) is green and secrets have been stripped.

## Security & Configuration Tips
Secrets live in `.env`; never commit this file. Document required keys (`AWS_REGION`, `AWS_ACCESS_KEY_ID`, `ZMQ_SMS_SUB`, `SENDER_ID`) in your PR description when adding new ones. Use AWS profiles or IAM roles rather than hard-coded credentials, and avoid logging PII—mask phone numbers or tokens before emitting logs.
