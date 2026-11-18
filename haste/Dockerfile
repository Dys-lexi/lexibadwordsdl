FROM rust:1.94.1-trixie AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --locked --release --bin haste-server

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/haste-server /usr/local/bin/haste-server
USER 10001:10001
ENV BIND_ADDR=0.0.0.0:80
EXPOSE 80
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:80/health"]
ENTRYPOINT ["/usr/local/bin/haste-server"]
