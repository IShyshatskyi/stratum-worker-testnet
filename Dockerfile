# SP-18 Cloud PoW Worker — multi-stage Docker build (~5 MB final image)

FROM rust:1.78-slim AS builder
WORKDIR /build
COPY Cargo.toml .
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /build/target/release/stratum-worker /usr/local/bin/stratum-worker
ENV POOL_ENDPOINT=127.0.0.1:3333 \
    WORKER_NAME=sp18-worker \
    THREADS=1 \
    LOG_INTERVAL_SECS=30
ENTRYPOINT ["/usr/local/bin/stratum-worker"]
