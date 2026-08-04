# Polymarket Copy Trader

🚀 **实时链上跟单机器人** - 通过 Polygon RPC WebSocket 监听，实现 1.5 秒级跟单

## 📊 架构

```
┌─────────────────────────────────────────────────────────┐
│              Polygon RPC WebSocket 监听                 │
├─────────────────────────────────────────────────────────┤
│  Alchemy WebSocket                                      │
│       │                                                  │
│       ▼                                                  │
│  eth_subscribe("logs", {                                │
│      address: CTF_EXCHANGE,                             │
│      event: OrderFilled                                 │
│  })                                                      │
│       │                                                  │
│       ▼                                                  │
│  过滤目标钱包: 0xf418...                                 │
│       │                                                  │
│       ▼                                                  │
│  风控检查 → 滑点检查 → 执行跟单                          │
└─────────────────────────────────────────────────────────┘
```

## 🎯 核心优势

| 对比项 | HTTP 轮询 | **链上 WS 监听** |
|--------|-----------|-----------------|
| 延迟 | 4-6 秒 | **1.5-2 秒** |
| 稳定性 | 易被封 IP | **长连接稳定** |
| 适用场景 | 长周期市场 | **5分钟高频** |

## 🛠️ 快速开始

### 1. 安装依赖

```bash
# 克隆项目
git clone https://github.com/yourusername/polymarket-copy-trader.git
cd polymarket-copy-trader

# 复制配置文件
cp .env.example .env

# 编辑配置
nano .env
```

### 2. 配置环境变量

```bash
# Alchemy Polygon WebSocket URL
POLYGON_WS_URL=wss://polygon-mainnet.g.alchemy.com/v2/YOUR_API_KEY

# 目标钱包地址
TARGET_WALLET=0xf418d3a1a941292f9c8707d62a14980c5beb95a3

# 私钥（用于签名交易）
PRIVATE_KEY=your_private_key_here

# 跟单金额（USDC）
COPY_TRADE_AMOUNT=20

# 最大滑点（15%）
MAX_SLIPPAGE=0.15

# 最小剩余时间（最后30秒不跟单）
MIN_REMAINING_TIME=30
```

### 3. 运行

```bash
# 监控模式（只看不下单）
cargo run -- --watch-only

# 跟单模式（自动执行）
cargo run

# 查看统计
cargo run -- --stats
```

## 🛡️ 风控机制

### 1. 最大滑点拦截

```
大户入场价: 0.60
当前盘口卖一: 0.75
滑点: (0.75 - 0.60) / 0.60 = 25% > 15% ❌ 放弃跟单
```

### 2. 残余时间拦截

```
剩余时间: 25s < 30s ❌ 放弃跟单
```

### 3. 固定金额跟单

```
跟单金额: $20 USDC（不按比例放大）
```

## 📁 项目结构

```
polymarket-copy-trader/
├── src/
│   ├── main.rs          # 主入口
│   ├── abi.rs           # Polymarket 合约 ABI
│   ├── config.rs        # 配置管理
│   ├── db.rs            # 数据库记录
│   ├── listener.rs      # Polygon WS 监听器
│   └── trader.rs        # 跟单执行器
├── Cargo.toml
├── .env.example
└── README.md
```

## 🔧 核心合约

| 合约 | 地址 | 说明 |
|------|------|------|
| CTF Exchange | `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E` | Polymarket 主交易所 |
| NegRisk Exchange | `0xd91E40c3570878C357392B0C93bF2C93f5b18D54` | 新版交易所 |

## ⚠️ 风险提示

1. **市场风险** - 跟单策略不保证盈利
2. **延迟风险** - 即使 1.5 秒延迟也可能错过最佳入场
3. **滑点风险** - 大额交易可能推高价格
4. **隐私风险** - 链上交易是公开的

## 📊 性能指标

- **监听延迟**: ~1.5 秒（Polygon 区块时间）
- **下单延迟**: ~0.5 秒
- **总延迟**: ~2 秒

## 📄 License

MIT

## 🙋 常见问题

### Q: 为什么不用 Polymarket WebSocket API?

A: Polymarket WebSocket 只能订阅**自己账户**的交易，不能监听其他地址。

### Q: 为什么选择 Alchemy 节点?

A: Alchemy 提供稳定的 Polygon WebSocket 节点，延迟低、稳定性高。

### Q: 跟单金额如何设置?

A: 建议设置固定金额（如 $20），不要按比例放大，避免资金管理失控。
