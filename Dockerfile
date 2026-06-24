# =============================================================
# Stage 1a: Builder — glibc
# =============================================================
FROM rust:bookworm AS builder-glibc
RUN apt-get update -qq && apt-get install -y -qq \
    pkg-config libssl-dev cmake perl && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml statusline.sh ./
COPY test_direct.sh test_litellm.sh test_routing.sh ./
COPY src ./src
COPY benches ./benches
COPY tests ./tests
COPY config ./config
RUN cargo build --release --target x86_64-unknown-linux-gnu && \
    cp target/x86_64-unknown-linux-gnu/release/ccm /app/ccm

# =============================================================
# Stage 1b: Builder — musl (静态链接)
# =============================================================
FROM rust:bookworm AS builder-musl
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
# 输出变体 (6 种)
# =============================================================

# ── glibc + distroless/cc ──
FROM gcr.io/distroless/cc-debian12:nonroot AS glibc-distroless
COPY --from=builder-glibc /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# ── glibc + busybox:glibc ──
FROM busybox:1.36-glibc AS glibc-busybox
COPY --from=builder-glibc /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# ── glibc + alpine+gcompat ──
FROM alpine:3.20 AS glibc-alpine
RUN apk add --no-cache ca-certificates curl gcompat libgcc
COPY --from=builder-glibc /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# ── musl(静态) + distroless/static ──
FROM gcr.io/distroless/static-debian12:nonroot AS musl-distroless
COPY --from=builder-musl /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# ── musl(静态) + busybox:glibc ──
FROM busybox:1.36-glibc AS musl-busybox
COPY --from=builder-musl /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# ── musl(静态) + alpine ──
FROM alpine:3.20 AS musl-alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder-musl /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
