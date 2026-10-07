//! App state in SQLite: settings, watchlists, workspaces, history, alerts,
//! portfolios, ASK transcripts, and non-secret provider settings.

use std::path::Path;

use meridian_types::TransactionKind;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Statement, params};
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
    // 2: transaction kinds, cash amounts, settle dates, currencies and
    // import fingerprints (broker CSV import). Rebuilt rather than altered
    // so `security` and `price` can be NULL (cash rows, transfers without a
    // cost basis). Existing rows become buys/sells by the sign of quantity.
    r"
    CREATE TABLE transactions_v2 (
        id INTEGER PRIMARY KEY,
        portfolio_id INTEGER NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
        security TEXT,
        kind TEXT NOT NULL,
        trade_date TEXT NOT NULL,
        settle_date TEXT,
        quantity REAL NOT NULL DEFAULT 0,
        price REAL,
        amount REAL,
        fees REAL NOT NULL DEFAULT 0,
        currency TEXT,
        note TEXT,
        source TEXT,
        fingerprint TEXT
    );
    INSERT INTO transactions_v2(id, portfolio_id, security, kind, trade_date, quantity, price, fees, note)
        SELECT id, portfolio_id, security, CASE WHEN quantity < 0 THEN 'sell' ELSE 'buy' END, trade_date, quantity, price, fees, note
        FROM transactions;
    DROP TABLE transactions;
    ALTER TABLE transactions_v2 RENAME TO transactions;
    CREATE UNIQUE INDEX transactions_fingerprint ON transactions(portfolio_id, fingerprint);
    CREATE INDEX transactions_portfolio_date ON transactions(portfolio_id, trade_date);
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

/// A portfolio transaction. Signs follow [`TransactionKind`]: `quantity`
/// is the change in shares, `amount` the cash flow (+ in, − out).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: i64,
    pub portfolio_id: i64,
    /// Security key (`AAPL US Equity`); `None` for cash-only rows.
    pub security: Option<String>,
    pub kind: TransactionKind,
    /// `YYYY-MM-DD`.
    pub trade_date: String,
    /// `YYYY-MM-DD`.
    pub settle_date: Option<String>,
    /// Signed change in shares (0 for cash rows).
    pub quantity: f64,
    /// Per share. For a transfer in, the cost basis per share if known.
    pub price: Option<f64>,
    /// Signed cash flow; `None` means derive it from quantity × price and
    /// fees (hand-entered trades).
    pub amount: Option<f64>,
    pub fees: f64,
    /// ISO 4217; `None` means the portfolio's base currency.
    pub currency: Option<String>,
    pub note: Option<String>,
    /// Import format id (`robinhood`, `positions` …); `None` when entered
    /// by hand.
    pub source: Option<String>,
    /// Import de-duplication key, unique per portfolio.
    pub fingerprint: Option<String>,
}

impl Transaction {
    /// A hand-entered trade: positive quantity buys, negative sells.
    #[must_use]
    pub fn trade(portfolio_id: i64, security: &str, trade_date: &str, quantity: f64, price: f64, fees: f64) -> Self {
        Self {
            id: 0,
            portfolio_id,
            security: Some(security.to_owned()),
            kind: if quantity < 0.0 { TransactionKind::Sell } else { TransactionKind::Buy },
            trade_date: trade_date.to_owned(),
            settle_date: None,
            quantity,
            price: Some(price),
            amount: None,
            fees,
            currency: None,
            note: None,
            source: None,
            fingerprint: None,
        }
    }
}

/// Where imported transactions go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortfolioTarget<'a> {
    Existing(i64),
    New { name: &'a str, base_currency: &'a str },
}

/// Outcome of [`AppStore::import_transactions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportCounts {
    pub portfolio_id: i64,
    pub inserted: usize,
    /// Rows whose fingerprint the portfolio already had.
    pub duplicates: usize,
}

const INSERT_TRANSACTION: &str = "INSERT INTO transactions(portfolio_id, security, kind, trade_date, settle_date, quantity, price, amount, fees, currency, note, source, fingerprint)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)";

