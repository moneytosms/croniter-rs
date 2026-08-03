# syntax=docker/dockerfile:1

# Stage 1: build the release croniter-conformance binary.
FROM rust:slim-bookworm AS builder
WORKDIR /build
COPY rust-toolchain.toml Cargo.toml ./
COPY src ./src
COPY bench ./bench
COPY fuzz ./fuzz
RUN cargo build --release --bin croniter-conformance

# Stage 2: python3 + pytest running the original suite against the port.
FROM python:3.12-slim AS runtime
RUN pip install --no-cache-dir pytest
WORKDIR /app
COPY --from=builder /build/target/release/croniter-conformance /app/target/release/croniter-conformance
COPY tools/bridge ./tools/bridge
COPY tests/original ./tests/original

ENV CRONITER_CONFORMANCE_BIN=/app/target/release/croniter-conformance
ENV PYTHONPATH=/app/tools/bridge

CMD ["pytest", "tests/original"]
