# =============================================================
# Stage 1: Build — Rust musl static binary
# =============================================================
FROM rust:bookworm AS builder

RUN apt-get update -qq && apt-get install -y -qq \
    musl-tools pkg-config libssl-dev cmake perl && \
    rm -rf /var/lib/apt/lists/* && \
    rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY Cargo.toml statusline.sh ./
COPY test_direct.sh test_litellm.sh test_routing.sh ./
COPY src ./src
COPY benches ./benches
COPY tests ./tests
COPY config ./config

RUN cargo build --release --target x86_64-unknown-linux-musl && \
    cp target/x86_64-unknown-linux-musl/release/ccm /app/ccm

# =============================================================
# Stage 2: distroless/static (~5MB, no libc dependency)
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: alpine (~8MB, with ca-certificates + curl)
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 4: busybox (~3MB, literally just binary)
# =============================================================
FROM busybox:1.36-glibc AS busybox
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
