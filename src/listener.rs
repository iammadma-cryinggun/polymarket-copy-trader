//! Polygon RPC WebSocket 监听器

use crate::abi::{addresses, event_sigs, CTFExchangeV1, CTFExchangeV2};
use crate::config::Config;
use crate::db::{Database, TargetTrade};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{BlockNumberOrTag, Filter, Log};
use alloy::sol_types::SolEvent;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

/// 将 wss:// 或 ws:// 端点转换为等价的 https:// 用于 eth_getLogs HTTP 轮询
fn ws_to_http(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{}", rest)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{}", rest)
    } else {
        url.to_string()
    }
}

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

        // 主 RPC + 备用 RPC（限流/不可用时自动轮换），均转为 HTTP 用于 getLogs 轮询
        let mut rpc_urls: Vec<String> = vec![ws_to_http(&self.config.polygon_ws_url)];
        rpc_urls.extend(self.config.polygon_ws_fallback_urls.iter().map(|u| ws_to_http(u)));

        // 请求失败退避：5s -> 10s -> 20s -> ... 封顶 300s，避免持续冲击限流端点
        let mut backoff_secs: u64 = 5;
        const MAX_BACKOFF_SECS: u64 = 300;
        let mut url_index: usize = 0;

        loop {
            let url = rpc_urls[url_index % rpc_urls.len()].clone();

            match self.listen_once(&target_wallets, &url).await {
                Ok(()) => {
                    // 正常退出（listen_once 一般不会 Ok 返回）：重置退避
                    backoff_secs = 5;
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    tracing::error!("❌ 轮询 RPC 异常: {} | {}", url, e);
                    if err_msg.contains("429") {
                        tracing::warn!("  ⚠️ 429 限流（多实例共用 RPC / 配额已满），轮询会自动切换备用 RPC");
                    }
                    if rpc_urls.len() > 1 {
                        url_index = (url_index + 1) % rpc_urls.len();
                        tracing::warn!("  🔄 已切换到备用 RPC: {}", rpc_urls[url_index]);
                    }
                    tracing::warn!("⏳ {} 秒后重试...", backoff_secs);
                    sleep(Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                }
            }
        }
    }

    /// 用 eth_getLogs 从上次处理的区块轮询到最新，逐块处理目标成交事件。
    /// getLogs 天然包含阻塞期间的所有事件，重连/429 后不会丢单。
    async fn listen_once(&self, target_wallets: &[Address], rpc_url: &str) -> Result<()> {
        tracing::info!("🔗 连接到 Polygon HTTP RPC（轮询模式）: {}", rpc_url);
        let provider = ProviderBuilder::new().connect_http(rpc_url.parse::<reqwest::Url>()?);

        let ctf_v1: Address = addresses::CTF_EXCHANGE.parse()?;
        let negrisk_v1: Address = addresses::NEGRISK_EXCHANGE.parse()?;
        let ctf_v2: Address = addresses::CTF_EXCHANGE_V2.parse()?;
        let negrisk_v2: Address = addresses::NEGRISK_EXCHANGE_V2.parse()?;

        tracing::info!("🎯 多合约轮询 [CTF V1, NegRisk V1, CTF V2, NegRisk V2]");
        tracing::info!("🎯 目标监听钱包: {}", self.config.target_wallets.join(", "));

        let contracts = vec![ctf_v1, negrisk_v1, ctf_v2, negrisk_v2];
        let sigs = vec![event_sigs::order_filled_v1(), event_sigs::order_filled_v2()];

        // 上次已处理到的区块；未命中目标时 last_block 也会持续推进，保证不漏
        let mut last_block = provider.get_block_number().await?;
        tracing::info!("🚀 轮询已就位，从区块 {} 起监听...", last_block);

        loop {
            let latest = match provider.get_block_number().await {
                Ok(b) => b,
                Err(e) => return Err(anyhow!("获取最新区块失败: {}", e)),
            };

            if latest > last_block {
                let from = last_block + 1;
                let filter = Filter::new()
                    .address(contracts.clone())
                    .event_signature(sigs.clone())
                    .from_block(BlockNumberOrTag::Number(from))
                    .to_block(BlockNumberOrTag::Number(latest));

                match provider.get_logs(&filter).await {
                    Ok(logs) => {
                        self.process_raw_logs(&logs, target_wallets).await?;
                        last_block = latest;
                    }
                    Err(e) => return Err(anyhow!("eth_getLogs 失败: {}", e)),
                }
            } else {
                // 链回滚时退到最新高度，避免从已被回滚的区块拉取
                last_block = latest;
            }

            sleep(Duration::from_millis(self.config.log_poll_interval)).await;
        }
    }

    /// 将一批日志按交易分组，每组（同一笔撮合）交给 process_batch 挑选目标事件
    async fn process_raw_logs(&self, logs: &[Log], target_wallets: &[Address]) -> Result<()> {
        let mut order: Vec<B256> = Vec::new();
        let mut batches: HashMap<B256, Vec<Log>> = HashMap::new();

        for l in logs {
            let h = l.transaction_hash.unwrap_or_default();
            if !batches.contains_key(&h) {
                order.push(h);
            }
            batches.entry(h).or_default().push(l.clone());
        }

        for h in order {
            if let Some(batch) = batches.get(&h) {
                self.process_batch(batch, target_wallets).await?;
            }
        }
        Ok(())
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
