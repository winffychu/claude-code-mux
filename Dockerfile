# =============================================================
# Stage 1: Build — Rust (musl 静态链接)
# =============================================================
FROM rust:bookworm AS builder

RUN apt-get update -qq && apt-get install -y -qq \
    musl-tools pkg-config libssl-dev cmake perl && \
    rm -rf /var/lib/apt/lists/* && \
    rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release --target x86_64-unknown-linux-musl --locked 2>&1 || true
COPY statusline.sh ./
COPY src ./src
COPY benches ./benches
COPY tests ./tests
COPY config ./config

RUN cargo build --release --target x86_64-unknown-linux-musl --locked \
    && cp target/x86_64-unknown-linux-musl/release/ccm /app/ccm

# =============================================================
# Stage 2: distroless — 生产推荐
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
ENV HOME=/home/nonroot
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
