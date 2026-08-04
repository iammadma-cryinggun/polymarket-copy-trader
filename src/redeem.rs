//! 自动赎回模块 - Polymarket CTF Exchange V2 链上赎回
//!
//! 借鉴 btc_doomsday_rust_final 的赎回逻辑：
//! 通过 data-api 扫描可赎回持仓，然后调用 CTF Exchange V2 的
//! `redeemPositions` 将中奖代币换成 pUSD。

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Duration;
use tracing::{error, info, warn};

/// Polymarket CTF Exchange V2 合约地址
const CTF_EXCHANGE_V2: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
/// pUSD 合约地址（当前抵押品）
const COLLATERAL_TOKEN: &str = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB";
/// Polygon 链 ID
const POLYGON_CHAIN_ID: u64 = 137;
/// 赎回交易的 Gas 上限
const REDEEM_GAS_LIMIT: u64 = 400_000;

sol! {
    /// CTF Exchange redeemPositions / payoutNumerators 接口
    interface ICTFExchange {
        function redeemPositions(
            address collateralToken,
            bytes32 parentCollectionId,
            bytes32 conditionId,
            uint256[] calldata indexSets
        ) external;

        function payoutNumerators(
            bytes32 conditionId,
            uint256[] calldata indexSets
        ) external view returns (uint256[] memory);
    }
}

/// 赎回配置
#[derive(Debug, Clone)]
pub struct RedeemConfig {
    /// 是否启用自动赎回
    pub enabled: bool,
    /// 扫描间隔（秒）
    pub scan_interval_secs: u64,
    /// 最小赎回金额（当前价值低于该值跳过，节省 Gas）
    pub min_redeem_amount: f64,
    /// Polygon HTTP RPC URL（用于发送赎回交易）
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
/// ⚠️ Polymarket API 字段类型不稳定，全部用 Option + serde(default)
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
    private_key: String,
    wallet_address: Address,
    /// 自定义 RPC 地址（从环境变量覆盖）
    rpc_url: String,
}

impl Redeemer {
    pub fn new(private_key: &str, config: RedeemConfig) -> Result<Self> {
        let signer = private_key
            .parse::<PrivateKeySigner>()
            .context("PRIVATE_KEY 解析失败")?;
        let wallet_address = signer.address();

        let rpc_url = if !config.polygon_rpc_url.is_empty() {
            config.polygon_rpc_url.clone()
        } else {
            // 从 POLYGON_RPC_URL 环境变量读取，否则使用公共 RPC
            std::env::var("POLYGON_RPC_URL").unwrap_or_else(|_| {
                "https://polygon-rpc.com".to_string()
            })
        };

        Ok(Self {
            config,
            http_client: HttpClient::new(),
            private_key: private_key.to_string(),
            wallet_address,
            rpc_url,
        })
    }

