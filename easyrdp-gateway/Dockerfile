# ---- 构建阶段 ----
FROM rust:1.79-slim AS builder
WORKDIR /build
# 依赖缓存：先拷贝清单拉依赖，再拷源码
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo fetch
COPY src ./src
RUN cargo build --release

# ---- 运行阶段 ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/easyrdp-gateway /usr/local/bin/easyrdp-gateway
COPY config.example.toml /etc/easyrdp/config.example.toml
ENV RUST_LOG=info
EXPOSE 8443
USER 10001
ENTRYPOINT ["/usr/local/bin/easyrdp-gateway"]
CMD ["--config", "/etc/easyrdp/config.toml"]
