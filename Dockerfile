# =============================================================
# Stage 1: Build — Rust (musl 静态链接)
#   编译产物无 libc 依赖, 单一 binary 适用于所有 Linux 发行版
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

RUN cargo build --release --target x86_64-unknown-linux-musl \
    && cp target/x86_64-unknown-linux-musl/release/ccm /app/ccm

# =============================================================
# Stage 2: distroless — 生产推荐 (~5MB 压缩)
#   包含 CA 证书链 /etc/ssl/certs, /etc/passwd, /tmp
#   无 shell, 无包管理器, 攻击面最小
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: alpine — 调试诊断 (~10MB 压缩)
#   含 ca-certificates + curl + sh
#   适合需要 exec 进入容器排查的场景
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 4: busybox-glibc — 资源受限环境 (~7MB 压缩)
#   注意: busybox 基础镜像不含 CA 证书
#   若需 HTTPS 请使用 distroless 或 alpine 变体
# =============================================================
FROM busybox:1.36-glibc AS busybox
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
