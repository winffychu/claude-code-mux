# =============================================================
# Stage 1: Build — Rust (Debian/glibc)
# =============================================================
FROM rust:slim-bookworm AS builder

RUN apt-get update -qq && apt-get install -y -qq \
    pkg-config libssl-dev cmake perl-modules && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
COPY benches ./benches
COPY tests ./tests
COPY config ./config

RUN cargo build --release && \
    cp target/release/ccm /app/ccm

# =============================================================
# Stage 2: distroless (glibc, ~15MB)
# =============================================================
FROM gcr.io/distroless/cc-debian12:nonroot AS distroless
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]

# =============================================================
# Stage 3: alpine (glibc compat, ~20MB)
# =============================================================
FROM alpine:3.20 AS alpine
RUN apk add --no-cache ca-certificates curl libgcc gcompat
COPY --from=builder /app/ccm /usr/local/bin/ccm
EXPOSE 13456
ENTRYPOINT ["ccm"]
CMD ["start"]
