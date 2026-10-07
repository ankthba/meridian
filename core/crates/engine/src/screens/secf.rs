//! SECF — security finder, and the security main menu (security + GO
//! without a function).

use std::sync::Arc;

use meridian_command::{IndexedInstrument, ParseContext, ParsedCommand, SuggestIndex, SuggestionKind, registry};
use meridian_provider::InstrumentQuery;
use meridian_types::{AssetClass, MarketSector, SecurityKey};
use parking_lot::RwLock;

use super::ScreenRequest;
use crate::core::Engine;
use crate::screen::{Action, Block, Cell, Column, Field, Input, InputKind, NoticeLevel, Row, Screen, Style, Table};

static SUGGEST: std::sync::LazyLock<RwLock<SuggestIndex>> = std::sync::LazyLock::new(|| RwLock::new(SuggestIndex::new(Vec::new())));

fn popularity(a: AssetClass) -> f32 {
    match a {
        AssetClass::Index | AssetClass::Etf => 0.8,
        AssetClass::Equity | AssetClass::Crypto | AssetClass::Fx => 0.6,
        _ => 0.3,
    }
}

/// Rebuilds the autocomplete index from the loaded universe.
pub(crate) fn rebuild(engine: &Engine) {
    let list: Vec<IndexedInstrument> = engine
        .universe()
        .into_iter()
        .map(|i| IndexedInstrument { popularity: popularity(i.asset_class), key: i.key, name: i.name })
        .collect();
    *SUGGEST.write() = SuggestIndex::new(list);
}

impl Engine {
    /// Command-line autocomplete, grouped for the completion popover.
    #[must_use]
    pub fn suggest(&self, input: &str, loaded: Option<&SecurityKey>, limit: usize) -> Vec<meridian_command::Suggestion> {
        let ctx = ParseContext { loaded: loaded.cloned() };
        SUGGEST.read().suggest(input, &ctx, limit)
    }

    /// What GO runs for `input`: plain language resolved against the loaded
    /// instrument index, or the mnemonic grammar's reading
    /// ([`meridian_command::interpret`]). Before the universe loads, bare
    /// tickers stay security searches.
    #[must_use]
    pub fn interpret(&self, input: &str, loaded: Option<&SecurityKey>) -> ParsedCommand {
        let ctx = ParseContext { loaded: loaded.cloned() };
        SUGGEST.read().interpret(input, &ctx)
    }

    #[must_use]
    pub fn suggest_index_len(&self) -> usize {
        SUGGEST.read().len()
    }
}

pub(crate) async fn secf(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let text = req.arg("q").unwrap_or("").trim().to_owned();
    let sector = req.arg("sector").and_then(MarketSector::parse_label);
    let mut s = Screen::new("SECF", "Security Finder", None);
    s.push(Block::Inputs {
        title: None,
        inputs: vec![
            Input { id: "q".into(), label: "Name or Ticker".into(), value: text.clone(), kind: InputKind::Text, options: vec![] },
            Input {
                id: "sector".into(),
                label: "Sector".into(),
                value: sector.map_or("All".into(), |x| x.key_cap().into()),
                kind: InputKind::Choice,
                options: std::iter::once("All".to_string()).chain(MarketSector::ALL.iter().map(|x| x.key_cap().to_string())).collect(),
            },
        ],
    });
    if text.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "Type a company name or ticker in the highlighted field".into() });
        return s;
    }
    // Local index first (fast), then providers.
    let mut results: Vec<(SecurityKey, String)> = engine
        .suggest(&text, None, 60)
        .into_iter()
        .filter(|x| x.kind == SuggestionKind::Security)
        .filter_map(|x| x.display.parse::<SecurityKey>().ok().map(|k| (k, x.detail)))
        .collect();
    if results.len() < 10
        && let Ok(more) = engine.router().search(InstrumentQuery { text: text.clone(), sector, limit: 60 }).await
    {
        for i in more {
            if !results.iter().any(|(k, _)| *k == i.key) {
                results.push((i.key, i.name));
            }
        }
    }
    if let Some(sec) = sector {
        results.retain(|(k, _)| k.sector == sec);
    }
    let rows: Vec<Row> = results
        .iter()
        .map(|(k, name)| {
            let ks = k.to_string();
            Row::new(vec![Cell::text(&ks).styled(Style::Link), Cell::text(name), Cell::text(k.sector.key_cap())])
                .action(Action::new("DES", Some(&ks)))
        })
        .collect();
    if rows.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: format!("No securities match \"{text}\"") });
        return s;
    }
    s.push(Block::Table(Table {
        title: None,
        columns: vec![Column::text("Security", 22), Column::text("Name", 44), Column::text("Sector", 8)],
        rows,
        page_size: Some(22),
        numbered: true,
    }));
    s
}

/// Numbered list of functions that apply to a security.
pub(crate) async fn security_menu(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let Some(key) = req.security.clone() else {
        return Screen::not_available("MENU", "Menu", None, "load a security first");
    };
    let ks = key.to_string();
    let name = engine.instrument(&key).await.map(|i| i.name).unwrap_or_default();
    let mut s = Screen::new("MENU", format!("{ks} — {name}"), Some(ks.clone()));
    if let Some(row) = engine.quote_row(&key).await {
        let dec = engine.price_decimals(&key);
        s.push(Block::Fields {
            title: None,
            columns: 3,
            fields: vec![
                Field::num("Last", row.last.is_finite().then_some(row.last), crate::screen::Format::Price { decimals: dec }).styled(Style::Emphasis),
                Field::num("Net Chg", row.net_change.is_finite().then_some(row.net_change), crate::screen::Format::Change { decimals: dec }),
                Field::num("% Chg", row.pct_change.is_finite().then_some(row.pct_change), crate::screen::Format::ChangePercent { decimals: 2 }),
            ],
        });
    }
    if let Some(p) = engine.quote_provenance(&key) {
        s.source(&p);
    }
    let rows: Vec<Row> = registry()
        .iter()
        .filter(|f| f.takes_security() && f.accepts_sector(key.sector))
        .map(|f| {
            Row::new(vec![Cell::text(f.mnemonic).styled(Style::Emphasis), Cell::text(f.title), Cell::text(f.description).styled(Style::Muted)])
                .action(Action::new(f.mnemonic, Some(&ks)))
        })
        .collect();
    s.push(Block::Table(Table {
        title: Some("Functions".into()),
        columns: vec![Column::text("Function", 9), Column::text("Title", 28), Column::text("Description", 60)],
        rows,
        page_size: Some(24),
        numbered: true,
    }));
    s
}
