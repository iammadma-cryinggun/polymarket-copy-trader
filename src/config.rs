//! 配置管理

use anyhow::{Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// Polygon WebSocket URL (Alchemy/QuickNode)
    pub polygon_ws_url: String,

    /// 目标钱包地址（要跟单的地址）
    pub target_wallet: String,

    /// 私钥（用于签名交易）
    pub private_key: String,

    /// 跟单金额（USDC）
    pub copy_trade_amount: f64,

    /// 最大滑点（0.15 = 15%）
    pub max_slippage: f64,

    /// 最小剩余时间（秒）
    pub min_remaining_time: u64,

    /// 数据库路径
    pub db_path: String,

    /// Polymarket CLOB API URL
    pub clob_api_url: String,
}

impl Config {
    /// 从环境变量加载配置
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        // 支持 ALCHEMY_API_KEY 或完整的 POLYGON_WS_URL
        let polygon_ws_url = if let Ok(api_key) = env::var("ALCHEMY_API_KEY") {
            format!("wss://polygon-mainnet.g.alchemy.com/v2/{}", api_key)
        } else {
            env::var("POLYGON_WS_URL")
                .context("需要设置 ALCHEMY_API_KEY 或 POLYGON_WS_URL")?
        };

        let config = Self {
            polygon_ws_url,

            target_wallet: env::var("TARGET_WALLET")
                .context("TARGET_WALLET 环境变量未设置")?,

            private_key: env::var("PRIVATE_KEY")
                .context("PRIVATE_KEY 环境变量未设置")?,

            copy_trade_amount: env::var("COPY_TRADE_AMOUNT")
                .unwrap_or_else(|_| "20".to_string())
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

            db_path: env::var("DB_PATH")
                .unwrap_or_else(|_| "copy_trades.db".to_string()),

            clob_api_url: env::var("CLOB_API_URL")
                .unwrap_or_else(|_| "https://clob.polymarket.com".to_string()),
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

        // 验证滑点范围
        if self.max_slippage <= 0.0 || self.max_slippage > 1.0 {
            anyhow::bail!("MAX_SLIPPAGE 应在 (0.0, 1.0] 范围内");
        }

        // 验证跟单金额
        if self.copy_trade_amount <= 0.0 {
            anyhow::bail!("COPY_TRADE_AMOUNT 应大于 0");
        }

        tracing::info!("✅ 配置验证通过");
        Ok(())
    }
}
