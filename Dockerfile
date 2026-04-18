FROM rust:1.94 as builder

WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/payce-ng /app/payce-ng
COPY --from=builder /app/migrations /app/migrations

EXPOSE 3000
CMD ["/app/payce-ng"]
