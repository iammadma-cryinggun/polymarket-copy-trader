//! 主入口

mod abi;
mod api;
mod config;
mod db;
mod listener;
mod redeem;
mod trader;

use crate::config::Config;
use crate::db::Database;
use crate::listener::Listener;
use crate::redeem::{RedeemConfig, Redeemer};
use crate::trader::CopyTrader;
use anyhow::Result;
use clap::Parser;
use fs2::FileExt;
use std::fs::File;
use std::io::Write;
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

/// 单例锁：防止多个实例同时运行。
/// 多个实例共用同一 RPC Key 会占满并发连接数，是 429 限流的常见根因。
/// 返回的 File 必须保持存活直到进程结束（进程退出/崩溃时系统自动释放锁）。
fn acquire_single_instance_lock() -> Option<File> {
    let path =
        std::env::var("SINGLE_INSTANCE_LOCK").unwrap_or_else(|_| ".copy-trader.lock".to_string());

    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("⚠️ 创建单例锁文件失败（{}: {}），跳过单例检查", path, e);
            return None;
        }
    };

    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = writeln!(&file, "pid={}", std::process::id());
            Some(file)
        }
        Err(_) => {
            tracing::error!(
                "❌ 检测到另一个实例正在运行（锁文件 {} 被占用）！\n\
                 多个实例同时连接会触发 RPC 429 限流。请先停止旧进程（如 Zeabur 重复部署）再启动。",
                path
            );
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 默认 info（避免每个市场成交都刷屏）；需要观察全量 OrderFilled 时设 RUST_LOG=debug
    let filter = if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        tracing_subscriber::EnvFilter::new("polymarket_copy_trader=info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("🚀 Polymarket Copy Trader 启动中...");

    let args = Args::parse();

    // 单例锁（保持存活直到进程结束）
    let _instance_lock = acquire_single_instance_lock();

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

    // 自动赎回后台任务（仅在真实交易模式启用）
    if !args.watch_only {
        let redeem_config = RedeemConfig {
            enabled: config.redeem_enabled,
            scan_interval_secs: config.redeem_scan_interval,
            min_redeem_amount: config.redeem_min_amount,
            polygon_rpc_url: config.polygon_rpc_url.clone().unwrap_or_default(),
        };

        if redeem_config.enabled {
            match Redeemer::new(&config.private_key, redeem_config) {
                Ok(redeemer) => {
                    tracing::info!("🔄 自动赎回已启用，启动后台赎回任务");
                    tokio::spawn(async move {
                        if let Err(e) = redeemer.run().await {
                            tracing::error!("❌ 自动赎回错误: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("❌ 自动赎回初始化失败: {}", e);
                }
            }
        } else {
            tracing::info!("⏸️ 自动赎回未启用（设置 REDEEM_ENABLED=true 开启）");
        }
    }

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
