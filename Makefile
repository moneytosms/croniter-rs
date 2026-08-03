.PHONY: build test test-original bench bench-compare corpus fuzz fmt lint

build:
	cargo build --release

test:
	cargo test

test-original:
	./tools/bridge/run_original_suite.sh

bench:
	cargo bench

# Cross-language comparison; writes bench/results.json. Needs the extractor venv.
bench-compare:
	python3 bench/compare.py

# Regenerates tests/port/corpus.json from the pinned upstream commit.
corpus:
	./tools/extract_corpus/run.sh

# Differential fuzz against the pinned Python; writes fuzz/log.txt.
fuzz:
	tools/extract_corpus/.venv/bin/python fuzz/harness.py --seconds 120 --seed 1 --log fuzz/log.txt

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings
