//! 主入口

mod abi;
mod api;
mod config;
mod db;
mod listener;
mod trader;

use crate::config::Config;
use crate::db::Database;
use crate::listener::Listener;
use crate::trader::CopyTrader;
use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    watch_only: bool,

    #[arg(short, long)]
    stats: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("polymarket_copy_trader=info".parse()?)
        )
        .init();

    tracing::info!("🚀 Polymarket Copy Trader 启动中...");

    let args = Args::parse();

    let config = Config::from_env()?;
    tracing::info!("✅ 配置加载成功");
    tracing::info!("🎯 目标钱包: {}", config.target_wallet);
    tracing::info!("💰 跟单金额: ${:.2}", config.copy_trade_amount);

    // Database 已经是线程安全的（内部用 Arc<Mutex>）
    let db = Database::new(&config.db_path)?;
    tracing::info!("✅ 数据库初始化成功");

    if args.stats {
        let stats = db.get_stats()?;
        println!("\n📊 跟单统计");
        println!("──────────────────────────");
        println!("监控到的交易数: {}", stats.total_trades);
        println!("已跟单: {}", stats.followed_trades);
        println!("已跳过: {}", stats.skipped_trades);
        return Ok(());
    }

    let (event_sender, mut event_receiver) = mpsc::channel::<listener::TradeEvent>(100);

    let listener = Listener::new(config.clone(), db.clone(), event_sender);
    let trader = Arc::new(CopyTrader::new(config.clone(), db.clone()));

    let listener_handle = tokio::spawn(async move {
        if let Err(e) = listener.run().await {
            tracing::error!("❌ 监听器错误: {}", e);
        }
    });

    tracing::info!("✅ 监听器启动成功");
    tracing::info!("📡 等待目标钱包交易...\n");

    if args.watch_only {
        tracing::info!("👁️ 监控模式");

        while let Some(event) = event_receiver.recv().await {
            tracing::info!(
                "👁️ [监控] TX: {} | {} | Token: {}",
                &event.tx_hash[..20],
                event.side,
                &event.token_id[..20]
            );
        }
    } else {
        tracing::info!("🤖 跟单模式");

        while let Some(event) = event_receiver.recv().await {
            let trader_clone = trader.clone();
            let event_clone = event.clone();

            tokio::spawn(async move {
                if let Err(e) = trader_clone.handle_trade_event(event_clone).await {
                    tracing::error!("❌ 跟单失败: {}", e);
                }
            });
        }
    }

    listener_handle.await?;

    Ok(())
}
