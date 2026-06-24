# =============================================================
# Stage 1: Build — Rust musl static binary
#   - 静态链接, 无 runtime libc 依赖
#   - 一个 binary 跑在所有 Linux 发行版上
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
# Stage 2: distroless/static — 安全最小化 (~5MB)
#   含 CA 证书、/etc/passwd、/tmp 等运行时必需项
#   无 shell、无包管理器。生产环境推荐。
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: alpine — 含 shell 工具 (~8MB)
#   有 ca-certificates + curl + sh, 适合调试/诊断
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 4: busybox — 极小含 shell (~4MB)
#   比 alpine 更小, 仅有 busybox + binary
#   适合资源极度受限的环境
# =============================================================
FROM busybox:1.36-glibc AS busybox
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
