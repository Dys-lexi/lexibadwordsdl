FROM rust:1.94.1-trixie AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --locked --release --bin haste-client

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/haste-client /usr/local/bin/haste-client
# USER 10001:10001
ENTRYPOINT ["/usr/local/bin/haste-client"]
