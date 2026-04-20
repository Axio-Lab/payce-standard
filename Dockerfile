# syntax=docker/dockerfile:1.6

FROM rust:1.88-bookworm AS builder
WORKDIR /app

# Cache dependencies separately from source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release --locked && \
    rm -rf src target/release/deps/payce_ng* target/release/payce-ng

COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --locked

FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin payce

COPY --from=builder /app/target/release/payce-ng /usr/local/bin/payce-ng
COPY --from=builder /app/migrations /app/migrations

USER payce
EXPOSE 3000
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/payce-ng"]
