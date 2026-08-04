//! Polygon RPC WebSocket 监听器
//!
//! 实时监听 Polymarket 合约的 OrderFilled 事件

use crate::abi::{addresses, event_sigs, CTFExchange};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256};
use alloy::providers::{ProviderBuilder, WsConnect};
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
    /// 交易哈希
    pub tx_hash: String,

    /// 吃单方地址
    pub taker: Address,

    /// 挂单方地址
    pub maker: Address,

    /// Token ID
    pub token_id: String,

    /// 交易方向（BUY/SELL）
    pub side: String,

    /// 吃单方数量
    pub taker_amount: u64,

    /// 挂单方数量
    pub maker_amount: u64,

    /// 区块号
    pub block_number: u64,
}

/// Polygon WebSocket 监听器
pub struct Listener {
    config: Config,
    db: Arc<Database>,
    event_sender: mpsc::Sender<TradeEvent>,
}

impl Listener {
    /// 创建监听器
    pub fn new(config: Config, db: Arc<Database>, event_sender: mpsc::Sender<TradeEvent>) -> Self {
        Self {
            config,
            db,
            event_sender,
        }
    }

    /// 启动监听
    pub async fn run(&self) -> Result<()> {
        tracing::info!("🔗 连接到 Polygon WebSocket...");

        // 创建 WebSocket Provider
        let ws = WsConnect::new(&self.config.polygon_ws_url);
        let provider = ProviderBuilder::new().connect_ws(ws).await?;

        tracing::info!("✅ Polygon WebSocket 连接成功");

        // 目标钱包地址
        let target_wallet: Address = self.config.target_wallet.parse()?;

        // 构建日志过滤器
        let ctf_exchange: Address = addresses::CTF_EXCHANGE.parse()?;
        let order_filled_sig = event_sigs::order_filled();

        tracing::info!("🎯 开始监听 CTF Exchange: {}", addresses::CTF_EXCHANGE);
        tracing::info!("🎯 目标钱包: {}", self.config.target_wallet);

        // 创建过滤器
        let filter = Filter::new()
            .address(ctf_exchange)
            .event_signature(order_filled_sig);

        // 订阅日志
        let sub = provider.subscribe_logs(&filter).await?;
        let mut stream = sub.into_stream();

        tracing::info!("🚀 监听器已就位，等待交易事件...");

        // 处理日志流
        while let Some(log) = stream.next().await {
            if let Err(e) = self.handle_log(&log, target_wallet).await {
                tracing::error!("❌ 处理日志失败: {}", e);
            }
        }

        Ok(())
    }

    /// 处理单条日志
    async fn handle_log(&self, log: &Log, target_wallet: Address) -> Result<()> {
        // 解析 OrderFilled 事件
        let event = match CTFExchange::OrderFilled::decode_raw_log(
            &log.topics,
            &log.data.data,
            true,
        ) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("⚠️ 解析事件失败: {}", e);
                return Ok(());
            }
        };

        // 检查是否是目标钱包的交易
        let is_target_taker = event.taker == target_wallet;
        let is_target_maker = event.maker == target_wallet;

        if !is_target_taker && !is_target_maker {
            return Ok(());
        }

        // 记录检测时间
        let detected_at = Utc::now();

        // 判断方向
        let side = if is_target_taker { "BUY" } else { "SELL" };

        // 计算入场价
        let maker_amt: u64 = event.makerAmount.to();
        let taker_amt: u64 = event.takerAmount.to();
        let entry_price = if maker_amt > 0 {
            taker_amt as f64 / maker_amt as f64
        } else {
            0.0
        };

        // 构建交易事件
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
            "🔥 [检测到目标交易] TX: {} | {} | Token: {} | 价格: {:.4} | 数量: {}",
            &trade_event.tx_hash[..20],
            side,
            &trade_event.token_id[..20],
            entry_price,
            taker_amt
        );

        // 记录到数据库
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

        // 发送事件
        self.event_sender.send(trade_event).await?;

        Ok(())
    }
}
