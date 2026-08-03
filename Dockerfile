# syntax=docker/dockerfile:1

# Stage 1: build the release croniter-conformance binary.
FROM rust:slim-bookworm AS builder
WORKDIR /build
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
COPY benches ./benches
RUN cargo build --release --bin croniter-conformance

# Stage 2: python3 + pytest running the original suite against the port.
#
# dateutil and pytz are the upstream *test suite's* own imports, not dependencies of
# croniter or of this port; without them collection fails before a single case runs.
FROM python:3.12-slim AS runtime
RUN pip install --no-cache-dir pytest 'python-dateutil>=2.9.0' 'pytz>2021.1'
WORKDIR /app
COPY --from=builder /build/target/release/croniter-conformance /app/target/release/croniter-conformance
COPY tools/bridge ./tools/bridge
COPY tests/original ./tests/original

ENV CRONITER_CONFORMANCE_BIN=/app/target/release/croniter-conformance
ENV PYTHONPATH=/app/tools/bridge

CMD ["pytest", "tests/original"]
