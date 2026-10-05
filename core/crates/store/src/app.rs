//! App state in SQLite: settings, watchlists, workspaces, history, alerts,
//! portfolios, ASK transcripts, and non-secret provider settings.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{StoreError, StoreResult};

/// Ordered migrations; index + 1 is the schema version.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r"
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE watchlists (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE watchlist_items (
        watchlist_id INTEGER NOT NULL REFERENCES watchlists(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        security TEXT NOT NULL,
        PRIMARY KEY (watchlist_id, security)
    );
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE command_history (
        id INTEGER PRIMARY KEY,
        panel TEXT NOT NULL,
        command TEXT NOT NULL,
        at INTEGER NOT NULL
    );
    CREATE INDEX command_history_panel ON command_history(panel, id);
    CREATE TABLE alerts (
        id INTEGER PRIMARY KEY,
        json TEXT NOT NULL,
        enabled INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        last_fired_at INTEGER
    );
    CREATE TABLE alert_events (
        id INTEGER PRIMARY KEY,
        alert_id INTEGER NOT NULL,
        fired_at INTEGER NOT NULL,
        message TEXT NOT NULL
    );
    CREATE TABLE portfolios (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        base_currency TEXT NOT NULL
    );
    CREATE TABLE transactions (
        id INTEGER PRIMARY KEY,
        portfolio_id INTEGER NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
        security TEXT NOT NULL,
        trade_date TEXT NOT NULL,
        quantity REAL NOT NULL,
        price REAL NOT NULL,
        fees REAL NOT NULL DEFAULT 0,
        note TEXT
    );
    CREATE TABLE ask_sessions (id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at INTEGER NOT NULL);
    CREATE TABLE ask_turns (
        id INTEGER PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES ask_sessions(id) ON DELETE CASCADE,
        json TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE provider_settings (provider TEXT PRIMARY KEY, json TEXT NOT NULL);
    ",
];

fn migrate(conn: &Connection) -> StoreResult<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql).map_err(|e| StoreError::Migration { version, message: e.to_string() })?;
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))?;
        tx.commit()?;
        tracing::info!(version, "sqlite migrated");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watchlist {
    pub id: i64,
    pub name: String,
    pub securities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRecord {
    pub id: i64,
    pub json: String,
    pub enabled: bool,
    pub created_at: i64,
    pub last_fired_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertEvent {
    pub id: i64,
    pub alert_id: i64,
    pub fired_at: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Portfolio {
    pub id: i64,
    pub name: String,
    pub base_currency: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: i64,
    pub portfolio_id: i64,
    pub security: String,
    /// `YYYY-MM-DD`.
    pub trade_date: String,
    /// Positive = buy, negative = sell.
    pub quantity: f64,
    pub price: f64,
    pub fees: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskSessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
}

/// SQLite-backed app state. One connection behind a mutex: app-state
/// operations are small and infrequent.
pub struct AppStore {
    conn: Mutex<Connection>,
}

impl AppStore {
    pub fn open(path: &Path) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
        migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.conn.lock().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0)
    }

    // --- settings -------------------------------------------------------

    pub fn setting(&self, key: &str) -> StoreResult<Option<String>> {
        Ok(self.conn.lock().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> StoreResult<()> {
        self.conn.lock().execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // --- watchlists -----------------------------------------------------

    pub fn watchlists(&self) -> StoreResult<Vec<Watchlist>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name FROM watchlists ORDER BY id")?;
        let heads: Vec<(i64, String)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let mut items = conn.prepare("SELECT security FROM watchlist_items WHERE watchlist_id = ?1 ORDER BY position")?;
        let mut out = Vec::with_capacity(heads.len());
        for (id, name) in heads {
            let securities = items.query_map([id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            out.push(Watchlist { id, name, securities });
        }
        Ok(out)
    }

    pub fn create_watchlist(&self, name: &str, now: i64) -> StoreResult<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO watchlists(name, created_at) VALUES (?1, ?2)", params![name, now])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn rename_watchlist(&self, id: i64, name: &str) -> StoreResult<()> {
        self.conn.lock().execute("UPDATE watchlists SET name = ?2 WHERE id = ?1", params![id, name])?;
        Ok(())
    }

    pub fn delete_watchlist(&self, id: i64) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM watchlists WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Replaces the contents (order preserved, duplicates dropped).
    pub fn set_watchlist_items(&self, id: i64, securities: &[String]) -> StoreResult<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM watchlist_items WHERE watchlist_id = ?1", [id])?;
        {
            let mut ins = tx.prepare("INSERT OR IGNORE INTO watchlist_items(watchlist_id, position, security) VALUES (?1, ?2, ?3)")?;
            for (pos, s) in securities.iter().enumerate() {
                ins.execute(params![id, pos as i64, s])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // --- workspaces -----------------------------------------------------

    pub fn save_workspace(&self, id: &str, name: &str, json: &str, now: i64) -> StoreResult<()> {
        self.conn.lock().execute(
            "INSERT INTO workspaces(id, name, json, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, json = excluded.json, updated_at = excluded.updated_at",
            params![id, name, json, now],
        )?;
        Ok(())
    }

    pub fn workspace(&self, id: &str) -> StoreResult<Option<(String, String)>> {
        Ok(self
            .conn
            .lock()
            .query_row("SELECT name, json FROM workspaces WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    pub fn workspaces(&self) -> StoreResult<Vec<WorkspaceSummary>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, updated_at FROM workspaces ORDER BY updated_at DESC")?;
        let rows = stmt
            .query_map([], |r| Ok(WorkspaceSummary { id: r.get(0)?, name: r.get(1)?, updated_at: r.get(2)? }))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn delete_workspace(&self, id: &str) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        Ok(())
    }

    // --- command history ------------------------------------------------

    pub fn push_history(&self, panel: &str, command: &str, now: i64) -> StoreResult<()> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO command_history(panel, command, at) VALUES (?1, ?2, ?3)", params![panel, command, now])?;
        // Keep the most recent 500 per panel.
        conn.execute(
            "DELETE FROM command_history WHERE panel = ?1 AND id NOT IN
               (SELECT id FROM command_history WHERE panel = ?1 ORDER BY id DESC LIMIT 500)",
            [panel],
        )?;
        Ok(())
    }

    /// Most recent first.
    pub fn history(&self, panel: &str, limit: usize) -> StoreResult<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT command FROM command_history WHERE panel = ?1 ORDER BY id DESC LIMIT ?2")?;
        let rows = stmt.query_map(params![panel, limit as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    // --- alerts ---------------------------------------------------------

    pub fn alerts(&self) -> StoreResult<Vec<AlertRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, json, enabled, created_at, last_fired_at FROM alerts ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AlertRecord {
                    id: r.get(0)?,
                    json: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                    created_at: r.get(3)?,
                    last_fired_at: r.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Inserts when `id` is None, else updates. Returns the id.
    pub fn upsert_alert(&self, id: Option<i64>, json: &str, enabled: bool, now: i64) -> StoreResult<i64> {
        let conn = self.conn.lock();
        if let Some(id) = id {
            conn.execute("UPDATE alerts SET json = ?2, enabled = ?3 WHERE id = ?1", params![id, json, i64::from(enabled)])?;
            Ok(id)
        } else {
            conn.execute(
                "INSERT INTO alerts(json, enabled, created_at) VALUES (?1, ?2, ?3)",
                params![json, i64::from(enabled), now],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }

    pub fn delete_alert(&self, id: i64) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM alerts WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn record_alert_fired(&self, id: i64, at: i64, message: &str) -> StoreResult<()> {
        let conn = self.conn.lock();
        conn.execute("UPDATE alerts SET last_fired_at = ?2 WHERE id = ?1", params![id, at])?;
        conn.execute("INSERT INTO alert_events(alert_id, fired_at, message) VALUES (?1, ?2, ?3)", params![id, at, message])?;
        Ok(())
    }

    pub fn alert_events(&self, limit: usize) -> StoreResult<Vec<AlertEvent>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, alert_id, fired_at, message FROM alert_events ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |r| Ok(AlertEvent { id: r.get(0)?, alert_id: r.get(1)?, fired_at: r.get(2)?, message: r.get(3)? }))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    // --- portfolios -----------------------------------------------------

    pub fn portfolios(&self) -> StoreResult<Vec<Portfolio>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, base_currency FROM portfolios ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok(Portfolio { id: r.get(0)?, name: r.get(1)?, base_currency: r.get(2)? }))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn create_portfolio(&self, name: &str, base_currency: &str) -> StoreResult<i64> {
        let conn = self.conn.lock();
        conn.execute("INSERT INTO portfolios(name, base_currency) VALUES (?1, ?2)", params![name, base_currency])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_portfolio(&self, id: i64) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM portfolios WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn add_transaction(&self, t: &Transaction) -> StoreResult<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO transactions(portfolio_id, security, trade_date, quantity, price, fees, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![t.portfolio_id, t.security, t.trade_date, t.quantity, t.price, t.fees, t.note],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_transaction(&self, id: i64) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM transactions WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn transactions(&self, portfolio_id: i64) -> StoreResult<Vec<Transaction>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, portfolio_id, security, trade_date, quantity, price, fees, note
             FROM transactions WHERE portfolio_id = ?1 ORDER BY trade_date, id",
        )?;
        let rows = stmt
            .query_map([portfolio_id], |r| {
                Ok(Transaction {
                    id: r.get(0)?,
                    portfolio_id: r.get(1)?,
                    security: r.get(2)?,
                    trade_date: r.get(3)?,
                    quantity: r.get(4)?,
                    price: r.get(5)?,
                    fees: r.get(6)?,
                    note: r.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    // --- ASK ------------------------------------------------------------

    pub fn create_ask_session(&self, id: &str, title: &str, now: i64) -> StoreResult<()> {
        self.conn.lock().execute(
            "INSERT OR IGNORE INTO ask_sessions(id, title, created_at) VALUES (?1, ?2, ?3)",
            params![id, title, now],
        )?;
        Ok(())
    }

    pub fn append_ask_turn(&self, session_id: &str, json: &str, now: i64) -> StoreResult<()> {
        self.conn.lock().execute(
            "INSERT INTO ask_turns(session_id, json, created_at) VALUES (?1, ?2, ?3)",
            params![session_id, json, now],
        )?;
        Ok(())
    }

    pub fn ask_sessions(&self, limit: usize) -> StoreResult<Vec<AskSessionSummary>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, title, created_at FROM ask_sessions ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |r| Ok(AskSessionSummary { id: r.get(0)?, title: r.get(1)?, created_at: r.get(2)? }))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn ask_turns(&self, session_id: &str) -> StoreResult<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT json FROM ask_turns WHERE session_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([session_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    // --- provider settings (never secrets) ------------------------------

    pub fn provider_settings(&self, provider: &str) -> StoreResult<Option<String>> {
        Ok(self
            .conn
            .lock()
            .query_row("SELECT json FROM provider_settings WHERE provider = ?1", [provider], |r| r.get(0))
            .optional()?)
    }

    pub fn set_provider_settings(&self, provider: &str, json: &str) -> StoreResult<()> {
        self.conn.lock().execute(
            "INSERT INTO provider_settings(provider, json) VALUES (?1, ?2)
             ON CONFLICT(provider) DO UPDATE SET json = excluded.json",
            params![provider, json],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("app.sqlite");
        {
            let s = AppStore::open(&p).unwrap();
            assert_eq!(s.schema_version(), MIGRATIONS.len() as u32);
            s.set_setting("theme", "amber").unwrap();
        }
        let s = AppStore::open(&p).unwrap();
        assert_eq!(s.setting("theme").unwrap().as_deref(), Some("amber"));
    }

    #[test]
    fn watchlists_round_trip() {
        let s = AppStore::open_in_memory().unwrap();
        let id = s.create_watchlist("Tech", 1).unwrap();
        s.set_watchlist_items(id, &["AAPL US Equity".into(), "MSFT US Equity".into(), "AAPL US Equity".into()]).unwrap();
        let w = s.watchlists().unwrap();
        assert_eq!(w[0].securities, vec!["AAPL US Equity", "MSFT US Equity"]);
        s.delete_watchlist(id).unwrap();
        assert!(s.watchlists().unwrap().is_empty());
    }

    #[test]
    fn history_is_capped_and_ordered() {
        let s = AppStore::open_in_memory().unwrap();
        for i in 0..510 {
            s.push_history("p1", &format!("cmd{i}"), i).unwrap();
        }
        let h = s.history("p1", 1000).unwrap();
        assert_eq!(h.len(), 500);
        assert_eq!(h[0], "cmd509");
    }

    #[test]
    fn alerts_and_events() {
        let s = AppStore::open_in_memory().unwrap();
        let id = s.upsert_alert(None, "{}", true, 5).unwrap();
        s.record_alert_fired(id, 9, "AAPL > 200").unwrap();
        let a = &s.alerts().unwrap()[0];
        assert_eq!(a.last_fired_at, Some(9));
        assert_eq!(s.alert_events(10).unwrap()[0].message, "AAPL > 200");
    }

    #[test]
    fn portfolio_transactions() {
        let s = AppStore::open_in_memory().unwrap();
        let pid = s.create_portfolio("Main", "USD").unwrap();
        s.add_transaction(&Transaction {
            id: 0,
            portfolio_id: pid,
            security: "AAPL US Equity".into(),
            trade_date: "2024-01-02".into(),
            quantity: 10.0,
            price: 185.0,
            fees: 1.0,
            note: None,
        })
        .unwrap();
        assert_eq!(s.transactions(pid).unwrap().len(), 1);
        s.delete_portfolio(pid).unwrap();
        assert!(s.transactions(pid).unwrap().is_empty());
    }
}
