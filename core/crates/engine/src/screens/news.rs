//! N, TOP, CN — news lists and story view.

use std::sync::Arc;

use meridian_provider::{CachePolicy, NewsQuery, NewsScope};
use meridian_types::{NANOS_PER_DAY, NewsItem};

use super::{ScreenRequest, error_screen, fmt_datetime};
use crate::core::Engine;
use crate::error::EngineResult;
use crate::screen::{Action, Block, Cell, Column, Field, Format, Input, InputKind, NoticeLevel, Row, Screen, Style, Table};

impl Engine {
    /// News through the router, persisted (when the provider's terms allow)
    /// so ASK and offline mode can search it.
    pub async fn news_items(&self, q: NewsQuery) -> EngineResult<Vec<NewsItem>> {
        match self.router().news(q.clone()).await {
            Ok(page) => {
                let caps = self.router().capabilities();
                let storable: Vec<NewsItem> = page
                    .items
                    .iter()
                    .filter(|n| {
                        caps.iter()
                            .find(|(id, _)| *id == n.provenance.provider)
                            .is_some_and(|(_, c)| !matches!(c.cache_policy, CachePolicy::NoStore))
                    })
                    .cloned()
                    .collect();
                if !storable.is_empty()
                    && let Err(e) = self.stores().market.put_news(&storable)
                {
                    tracing::warn!(error = %e, "news cache write failed");
                }
                Ok(page.items)
            }
            Err(e) => {
                // Offline: fall back to cached stories.
                let tickers: Vec<String> = q.keys.iter().map(|k| k.symbol.clone()).collect();
                let cached = self.stores().market.search_news(&tickers, q.text.as_deref(), q.limit).unwrap_or_default();
                if cached.is_empty() { Err(e.into()) } else { Ok(cached) }
            }
        }
    }
}

fn title_for(function: &str) -> &'static str {
    match function {
        "TOP" => "Top News",
        "CN" => "Company News",
        _ => "News",
    }
}

pub(crate) async fn news(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let function = req.function.clone();
    let title = title_for(&function);
    let scope = match function.as_str() {
        "TOP" => NewsScope::Top,
        "CN" => NewsScope::Company,
        _ => match req.arg("scope") {
            Some("press") => NewsScope::PressReleases,
            _ => NewsScope::Market,
        },
    };
    if scope == NewsScope::Company && req.security.is_none() {
        return Screen::not_available("CN", title, None, "CN needs a security, e.g. AAPL US <EQUITY> CN <GO>");
    }
    let text = req.arg("q").map(str::to_owned).filter(|t| !t.trim().is_empty());
    let now = engine.now();
    let q = NewsQuery {
        scope,
        keys: req.security.iter().cloned().collect(),
        text: text.clone(),
        from: Some(now - 7 * NANOS_PER_DAY),
        to: None,
        limit: 200,
    };
    let items = match engine.news_items(q).await {
        Ok(i) => i,
        Err(e) => return error_screen(&function, title, req.security.as_ref(), &e),
    };
    let sec = req.security.as_ref().map(ToString::to_string);

    if let Some(story_id) = req.arg("story") {
        return story(&function, sec, &items, story_id);
    }

    let heading = match &req.security {
        Some(k) => format!("{k} — {title}"),
        None => title.to_string(),
    };
    let mut s = Screen::new(&function, heading, sec.clone());
    for it in items.iter().take(5) {
        s.source(&it.provenance);
    }
    if function == "N" {
        s.menu_item("Top News", Action::new("TOP", None), false);
        s.menu_item("Market News", Action::new("N", None), scope == NewsScope::Market);
        s.menu_item("Press Releases", Action::new("N", None).arg("scope", "press"), scope == NewsScope::PressReleases);
        if let Some(k) = &sec {
            s.menu_item("Company News", Action::new("CN", Some(k)), false);
        }
    }
    s.push(Block::Inputs {
        title: None,
        inputs: vec![Input { id: "q".into(), label: "Search".into(), value: text.unwrap_or_default(), kind: InputKind::Text, options: vec![] }],
    });
    if items.is_empty() {
        s.push(Block::Notice { level: NoticeLevel::Info, text: "No stories match".into() });
        return s;
    }
    let mut rows = Vec::with_capacity(items.len());
    for it in &items {
        let tickers = it.tickers.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        rows.push(
            Row::new(vec![
                Cell::num(Some(it.published_at as f64)),
                Cell::text(&it.source).styled(Style::Muted),
                Cell::text(&it.headline).styled(Style::Emphasis),
                Cell::text(tickers),
            ])
            .action(Action::new(&function, sec.as_deref()).with_args(&req.args).arg("story", it.id.clone())),
        );
    }
    s.push(Block::Table(Table {
        title: None,
        columns: vec![
            Column::num("Time", Format::DateTime, 16),
            Column::text("Source", 14),
            Column::text("Headline", 80),
            Column::text("Tickers", 14),
        ],
        rows,
        page_size: Some(20),
        numbered: true,
    }));
    s.refresh_ms = Some(60_000);
    s
}

fn story(function: &str, sec: Option<String>, items: &[NewsItem], id: &str) -> Screen {
    let Some(it) = items.iter().find(|i| i.id == id) else {
        return Screen::not_available(function, "Story", sec, "story is no longer in the feed window");
    };
    let mut s = Screen::new(function, it.headline.clone(), sec);
    s.source(&it.provenance);
    let mut fields = vec![
        Field::text("Source", &it.source),
        Field::text("Published", fmt_datetime(it.published_at)),
    ];
    if !it.tickers.is_empty() {
        fields.push(Field::text("Tickers", it.tickers.join(", ")));
    }
    if !it.topics.is_empty() {
        fields.push(Field::text("Topics", it.topics.join(", ")));
    }
    if let Some(u) = &it.url {
        fields.push(Field::text("Link", u).styled(Style::Link));
    }
    s.push(Block::Fields { title: None, columns: 2, fields });
    match (&it.body, &it.summary) {
        (Some(body), _) => s.push(Block::Text { title: None, body: body.clone() }),
        (None, Some(sum)) => {
            s.push(Block::Text { title: None, body: sum.clone() });
            s.push(Block::Notice {
                level: NoticeLevel::Info,
                text: "Full text is not licensed by this source; open the link to read the story.".into(),
            });
        }
        (None, None) => s.push(Block::Notice { level: NoticeLevel::Info, text: "Headline only; open the link to read the story.".into() }),
    }
    s
}
