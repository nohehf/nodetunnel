FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY nodetunnel /app/nodetunnel
WORKDIR /app/nodetunnel
RUN cargo build --release --bin relay-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/nodetunnel/target/release/relay-server /usr/local/bin/app
WORKDIR /
EXPOSE 8080/udp
EXPOSE 8081/tcp
ENTRYPOINT ["/usr/local/bin/app"]
