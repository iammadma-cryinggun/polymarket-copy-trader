//! 配置管理

use anyhow::{Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// Polygon WebSocket URL (Alchemy/QuickNode)
    pub polygon_ws_url: String,

    /// 备用 WebSocket URL 列表（主 RPC 限流/不可用时自动切换）
    pub polygon_ws_fallback_urls: Vec<String>,

    /// eth_getLogs 轮询间隔（毫秒）。轮询代替了 WS 订阅，重连/429 时也不丢事件
    pub log_poll_interval: u64,

    /// 目标钱包地址（要跟单的地址，兼容 EOA 与 Gnosis Safe Proxy）
    /// 支持逗号分隔的多个地址
    pub target_wallet: String,

    /// 目标钱包地址列表（兼容 EOA 与其对应的 Gnosis Safe Proxy）
    pub target_wallets: Vec<String>,

    /// 私钥（用于签名交易）
    pub private_key: String,

    /// CLOB V2 签名类型：0=EOA（默认）, 1=PolyProxy(Magic/邮箱), 2=GnosisSafe, 3=Poly1271(存款钱包)
    pub signature_type: u8,

    /// 资金地址（代理/Safe/存款钱包时需设置；EOA 留空）
    pub funder: Option<String>,

    /// 跟单金额（USDC）
    pub copy_trade_amount: f64,

    /// 最大滑点（0.15 = 15%）
    pub max_slippage: f64,

    /// 最小剩余时间（秒）
    pub min_remaining_time: u64,

    /// 1 分钟内最多跟单次数（熔断，防目标钱包刷单/做市）
    pub max_orders_per_minute: u64,

    /// 当日累计投入上限（USDC，熔断保护）
    pub daily_spend_cap: f64,

    /// 数据库路径
    pub db_path: String,

    /// Polymarket CLOB API URL
    pub clob_api_url: String,

    /// 是否启用自动赎回
    pub redeem_enabled: bool,

    /// 自动赎回扫描间隔（秒）
    pub redeem_scan_interval: u64,

    /// 最小赎回金额（当前价值低于该值跳过）
    pub redeem_min_amount: f64,

    /// Polygon HTTP RPC URL（赎回交易用，可选）
    pub polygon_rpc_url: Option<String>,
}

impl Config {
    /// 从环境变量加载配置
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        // 支持 ALCHEMY_API_KEY 或完整的 POLYGON_WS_URL，如果都没设置则使用公共 RPC
        let polygon_ws_url = if let Ok(api_key) = env::var("ALCHEMY_API_KEY") {
            // 如果是完整 URL，自动转换 scheme
            if api_key.starts_with("https://") {
                api_key.replace("https://", "wss://")
            } else if api_key.starts_with("http://") {
                api_key.replace("http://", "ws://")
            } else {
                // 纯 API Key，构建 URL
                format!("wss://polygon-mainnet.g.alchemy.com/v2/{}", api_key)
            }
        } else if let Ok(url) = env::var("POLYGON_WS_URL") {
            // 转换 POLYGON_WS_URL 的 scheme
            if url.starts_with("https://") {
                url.replace("https://", "wss://")
            } else if url.starts_with("http://") {
                url.replace("http://", "ws://")
            } else {
                url
            }
        } else {
            // 使用公共 RPC（无需 Alchemy API key）
            tracing::info!("🌐 未设置 ALCHEMY_API_KEY/POLYGON_WS_URL，使用公共 RPC");
            "wss://polygon-bor-rpc.publicnode.com".to_string()
        };

        let config = Self {
            polygon_ws_url,

            // 备用 WS：优先用 POLYGON_WS_FALLBACK 环境变量（逗号分隔），否则用内置公共端点
            polygon_ws_fallback_urls: env::var("POLYGON_WS_FALLBACK")
                .map(|s| {
                    s.split(',')
                        .map(|u| u.trim().to_string())
                        .filter(|u| !u.is_empty())
                        .collect()
                })
                .unwrap_or_else(|_| {
                    vec![
                        "wss://polygon-bor-rpc.publicnode.com".to_string(),
                        "wss://polygon.api.onfinality.io/public".to_string(),
                        "wss://polygon.drpc.org".to_string(),
                    ]
                }),

            log_poll_interval: env::var("LOG_POLL_INTERVAL")
                .unwrap_or_else(|_| "4000".to_string())
                .parse()
                .context("LOG_POLL_INTERVAL 解析失败")?,

            target_wallet: env::var("TARGET_WALLET").context("TARGET_WALLET 环境变量未设置")?,
            target_wallets: env::var("TARGET_WALLETS")
                .unwrap_or_else(|_| env::var("TARGET_WALLET").unwrap_or_default())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),