fn insert_transaction(stmt: &mut Statement<'_>, portfolio_id: i64, t: &Transaction) -> rusqlite::Result<usize> {
    stmt.execute(params![
        portfolio_id,
        t.security,
        t.kind.code(),
        t.trade_date,
        t.settle_date,
        t.quantity,
        t.price,
        t.amount,
        t.fees,
        t.currency,
        t.note,
        t.source,
        t.fingerprint
    ])
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

    pub fn portfolio(&self, id: i64) -> StoreResult<Option<Portfolio>> {
        Ok(self
            .conn
            .lock()
            .query_row("SELECT id, name, base_currency FROM portfolios WHERE id = ?1", [id], |r| {
                Ok(Portfolio { id: r.get(0)?, name: r.get(1)?, base_currency: r.get(2)? })
            })
            .optional()?)
    }

    pub fn add_transaction(&self, t: &Transaction) -> StoreResult<i64> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(INSERT_TRANSACTION)?;
        insert_transaction(&mut stmt, t.portfolio_id, t)?;
        Ok(conn.last_insert_rowid())
    }

    /// Inserts imported transactions in one SQLite transaction, creating
    /// the portfolio first for [`PortfolioTarget::New`]. Rows whose
    /// fingerprint the portfolio already has are skipped and counted as
    /// duplicates; any other failure rolls the whole import back.
    pub fn import_transactions(&self, target: PortfolioTarget<'_>, txs: &[Transaction]) -> StoreResult<ImportCounts> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let portfolio_id = match target {
            PortfolioTarget::Existing(id) => {
                let found: Option<i64> = tx.query_row("SELECT id FROM portfolios WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                found.ok_or(StoreError::NotFound)?
            }
            PortfolioTarget::New { name, base_currency } => {
                tx.execute("INSERT INTO portfolios(name, base_currency) VALUES (?1, ?2)", params![name, base_currency])?;
                tx.last_insert_rowid()
            }
        };
        let (mut inserted, mut duplicates) = (0, 0);
        {
            let mut stmt = tx.prepare(&format!("{INSERT_TRANSACTION} ON CONFLICT(portfolio_id, fingerprint) DO NOTHING"))?;
            for t in txs {
                if insert_transaction(&mut stmt, portfolio_id, t)? == 0 {
                    duplicates += 1;
                } else {
                    inserted += 1;
                }
            }
        }
        tx.commit()?;
        Ok(ImportCounts { portfolio_id, inserted, duplicates })
    }

    /// Number of a portfolio's transactions, or only those imported from
    /// `source`.
    pub fn transaction_count(&self, portfolio_id: i64, source: Option<&str>) -> StoreResult<usize> {
        let conn = self.conn.lock();
        let n: i64 = match source {
            Some(s) => conn.query_row(
                "SELECT COUNT(*) FROM transactions WHERE portfolio_id = ?1 AND source = ?2",
                params![portfolio_id, s],
                |r| r.get(0),
            )?,
            None => conn.query_row("SELECT COUNT(*) FROM transactions WHERE portfolio_id = ?1", [portfolio_id], |r| r.get(0))?,
        };
        Ok(usize::try_from(n).unwrap_or(0))
    }

    pub fn delete_transaction(&self, id: i64) -> StoreResult<()> {
        self.conn.lock().execute("DELETE FROM transactions WHERE id = ?1", [id])?;
        Ok(())
    }

    /// A portfolio's transactions in trade-date order, then entry order
    /// (file order for imports).
    pub fn transactions(&self, portfolio_id: i64) -> StoreResult<Vec<Transaction>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, portfolio_id, security, kind, trade_date, settle_date, quantity, price, amount, fees, currency, note, source, fingerprint
             FROM transactions WHERE portfolio_id = ?1 ORDER BY trade_date, id",
        )?;
        let rows = stmt
            .query_map([portfolio_id], |r| {
                let kind: String = r.get(3)?;
                Ok(Transaction {
                    id: r.get(0)?,
                    portfolio_id: r.get(1)?,
                    security: r.get(2)?,
                    // A code written by a newer version reads as Other.
                    kind: TransactionKind::from_code(&kind).unwrap_or(TransactionKind::Other),
                    trade_date: r.get(4)?,
                    settle_date: r.get(5)?,
                    quantity: r.get(6)?,
                    price: r.get(7)?,
                    amount: r.get(8)?,
                    fees: r.get(9)?,
                    currency: r.get(10)?,
                    note: r.get(11)?,
                    source: r.get(12)?,
                    fingerprint: r.get(13)?,
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
        s.add_transaction(&Transaction::trade(pid, "AAPL US Equity", "2024-01-02", 10.0, 185.0, 1.0)).unwrap();
        let t = &s.transactions(pid).unwrap()[0];
        assert_eq!((t.kind, t.quantity, t.price, t.amount, t.fees), (TransactionKind::Buy, 10.0, Some(185.0), None, 1.0));
        assert_eq!(s.portfolio(pid).unwrap().map(|p| p.name), Some("Main".into()));
        s.delete_portfolio(pid).unwrap();
        assert!(s.transactions(pid).unwrap().is_empty());
        assert_eq!(s.portfolio(pid).unwrap(), None);
    }

    fn imported(fp: &str, kind: TransactionKind, security: Option<&str>, quantity: f64, amount: Option<f64>) -> Transaction {
        Transaction {
            id: 0,
            portfolio_id: 0,
            security: security.map(Into::into),
            kind,
            trade_date: "2026-01-05".into(),
            settle_date: Some("2026-01-06".into()),
            quantity,
            price: None,
            amount,
            fees: 0.0,
            currency: Some("USD".into()),
            note: Some("imported".into()),
            source: Some("robinhood".into()),
            fingerprint: Some(fp.into()),
        }
    }

    #[test]
    fn import_creates_portfolio_and_skips_known_fingerprints() {
        let s = AppStore::open_in_memory().unwrap();
        let rows = vec![
            imported("a-0", TransactionKind::Deposit, None, 0.0, Some(100.0)),
            imported("b-0", TransactionKind::Buy, Some("AAPL US Equity"), 1.0, Some(-50.0)),
        ];
        let first = s.import_transactions(PortfolioTarget::New { name: "Robinhood", base_currency: "USD" }, &rows).unwrap();
        assert_eq!((first.inserted, first.duplicates), (2, 0));
        let mut again = rows.clone();
        again.push(imported("c-0", TransactionKind::Dividend, Some("AAPL US Equity"), 0.0, Some(0.25)));
        let second = s.import_transactions(PortfolioTarget::Existing(first.portfolio_id), &again).unwrap();
        assert_eq!((second.portfolio_id, second.inserted, second.duplicates), (first.portfolio_id, 1, 2));
        let txs = s.transactions(first.portfolio_id).unwrap();
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].security, None);
        assert_eq!(txs[0].kind, TransactionKind::Deposit);
        assert_eq!(txs[0].settle_date.as_deref(), Some("2026-01-06"));
        assert_eq!(s.transaction_count(first.portfolio_id, Some("robinhood")).unwrap(), 3);
        assert_eq!(s.transaction_count(first.portfolio_id, Some("positions")).unwrap(), 0);
        // The same fingerprint may exist in another portfolio.
        let other = s.import_transactions(PortfolioTarget::New { name: "Copy", base_currency: "USD" }, &rows).unwrap();
        assert_eq!(other.inserted, 2);
        // Hand-entered rows have no fingerprint and never collide.
        s.add_transaction(&Transaction::trade(other.portfolio_id, "AAPL US Equity", "2026-01-07", 1.0, 1.0, 0.0)).unwrap();
        s.add_transaction(&Transaction::trade(other.portfolio_id, "AAPL US Equity", "2026-01-07", 1.0, 1.0, 0.0)).unwrap();
        assert_eq!(s.transaction_count(other.portfolio_id, None).unwrap(), 4);
    }

    #[test]
    fn import_into_a_missing_portfolio_or_a_taken_name_rolls_back() {
        let s = AppStore::open_in_memory().unwrap();
        let rows = vec![imported("a-0", TransactionKind::Deposit, None, 0.0, Some(1.0))];
        assert_eq!(s.import_transactions(PortfolioTarget::Existing(42), &rows), Err(StoreError::NotFound));
        s.create_portfolio("Main", "USD").unwrap();
        assert!(s.import_transactions(PortfolioTarget::New { name: "Main", base_currency: "USD" }, &rows).is_err());
        assert_eq!(s.portfolios().unwrap().len(), 1);
    }

    #[test]
    fn migration_2_keeps_existing_trades() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("app.sqlite");
        {
            // A version-1 database with a buy and a sell.
            let conn = Connection::open(&p).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch(
                "PRAGMA user_version = 1;
                 INSERT INTO portfolios(id, name, base_currency) VALUES (1, 'Main', 'USD');
                 INSERT INTO transactions(portfolio_id, security, trade_date, quantity, price, fees, note)
                   VALUES (1, 'AAPL US Equity', '2024-01-02', 10, 185, 1, NULL),
                          (1, 'AAPL US Equity', '2024-02-02', -4, 190, 0, 'trim');",
            )
            .unwrap();
        }
        let s = AppStore::open(&p).unwrap();
        assert_eq!(s.schema_version(), MIGRATIONS.len() as u32);
        let txs = s.transactions(1).unwrap();
        assert_eq!(txs.len(), 2);
        assert_eq!((txs[0].kind, txs[0].quantity, txs[0].price, txs[0].fees), (TransactionKind::Buy, 10.0, Some(185.0), 1.0));
        assert_eq!((txs[1].kind, txs[1].quantity, txs[1].note.as_deref()), (TransactionKind::Sell, -4.0, Some("trim")));
        assert_eq!((txs[0].amount, txs[0].fingerprint.as_deref(), txs[0].source.as_deref()), (None, None, None));
    }
}
