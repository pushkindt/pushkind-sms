check:
	cargo fmt --all
	cargo clippy --all-features --all-targets --tests -- -Dwarnings
	cargo test --all-features
