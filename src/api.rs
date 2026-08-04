//! Polymarket API 客户端
//!
//! 复用 btc_doomsday_rust_final 的 API 调用逻辑

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

// ==========================================
// Polymarket SDK 类型
// ==========================================

use polymarket_client_sdk::auth::LocalSigner;
use polymarket_client_sdk::auth::Signer;
use polymarket_client_sdk::clob::types::Side;
use polymarket_client_sdk::types::{Decimal, U256};

/// Polymarket CLOB API 客户端
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

        // 并发获取
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

    /// 获取单个价格
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
        // 使用 Gamma API 查询市场
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

        // 解析市场信息
        let slug = market["slug"].as_str().unwrap_or("unknown").to_string();
        let question = market["question"].as_str().unwrap_or("").to_string();
        let condition_id = market["conditionId"].as_str().unwrap_or("").to_string();

        // 获取 token IDs
        let tokens = market["tokens"].as_array().context("无token信息")?;
        let yes_token = tokens.get(0).and_then(|t| t["token_id"].as_str()).unwrap_or("").to_string();
        let no_token = tokens.get(1).and_then(|t| t["token_id"].as_str()).unwrap_or("").to_string();

        // 获取结束时间
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
        side: &str,  // "BUY" or "SELL"
        price: f64,
        size: f64,
    ) -> Result<TradeResult> {
        // Paper trading 模式
        if self.paper_trading {
            let is_buy = side.eq_ignore_ascii_case("BUY");
            let filled_shares = if is_buy && price > 0.0 { size / price } else { size };

            tracing::info!(
                "📄 [Paper Trading] {} {} shares @ ${:.3}",
                side,
                filled_shares,
                price
            );

            return Ok(TradeResult {
                order_id: format!("paper_{}", uuid::Uuid::new_v4()),
                success: true,
                filled_size: filled_shares,
                status: Some("filled".to_string()),
                message: "Paper trade executed".to_string(),
            });
        }

        // 真实交易
        let private_key = self.private_key.as_ref()
            .context("未配置私钥")?;

        // 创建签名器
        let signer = LocalSigner::from_str(private_key)
            .context("无效的私钥")?
            .with_chain_id(Some(137u64));

        // 创建 CLOB 客户端
        let clob_client = polymarket_client_sdk::clob::Client::new(
            &self.clob_url,
            polymarket_client_sdk::clob::Config::default(),
        )
        .context("创建CLOB客户端失败")?
        .authentication_builder(&signer)
        .authenticate()
        .await
        .context("认证失败")?;

        // 解析价格和数量
        let price_dec: Decimal = price.to_string().parse().context("无效价格")?;
        let size_dec: Decimal = size.to_string().parse().context("无效数量")?;

        let is_buy = side.eq_ignore_ascii_case("BUY");

        // 创建订单
        let size_amount = if is_buy {
            polymarket_client_sdk::clob::types::Amount::usdc(size_dec)
                .context("创建USDC金额失败")?
        } else {
            polymarket_client_sdk::clob::types::Amount::shares(size_dec)
                .context("创建shares金额失败")?
        };

        let order_builder = clob_client.market_order()
            .token_id(token_id.parse::<U256>().context("无效token ID")?)
            .side(if is_buy { Side::Buy } else { Side::Sell })
            .amount(size_amount)
            .price(price_dec);

        let order = order_builder.build().await.context("构建订单失败")?;

        tracing::info!("📝 订单构建完成: {:?}", order);

        // 签名订单
        let signed_order = clob_client.sign(&signer, order).await.context("签名失败")?;

        // 发送订单
        let resp = clob_client.post_order(signed_order).await.context("发送订单失败")?;

        tracing::info!("✅ 订单已发送: {:?}", resp);

        // 解析成交结果
        let filled = if is_buy {
            resp.taking_amount.to_string().parse().unwrap_or(size)
        } else {
            resp.making_amount.to_string().parse().unwrap_or(size)
        };

        Ok(TradeResult {
            order_id: resp.order_id.clone(),
            success: resp.success,
            filled_size: filled,
            status: Some(format!("{:?}", resp.status)),
            message: "Order executed".to_string(),
        })
    }
}
