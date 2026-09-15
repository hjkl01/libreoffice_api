FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive \
    PORT=8080 \
    LO_MAX_CONCURRENCY=2 \
    MAX_UPLOAD_SIZE=104857600 \
    CONVERSION_TIMEOUT_SECS=300 \
    RUST_LOG=info

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    fontconfig \
    fonts-dejavu \
    fonts-liberation \
    fonts-noto-cjk \
    fonts-noto-core \
    fonts-wqy-zenhei \
    libreoffice \
    libreoffice-writer \
    libreoffice-calc \
    libreoffice-impress \
    && fc-cache -f -v \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin appuser
WORKDIR /app
COPY --from=builder /app/target/release/libreoffice_api /app/libreoffice_api
RUN mkdir -p /tmp/libreoffice-api && chown -R appuser:appuser /app /tmp/libreoffice-api
USER appuser

EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD /usr/bin/curl -fsS http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/app/libreoffice_api"]
