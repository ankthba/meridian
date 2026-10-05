//! Validation for ASK's read-only SQL. The model may only read allowlisted
//! views; no DDL/DML, no file access, no settings.

use crate::error::{StoreError, StoreResult};

/// Views ASK may query.
pub const ASK_VIEWS: &[&str] = &["v_bars", "v_fundamentals", "v_news", "v_filings", "v_econ"];

const FORBIDDEN_WORDS: &[&str] = &[
    "insert", "update", "delete", "create", "drop", "alter", "copy", "attach", "detach", "pragma", "set", "reset",
    "install", "load", "export", "import", "call", "checkpoint", "vacuum", "begin", "commit", "rollback", "grant",
    "truncate",
];

fn words(sql: &str) -> impl Iterator<Item = String> + '_ {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase)
}

/// Cheap lexical checks run before asking DuckDB to parse the statement.
pub fn precheck_ask_sql(sql: &str) -> StoreResult<()> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(StoreError::Rejected("empty query".into()));
    }
    if trimmed.contains(';') {
        return Err(StoreError::Rejected("only a single statement is allowed".into()));
    }
    let first = words(trimmed).next().unwrap_or_default();
    if first != "select" && first != "with" {
        return Err(StoreError::Rejected("only SELECT queries are allowed".into()));
    }
    // Strip string literals before scanning keywords so `WHERE headline LIKE '%update%'` is fine.
    let mut no_strings = String::with_capacity(trimmed.len());
    let mut in_str = false;
    for c in trimmed.chars() {
        if c == '\'' {
            in_str = !in_str;
            no_strings.push(' ');
        } else if !in_str {
            no_strings.push(c);
        }
    }
    for w in words(&no_strings) {
        if FORBIDDEN_WORDS.contains(&w.as_str()) {
            return Err(StoreError::Rejected(format!("keyword `{w}` is not allowed")));
        }
    }
    Ok(())
}

/// Walks DuckDB's `json_serialize_sql` AST and checks every relation is an
/// allowlisted view and no table functions (file readers) are used.
pub fn check_ast_relations(ast: &serde_json::Value) -> StoreResult<()> {
    if ast.get("error").and_then(serde_json::Value::as_bool) == Some(true) {
        let msg = ast.get("error_message").and_then(|v| v.as_str()).unwrap_or("parse error");
        return Err(StoreError::Rejected(msg.to_string()));
    }
    let mut stack = vec![ast];
    let mut ctes: Vec<String> = Vec::new();
    // First pass: collect CTE names so references to them are allowed.
    while let Some(v) = stack.pop() {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Array(entries)) = map.get("cte_map").and_then(|c| c.get("map")) {
                    for e in entries {
                        if let Some(k) = e.get("key").and_then(|k| k.as_str()) {
                            ctes.push(k.to_ascii_lowercase());
                        }
                    }
                }
                stack.extend(map.values());
            }
            serde_json::Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
    let mut stack = vec![ast];
    while let Some(v) = stack.pop() {
        match v {
            serde_json::Value::Object(map) => {
                match map.get("type").and_then(|t| t.as_str()) {
                    Some("BASE_TABLE") => {
                        let name = map.get("table_name").and_then(|t| t.as_str()).unwrap_or("").to_ascii_lowercase();
                        let schema = map.get("schema_name").and_then(|t| t.as_str()).unwrap_or("");
                        let catalog = map.get("catalog_name").and_then(|t| t.as_str()).unwrap_or("");
                        if !schema.is_empty() || !catalog.is_empty() {
                            return Err(StoreError::Rejected("qualified table names are not allowed".into()));
                        }
                        if !ASK_VIEWS.contains(&name.as_str()) && !ctes.contains(&name) {
                            return Err(StoreError::Rejected(format!(
                                "table `{name}` is not available; use one of: {}",
                                ASK_VIEWS.join(", ")
                            )));
                        }
                    }
                    Some("TABLE_FUNCTION") => {
                        return Err(StoreError::Rejected("table functions are not allowed".into()));
                    }
                    _ => {}
                }
                stack.extend(map.values());
            }
            serde_json::Value::Array(a) => stack.extend(a.iter()),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_rejects_writes_and_multi() {
        assert!(precheck_ask_sql("SELECT * FROM v_bars").is_ok());
        assert!(precheck_ask_sql("with x as (select 1) select * from x").is_ok());
        assert!(precheck_ask_sql("SELECT 1; DROP TABLE bars").is_err());
        assert!(precheck_ask_sql("DELETE FROM bars").is_err());
        assert!(precheck_ask_sql("SELECT * FROM v_bars WHERE 1=1 AND update_x = 1").is_ok());
        assert!(precheck_ask_sql("SELECT headline FROM v_news WHERE headline LIKE '%update%'").is_ok());
        assert!(precheck_ask_sql("SELECT * FROM v_bars; ").is_ok());
        assert!(precheck_ask_sql("COPY v_bars TO 'x.csv'").is_err());
    }
}
