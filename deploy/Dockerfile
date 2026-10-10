# syntax=docker/dockerfile:1
# Release image. Build context is server/ (see deploy/compose.dev.yml).
FROM rust:1.99-alpine@sha256:0cce0a5e0e8ba67b455257a3a02a1d99005f382748789d6464460028810f1627 AS chef
RUN apk add --no-cache musl-dev
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ENV SQLX_OFFLINE=true
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked --target x86_64-unknown-linux-musl --bin porchlight --bin porch

# Export the very same musl CLI that goes into the API image, without rebuilding it elsewhere.
FROM scratch AS cli-export
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/porch /porch

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3 AS runtime
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/porchlight /usr/local/bin/porchlight
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/porch /usr/local/bin/porch
ENTRYPOINT ["/usr/local/bin/porchlight"]
CMD ["serve"]
