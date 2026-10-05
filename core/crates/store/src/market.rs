//! Market data in DuckDB: bars, fundamentals, news, filings, economic
//! observations, and a JSON blob cache for everything else. Bulk history can
//! be archived to hive-partitioned Parquet.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use duckdb::{Connection, params};
use meridian_types::{
    Adjustment, BarInterval, BarSeries, EconomicSeries, Filing, Fundamentals, NewsItem, Observation, PeriodType,
    Provenance, SecurityKey, Statement, StatementKind, StatementLine, UnixNanos,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{StoreError, StoreResult};
use crate::sql_guard::{check_ast_relations, precheck_ask_sql};

const MIGRATIONS: &[&str] = &[
    // 1
    r"
    CREATE TABLE schema_version (version INTEGER NOT NULL);
    INSERT INTO schema_version VALUES (0);
    CREATE TABLE bars (
        provider VARCHAR NOT NULL, security VARCHAR NOT NULL, interval VARCHAR NOT NULL,
        ts BIGINT NOT NULL, open DOUBLE, high DOUBLE, low DOUBLE, close DOUBLE, volume DOUBLE,
        PRIMARY KEY (provider, security, interval, ts)
    );
    CREATE TABLE cache_meta (
        dataset VARCHAR NOT NULL, provider VARCHAR NOT NULL, key VARCHAR NOT NULL,
        fetched_at BIGINT NOT NULL, expires_at BIGINT,
        PRIMARY KEY (dataset, provider, key)
    );
    CREATE TABLE blobs (
        dataset VARCHAR NOT NULL, provider VARCHAR NOT NULL, key VARCHAR NOT NULL,
        json VARCHAR NOT NULL, fetched_at BIGINT NOT NULL, expires_at BIGINT,
        PRIMARY KEY (dataset, provider, key)
    );
    CREATE TABLE fundamentals (
        provider VARCHAR NOT NULL, security VARCHAR NOT NULL, statement VARCHAR NOT NULL,
        period_type VARCHAR NOT NULL, fiscal_year INTEGER NOT NULL, fiscal_period VARCHAR NOT NULL,
        period_end DATE NOT NULL, currency VARCHAR, line_no INTEGER NOT NULL, code VARCHAR NOT NULL,
        label VARCHAR, value DOUBLE, depth INTEGER, source_tag VARCHAR
    );
    CREATE INDEX fundamentals_sec ON fundamentals(security, period_type);
    CREATE TABLE news (
        provider VARCHAR NOT NULL, id VARCHAR NOT NULL, source VARCHAR, headline VARCHAR NOT NULL,
        summary VARCHAR, url VARCHAR, published_at BIGINT NOT NULL, tickers VARCHAR, topics VARCHAR,
        json VARCHAR NOT NULL,
        PRIMARY KEY (provider, id)
    );
    CREATE TABLE filings (
        provider VARCHAR NOT NULL, accession VARCHAR NOT NULL, cik BIGINT, company VARCHAR,
        form VARCHAR, filed DATE, period_of_report DATE, json VARCHAR NOT NULL,
        PRIMARY KEY (provider, accession)
    );
    CREATE TABLE econ_obs (
        provider VARCHAR NOT NULL, series_id VARCHAR NOT NULL, date DATE NOT NULL, value DOUBLE,
        PRIMARY KEY (provider, series_id, date)
    );
    CREATE TABLE econ_meta (
        provider VARCHAR NOT NULL, series_id VARCHAR NOT NULL, json VARCHAR NOT NULL,
        PRIMARY KEY (provider, series_id)
    );
    CREATE VIEW v_bars AS
        SELECT provider, security, interval, make_timestamp(ts // 1000) AS ts, open, high, low, close, volume FROM bars;
    CREATE VIEW v_fundamentals AS
        SELECT provider, security, statement, period_type, fiscal_year, fiscal_period, period_end, currency,
               code, label, value FROM fundamentals;
    CREATE VIEW v_news AS
        SELECT provider, source, headline, summary, url, make_timestamp(published_at // 1000) AS published_at,
               tickers, topics FROM news;
    CREATE VIEW v_filings AS
        SELECT provider, accession, cik, company, form, filed, period_of_report FROM filings;
    CREATE VIEW v_econ AS SELECT provider, series_id, date, value FROM econ_obs;
    ",
];

fn migrate(conn: &Connection) -> StoreResult<()> {
    let has_table: bool = conn
        .query_row(
            "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    let current: u32 = if has_table {
        conn.query_row("SELECT version FROM schema_version", [], |r| r.get::<_, i64>(0)).map(|v| v as u32)?
    } else {
        0
    };
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = u32::try_from(i + 1).unwrap_or(u32::MAX);
        conn.execute_batch("BEGIN TRANSACTION")?;
        let res = conn
            .execute_batch(sql)
            .and_then(|()| conn.execute("UPDATE schema_version SET version = ?", params![i64::from(version)]).map(|_| ()));
        match res {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(StoreError::Migration { version, message: e.to_string() });
            }
        }
        tracing::info!(version, "duckdb migrated");
    }
    Ok(())
}

/// Result of an ASK SQL query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// Values rendered as JSON (numbers stay numbers).
    pub rows: Vec<Vec<serde_json::Value>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReport {
    pub rows_deleted: u64,
    pub files_deleted: u64,
}

/// A cached blob and whether it's past its expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBlob {
    pub provider: String,
    pub json: String,
    pub fetched_at: UnixNanos,
    pub expired: bool,
}

