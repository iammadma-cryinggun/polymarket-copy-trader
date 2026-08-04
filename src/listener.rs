//! Polygon RPC WebSocket 监听器

use crate::abi::{addresses, event_sigs, CTFExchangeV1, CTFExchangeV2};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use chrono::Utc;
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// 没有数据推送时判定为连接假死的超时（秒）
const STALL_TIMEOUT_SECS: u64 = 120;

/// 监听到的交易事件
#[derive(Debug, Clone)]
pub struct TradeEvent {
    pub tx_hash: String,
    pub taker: Address,
    pub maker: Address,
    pub token_id: String,
    pub side: String,
    pub taker_amount: u64,
    pub maker_amount: u64,
    pub block_number: u64,
}

/// Polygon WebSocket 监听器
pub struct Listener {
    config: Config,
    db: Database,
    event_sender: mpsc::Sender<TradeEvent>,
}

impl Listener {
    pub fn new(config: Config, db: Database, event_sender: mpsc::Sender<TradeEvent>) -> Self {
        Self {
            config,
            db,
            event_sender,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let target_wallets: Vec<Address> = self
            .config
            .target_wallets
            .iter()
            .map(|w| w.parse::<Address>().context("目标钱包地址解析失败"))
            .collect::<Result<_>>()?;

        // 持续监听，连接中断时自动重连
        loop {
            if let Err(e) = self.listen_once(&target_wallets).await {
                tracing::error!("❌ 监听连接异常: {}，5 秒后重连...", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn listen_once(&self, target_wallets: &[Address]) -> Result<()> {
        tracing::info!("🔗 连接到 Polygon WebSocket...");
        tracing::info!("📌 URL: {}", &self.config.polygon_ws_url);

        let ws = WsConnect::new(&self.config.polygon_ws_url);
        let provider = ProviderBuilder::new().connect_ws(ws).await?;

        tracing::info!("✅ Polygon WebSocket 连接成功");

        // 兼容 Polymarket 所有核心交易所合约（V1 + V2）
        let ctf_v1: Address = addresses::CTF_EXCHANGE.parse()?;
        let negrisk_v1: Address = addresses::NEGRISK_EXCHANGE.parse()?;
        let ctf_v2: Address = addresses::CTF_EXCHANGE_V2.parse()?;
        let negrisk_v2: Address = addresses::NEGRISK_EXCHANGE_V2.parse()?;

        tracing::info!("🎯 开启多合约监听 [CTF V1, NegRisk V1, CTF V2, NegRisk V2]");
        tracing::info!("🎯 目标监听钱包: {}", self.config.target_wallets.join(", "));

        // 同时监听 V1 与 V2 的 OrderFilled 事件签名（两者 topic0 不同）
        let filter = Filter::new()
            .address(vec![ctf_v1, negrisk_v1, ctf_v2, negrisk_v2])
            .event_signature(vec![event_sigs::order_filled_v1(), event_sigs::order_filled_v2()]);

        let sub = provider.subscribe_logs(&filter).await?;
        let mut stream = sub.into_stream();

        tracing::info!("🚀 监听器已就位，等待交易事件...");

        // 一笔撮合会发出多个 OrderFilled 事件（每个 maker 单 + 1 个 taker 单汇总事件），
        // 按 tx_hash 聚合成批，再挑选目标钱包自己的订单事件处理，避免重复跟单
        let mut pending: Vec<Log> = Vec::new();
        let mut pending_tx: Option<B256> = None;

        // 心跳检测：长时间无数据推送则判定为连接假死，触发重连
        loop {
            let res = timeout(Duration::from_secs(STALL_TIMEOUT_SECS), stream.next()).await;

            let should_break = match &res {
                Ok(None) => {
                    tracing::warn!("⚠️ 监听流已结束，准备重连...");
                    true
                }
                Err(_) => {
                    tracing::warn!(
                        "⚠️ 超过 {} 秒无数据推送，WebSocket 可能假死，触发重连",
                        STALL_TIMEOUT_SECS
                    );
                    true
                }
                Ok(Some(_)) => false,
            };

            if should_break {
                // 重连前处理剩余批次，避免遗漏最后一批交易
                if !pending.is_empty() {
                    if let Err(e) = self.process_batch(&pending, target_wallets).await {
                        tracing::error!("❌ 处理日志失败: {}", e);
                    }
                }
                return Ok(());
            }

            let log = res.unwrap().unwrap();
            let tx_hash = log.transaction_hash.unwrap_or_default();

            tracing::trace!(
                "📥 收到链上 OrderFilled 事件，TxHash: {:?}",
                log.transaction_hash
            );

            if pending_tx == Some(tx_hash) {
                pending.push(log);
            } else {
                if !pending.is_empty() {
                    if let Err(e) = self.process_batch(&pending, target_wallets).await {
                        tracing::error!("❌ 处理日志失败: {}", e);
                    }
                }
                pending = vec![log];
                pending_tx = Some(tx_hash);
            }
        }
    }

    /// 从 topics 中直接提取 maker（topic2）和 taker（topic3），无需完整解码
    fn extract_parties(&self, log: &Log) -> (Address, Address) {
        let maker = log
            .topics()
            .get(2)
            .map(|t| Address::from_word(*t))
            .unwrap_or_default();
        let taker = log
            .topics()
            .get(3)
            .map(|t| Address::from_word(*t))
            .unwrap_or_default();
        (maker, taker)
    }

    /// 处理同一笔交易的日志批次：
    /// 优先挑选目标钱包自己的订单事件（maker == target），
    /// 因为目标作为 taker 时，对家 maker 事件的 tokenId 可能是相反方向的仓位
    async fn process_batch(&self, logs: &[Log], target_wallets: &[Address]) -> Result<()> {
        let chosen = logs
            .iter()
            .find(|l| {
                let (maker, _) = self.extract_parties(l);
                target_wallets.contains(&maker)
            })
            .or_else(|| {
                logs.iter().find(|l| {
                    let (maker, taker) = self.extract_parties(l);
                    target_wallets.contains(&maker) || target_wallets.contains(&taker)
                })
            });

        if let Some(log) = chosen {
            self.handle_log(log, target_wallets).await?;
        }

        Ok(())
    }

    async fn handle_log(&self, log: &Log, target_wallets: &[Address]) -> Result<()> {
        let topics: Vec<B256> = log.topics().iter().copied().collect();
        let data = log.data().data.clone();

        if topics.is_empty() {
            return Ok(());
        }

        let sig = topics[0];
        let v1_sig = event_sigs::order_filled_v1();
        let v2_sig = event_sigs::order_filled_v2();

        // 解析出事件公共字段
        // maker: 事件所描述订单的持有人
        // maker_side: 该订单的方向（0=BUY, 1=SELL）；V1 无 side 字段，由 makerAssetId==0 推断
        let (maker, taker, token_id, maker_amount, taker_amount, maker_side): (
            Address,
            Address,
            U256,
            u64,
            u64,
            Option<u8>,
        ) = if sig == v2_sig {
            match CTFExchangeV2::OrderFilled::decode_raw_log(&topics, &data) {
                Ok(e) => (
                    e.maker,
                    e.taker,
                    e.tokenId,
                    e.makerAmountFilled.to(),
                    e.takerAmountFilled.to(),
                    Some(e.side),
                ),
                Err(e) => {
                    tracing::debug!("⚠️ V2 事件解析失败: {}", e);
                    return Ok(());
                }
            }
        } else if sig == v1_sig {
            match CTFExchangeV1::OrderFilled::decode_raw_log(&topics, &data) {
                Ok(e) => {
                    // V1 中 makerAssetId==0 表示 maker 买入，否则为卖出；
                    // 非零的 asset id 即为该订单交易的仓位 tokenId
                    let token_id = if e.makerAssetId != U256::ZERO {
                        e.makerAssetId
                    } else {
                        e.takerAssetId
                    };
                    let maker_side = if e.makerAssetId != U256::ZERO { 1 } else { 0 };
                    (
                        e.maker,
                        e.taker,
                        token_id,
                        e.makerAmountFilled.to(),
                        e.takerAmountFilled.to(),
                        Some(maker_side),
                    )
                }
                Err(e) => {
                    tracing::debug!("⚠️ V1 事件解析失败: {}", e);
                    return Ok(());
                }
            }
        } else {
            return Ok(());
        };

        // 匹配任一目标钱包（EOA 或 Gnosis Safe Proxy）
        let is_target_taker = target_wallets.contains(&taker);
        let is_target_maker = target_wallets.contains(&maker);

        if !is_target_taker && !is_target_maker {
            return Ok(());
        }

        let detected_at = Utc::now();

        // 方向标签：事件中 maker 字段是订单持有人，maker_side 是该订单方向；
        // 目标作为对家（taker）时方向相反
        let side = if is_target_maker {
            match maker_side {
                Some(0) => "BUY".to_string(),
                _ => "SELL".to_string(),
            }
        } else {
            match maker_side {
                Some(0) => "SELL".to_string(),
                _ => "BUY".to_string(),
            }
        };

        let maker_amt: u64 = maker_amount;
        let taker_amt: u64 = taker_amount;
        let entry_price = if maker_amt > 0 {
            taker_amt as f64 / maker_amt as f64
        } else {
            0.0
        };

        let trade_event = TradeEvent {
            tx_hash: format!("{:?}", log.transaction_hash.unwrap_or_default()),
            taker,
            maker,
            token_id: token_id.to_string(),
            side: side.clone(),
            taker_amount: taker_amt,
            maker_amount: maker_amt,
            block_number: log.block_number.unwrap_or(0),
        };

        tracing::info!(
            "🔥 [检测到目标交易] TX: {} | {} | Token: {} | 价格: {:.4}",
            &trade_event.tx_hash[..20],
            side,
            &trade_event.token_id[..20],
            entry_price
        );

        let target_trade = TargetTrade {
            tx_hash: trade_event.tx_hash.clone(),
            target_wallet: self.config.target_wallet.clone(),
            market_slug: None,
            token_id: trade_event.token_id.clone(),
            token_side: side,
            entry_price,
            size: taker_amt as f64,
            detected_at,
            followed: false,
            follow_reason: None,
        };

        // 同一笔交易只跟单一次（单笔撮合会发出多个 OrderFilled 事件）
        if self.db.target_trade_exists(&trade_event.tx_hash)? {
            return Ok(());
        }

        self.db.insert_target_trade(&target_trade)?;
        self.event_sender.send(trade_event).await?;

        Ok(())
    }
}
