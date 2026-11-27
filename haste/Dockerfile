FROM rust:1.94.1-trixie AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev cmake libzstd-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
ENV ZSTD_SYS_USE_PKG_CONFIG=1
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# Cache third-party dependencies and parser crates separately from worker code.
RUN mkdir -p src && touch src/lib.rs \
    && cargo build --locked --release --lib \
    && cargo clean --release -p haste
COPY src/ src/
COPY data/hero-classes.json data/hero-classes.json
RUN cargo build --locked --release --bin haste-client

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/haste-client /usr/local/bin/haste-client
# USER 10001:10001
ENTRYPOINT ["/usr/local/bin/haste-client"]
