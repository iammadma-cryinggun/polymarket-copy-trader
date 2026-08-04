//! 跟单执行器
//!
//! 执行跟单逻辑，包含三道风控检查

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
    db: Arc<Database>,
    api_client: Arc<PolymarketClient>,
}

/// 风控检查结果
struct CheckResult {
    passed: bool,
    reason: String,
}

impl CopyTrader {
    /// 创建跟单执行器
    pub fn new(config: Config, db: Arc<Database>) -> Self {
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

    /// 处理交易事件
    pub async fn handle_trade_event(&self, event: TradeEvent) -> Result<()> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "⚡ [跟单处理器] 开始处理 | Token: {} | 延迟: {}ms",
            event.token_id,
            start_time.elapsed().as_millis()
        );

        // ========== 第一步：查询市场信息 ==========
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

        // ========== 第二步：风控检查 ==========
        let check_result = self.run_risk_checks(&event, &market_info)?;

        if !check_result.passed {
            tracing::warn!(
                "🛑 [风控拦截] {} | Token: {}",
                check_result.reason,
                event.token_id
            );

            // 记录跳过原因
            self.update_skip_reason(&event, &check_result.reason).await?;
            return Ok(());
        }

        // ========== 第三步：查询当前盘口 ==========
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

        // ========== 第四步：滑点检查 ==========
        let slippage = self.calculate_slippage(&event, &orderbook)?;

        if slippage > self.config.max_slippage {
            tracing::warn!(
                "🛑 [滑点拦截] {:.2}% > {:.2}% | Token: {}",
                slippage * 100.0,
                self.config.max_slippage * 100.0,
                event.token_id
            );

            self.update_skip_reason(&event, &format!("滑点 {:.2}% > {:.2}%", slippage * 100.0, self.config.max_slippage * 100.0)).await?;
            return Ok(());
        }

        // ========== 第五步：执行跟单 ==========
        let order_price = orderbook.best_ask + 0.01; // 加1分钱滑点

        tracing::info!(
            "🚀 [执行跟单] Token: {} | 价格: {:.4} | 金额: ${:.2}",
            event.token_id,
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
            result.order_id,
            result.filled_size
        );

        // ========== 第六步：记录到数据库 ==========
        let copy_trade = CopyTrade {
            id: None,
            tx_hash: event.tx_hash.clone(),
            target_wallet: self.config.target_wallet.clone(),
            market_slug: Some(market_info.slug),
            token_id: event.token_id.clone(),
            token_side: event.token_side.clone(),
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

    /// 风控检查（三道防线）
    fn run_risk_checks(&self, event: &TradeEvent, market_info: &MarketInfo) -> Result<CheckResult> {
        // 检查1：残余时间拦截
        if market_info.remaining_time < self.config.min_remaining_time as i64 {
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
                reason: "价格数据无效（数量为0）".to_string(),
            });
        }

        // 检查3：金额限制
        if self.config.copy_trade_amount <= 0.0 {
            return Ok(CheckResult {
                passed: false,
                reason: "跟单金额无效".to_string(),
            });
        }

        // 检查4：入场价过滤（>= 0.90 是负EV）
        let target_price = event.taker_amount as f64 / event.maker_amount as f64;
        if target_price >= 0.90 {
            return Ok(CheckResult {
                passed: false,
                reason: format!("入场价 {:.3} >= 0.90，负EV区间", target_price),
            });
        }

        Ok(CheckResult {
            passed: true,
            reason: String::new(),
        })
    }

    /// 计算滑点
    fn calculate_slippage(&self, event: &TradeEvent, orderbook: &OrderBook) -> Result<f64> {
        // 目标入场价（大户的入场价）
        let target_price = if event.maker_amount > 0 {
            event.taker_amount as f64 / event.maker_amount as f64
        } else {
            0.0
        };

        // 当前卖一价（我们需要支付的价格）
        let current_ask = orderbook.best_ask;

        // 滑点 = (current_ask - target_price) / target_price
        if target_price > 0.0 && target_price < 1.0 {
            let slippage = (current_ask - target_price).abs() / target_price;
            Ok(slippage)
        } else {
            Ok(1.0) // 无法计算，默认100%滑点
        }
    }

    /// 更新跳过原因
    async fn update_skip_reason(&self, event: &TradeEvent, reason: &str) -> Result<()> {
        // 这里可以更新 target_trades 表的 follow_reason 字段
        tracing::debug!("📝 记录跳过原因: {}", reason);
        Ok(())
    }
}
