//! Polygon RPC WebSocket 监听器

use crate::abi::{addresses, event_sigs, CTFExchange};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256};
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

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
        tracing::info!("🔗 连接到 Polygon WebSocket...");

        let ws = WsConnect::new(&self.config.polygon_ws_url);
        let provider = ProviderBuilder::new().connect_ws(ws).await?;

        tracing::info!("✅ Polygon WebSocket 连接成功");

        let target_wallet: Address = self.config.target_wallet.parse()?;
        let ctf_exchange: Address = addresses::CTF_EXCHANGE.parse()?;
        let order_filled_sig = event_sigs::order_filled();

        tracing::info!("🎯 开始监听 CTF Exchange: {}", addresses::CTF_EXCHANGE);
        tracing::info!("🎯 目标钱包: {}", self.config.target_wallet);

        let filter = Filter::new()
            .address(ctf_exchange)
            .event_signature(order_filled_sig);

        let sub = provider.subscribe_logs(&filter).await?;
        let mut stream = sub.into_stream();

        tracing::info!("🚀 监听器已就位，等待交易事件...");

        while let Some(log) = stream.next().await {
            if let Err(e) = self.handle_log(&log, target_wallet).await {
                tracing::error!("❌ 处理日志失败: {}", e);
            }
        }

        Ok(())
    }

    async fn handle_log(&self, log: &Log, target_wallet: Address) -> Result<()> {
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

        let is_target_taker = event.taker == target_wallet;
        let is_target_maker = event.maker == target_wallet;

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
