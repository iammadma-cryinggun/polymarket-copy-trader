//! 自动赎回模块 - 调用Python脚本版
//!
//! 通过 Python 脚本调用 CTF Exchange V2 的 redeemPositions
//! 将中奖代币换成 pUSD

use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{timeout, Duration as TokioDuration};
use tracing::{error, info, warn};

/// 赎回配置
#[derive(Debug, Clone)]
pub struct RedeemConfig {
    /// 是否启用自动赎回
    pub enabled: bool,
    /// 扫描间隔（秒）
    pub scan_interval_secs: u64,
    /// 最小赎回金额（当前价值低于该值跳过，节省 Gas）
    pub min_redeem_amount: f64,
    /// Polygon HTTP RPC URL（Python脚本使用）
    pub polygon_rpc_url: String,
}

impl Default for RedeemConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_secs: 300,
            min_redeem_amount: 0.10,
            polygon_rpc_url: String::new(),
        }
    }
}

/// data-api 返回的持仓结构
#[derive(Debug, Clone, Deserialize)]
struct GammaPosition {
    #[serde(rename = "conditionId", default)]
    condition_id: Option<String>,
    #[serde(rename = "question", default)]
    question: Option<String>,
    #[serde(rename = "balance", default)]
    balance: Option<f64>,
    #[serde(rename = "currentValue", default)]
    current_value: Option<f64>,
    #[serde(rename = "redeemable", default)]
    redeemable: Option<bool>,
    #[serde(rename = "closedAt", default)]
    closed_at: Option<String>,
}

/// 可赎回的持仓
#[derive(Debug, Clone)]
pub struct RedeemablePosition {
    pub condition_id: String,
    pub question: String,
    pub current_value: f64,
}

/// 自动赎回引擎
pub struct Redeemer {
    config: RedeemConfig,
    http_client: HttpClient,
    wallet_address: String,
}

impl Redeemer {
    pub fn new(private_key: &str, config: RedeemConfig) -> Result<Self> {
        // 从私钥推导钱包地址
        use alloy::signers::local::PrivateKeySigner;
        let signer: PrivateKeySigner = private_key
            .parse()
            .context("PRIVATE_KEY 解析失败")?;
        let wallet_address = format!("{:?}", signer.address());

        info!("[Redeem] 🚀 赎回引擎初始化，钱包: {}", wallet_address);

        Ok(Self {
            config,
            http_client: HttpClient::new(),
            wallet_address,
        })
    }

    /// 启动后台扫描赎回循环
    pub async fn run(self) -> Result<()> {
        if !self.config.enabled {
            info!("[Redeem] 自动赎回已禁用");
            return Ok(());
        }

        info!(
            "[Redeem] 🚀 自动赎回启动（Python脚本模式），每 {} 秒扫描一次",
            self.config.scan_interval_secs
        );

        loop {
            if let Err(e) = self.scan_and_redeem().await {
                error!("[Redeem] ❌ 扫描失败: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(self.config.scan_interval_secs)).await;
        }
    }

    /// 扫描并赎回
    async fn scan_and_redeem(&self) -> Result<()> {
        let positions = self.fetch_redeemable_positions().await?;

        if positions.is_empty() {
            info!("[Redeem] ✨ 无待赎回持仓");
            return Ok(());
        }

        info!("[Redeem] 🎯 发现 {} 个可赎回持仓", positions.len());
        for p in &positions {
            info!(
                "[Redeem]   - condition: {} | 价值: {:.4} | {}",
                &p.condition_id[..p.condition_id.len().min(20)],
                p.current_value,
                p.question
            );
        }

        // 调用Python脚本执行赎回
        self.execute_python_redeem().await
    }

    /// 从 data-api 获取可赎回持仓
    async fn fetch_redeemable_positions(&self) -> Result<Vec<RedeemablePosition>> {
        let wallet_lower = self.wallet_address.to_lowercase();
        let url = format!(
            "https://data-api.polymarket.com/positions?user={}",
            wallet_lower
        );

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("查询 data-api 持仓失败")?;

        let positions: Vec<GammaPosition> = response
            .json()
            .await
            .context("解析 data-api 持仓失败")?;

        let redeemable: Vec<RedeemablePosition> = positions
            .into_iter()
            .filter(|p| {
                p.redeemable.unwrap_or(false)
                    && p.closed_at.is_none()
                    && p.condition_id.is_some()
            })
            .filter(|p| {
                let current_value = p.current_value.unwrap_or(0.0);
                current_value >= self.config.min_redeem_amount
            })
            .map(|p| RedeemablePosition {
                condition_id: p.condition_id.unwrap_or_default(),
                question: p.question.unwrap_or_default(),
                current_value: p.current_value.unwrap_or(0.0),
            })
            .collect();

        info!("[Redeem] 🔍 扫描持仓完成，可赎回 {} 个", redeemable.len());
        Ok(redeemable)
    }

    /// 调用Python脚本执行赎回
    async fn execute_python_redeem(&self) -> Result<()> {
        info!("[Redeem] 🚀 调用Python赎回脚本...");

        let script_path = "/app/scripts/cloud_redeem.py";

        let result = timeout(
            TokioDuration::from_secs(120),
            async {
                Command::new("python3")
                    .arg(script_path)
                    .output()
                    .await
            }
        ).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.success() {
                    info!("[Redeem] ✅ Python脚本执行成功");
                    if !stdout.is_empty() {
                        for line in stdout.lines() {
                            info!("[Redeem] {}", line);
                        }
                    }
                } else {
                    warn!("[Redeem] ❌ Python脚本执行失败: {}", stderr);
                }
            }
            Ok(Err(e)) => {
                warn!("[Redeem] ❌ 启动Python脚本失败: {}", e);
            }
            Err(_) => {
                warn!("[Redeem] ⏰ Python脚本执行超时(120秒)");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config() {
        let config = RedeemConfig::default();
        assert!(!config.enabled);
    }
}
