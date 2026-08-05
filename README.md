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
│  三道风控 → 滑点检查 → 执行跟单                          │
└─────────────────────────────────────────────────────────┘
```

## 🎯 核心优势

| 对比项 | HTTP 轮询 | **链上 WS 监听** |
|--------|-----------|-----------------|
| 延迟 | 4-6 秒 | **1.5-2 秒** |
| 稳定性 | 易被封 IP | **长连接稳定** |
| 适用场景 | 长周期市场 | **5分钟高频** |

## 🛠️ 快速开始

### 1. 安装 Rust

```bash
# Windows: 下载并运行 rustup-init.exe
# https://rustup.rs/

# Linux/macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. 克隆项目

```bash
git clone https://github.com/iammadma-cryinggun/polymarket-copy-trader.git
cd polymarket-copy-trader
```

### 3. 配置环境变量

```bash
# 复制配置模板
cp .env.example .env

# 编辑配置（填入你的 API Key 和私钥）
nano .env
```

**必填项**：

| 变量 | 说明 | 获取方式 |
|------|------|----------|
| `POLYGON_WS_URL` | Alchemy Polygon WebSocket | [Alchemy Dashboard](https://dashboard.alchemy.com/) |
| `POLYGON_WS_FALLBACK` | 备用 WebSocket URL（逗号分隔，可选；主 RPC 限流时自动轮换） | 默认公共端点：PublicNode / OnFinality / dRPC |
| `PRIVATE_KEY` | MetaMask 私钥 | MetaMask → 账户详情 → 导出私钥 |
| `TARGET_WALLET` | 要跟单的钱包地址（EOA 或其 Gnosis Safe Proxy，可用 `TARGET_WALLETS` 逗号分隔多个） | Polymarket 用户主页 / Polygonscan |
| `REDEEM_ENABLED` | 是否启用自动赎回（`true`/`false`） | 中奖后自动调用链上 `redeemPositions` |
| `REDEEM_SCAN_INTERVAL` | 赎回扫描间隔（秒），默认 300 | |
| `REDEEM_MIN_AMOUNT` | 最小赎回金额，低于此值跳过以节省 Gas，默认 0.10 | |
| `POLYGON_RPC_URL` | Polygon HTTP RPC（赎回交易用，可选） | 不设置则用 `https://polygon-rpc.com` |

### 4. 运行

```bash
# 监控模式（只看不下单，推荐先用这个测试）
cargo run --release -- --watch-only

# 跟单模式（自动执行，需要配置私钥）
cargo run --release

# 查看统计
cargo run --release -- --stats
```

## 🛡️ 三道风控防线

### 1. 残余时间拦截

```
剩余时间: 25s < 30s ❌ 放弃跟单
```

**原因**：5分钟市场的最后30秒流动性差、滑点大

### 2. 入场价过滤

```
入场价: 0.92 >= 0.90 ❌ 放弃跟单
```

**原因**：入场价 >= 0.90 是负EV区间（数据验证）

### 3. 最大滑点拦截

```
大户入场价: 0.60
当前盘口卖一: 0.75
滑点: (0.75 - 0.60) / 0.60 = 25% > 15% ❌ 放弃跟单
```

**原因**：防止高位接盘

## 📁 项目结构

```
polymarket-copy-trader/
├── src/
│   ├── main.rs          # 主入口（CLI + 事件循环 + 赎回后台任务）
│   ├── abi.rs           # Polymarket 合约 ABI
│   ├── api.rs           # Polymarket CLOB API 客户端
│   ├── config.rs        # 配置管理
│   ├── db.rs            # 数据库记录
│   ├── listener.rs      # Polygon WS 监听器
│   ├── redeem.rs        # 自动赎回（CTF Exchange V2 redeemPositions）
│   └── trader.rs        # 跟单执行器（风控+下单）
├── Cargo.toml           # Rust 依赖
├── .env.example         # 环境变量模板
├── .gitignore           # Git 忽略规则
└── README.md            # 项目说明
```

## 🔧 核心合约

| 合约 | 地址 | 说明 |
|------|------|------|
| CTF Exchange V1 | `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E` | 旧版主交易所（2026-04 已切换 V2） |
| NegRisk Exchange V1 | `0xC5d563A36AE78145C45a50134d48A1215220f80a` | 旧版交易所（2026-04 已切换 V2） |
| CTF Exchange V2 | `0xE111180000d2663C0091e4f400237545B87B996B` | 当前主力交易所 |
| NegRisk Exchange V2 | `0xe2222d279d744050d28e00520010520000310F59` | 短线/多选项市场主力 |

## 📊 性能指标

- **监听延迟**: ~1.5 秒（Polygon 区块时间）
- **下单延迟**: ~0.5 秒
- **总延迟**: ~2 秒

## ⚠️ 风险提示

1. **市场风险** - 跟单策略不保证盈利
2. **延迟风险** - 即使 1.5 秒延迟也可能错过最佳入场
3. **滑点风险** - 大额交易可能推高价格
4. **隐私风险** - 链上交易是公开的
5. **私钥安全** - **永远不要把 .env 文件提交到 Git**

## 🚀 路线图

- [x] Polygon WS 监听
- [x] 三道风控防线
- [x] Polymarket CLOB API 下单
- [ ] 多地址同时监听
- [ ] Telegram 通知
- [ ] Web Dashboard
- [ ] 胜率统计分析

## 📄 License

MIT

## 🙋 常见问题

### Q: 日志一直提示 `HTTP error: 429 Too Many Requests`？

A: 这是 RPC 服务商限流，常见于 Alchemy 免费版。
1. **多个实例同时在跑**（旧进程未关 / Zeabur 也在部署）会占满并发 WS 连接，务必只保留一个实例。
2. 程序已内置退避重连 + 备用 RPC 自动轮换（`POLYGON_WS_FALLBACK`），限流恢复后会自动切回。
3. 若持续限流，可手动把 `POLYGON_WS_URL` 换成一个公共 WS 端点（如 `wss://polygon-bor-rpc.publicnode.com`）。

### Q: 为什么不用 Polymarket WebSocket API?

A: Polymarket WebSocket 只能订阅**自己账户**的交易，不能监听其他地址。

### Q: 为什么选择 Alchemy 节点?

A: Alchemy 提供稳定的 Polygon WebSocket 节点，延迟低、稳定性高。免费套餐每月 300M 计算单位足够使用。

### Q: 跟单金额如何设置?

A: 建议设置固定金额（如 $20），不要按比例放大，避免资金管理失控。

### Q: 如何判断目标钱包是否值得跟单?

A: 先运行 `--watch-only` 模式观察几天，查看胜率和交易频率，再决定是否跟单。

### Q: Paper Trading 模式是什么?

A: 如果不配置 `PRIVATE_KEY` 或配置为空，程序会自动进入 Paper Trading 模式，模拟交易不真实下单。

---

**⚠️ 重要提示**: 本项目仅供学习和研究目的，使用本项目进行交易的一切风险由用户自行承担。
