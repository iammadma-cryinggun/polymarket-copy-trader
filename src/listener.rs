//! Polygon RPC WebSocket 监听器

use crate::abi::{addresses, event_sigs, CTFExchange};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256};
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

        // 兼容 Polymarket 所有核心交易所合约
        let ctf_v1: Address = addresses::CTF_EXCHANGE.parse()?;
        let ctf_v2: Address = addresses::CTF_EXCHANGE_V2.parse()?;
        let negrisk_v2: Address = addresses::NEGRISK_EXCHANGE_V2.parse()?;

        let order_filled_sig = event_sigs::order_filled();

        tracing::info!("🎯 开启多合约监听 [CTF V1, CTF V2, NegRisk V2]");
        tracing::info!("🎯 目标监听钱包: {}", self.config.target_wallets.join(", "));

        // 同时监听所有核心交易所合约
        let filter = Filter::new()
            .address(vec![ctf_v1, ctf_v2, negrisk_v2])
            .event_signature(order_filled_sig);

        let sub = provider.subscribe_logs(&filter).await?;
        let mut stream = sub.into_stream();

        tracing::info!("🚀 监听器已就位，等待交易事件...");

        // 心跳检测：长时间无数据推送则判定为连接假死，触发重连
        loop {
            match timeout(Duration::from_secs(STALL_TIMEOUT_SECS), stream.next()).await {
                Ok(Some(log)) => {
                    tracing::trace!(
                        "📥 收到链上 OrderFilled 事件，TxHash: {:?}",
                        log.transaction_hash
                    );

                    if let Err(e) = self.handle_log(&log, target_wallets).await {
                        tracing::error!("❌ 处理日志失败: {}", e);
                    }
                }
                Ok(None) => {
                    tracing::warn!("⚠️ 监听流已结束，准备重连...");
                    return Ok(());
                }
                Err(_) => {
                    tracing::warn!(
                        "⚠️ 超过 {} 秒无数据推送，WebSocket 可能假死，触发重连",
                        STALL_TIMEOUT_SECS
                    );
                    return Ok(());
                }
            }
        }
    }

    async fn handle_log(&self, log: &Log, target_wallets: &[Address]) -> Result<()> {
        // 解析事件
        let topics: Vec<B256> = log.topics().iter().copied().collect();
        let data = log.data().data.clone();

        let event = match CTFExchange::OrderFilled::decode_raw_log(&topics, &data) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("⚠️ 解析事件失败: {}", e);
                return Ok(());
            }
        };

        // 匹配任一目标钱包（EOA 或 Gnosis Safe Proxy）
        let is_target_taker = target_wallets.contains(&event.taker);
        let is_target_maker = target_wallets.contains(&event.maker);

        if !is_target_taker && !is_target_maker {
            return Ok(());
        }

        let detected_at = Utc::now();
        let side = if is_target_taker { "BUY" } else { "SELL" };

        let maker_amt: u64 = event.makerAmount.to();
        let taker_amt: u64 = event.takerAmount.to();
        let entry_price = if maker_amt > 0 {
            taker_amt as f64 / maker_amt as f64
        } else {
            0.0
        };

        let trade_event = TradeEvent {
            tx_hash: format!("{:?}", log.transaction_hash.unwrap_or_default()),
            taker: event.taker,
            maker: event.maker,
            token_id: event.tokenId.to_string(),
            side: side.to_string(),
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
            token_side: side.to_string(),
            entry_price,
            size: taker_amt as f64,
            detected_at,
            followed: false,
            follow_reason: None,
        };

        if !self.db.target_trade_exists(&trade_event.tx_hash)? {
            self.db.insert_target_trade(&target_trade)?;
        }

        self.event_sender.send(trade_event).await?;

        Ok(())
    }
}
