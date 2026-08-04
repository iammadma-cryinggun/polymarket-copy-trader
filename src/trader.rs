//! 跟单执行器
//!
//! 执行跟单逻辑，包含风控检查

use crate::config::Config;
use crate::db::{CopyTrade, Database};
use crate::listener::TradeEvent;
use alloy::primitives::U256;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 跟单执行器
pub struct CopyTrader {
    config: Config,
    db: Arc<Database>,
    http_client: Client,
}

impl CopyTrader {
    /// 创建跟单执行器
    pub fn new(config: Config, db: Arc<Database>) -> Self {
        Self {
            config,
            db,
            http_client: Client::new(),
        }
    }

    /// 处理交易事件
    pub async fn handle_trade_event(&self, event: TradeEvent) -> Result<()> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "⚡ [跟单处理器] 开始处理 | Token: {} | 延迟: {}ms",
            event.token_id,
            start_time.elapsed().as_millis()
        );

        // ========== 第一步：查询市场信息 ==========
        let market_info = self.get_market_info(&event.token_id).await?;

        // ========== 第二步：风控检查 ==========
        let check_result = self.run_risk_checks(&event, &market_info)?;

        if !check_result.passed {
            tracing::warn!(
                "🛑 [风控拦截] {} | Token: {}",
                check_result.reason,
                event.token_id
            );
            return Ok(());
        }

        // ========== 第三步：查询当前盘口 ==========
        let orderbook = self.get_orderbook(&event.token_id).await?;

        // ========== 第四步：滑点检查 ==========
        let slippage = self.calculate_slippage(&event, &orderbook)?;

        if slippage > self.config.max_slippage {
            tracing::warn!(
                "🛑 [滑点拦截] {:.2}% > {:.2}% | Token: {}",
                slippage * 100.0,
                self.config.max_slippage * 100.0,
                event.token_id
            );
            return Ok(());
        }

        // ========== 第五步：执行跟单 ==========
        tracing::info!(
            "🚀 [执行跟单] Token: {} | 价格: {:.4} | 金额: ${:.2}",
            event.token_id,
            orderbook.best_ask,
            self.config.copy_trade_amount
        );

        let result = self.execute_order(&event, &orderbook).await?;

        let total_time = start_time.elapsed();
        tracing::info!(
            "✅ [跟单完成] 总耗时: {}ms | TX: {}",
            total_time.as_millis(),
            result.order_id
        );

        // ========== 第六步：记录到数据库 ==========
        let copy_trade = CopyTrade {
            id: None,
            tx_hash: event.tx_hash,
            target_wallet: self.config.target_wallet.clone(),
            market_slug: market_info.slug,
            token_id: event.token_id,
            token_side: event.token_side.clone(),
            entry_price: orderbook.best_ask,
            size: self.config.copy_trade_amount / orderbook.best_ask,
            status: "filled".to_string(),
            created_at: Utc::now(),
            result: None,
            pnl: None,
        };

        self.db.insert_copy_trade(&copy_trade)?;

        Ok(())
    }

    /// 风控检查
    fn run_risk_checks(&self, event: &TradeEvent, market_info: &MarketInfo) -> Result<CheckResult> {
        // 检查1：剩余时间
        if market_info.remaining_time < self.config.min_remaining_time {
            return Ok(CheckResult {
                passed: false,
                reason: format!(
                    "剩余时间 {}s < {}s",
                    market_info.remaining_time, self.config.min_remaining_time
                ),
            });
        }

        // 检查2：价格有效性
        if event.taker_amount == 0 || event.maker_amount == 0 {
            return Ok(CheckResult {
                passed: false,
                reason: "价格数据无效".to_string(),
            });
        }

        // 检查3：金额限制
        if self.config.copy_trade_amount <= 0.0 {
            return Ok(CheckResult {
                passed: false,
                reason: "跟单金额无效".to_string(),
            });
        }

        Ok(CheckResult {
            passed: true,
            reason: String::new(),
        })
    }

    /// 查询市场信息
    async fn get_market_info(&self, token_id: &str) -> Result<MarketInfo> {
        // TODO: 调用 Polymarket API 获取市场信息
        // GET https://clob.polymarket.com/markets/{token_id}

        // 占位符返回
        Ok(MarketInfo {
            slug: "btc-updown-5m".to_string(),
            remaining_time: 180, // 假设还剩3分钟
            end_time: Utc::now() + chrono::Duration::seconds(180),
        })
    }

    /// 查询当前盘口
    async fn get_orderbook(&self, token_id: &str) -> Result<OrderBook> {
        // TODO: 调用 Polymarket API 获取盘口
        // GET https://clob.polymarket.com/book?token_id={token_id}

        // 占位符返回
        Ok(OrderBook {
            best_bid: 0.50,
            best_ask: 0.51,
            spread: 0.01,
        })
    }

    /// 计算滑点
    fn calculate_slippage(&self, event: &TradeEvent, orderbook: &OrderBook) -> Result<f64> {
        // 目标入场价
        let target_price = if event.maker_amount > 0 {
            event.taker_amount as f64 / event.maker_amount as f64
        } else {
            0.0
        };

        // 当前卖一价
        let current_ask = orderbook.best_ask;

        // 滑点 = (current_ask - target_price) / target_price
        if target_price > 0.0 {
            let slippage = (current_ask - target_price).abs() / target_price;
            Ok(slippage)
        } else {
            Ok(1.0) // 无法计算，默认100%滑点
        }
    }

    /// 执行下单
    async fn execute_order(&self, event: &TradeEvent, orderbook: &OrderBook) -> Result<OrderResult> {
        // TODO: 调用 Polymarket CLOB API 下单
        // POST https://clob.polymarket.com/order

        // 占位符返回
        Ok(OrderResult {
            order_id: format!("order_{}", chrono::Utc::now().timestamp()),
            status: "filled".to_string(),
        })
    }
}

/// 市场信息
#[derive(Debug, Clone)]
struct MarketInfo {
    slug: String,
    remaining_time: u64,
    end_time: chrono::DateTime<Utc>,
}

/// 盘口信息
#[derive(Debug, Clone)]
struct OrderBook {
    best_bid: f64,
    best_ask: f64,
    spread: f64,
}

/// 风控检查结果
struct CheckResult {
    passed: bool,
    reason: String,
}

/// 下单结果
struct OrderResult {
    order_id: String,
    status: String,
}
