//! Polygon RPC WebSocket 监听器
//!
//! 实时监听 Polymarket 合约的 OrderFilled 事件

use crate::abi::{addresses, event_sigs, CTFExchange};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256};
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::{Filter, Log};
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
        let provider = ProviderBuilder::new().on_ws(ws).await?;

        tracing::info!("✅ Polygon WebSocket 连接成功");

        // 目标钱包地址
        let target_wallet: Address = self.config.target_wallet.parse()?;

        // 构建日志过滤器
        // 监听 CTF Exchange 的 OrderFilled 事件
        let ctf_exchange: Address = addresses::CTF_EXCHANGE.parse()?;
        let order_filled_sig = event_sigs::order_filled();

        tracing::info!("🎯 开始监听 CTF Exchange: {}", addresses::CTF_EXCHANGE);
        tracing::info!("🎯 目标钱包: {}", self.config.target_wallet);
        tracing::info!("📡 事件签名: {:?}", order_filled_sig);

        // 创建过滤器：监听 OrderFilled 事件
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
        let event = match CTFExchange::OrderFilled::decode_log(log, true) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("⚠️ 解析事件失败: {}", e);
                return Ok(());
            }
        };

        // 检查是否是目标钱包的交易
        let is_target_taker = event.taker == target_wallet;
        let is_target_maker = event.maker == target_wallet;

        if !is_target_taker && !is_target_maker {
            // 不是目标钱包的交易，忽略
            return Ok(());
        }

        // 记录检测时间
        let detected_at = Utc::now();

        // 构建交易事件
        let trade_event = TradeEvent {
            tx_hash: format!("{:?}", log.transaction_hash.unwrap_or_default()),
            taker: event.taker,
            maker: event.maker,
            token_id: event.tokenId.to_string(),
            taker_amount: event.takerAmount.to(),
            maker_amount: event.makerAmount.to(),
            block_number: log.block_number.unwrap_or(0),
        };

        // 判断方向
        let token_side = if is_target_taker {
            "BUY" // 吃单方买入
        } else {
            "SELL" // 挂单方卖出
        };

        // 计算入场价（近似）
        // entry_price = taker_amount / maker_amount
        let entry_price = if event.makerAmount.to::<u64>() > 0 {
            event.takerAmount.to::<u64>() as f64 / event.makerAmount.to::<u64>() as f64
        } else {
            0.0
        };

        tracing::info!(
            "🔥 [检测到目标交易] TX: {:?} | {} | Token: {} | 价格: {:.4} | 数量: {}",
            log.transaction_hash.unwrap_or_default(),
            token_side,
            event.tokenId,
            entry_price,
            event.takerAmount
        );

        // 记录到数据库
        let target_trade = TargetTrade {
            tx_hash: trade_event.tx_hash.clone(),
            target_wallet: self.config.target_wallet.clone(),
            market_slug: None, // 需要后续查询
            token_id: trade_event.token_id.clone(),
            token_side: token_side.to_string(),
            entry_price,
            size: trade_event.taker_amount as f64,
            detected_at,
            followed: false,
            follow_reason: None,
        };

        // 检查是否已记录
        if !self.db.target_trade_exists(&trade_event.tx_hash)? {
            self.db.insert_target_trade(&target_trade)?;
        }

        // 发送事件到跟单逻辑
        self.event_sender.send(trade_event).await?;

        Ok(())
    }
}

/// 解析 Token ID 获取市场信息
pub fn parse_token_id(token_id: &str) -> Option<(String, bool)> {
    // Token ID 格式需要从 Polymarket API 获取
    // 这里返回占位符
    None
}