            private_key: env::var("PRIVATE_KEY").context("PRIVATE_KEY 环境变量未设置")?,

            signature_type: env::var("SIGNATURE_TYPE")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .context("SIGNATURE_TYPE 解析失败")?,

            funder: env::var("FUNDER_ADDRESS").ok().filter(|s| !s.is_empty()),

            copy_trade_amount: env::var("COPY_TRADE_AMOUNT")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .context("COPY_TRADE_AMOUNT 解析失败")?,

            max_slippage: env::var("MAX_SLIPPAGE")
                .unwrap_or_else(|_| "0.15".to_string())
                .parse()
                .context("MAX_SLIPPAGE 解析失败")?,

            min_remaining_time: env::var("MIN_REMAINING_TIME")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .context("MIN_REMAINING_TIME 解析失败")?,

            max_orders_per_minute: env::var("MAX_ORDERS_PER_MINUTE")
                .unwrap_or_else(|_| "2".to_string())
                .parse()
                .context("MAX_ORDERS_PER_MINUTE 解析失败")?,

            daily_spend_cap: env::var("DAILY_SPEND_CAP")
                .unwrap_or_else(|_| "50".to_string())
                .parse()
                .context("DAILY_SPEND_CAP 解析失败")?,

            db_path: env::var("DB_PATH").unwrap_or_else(|_| "copy_trades.db".to_string()),

            clob_api_url: env::var("CLOB_API_URL")
                .unwrap_or_else(|_| "https://clob.polymarket.com".to_string()),

            redeem_enabled: env::var("REDEEM_ENABLED")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),

            redeem_scan_interval: env::var("REDEEM_SCAN_INTERVAL")
                .unwrap_or_else(|_| "300".to_string())
                .parse()
                .context("REDEEM_SCAN_INTERVAL 解析失败")?,

            redeem_min_amount: env::var("REDEEM_MIN_AMOUNT")
                .unwrap_or_else(|_| "0.10".to_string())
                .parse()
                .context("REDEEM_MIN_AMOUNT 解析失败")?,

            polygon_rpc_url: env::var("POLYGON_RPC_URL").ok(),
        };

        // 验证配置
        config.validate()?;

        Ok(config)
    }

    /// 验证配置
    fn validate(&self) -> Result<()> {
        // 验证目标钱包地址格式
        if !self.target_wallet.starts_with("0x") || self.target_wallet.len() != 42 {
            anyhow::bail!("TARGET_WALLET 格式无效，应为 0x 开头的 42 字符地址");
        }

        // 验证附加钱包地址格式
        for w in &self.target_wallets {
            if !w.starts_with("0x") || w.len() != 42 {
                anyhow::bail!("TARGET_WALLETS 中存在无效地址: {}", w);
            }
        }

        // 验证滑点范围
        if self.max_slippage <= 0.0 || self.max_slippage > 1.0 {
            anyhow::bail!("MAX_SLIPPAGE 应在 (0.0, 1.0] 范围内");
        }

        // 验证跟单金额
        if self.copy_trade_amount <= 0.0 {
            anyhow::bail!("COPY_TRADE_AMOUNT 应大于 0");
        }

        // Polymarket CLOB 单笔订单下限为 5 USDC；低于此值会被拒单
        if self.copy_trade_amount < 5.0 {
            tracing::warn!(
                "⚠️ COPY_TRADE_AMOUNT={:.2} 低于 Polymarket CLOB 最小下单金额 $5，\
                 实盘下单会被拒单（监测/纸面模式无影响）",
                self.copy_trade_amount
            );
        }

        // 验证熔断频次
        if self.max_orders_per_minute == 0 {
            anyhow::bail!("MAX_ORDERS_PER_MINUTE 应大于 0");
        }

        // 验证熔断额度
        if self.daily_spend_cap <= 0.0 {
            anyhow::bail!("DAILY_SPEND_CAP 应大于 0");
        }

        // 验证签名类型
        if self.signature_type > 3 {
            anyhow::bail!("SIGNATURE_TYPE 应为 0(EOA)/1(PolyProxy)/2(GnosisSafe)/3(Poly1271)");
        }

        tracing::info!("✅ 配置验证通过");
        Ok(())
    }
}