pub struct MarketStore {
    writer: Mutex<Connection>,
    parquet_root: PathBuf,
}

fn period_type_str(p: PeriodType) -> &'static str {
    match p {
        PeriodType::Annual => "annual",
        PeriodType::Quarterly => "quarterly",
        PeriodType::Ttm => "ttm",
    }
}

fn statement_str(k: StatementKind) -> &'static str {
    match k {
        StatementKind::Income => "income",
        StatementKind::Balance => "balance",
        StatementKind::CashFlow => "cashflow",
    }
}

fn statement_from(s: &str) -> Option<StatementKind> {
    Some(match s {
        "income" => StatementKind::Income,
        "balance" => StatementKind::Balance,
        "cashflow" => StatementKind::CashFlow,
        _ => return None,
    })
}

impl MarketStore {
    pub fn open(path: &Path, parquet_root: PathBuf) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn, parquet_root)
    }

    pub fn open_in_memory(parquet_root: PathBuf) -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?, parquet_root)
    }

    fn init(conn: Connection, parquet_root: PathBuf) -> StoreResult<Self> {
        // Extensions are statically linked; never download anything.
        conn.execute_batch("SET autoinstall_known_extensions = false; SET autoload_known_extensions = false;")?;
        migrate(&conn)?;
        Ok(Self { writer: Mutex::new(conn), parquet_root })
    }

    /// A fresh connection to the same database for a reader thread.
    fn reader(&self) -> StoreResult<Connection> {
        Ok(self.writer.lock().try_clone()?)
    }

    // --- blobs ----------------------------------------------------------

    pub fn put_blob(
        &self,
        dataset: &str,
        provider: &str,
        key: &str,
        json: &str,
        fetched_at: UnixNanos,
        expires_at: Option<UnixNanos>,
    ) -> StoreResult<()> {
        self.writer.lock().execute(
            "INSERT OR REPLACE INTO blobs(dataset, provider, key, json, fetched_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![dataset, provider, key, json, fetched_at, expires_at],
        )?;
        Ok(())
    }

    /// Most recently fetched blob for (dataset, key) from any provider.
    pub fn get_blob(&self, dataset: &str, key: &str, now: UnixNanos) -> StoreResult<Option<CachedBlob>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT provider, json, fetched_at, expires_at FROM blobs WHERE dataset = ? AND key = ?
             ORDER BY fetched_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![dataset, key])?;
        if let Some(r) = rows.next()? {
            let expires: Option<i64> = r.get(3)?;
            return Ok(Some(CachedBlob {
                provider: r.get(0)?,
                json: r.get(1)?,
                fetched_at: r.get(2)?,
                expired: expires.is_some_and(|e| e <= now),
            }));
        }
        Ok(None)
    }

    /// Deletes blobs past their expiry (cache-policy enforcement).
    pub fn evict_expired(&self, now: UnixNanos) -> StoreResult<u64> {
        let n = self.writer.lock().execute("DELETE FROM blobs WHERE expires_at IS NOT NULL AND expires_at <= ?", params![now])?;
        Ok(n as u64)
    }

    // --- bars -----------------------------------------------------------

    pub fn put_bars(&self, series: &BarSeries) -> StoreResult<usize> {
        let provider = series.provenance.provider.as_str().to_owned();
        let security = series.key.to_string();
        let interval = series.interval.code();
        let conn = self.writer.lock();
        conn.execute(
            "DELETE FROM bars WHERE provider = ? AND security = ? AND interval = ? AND ts >= ? AND ts <= ?",
            params![
                provider,
                security,
                interval,
                series.ts.first().copied().unwrap_or(0),
                series.ts.last().copied().unwrap_or(0)
            ],
        )?;
        let mut app = conn.appender("bars")?;
        for i in 0..series.len() {
            app.append_row(params![
                provider,
                security,
                interval,
                series.ts[i],
                series.open[i],
                series.high[i],
                series.low[i],
                series.close[i],
                series.volume[i]
            ])?;
        }
        app.flush()?;
        Ok(series.len())
    }

    /// Cached bars for `key`, preferring `provider` if given.
    pub fn get_bars(
        &self,
        key: &SecurityKey,
        interval: BarInterval,
        from: Option<UnixNanos>,
        to: Option<UnixNanos>,
        provider: Option<&str>,
    ) -> StoreResult<Option<BarSeries>> {
        let conn = self.reader()?;
        let security = key.to_string();
        let provider: Option<String> = match provider {
            Some(p) => Some(p.to_owned()),
            None => conn
                .query_row(
                    "SELECT provider FROM bars WHERE security = ? AND interval = ? GROUP BY provider ORDER BY count(*) DESC LIMIT 1",
                    params![security, interval.code()],
                    |r| r.get(0),
                )
                .ok(),
        };
        let Some(provider) = provider else { return Ok(None) };
        let mut stmt = conn.prepare(
            "SELECT ts, open, high, low, close, volume FROM bars
             WHERE provider = ? AND security = ? AND interval = ? AND ts >= ? AND ts < ? ORDER BY ts",
        )?;
        let mut rows = stmt.query(params![
            provider,
            security,
            interval.code(),
            from.unwrap_or(i64::MIN),
            to.unwrap_or(i64::MAX)
        ])?;
        let mut series = BarSeries::new(
            key.clone(),
            interval,
            Adjustment::Splits,
            Provenance {
                provider: meridian_types::ProviderId::new(&provider),
                synthetic: false,
                delay: meridian_types::DataDelay::EndOfDay,
                source: meridian_types::FeedSource::Aggregated,
                as_of: 0,
                source_ref: Some("local cache".into()),
                attribution: None,
            },
        );
        while let Some(r) = rows.next()? {
            series.push(meridian_types::Bar {
                ts: r.get(0)?,
                open: r.get(1)?,
                high: r.get(2)?,
                low: r.get(3)?,
                close: r.get(4)?,
                volume: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            });
        }
        if series.is_empty() { Ok(None) } else { Ok(Some(series)) }
    }

    // --- fundamentals ---------------------------------------------------

    pub fn put_fundamentals(&self, f: &Fundamentals) -> StoreResult<()> {
        let provider = f.provenance.provider.as_str().to_owned();
        let security = f.key.to_string();
        let conn = self.writer.lock();
        conn.execute("DELETE FROM fundamentals WHERE provider = ? AND security = ?", params![provider, security])?;
        let mut app = conn.appender("fundamentals")?;
        for st in &f.statements {
            for (line_no, l) in st.lines.iter().enumerate() {
                app.append_row(params![
                    provider,
                    security,
                    statement_str(st.kind),
                    period_type_str(st.period_type),
                    st.fiscal_year,
                    st.fiscal_period,
                    st.period_end.format("%Y-%m-%d").to_string(),
                    st.currency,
                    line_no as i32,
                    l.code,
                    l.label,
                    l.value,
                    i32::from(l.depth),
                    l.source_tag
                ])?;
            }
        }
        app.flush()?;
        Ok(())
    }

    pub fn get_fundamentals(&self, key: &SecurityKey, period_type: PeriodType) -> StoreResult<Option<Fundamentals>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT provider, statement, fiscal_year, fiscal_period, strftime(period_end, '%Y-%m-%d'), currency,
                    code, label, value, depth, source_tag
             FROM fundamentals WHERE security = ? AND period_type = ?
             ORDER BY statement, period_end, line_no",
        )?;
        let mut rows = stmt.query(params![key.to_string(), period_type_str(period_type)])?;
        let mut provider = None;
        let mut statements: Vec<Statement> = Vec::new();
        while let Some(r) = rows.next()? {
            let p: String = r.get(0)?;
            provider.get_or_insert(p);
            let kind = statement_from(&r.get::<_, String>(1)?).unwrap_or(StatementKind::Income);
            let fy: i32 = r.get(2)?;
            let fp: String = r.get(3)?;
            let end: String = r.get(4)?;
            let period_end = chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d").map_err(|e| StoreError::Serde(e.to_string()))?;
            let line = StatementLine {
                code: r.get(6)?,
                label: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                value: r.get(8)?,
                depth: r.get::<_, Option<i32>>(9)?.unwrap_or(0) as u8,
                source_tag: r.get(10)?,
            };
            match statements.last_mut() {
                Some(s) if s.kind == kind && s.period_end == period_end && s.fiscal_period == fp => s.lines.push(line),
                _ => statements.push(Statement {
                    kind,
                    period_type,
                    fiscal_year: fy,
                    fiscal_period: fp,
                    period_end,
                    currency: r.get::<_, Option<String>>(5)?.unwrap_or_else(|| "USD".into()),
                    lines: vec![line],
                }),
            }
        }
        let Some(provider) = provider else { return Ok(None) };
        Ok(Some(Fundamentals {
            key: key.clone(),
            statements,
            provenance: Provenance {
                provider: meridian_types::ProviderId::new(&provider),
                synthetic: false,
                delay: meridian_types::DataDelay::EndOfDay,
                source: meridian_types::FeedSource::Official,
                as_of: 0,
                source_ref: Some("local cache".into()),
                attribution: None,
            },
        }))
    }

    // --- news -----------------------------------------------------------

    pub fn put_news(&self, items: &[NewsItem]) -> StoreResult<()> {
        let conn = self.writer.lock();
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO news(provider, id, source, headline, summary, url, published_at, tickers, topics, json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        for n in items {
            stmt.execute(params![
                n.provenance.provider.as_str(),
                n.id,
                n.source,
                n.headline,
                n.summary,
                n.url,
                n.published_at,
                n.tickers.join(","),
                n.topics.join(","),
                serde_json::to_string(n)?
            ])?;
        }
        Ok(())
    }

    /// Cached news, newest first, optionally filtered by ticker and text.
    pub fn search_news(&self, tickers: &[String], text: Option<&str>, limit: usize) -> StoreResult<Vec<NewsItem>> {
        let conn = self.reader()?;
        let mut sql = String::from("SELECT json FROM news WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if !tickers.is_empty() {
            sql.push_str(" AND (");
            for (i, t) in tickers.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("list_contains(string_split(tickers, ','), ?)");
                args.push(t.clone());
            }
            sql.push(')');
        }
        if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
            sql.push_str(" AND (headline ILIKE ? OR summary ILIKE ?)");
            let pat = format!("%{}%", t.trim());
            args.push(pat.clone());
            args.push(pat);
        }
        let _ = write!(sql, " ORDER BY published_at DESC LIMIT {}", limit.min(10_000));
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn duckdb::ToSql> = args.iter().map(|a| a as &dyn duckdb::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for j in rows {
            out.push(serde_json::from_str(&j?)?);
        }
        Ok(out)
    }

    // --- filings --------------------------------------------------------

    pub fn put_filings(&self, filings: &[Filing]) -> StoreResult<()> {
        let conn = self.writer.lock();
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO filings(provider, accession, cik, company, form, filed, period_of_report, json)
             VALUES (?, ?, ?, ?, ?, CAST(? AS DATE), CAST(? AS DATE), ?)",
        )?;
        for f in filings {
            stmt.execute(params![
                f.provenance.provider.as_str(),
                f.accession,
                f.cik as i64,
                f.company,
                f.form,
                f.filed.format("%Y-%m-%d").to_string(),
                f.period_of_report.map(|d| d.format("%Y-%m-%d").to_string()),
                serde_json::to_string(f)?
            ])?;
        }
        Ok(())
    }

    pub fn filings_for_cik(&self, cik: u64, limit: usize) -> StoreResult<Vec<Filing>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!("SELECT json FROM filings WHERE cik = ? ORDER BY filed DESC LIMIT {}", limit.min(10_000)))?;
        let rows = stmt.query_map(params![cik as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for j in rows {
            out.push(serde_json::from_str(&j?)?);
        }
        Ok(out)
    }

    // --- economic series ------------------------------------------------

    pub fn put_series(&self, s: &EconomicSeries) -> StoreResult<()> {
        let provider = s.provenance.provider.as_str().to_owned();
        let conn = self.writer.lock();
        let mut meta = s.clone();
        meta.observations.clear();
        conn.execute(
            "INSERT OR REPLACE INTO econ_meta(provider, series_id, json) VALUES (?, ?, ?)",
            params![provider, s.id, serde_json::to_string(&meta)?],
        )?;
        conn.execute("DELETE FROM econ_obs WHERE provider = ? AND series_id = ?", params![provider, s.id])?;
        let mut app = conn.appender("econ_obs")?;
        for o in &s.observations {
            app.append_row(params![provider, s.id, o.date.format("%Y-%m-%d").to_string(), o.value])?;
        }
        app.flush()?;
        Ok(())
    }

    pub fn get_series(&self, id: &str) -> StoreResult<Option<EconomicSeries>> {
        let conn = self.reader()?;
        let meta: Option<(String, String)> = conn
            .query_row("SELECT provider, json FROM econ_meta WHERE series_id = ? LIMIT 1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        let Some((provider, json)) = meta else { return Ok(None) };
        let mut s: EconomicSeries = serde_json::from_str(&json)?;
        let mut stmt = conn.prepare(
            "SELECT strftime(date, '%Y-%m-%d'), value FROM econ_obs WHERE provider = ? AND series_id = ? ORDER BY date",
        )?;
        let rows = stmt.query_map(params![provider, id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?)))?;
        for row in rows {
            let (d, v) = row?;
            let date = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").map_err(|e| StoreError::Serde(e.to_string()))?;
            s.observations.push(Observation { date, value: v });
        }
        Ok(Some(s))
    }

    // --- ASK SQL --------------------------------------------------------

    /// Runs a validated, read-only SELECT over the allowlisted views.
    pub fn query_ask(&self, sql: &str, max_rows: usize, timeout: Duration) -> StoreResult<QueryResult> {
        precheck_ask_sql(sql)?;
        let sql = sql.trim().trim_end_matches(';').trim().to_owned();
        let conn = self.reader()?;
        let ast_json: String = conn.query_row("SELECT json_serialize_sql(CAST(? AS VARCHAR))", params![sql], |r| r.get(0))?;
        let ast: serde_json::Value = serde_json::from_str(&ast_json)?;
        check_ast_relations(&ast)?;

        let interrupt = conn.interrupt_handle();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        let watchdog = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while start.elapsed() < timeout {
                if done2.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            interrupt.interrupt();
        });

        let wrapped = format!("SELECT * FROM ({sql}) AS q LIMIT {}", max_rows + 1);
        let result = (|| -> StoreResult<QueryResult> {
            let mut stmt = conn.prepare(&wrapped)?;
            let mut rows = stmt.query([])?;
            let columns = rows.as_ref().map(duckdb::Statement::column_names).unwrap_or_default();
            let ncols = columns.len();
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let mut row = Vec::with_capacity(ncols);
                for i in 0..ncols {
                    row.push(value_to_json(r.get::<_, duckdb::types::Value>(i)?));
                }
                out.push(row);
            }
            let truncated = out.len() > max_rows;
            out.truncate(max_rows);
            Ok(QueryResult { columns, rows: out, truncated })
        })();
        done.store(true, std::sync::atomic::Ordering::Release);
        let _ = watchdog.join();
        result.map_err(|e| match e {
            StoreError::DuckDb(m) if m.contains("INTERRUPT") || m.contains("nterrupt") => {
                StoreError::Rejected(format!("query exceeded {} ms", timeout.as_millis()))
            }
            other => other,
        })
    }

    // --- Parquet archive ------------------------------------------------

    /// Writes all cached bars of one provider/security/interval to
    /// `parquet/bars/provider=<p>/security=<s>/interval=<i>/data.parquet`.
    pub fn archive_bars_to_parquet(&self, provider: &str, key: &SecurityKey, interval: BarInterval) -> StoreResult<PathBuf> {
        let dir = self
            .parquet_root
            .join("bars")
            .join(format!("provider={provider}"))
            .join(format!("security={}", sanitize(&key.to_string())))
            .join(format!("interval={}", interval.code()));
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(e.to_string()))?;
        let file = dir.join("data.parquet");
        let path = file.to_string_lossy().replace('\'', "''");
        let conn = self.writer.lock();
        conn.execute_batch(&format!(
            "COPY (SELECT ts, open, high, low, close, volume FROM bars
                   WHERE provider = '{}' AND security = '{}' AND interval = '{}' ORDER BY ts)
             TO '{path}' (FORMAT parquet, COMPRESSION zstd)",
            provider.replace('\'', "''"),
            key.to_string().replace('\'', "''"),
            interval.code()
        ))?;
        Ok(file)
    }

    /// Loads an archived Parquet file back into the bars table.
    pub fn restore_bars_from_parquet(&self, file: &Path, provider: &str, key: &SecurityKey, interval: BarInterval) -> StoreResult<u64> {
        let path = file.to_string_lossy().replace('\'', "''");
        let n = self.writer.lock().execute(
            &format!(
                "INSERT OR REPLACE INTO bars SELECT ?, ?, ?, ts, open, high, low, close, volume FROM read_parquet('{path}')"
            ),
            params![provider, key.to_string(), interval.code()],
        )?;
        Ok(n as u64)
    }

    // --- purge ----------------------------------------------------------

    pub fn purge_provider(&self, provider: &str) -> StoreResult<PurgeReport> {
        let conn = self.writer.lock();
        let mut rows = 0u64;
        for table in ["bars", "cache_meta", "blobs", "fundamentals", "news", "filings", "econ_obs", "econ_meta"] {
            rows += conn.execute(&format!("DELETE FROM {table} WHERE provider = ?"), params![provider])? as u64;
        }
        drop(conn);
        let mut files = 0u64;
        let dir = self.parquet_root.join("bars").join(format!("provider={provider}"));
        if dir.exists() {
            files = count_files(&dir);
            std::fs::remove_dir_all(&dir).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        Ok(PurgeReport { rows_deleted: rows, files_deleted: files })
    }
}

