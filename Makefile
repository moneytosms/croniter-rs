.PHONY: build test test-original bench fmt lint

build:
	cargo build --release

test:
	cargo test

test-original:
	./tools/bridge/run_original_suite.sh

bench:
	cargo bench

fmt:
	cargo fmt

lint:
	cargo clippy -- -D warnings
