//! Polymarket API 客户端
//!
//! 封装 CLOB API 调用

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Polymarket 客户端
pub struct PolymarketClient {
    http_client: Client,
    clob_url: String,
    gamma_url: String,
    private_key: Option<String>,
    paper_trading: bool,
}

/// 市场信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub slug: String,
    pub question: String,
    pub condition_id: String,
    pub yes_token: String,
    pub no_token: String,
    pub strike_price: Option<f64>,
    pub end_time: i64,
    pub remaining_time: i64,
}

/// 盘口信息
#[derive(Debug, Clone)]
pub struct OrderBook {
    pub best_bid: f64,
    pub best_ask: f64,
    pub spread: f64,
}

/// 下单结果
#[derive(Debug, Clone)]
pub struct TradeResult {
    pub order_id: String,
    pub success: bool,
    pub filled_size: f64,
    pub status: Option<String>,
    pub message: String,
}

impl PolymarketClient {
    /// 创建客户端
    pub fn new(private_key: Option<String>, paper_trading: bool) -> Self {
        Self {
            http_client: Client::new(),
            clob_url: "https://clob.polymarket.com".to_string(),
            gamma_url: "https://gamma-api.polymarket.com".to_string(),
            private_key,
            paper_trading,
        }
    }

    /// 获取盘口价格
    pub async fn fetch_best_prices(&self, token_id: &str) -> Result<OrderBook> {
        let buy_url = format!("{}/price?token_id={}&side=BUY", self.clob_url, token_id);
        let sell_url = format!("{}/price?token_id={}&side=SELL", self.clob_url, token_id);

        let (bid_result, ask_result) = tokio::join!(
            self.fetch_single_price(&buy_url),
            self.fetch_single_price(&sell_url)
        );

        let best_bid = bid_result.unwrap_or(0.0);
        let best_ask = ask_result.unwrap_or(0.0);

        if best_bid > 0.0 || best_ask > 0.0 {
            Ok(OrderBook {
                best_bid,
                best_ask,
                spread: best_ask - best_bid,
            })
        } else {
            anyhow::bail!("无法获取价格")
        }
    }

    async fn fetch_single_price(&self, url: &str) -> Result<f64> {
        let response = self.http_client
            .get(url)
            .send()
            .await
            .context("HTTP请求失败")?;

        let json: serde_json::Value = response.json().await.context("解析JSON失败")?;

        json.get("price")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .context("价格字段不存在")
    }

    /// 根据 Token ID 获取市场信息
    pub async fn get_market_by_token(&self, token_id: &str) -> Result<MarketInfo> {
        let url = format!("{}/markets?token_id={}", self.gamma_url, token_id);

        let response = self.http_client
            .get(&url)
            .send()
            .await
            .context("请求Gamma API失败")?;

        let markets: Vec<serde_json::Value> = response.json().await.context("解析市场数据失败")?;

        if markets.is_empty() {
            anyhow::bail!("未找到市场");
        }

        let market = &markets[0];

        let slug = market["slug"].as_str().unwrap_or("unknown").to_string();
        let question = market["question"].as_str().unwrap_or("").to_string();
        let condition_id = market["conditionId"].as_str().unwrap_or("").to_string();

        let tokens = market["tokens"].as_array().context("无token信息")?;
        let yes_token = tokens.get(0).and_then(|t| t["token_id"].as_str()).unwrap_or("").to_string();
        let no_token = tokens.get(1).and_then(|t| t["token_id"].as_str()).unwrap_or("").to_string();

        let end_date = market["endDate"].as_str().unwrap_or("");
        let end_time = chrono::DateTime::parse_from_rfc3339(end_date)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);

        let now = chrono::Utc::now().timestamp();
        let remaining_time = (end_time - now).max(0);

        Ok(MarketInfo {
            slug,
            question,
            condition_id,
            yes_token,
            no_token,
            strike_price: None,
            end_time,
            remaining_time,
        })
    }

    /// 下单
    pub async fn place_order(
        &self,
        token_id: &str,
        side: &str,
        price: f64,
        size: f64,
    ) -> Result<TradeResult> {
        if self.paper_trading {
            let is_buy = side.eq_ignore_ascii_case("BUY");
            let filled_shares = if is_buy && price > 0.0 { size / price } else { size };

            tracing::info!(
                "📄 [Paper Trading] {} {:.2} shares @ ${:.3}",
                side,
                filled_shares,
                price
            );

            return Ok(TradeResult {
                order_id: format!("paper_{}", uuid::Uuid::new_v4()),
                success: true,
                filled_size: filled_shares,
                status: Some("filled".to_string()),
                message: "Paper trade".to_string(),
            });
        }

        // 真实交易需要私钥
        let _private_key = self.private_key.as_ref().context("未配置私钥")?;

        // TODO: 实现真实下单逻辑
        // 这里暂时返回 paper trading 结果
        tracing::warn!("⚠️ 真实交易暂未实现，使用 Paper Trading 模式");

        Ok(TradeResult {
            order_id: format!("mock_{}", uuid::Uuid::new_v4()),
            success: true,
            filled_size: size / price,
            status: Some("mock".to_string()),
            message: "Mock trade (real trading not implemented)".to_string(),
        })
    }
}