fn count_files(dir: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

fn value_to_json(v: duckdb::types::Value) -> serde_json::Value {
    use duckdb::types::Value as V;
    match v {
        V::Null => serde_json::Value::Null,
        V::Boolean(b) => b.into(),
        V::TinyInt(i) => i.into(),
        V::SmallInt(i) => i.into(),
        V::Int(i) => i.into(),
        V::BigInt(i) => i.into(),
        V::UTinyInt(i) => i.into(),
        V::USmallInt(i) => i.into(),
        V::UInt(i) => i.into(),
        V::UBigInt(i) => i.into(),
        V::HugeInt(i) => (i as f64).into(),
        V::Float(f) => f64::from(f).into(),
        V::Double(f) => serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number),
        V::Decimal(d) => d.to_string().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(serde_json::Value::Null, serde_json::Value::Number),
        V::Text(s) => s.into(),
        other => format!("{other:?}").into(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use meridian_types::{Bar, DataDelay, FeedSource, ProviderId};

    use super::*;

    fn prov(p: &str) -> Provenance {
        Provenance {
            provider: ProviderId::new(p),
            synthetic: false,
            delay: DataDelay::EndOfDay,
            source: FeedSource::Official,
            as_of: 0,
            source_ref: None,
            attribution: None,
        }
    }

    fn store() -> MarketStore {
        MarketStore::open_in_memory(std::env::temp_dir().join(format!("meridian-test-{}", std::process::id()))).unwrap()
    }

    fn series(p: &str, n: usize) -> BarSeries {
        let mut s = BarSeries::new(SecurityKey::equity("AAPL"), BarInterval::Day, Adjustment::Splits, prov(p));
        for i in 0..n {
            let c = 100.0 + i as f64;
            s.push(Bar { ts: i as i64 * 86_400_000_000_000, open: c, high: c + 1.0, low: c - 1.0, close: c, volume: 1000.0 });
        }
        s
    }

    #[test]
    fn bars_round_trip_and_upsert() {
        let st = store();
        st.put_bars(&series("p1", 10)).unwrap();
        st.put_bars(&series("p1", 10)).unwrap();
        let got = st.get_bars(&SecurityKey::equity("AAPL"), BarInterval::Day, None, None, None).unwrap().unwrap();
        assert_eq!(got.len(), 10);
        assert_eq!(got.close[9], 109.0);
        let sliced = st
            .get_bars(&SecurityKey::equity("AAPL"), BarInterval::Day, Some(2 * 86_400_000_000_000), Some(5 * 86_400_000_000_000), Some("p1"))
            .unwrap()
            .unwrap();
        assert_eq!(sliced.len(), 3);
    }

    #[test]
    fn fundamentals_round_trip() {
        let st = store();
        let f = Fundamentals {
            key: SecurityKey::equity("AAPL"),
            statements: vec![Statement {
                kind: StatementKind::Income,
                period_type: PeriodType::Annual,
                fiscal_year: 2025,
                fiscal_period: "FY".into(),
                period_end: NaiveDate::from_ymd_opt(2025, 9, 27).unwrap(),
                currency: "USD".into(),
                lines: vec![
                    StatementLine { code: "revenue".into(), label: "Revenue".into(), value: Some(1.0e11), depth: 0, source_tag: None },
                    StatementLine { code: "net_income".into(), label: "Net Income".into(), value: None, depth: 0, source_tag: None },
                ],
            }],
            provenance: prov("edgar"),
        };
        st.put_fundamentals(&f).unwrap();
        let got = st.get_fundamentals(&f.key, PeriodType::Annual).unwrap().unwrap();
        assert_eq!(got.statements.len(), 1);
        assert_eq!(got.statements[0].value("revenue"), Some(1.0e11));
        assert_eq!(got.statements[0].lines[1].value, None);
    }

    #[test]
    fn blobs_expire() {
        let st = store();
        st.put_blob("profile", "p", "AAPL", "{}", 10, Some(20)).unwrap();
        assert!(!st.get_blob("profile", "AAPL", 15).unwrap().unwrap().expired);
        assert!(st.get_blob("profile", "AAPL", 25).unwrap().unwrap().expired);
        assert_eq!(st.evict_expired(25).unwrap(), 1);
        assert!(st.get_blob("profile", "AAPL", 25).unwrap().is_none());
    }

    #[test]
    fn ask_sql_guard_and_limits() {
        let st = store();
        st.put_bars(&series("p1", 50)).unwrap();
        let r = st.query_ask("SELECT security, count(*) AS n FROM v_bars GROUP BY 1", 10, Duration::from_secs(5)).unwrap();
        assert_eq!(r.columns, vec!["security", "n"]);
        assert_eq!(r.rows[0][1], serde_json::json!(50));
        let r = st.query_ask("SELECT close FROM v_bars ORDER BY ts", 5, Duration::from_secs(5)).unwrap();
        assert!(r.truncated);
        assert_eq!(r.rows.len(), 5);
        assert!(st.query_ask("SELECT * FROM bars", 5, Duration::from_secs(5)).is_err());
        assert!(st.query_ask("SELECT * FROM read_csv('/etc/passwd')", 5, Duration::from_secs(5)).is_err());
        assert!(st.query_ask("SELECT * FROM main.v_bars", 5, Duration::from_secs(5)).is_err());
        assert!(st.query_ask("WITH x AS (SELECT close FROM v_bars) SELECT avg(close) FROM x", 5, Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn purge_removes_provider_rows_and_files() {
        let st = store();
        st.put_bars(&series("finnhub", 5)).unwrap();
        st.put_bars(&series("other", 5)).unwrap();
        let f = st.archive_bars_to_parquet("finnhub", &SecurityKey::equity("AAPL"), BarInterval::Day).unwrap();
        assert!(f.exists());
        let rep = st.purge_provider("finnhub").unwrap();
        assert_eq!(rep.rows_deleted, 5);
        assert_eq!(rep.files_deleted, 1);
        assert!(st.get_bars(&SecurityKey::equity("AAPL"), BarInterval::Day, None, None, Some("finnhub")).unwrap().is_none());
        assert!(st.get_bars(&SecurityKey::equity("AAPL"), BarInterval::Day, None, None, Some("other")).unwrap().is_some());
    }

    #[test]
    fn parquet_round_trip() {
        let st = store();
        st.put_bars(&series("p1", 20)).unwrap();
        let key = SecurityKey::equity("AAPL");
        let f = st.archive_bars_to_parquet("p1", &key, BarInterval::Day).unwrap();
        st.purge_provider("p1").unwrap();
        // purge removed the file too; re-archive to test restore separately.
        assert!(!f.exists());
        st.put_bars(&series("p1", 20)).unwrap();
        let f = st.archive_bars_to_parquet("p1", &key, BarInterval::Day).unwrap();
        st.writer.lock().execute_batch("DELETE FROM bars").unwrap();
        assert_eq!(st.restore_bars_from_parquet(&f, "p1", &key, BarInterval::Day).unwrap(), 20);
    }
}
