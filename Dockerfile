# Zeabur 部署配置 - Polymarket Copy Trader
# 基于 Rust 官方镜像

# 1. 构建阶段
FROM rust:1.91-bookworm AS builder

# 构建参数
ARG BUILD_TIMESTAMP=2026-08-04T00:00:00Z
LABEL build.timestamp=$BUILD_TIMESTAMP
LABEL build.reason="Initial deployment"

# 检查 Rust 版本
RUN rustc --version && cargo --version

# 安装编译依赖
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# 创建工作目录
WORKDIR /app

# 复制源码
COPY . .

# 清除构建缓存，确保使用最新代码
RUN cargo clean || true

# 编译发布版本
RUN cargo build --release

# 2. 运行阶段
FROM debian:bookworm-slim

# 安装运行时依赖 + Python（用于赎回脚本）
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    curl \
    python3 \
    python3-pip \
    python3-venv \
    && rm -rf /var/lib/apt/lists/*

# 安装Python依赖
RUN pip3 install --break-system-packages \
    web3==6.11.3 \
    requests==2.31.0 \
    python-dotenv==1.0.0

# 创建目录
WORKDIR /app

# 从 builder 复制二进制文件
COPY --from=builder /app/target/release/polymarket-copy-trader /app/polymarket-copy-trader

# 复制Python赎回脚本
COPY scripts/cloud_redeem.py /app/scripts/cloud_redeem.py
RUN chmod +x /app/scripts/cloud_redeem.py

# 创建数据目录
RUN mkdir -p /app/data

# 环境变量
ENV RUST_LOG=info

# 数据库路径（使用 volume）
ENV DB_PATH=/app/data/copy_trades.db

# 入口点
ENTRYPOINT ["/app/polymarket-copy-trader"]
