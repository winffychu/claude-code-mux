# =============================================================
# Stage 1: 构建阶段 — Rust cross-compile (musl 静态链接)
# =============================================================
FROM rust:1.81-alpine3.20 AS builder

RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    openssl-libs-static \
    perl make gcc pkgconfig cmake

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY benches ./benches
COPY tests ./tests
COPY config ./config

RUN cargo build --release --target x86_64-unknown-linux-musl && \
    cp target/x86_64-unknown-linux-musl/release/ccm /app/ccm && \
    strip /app/ccm

# =============================================================
# Stage 2: Distroless — 最精简 (~5MB)
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: Alpine — 含 shell 工具 (~12MB)
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 4: Busybox — 超极小 (~3MB)
# =============================================================
FROM busybox:1.36-glibc AS busybox
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
