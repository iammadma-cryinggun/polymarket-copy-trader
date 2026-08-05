//! 数据库管理

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// 跟单记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyTrade {
    pub id: Option<i64>,
    pub tx_hash: String,
    pub target_wallet: String,
    pub market_slug: Option<String>,
    pub token_id: String,
    pub token_side: String,
    pub entry_price: f64,
    pub size: f64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub result: Option<String>,
    pub pnl: Option<f64>,
}

/// 目标交易记录
#[derive(Debug, Clone)]
pub struct TargetTrade {
    pub tx_hash: String,
    pub target_wallet: String,
    pub market_slug: Option<String>,
    pub token_id: String,
    pub token_side: String,
    pub entry_price: f64,
    pub size: f64,
    pub detected_at: DateTime<Utc>,
    pub followed: bool,
    pub follow_reason: Option<String>,
}

/// 统计数据
#[derive(Debug, Clone)]
pub struct Stats {
    pub total_trades: i64,
    pub followed_trades: i64,
    pub skipped_trades: i64,
}

/// 数据库管理器（线程安全）
#[derive(Clone)]
pub struct Database {
    inner: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            inner: Arc::new(Mutex::new(conn)),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<()> {
        let conn = self.inner.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS copy_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tx_hash TEXT NOT NULL,
                target_wallet TEXT NOT NULL,
                market_slug TEXT,
                token_id TEXT NOT NULL,
                token_side TEXT NOT NULL,
                entry_price REAL NOT NULL,
                size REAL NOT NULL,
                status TEXT DEFAULT 'pending',
                created_at TEXT NOT NULL,
                result TEXT,
                pnl REAL
            );

            CREATE TABLE IF NOT EXISTS target_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tx_hash TEXT NOT NULL UNIQUE,
                target_wallet TEXT NOT NULL,
                market_slug TEXT,
                token_id TEXT NOT NULL,
                token_side TEXT NOT NULL,
                entry_price REAL NOT NULL,
                size REAL NOT NULL,
                detected_at TEXT NOT NULL,
                followed BOOLEAN DEFAULT 0,
                follow_reason TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_target_trades_wallet ON target_trades(target_wallet);
            CREATE INDEX IF NOT EXISTS idx_target_trades_detected ON target_trades(detected_at);
            "#,
        )?;
        Ok(())
    }

    pub fn insert_target_trade(&self, trade: &TargetTrade) -> Result<i64> {
        let conn = self.inner.lock().unwrap();
        let id = conn.query_row(
            r#"
            INSERT INTO target_trades (
                tx_hash, target_wallet, market_slug, token_id, token_side,
                entry_price, size, detected_at, followed, follow_reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            RETURNING id
            "#,
            params![
                trade.tx_hash,
                trade.target_wallet,
                trade.market_slug,
                trade.token_id,
                trade.token_side,
                trade.entry_price,
                trade.size,
                trade.detected_at.to_rfc3339(),
                trade.followed,
                trade.follow_reason,
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn target_trade_exists(&self, tx_hash: &str) -> Result<bool> {
        let conn = self.inner.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM target_trades WHERE tx_hash = ?1",
            params![tx_hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn insert_copy_trade(&self, trade: &CopyTrade) -> Result<i64> {
        let conn = self.inner.lock().unwrap();
        let id = conn.query_row(
            r#"
            INSERT INTO copy_trades (
                tx_hash, target_wallet, market_slug, token_id, token_side,
                entry_price, size, status, created_at, result, pnl
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            RETURNING id
            "#,
            params![
                trade.tx_hash,
                trade.target_wallet,
                trade.market_slug,
                trade.token_id,
                trade.token_side,
                trade.entry_price,
                trade.size,
                trade.status,
                trade.created_at.to_rfc3339(),
                trade.result,
                trade.pnl,
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn get_stats(&self) -> Result<Stats> {
        let conn = self.inner.lock().unwrap();
        let total_trades: i64 =
            conn.query_row("SELECT COUNT(*) FROM target_trades", [], |row| row.get(0))?;

        let followed_trades: i64 = conn.query_row(
            "SELECT COUNT(*) FROM target_trades WHERE followed = 1",
            [],
            |row| row.get(0),
        )?;

        let skipped_trades: i64 = conn.query_row(
            "SELECT COUNT(*) FROM target_trades WHERE followed = 0",
            [],
            |row| row.get(0),
        )?;

        Ok(Stats {
            total_trades,
            followed_trades,
            skipped_trades,
        })
    }

    /// 当日累计投入（USDC，按份数 × 成交价求和；created_at 为 UTC ISO 文本）
    pub fn today_spend(&self) -> Result<f64> {
        let conn = self.inner.lock().unwrap();
        let sum: f64 = conn.query_row(
            "SELECT COALESCE(SUM(size * entry_price), 0) FROM copy_trades
             WHERE status = 'filled' AND date(created_at) = date('now')",
            [],
            |row| row.get(0),
        )?;
        Ok(sum)
    }
}