    /// 启动后台扫描赎回循环
    pub async fn run(self) -> Result<()> {
        if !self.config.enabled {
            info!("[Redeem] 自动赎回已禁用");
            return Ok(());
        }

        info!(
            "[Redeem] 🚀 自动赎回启动，钱包: {}，每 {} 秒扫描一次",
            self.wallet_address, self.config.scan_interval_secs
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

        // 创建带钱包的 provider（每个扫描周期复用）
        let provider = ProviderBuilder::default()
            .with_recommended_fillers()
            .wallet(EthereumWallet::from(
                self.private_key
                    .parse::<PrivateKeySigner>()
                    .context("PRIVATE_KEY 解析失败")?
                    .with_chain_id(Some(POLYGON_CHAIN_ID)),
            ))
            .connect_http(reqwest::Url::parse(&self.rpc_url).context("RPC URL 解析失败")?);

        for (i, position) in positions.iter().enumerate() {
            info!(
                "[Redeem] [{}/{}] 开始赎回 {}",
                i + 1,
                positions.len(),
                &position.condition_id[..position.condition_id.len().min(20)]
            );

            match self
                .redeem_condition(&provider, &position.condition_id)
                .await
            {
                Ok(tx_hash) => {
                    info!("[Redeem] ✅ [{}/{}] 赎回成功: {:?}", i + 1, positions.len(), tx_hash);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                Err(e) => {
                    warn!("[Redeem] ❌ [{}/{}] 赎回失败: {}", i + 1, positions.len(), e);
                }
            }
        }

        Ok(())
    }

    /// 从 data-api 获取可赎回持仓
    async fn fetch_redeemable_positions(&self) -> Result<Vec<RedeemablePosition>> {
        let wallet_hex = format!("{:?}", self.wallet_address).to_lowercase();
        let url = format!(
            "https://data-api.polymarket.com/positions?user={}",
            wallet_hex
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
                // 过滤：redeemable=true 且未关闭 且 当前价值大于阈值
                p.redeemable.unwrap_or(false)
                    && p.closed_at.is_none()
                    && p.condition_id.is_some()
            })
            .filter(|p| {
                let current_value = p.current_value.unwrap_or(0.0);
                let balance = p.balance.unwrap_or(0.0);
                (current_value > 0.0 || balance > 0.0)
                    && current_value >= self.config.min_redeem_amount
            })
            .map(|p| RedeemablePosition {
                condition_id: p.condition_id.unwrap_or_default(),
                question: p.question.unwrap_or_default(),
                current_value: p.current_value.unwrap_or(0.0),
            })
            .collect();

        info!(
            "[Redeem] 🔍 扫描持仓完成，可赎回 {} 个",
            redeemable.len()
        );

        Ok(redeemable)
    }

    /// 赎回单个 condition_id
    async fn redeem_condition<P: Provider<alloy::network::Ethereum> + Send + Sync>(
        &self,
        provider: &P,
        condition_id: &str,
    ) -> Result<alloy::primitives::TxHash> {
        let condition_id_bytes = parse_condition_id(condition_id)?;

        // 先查链上 payout，确定胜出 index（失败也不中断）
        let (has_winner, winner_index) = self
            .check_chain_payout(provider, &condition_id_bytes)
            .await
            .unwrap_or_else(|e| {
                warn!("[Redeem] ⚠️ payout 查询失败，使用默认组合: {}", e);
                (false, 0usize)
            });

        // 构建 indexSets 重试组合
        let mut index_sets_options: Vec<Vec<U256>> = Vec::new();

        if has_winner {
            if winner_index == 0 {
                index_sets_options.push(vec![U256::from(1)]); // YES 赢了
                info!("[Redeem] 🎯 优先尝试 index=1 (YES 赢了)");
            } else {
                index_sets_options.push(vec![U256::from(2)]); // NO 赢了
                info!("[Redeem] 🎯 优先尝试 index=2 (NO 赢了)");
            }
        }

        index_sets_options.push(vec![U256::from(1), U256::from(2)]); // 组合仓位
        if !has_winner || winner_index != 0 {
            index_sets_options.push(vec![U256::from(1)]);
        }
        if !has_winner || winner_index != 1 {
            index_sets_options.push(vec![U256::from(2)]);
        }

        let ctf_address = Address::from_str(CTF_EXCHANGE_V2)?;
        let collateral = Address::from_str(COLLATERAL_TOKEN)?;

        for (attempt, index_sets) in index_sets_options.iter().enumerate() {
            let index_values: Vec<u32> = index_sets.iter().map(|x| x.as_limbs()[0] as u32).collect();
            info!(
                "[Redeem] 🔄 [尝试 {}/{}] indexSets={:?}",
                attempt + 1,
                index_sets_options.len(),
                index_values
            );

            // 编码 redeemPositions 调用数据
            let call = ICTFExchange::redeemPositionsCall {
                collateralToken: collateral,
                parentCollectionId: B256::ZERO,
                conditionId: condition_id_bytes,
                indexSets: index_sets.clone(),
            };
            let calldata = call.abi_encode();

            // 动态 Gas 策略：获取最新区块 base fee
            let base_fee_gwei = self.get_base_fee_gwei(provider).await?;
            let (priority_gwei, multiplier) = if base_fee_gwei > 100 {
                (80u64, 3u64)
            } else if base_fee_gwei > 50 {
                (60u64, 2u64)
            } else {
                (50u64, 1u64)
            };

            let max_priority: u128 = (priority_gwei * 1_000_000_000u64) as u128;
            let max_fee: u128 =
                (base_fee_gwei * 1_000_000_000u64 * multiplier) as u128 + max_priority;

            let tx = TransactionRequest::default()
                .with_to(ctf_address)
                .with_input(calldata)
                .with_gas_limit(REDEEM_GAS_LIMIT)
                .with_max_priority_fee_per_gas(max_priority)
                .with_max_fee_per_gas(max_fee);

            // 模拟执行，跳过会 revert 的组合
            if let Err(e) = provider.call(tx.clone()).await {
                warn!("[Redeem] 🛑 模拟执行被拒绝，跳过此组合: {}", e);
                continue;
            }

            // 发送交易
            let pending = match provider.send_transaction(tx).await {
                Ok(p) => p,
                Err(e) => {
                    warn!("[Redeem] ⚠️ 发送失败，尝试下一个组合: {}", e);
                    continue;
                }
            };

            let tx_hash = *pending.tx_hash();
            info!("[Redeem] 🚀 赎回交易已广播: {:?}", tx_hash);

            // 等待确认
            match pending.get_receipt().await {
                Ok(receipt) => {
                    if receipt.status() {
                        info!("[Redeem] 🎉 赎回成功! indexSets={:?}", index_values);
                        return Ok(tx_hash);
                    } else {
                        warn!("[Redeem] ⚠️ 交易 revert (status=0)，尝试下一个组合...");
                    }
                }
                Err(e) => {
                    warn!("[Redeem] ⚠️ 等待确认失败: {}", e);
                }
            }
        }

        anyhow::bail!("所有 indexSets 组合都失败")
    }

    /// 获取最新区块 base fee（Gwei）
    async fn get_base_fee_gwei<P: Provider<alloy::network::Ethereum> + Send + Sync>(
        &self,
        provider: &P,
    ) -> Result<u64> {
        let block = provider
            .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
            .await?
            .context("无法获取最新区块")?;

        Ok(block
            .header
            .base_fee_per_gas
            .unwrap_or(30_000_000_000u64)
            / 1_000_000_000u64)
    }

    /// 查询链上 payoutNumerators，返回 (是否有胜出方, 胜出 index)
    async fn check_chain_payout<P: Provider<alloy::network::Ethereum> + Send + Sync>(
        &self,
        provider: &P,
        condition_id_bytes: &B256,
    ) -> Result<(bool, usize)> {
        let ctf_address = Address::from_str(CTF_EXCHANGE_V2)?;

        let call = ICTFExchange::payoutNumeratorsCall {
            conditionId: *condition_id_bytes,
            indexSets: vec![U256::from(1), U256::from(2)],
        };

        let tx = TransactionRequest::default()
            .with_to(ctf_address)
            .with_input(call.abi_encode());

        let result = provider.call(tx.clone()).await?;

        let payouts: Vec<U256> =
            ICTFExchange::payoutNumeratorsCall::abi_decode_returns(&result)?;

        info!("[Redeem] 💰 链上 payoutNumerators: {:?}", payouts);

        let winner_index = payouts
            .iter()
            .position(|p| !p.is_zero())
            .unwrap_or(0usize);
        let has_winner = payouts.iter().any(|p| !p.is_zero());

        Ok((has_winner, winner_index))
    }
}

/// 解析 condition_id 为 32 字节（不足 64 位右侧补零）
fn parse_condition_id(condition_id: &str) -> Result<B256> {
    let cleaned = condition_id.trim_start_matches("0x");

    let padded = if cleaned.len() < 64 {
        let mut padded = cleaned.to_string();
        while padded.len() < 64 {
            padded.push('0');
        }
        padded
    } else if cleaned.len() == 64 {
        cleaned.to_string()
    } else {
        cleaned[..64].to_string()
    };

    if padded.len() != 64 {
        anyhow::bail!("condition_id 长度错误: {}", padded.len());
    }

    let mut bytes = [0u8; 32];
    alloy::hex::decode_to_slice(&padded, &mut bytes)
        .context("condition_id hex 解码失败")?;

    Ok(B256::from(bytes))
}
