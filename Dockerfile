# =============================================================
# Stage 1: Build — Rust (musl 静态链接)
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
    && cp target/x86_64-unknown-linux-musl/release/ccm /app/ccm \
    && mkdir -p /home/nonroot/.claude-code-mux

# =============================================================
# Stage 2: distroless — 生产推荐
#   包含 CA 证书链, /etc/ssl/certs, /etc/passwd (nonroot 用户)
#   /home/nonroot/.claude-code-mux 预建目录, 属主 nonroot uid 65532
#   首次启动无需 volume 也能自动创建 config.toml
#   若挂 volume, 确保 volume 属主为 65532 或添加 user: root
# =============================================================
FROM gcr.io/distroless/static-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
COPY --from=builder --chown=65532:65532 /home/nonroot /home/nonroot
ENV HOME=/home/nonroot
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: alpine — 调试诊断
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl
COPY --from=builder /app/ccm /usr/local/bin/ccm
ENV HOME=/root
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 4: busybox — 资源受限
# =============================================================
FROM busybox:1.36-glibc AS busybox
COPY --from=builder /app/ccm /usr/local/bin/ccm
ENV HOME=/root
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
