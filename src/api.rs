//! Polymarket API 客户端
//!
//! 封装 CLOB API 调用

use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use alloy::signers::{Signer, local::LocalSigner};
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::clob::types::{Amount, Side, SignatureType};
use polymarket_client_sdk_v2::clob::{Client, Config};
use polymarket_client_sdk_v2::types::{Decimal, U256};
use polymarket_client_sdk_v2::POLYGON;

/// Polymarket 客户端
pub struct PolymarketClient {
    http_client: HttpClient,
    clob_url: String,
    gamma_url: String,
    private_key: Option<String>,
    paper_trading: bool,
    /// V2 签名类型（0=EOA, 1=PolyProxy, 2=GnosisSafe, 3=Poly1271）
    signature_type: SignatureType,
    /// 资金地址（代理/Safe/存款钱包时设置；EOA 留空）
    funder: Option<String>,
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
    pub fn new(
        private_key: Option<String>,
        paper_trading: bool,
        signature_type: SignatureType,
        funder: Option<String>,
    ) -> Self {
        Self {
            http_client: HttpClient::new(),
            clob_url: "https://clob.polymarket.com".to_string(),
            gamma_url: "https://gamma-api.polymarket.com".to_string(),
            private_key,
            paper_trading,
            signature_type,
            funder,
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

    /// 根据 Token ID 获取市场信息。
    ///
    /// Gamma `/markets` 官方查询参数为 `clob_token_ids`（不是 `token_id`）。
    /// 之前的 `?token_id=` 会被接口忽略，返回默认市场列表，且当前结构下 `tokens`
    /// 数组未必存在，从而报“无token信息”。这里改用 `clob_token_ids`，并对
    /// `tokens` 数组与 `clobTokenIds`(JSON 字符串) 两种字段做兼容解析。
    pub async fn get_market_by_token(&self, token_id: &str) -> Result<MarketInfo> {
        let url = format!(
            "{}/markets?clob_token_ids={}",
            self.gamma_url, token_id
        );

        let response = self.http_client
            .get(&url)
            .send()
            .await
            .context("请求Gamma API失败")?;

        let markets: Vec<serde_json::Value> = response.json().await.context(format!(
            "解析市场数据失败 (token_id={})",
            &token_id[..std::cmp::min(20, token_id.len())]
        ))?;

        // 优先挑选包含该 token 的市场；找不到就退回首个结果
        let mine = token_id.to_string();
        let market = markets
            .iter()
            .find(|m| self.market_contains_token(m, &mine))
            .or_else(|| markets.first())
            .context("未找到市场")?;

        let slug = market["slug"].as_str().unwrap_or("unknown").to_string();
        let question = market["question"].as_str().unwrap_or("").to_string();
        let condition_id = market["conditionId"].as_str().unwrap_or("").to_string();

        // 兼容两种 token 字段：`tokens` 数组 或 `clobTokenIds`(JSON 字符串)
        let mut yes_token = String::new();
        let mut no_token = String::new();
        if let Some(tokens) = market["tokens"].as_array() {
            let mut iter = tokens
                .iter()
                .map(|t| t["token_id"].as_str().unwrap_or("").to_string());
            yes_token = iter.next().unwrap_or_default();
            no_token = iter.next().unwrap_or_default();
        } else if let Some(ids) = market["clobTokenIds"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        {
            yes_token = ids.get(0).cloned().unwrap_or_default();
            no_token = ids.get(1).cloned().unwrap_or_default();
        }

        // 结束时间：兼容多种字段名，取第一个能解析的
        let end_time = [
            "endDateIso",
            "end_date_iso",
            "endDate",
            "end_date",
            "endTime",
            "end_time",
        ]
        .iter()
        .find_map(|f| {
            market[f].as_str().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
            })
        })
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

    /// 判断某市场是否包含指定 token（通过 `tokens` 数组或 `clobTokenIds` 字符串）
    fn market_contains_token(&self, market: &serde_json::Value, token_id: &str) -> bool {
        if let Some(tokens) = market["tokens"].as_array() {
            if tokens
                .iter()
                .any(|t| t["token_id"].as_str() == Some(token_id))
            {
                return true;
            }
        }
        if let Some(ids) = market["clobTokenIds"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        {
            return ids.iter().any(|id| id == token_id);
        }
        false
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

        // 真实交易
        let private_key = self.private_key.as_ref().context("未配置私钥")?;

        tracing::info!(
            "📝 [真实交易] Token: {} | {} | 价格: {:.3} | 数量: {:.2}",
            &token_id[..std::cmp::min(20, token_id.len())],
            side,
            price,
            size
        );

        match self.execute_real_trade(token_id, side, price, size, private_key).await {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!("❌ 下单失败: {}", e);
                Ok(TradeResult {
                    order_id: String::new(),
                    success: false,
                    filled_size: 0.0,
                    status: Some("failed".to_string()),
                    message: e.to_string(),
                })
            }
        }
    }

    /// 执行真实交易
    async fn execute_real_trade(
        &self,
        token_id: &str,
        side: &str,
        price: f64,
        size_usdc: f64,
        private_key: &str,
    ) -> Result<TradeResult> {
        // 创建签名器
        let signer = LocalSigner::from_str(private_key)?
            .with_chain_id(Some(POLYGON));

        // 创建 CLOB 客户端（CLOB V2；V1 SDK 已无法下单）
        let config = Config::builder().use_server_time(true).build();
        let mut auth_builder = Client::new(&self.clob_url, config)?
            .authentication_builder(&signer)
            .signature_type(self.signature_type);

        if let Some(funder) = &self.funder {
            let funder = funder.parse::<alloy::primitives::Address>()?;
            auth_builder = auth_builder.funder(funder);
        }

        let client = auth_builder.authenticate().await?;

        // 解析 token_id
        let token_id_u256 = U256::from_str(token_id)?;

        // 转换金额 (size_usdc 是 USDC 金额)
        let amount = Amount::usdc(Decimal::from_str_exact(&size_usdc.to_string())?)?;

        // 确定买卖方向
        let trade_side = if side.eq_ignore_ascii_case("BUY") {
            Side::Buy
        } else {
            Side::Sell
        };

        tracing::info!(
            "🎯 下单参数: token_id={}, side={:?}, amount={:.2} USDC",
            &token_id[..20],
            trade_side,
            size_usdc
        );

        // 创建市价单
        let market_order = client
            .market_order()
            .token_id(token_id_u256)
            .amount(amount)
            .side(trade_side)
            .build()
            .await?;

        // 签名订单
        let signed_order = client.sign(&signer, market_order).await?;

        // 提交订单
        let result = client.post_order(signed_order).await?;

        tracing::info!(
            "✅ 下单成功! order_id={}, success={}",
            result.order_id,
            result.success
        );

        Ok(TradeResult {
            order_id: result.order_id,
            success: result.success,
            filled_size: size_usdc / price, // 估算成交股数
            status: Some("submitted".to_string()),
            message: "Order submitted successfully".to_string(),
        })
    }

    /// 检查余额
    pub async fn check_balance(&self) -> Result<f64> {
        let private_key = self.private_key.as_ref().context("未配置私钥")?;

        let signer = LocalSigner::from_str(private_key)?
            .with_chain_id(Some(POLYGON));

        let config = Config::builder().use_server_time(true).build();
        let mut auth_builder = Client::new(&self.clob_url, config)?
            .authentication_builder(&signer)
            .signature_type(self.signature_type);

        if let Some(funder) = &self.funder {
            let funder = funder.parse::<alloy::primitives::Address>()?;
            auth_builder = auth_builder.funder(funder);
        }

        let client = auth_builder.authenticate().await?;

        let balance_info = client.balance_allowance(BalanceAllowanceRequest::default()).await?;

        // balance 已经是 Decimal，直接转换为 f64
        let balance = balance_info.balance.to_string().parse::<f64>().unwrap_or(0.0);

        Ok(balance)
    }
}
