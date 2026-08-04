//! 跟单执行器

use crate::api::{MarketInfo, OrderBook, PolymarketClient, TradeResult};
use crate::config::Config;
use crate::db::{CopyTrade, Database};
use crate::listener::TradeEvent;
use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;

/// 跟单执行器
pub struct CopyTrader {
    config: Config,
    db: Database,
    api_client: Arc<PolymarketClient>,
}

struct CheckResult {
    passed: bool,
    reason: String,
}

impl CopyTrader {
    pub fn new(config: Config, db: Database) -> Self {
        let paper_trading = config.private_key.is_empty() || config.private_key == "your_private_key_here";

        let api_client = Arc::new(PolymarketClient::new(
            if paper_trading { None } else { Some(config.private_key.clone()) },
            paper_trading,
        ));

        if paper_trading {
            tracing::info!("👁️ 监控模式（Paper Trading）");
        } else {
            tracing::info!("🤖 实盘模式");
        }

        Self {
            config,
            db,
            api_client,
        }
    }

    pub async fn handle_trade_event(&self, event: TradeEvent) -> Result<()> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "⚡ [跟单处理器] 开始处理 | Token: {} | 延迟: {}ms",
            &event.token_id[..20],
            start_time.elapsed().as_millis()
        );

        let market_info = match self.api_client.get_market_by_token(&event.token_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("❌ 获取市场信息失败: {}", e);
                return Err(e);
            }
        };

        tracing::info!(
            "📊 市场信息: {} | 剩余时间: {}s",
            market_info.slug,
            market_info.remaining_time
        );

        let check_result = self.run_risk_checks(&event, &market_info)?;

        if !check_result.passed {
            tracing::warn!(
                "🛑 [风控拦截] {} | Token: {}",
                check_result.reason,
                &event.token_id[..20]
            );
            return Ok(());
        }

        let orderbook = match self.api_client.fetch_best_prices(&event.token_id).await {
            Ok(ob) => ob,
            Err(e) => {
                tracing::warn!("❌ 获取盘口失败: {}", e);
                return Err(e);
            }
        };

        tracing::info!(
            "📈 盘口: bid={:.3} | ask={:.3} | spread={:.3}",
            orderbook.best_bid,
            orderbook.best_ask,
            orderbook.spread
        );

        let slippage = self.calculate_slippage(&event, &orderbook)?;

        if slippage > self.config.max_slippage {
            tracing::warn!(
                "🛑 [滑点拦截] {:.2}% > {:.2}%",
                slippage * 100.0,
                self.config.max_slippage * 100.0
            );
            return Ok(());
        }

        let order_price = orderbook.best_ask + 0.01;

        tracing::info!(
            "🚀 [执行跟单] Token: {} | 价格: {:.4} | 金额: ${:.2}",
            &event.token_id[..20],
            order_price,
            self.config.copy_trade_amount
        );

        let result = match self.api_client.place_order(
            &event.token_id,
            "BUY",
            order_price,
            self.config.copy_trade_amount,
        ).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("❌ 下单失败: {}", e);
                return Err(e);
            }
        };

        let total_time = start_time.elapsed();
        tracing::info!(
            "✅ [跟单完成] 总耗时: {}ms | TX: {} | 成交: {:.2} shares",
            total_time.as_millis(),
            &result.order_id[..20],
            result.filled_size
        );

        let copy_trade = CopyTrade {
            id: None,
            tx_hash: event.tx_hash.clone(),
            target_wallet: self.config.target_wallet.clone(),
            market_slug: Some(market_info.slug),
            token_id: event.token_id.clone(),
            token_side: event.side.clone(),
            entry_price: order_price,
            size: result.filled_size,
            status: "filled".to_string(),
            created_at: Utc::now(),
            result: None,
            pnl: None,
        };

        self.db.insert_copy_trade(&copy_trade)?;

        tracing::info!("📝 跟单记录已保存");

        Ok(())
    }

    fn run_risk_checks(&self, event: &TradeEvent, market_info: &MarketInfo) -> Result<CheckResult> {
        if market_info.remaining_time < self.config.min_remaining_time as i64 {
            return Ok(CheckResult {
                passed: false,
                reason: format!(
                    "剩余时间 {}s < {}s",
                    market_info.remaining_time, self.config.min_remaining_time
                ),
            });
        }

        if event.taker_amount == 0 || event.maker_amount == 0 {
            return Ok(CheckResult {
                passed: false,
                reason: "价格数据无效".to_string(),
            });
        }

        if self.config.copy_trade_amount <= 0.0 {
            return Ok(CheckResult {
                passed: false,
                reason: "跟单金额无效".to_string(),
            });
        }

        let target_price = event.taker_amount as f64 / event.maker_amount as f64;
        if target_price >= 0.90 {
            return Ok(CheckResult {
                passed: false,
                reason: format!("入场价 {:.3} >= 0.90", target_price),
            });
        }

        Ok(CheckResult {
            passed: true,
            reason: String::new(),
        })
    }

    fn calculate_slippage(&self, event: &TradeEvent, orderbook: &OrderBook) -> Result<f64> {
        let target_price = if event.maker_amount > 0 {
            event.taker_amount as f64 / event.maker_amount as f64
        } else {
            0.0
        };

        let current_ask = orderbook.best_ask;

        if target_price > 0.0 && target_price < 1.0 {
            Ok((current_ask - target_price).abs() / target_price)
        } else {
            Ok(1.0)
        }
    }
}
