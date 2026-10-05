//! DES — security description.

use std::sync::Arc;

use meridian_types::{AssetClass, BarInterval, MarketSector, NANOS_PER_DAY, PeriodType};

use super::{ScreenRequest, error_screen, require_security};
use crate::core::Engine;
use crate::screen::{Action, Block, ChartSpec, ChartStyle, Field, Format, Screen, Style};

const TITLE: &str = "Description";

pub(crate) async fn des(engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let key = match require_security("DES", TITLE, &req) {
        Ok(k) => k,
        Err(s) => return *s,
    };
    let inst = match engine.instrument(&key).await {
        Ok(i) => i,
        Err(e) => return error_screen("DES", TITLE, Some(&key), &e),
    };
    let ks = key.to_string();
    let now = engine.now();
    let is_company = matches!(inst.asset_class, AssetClass::Equity) && key.sector == MarketSector::Equity;

    let (quote, profile, bars, fundamentals, dividends) = tokio::join!(
        engine.quote_row(&key),
        async { if is_company { engine.router().profile(&key).await.ok() } else { None } },
        engine.bars(&key, BarInterval::Day, Some(now - 366 * NANOS_PER_DAY), now + NANOS_PER_DAY),
        async {
            if is_company {
                engine
                    .router()
                    .fundamentals(meridian_provider::FundamentalsRequest { key: key.clone(), period_type: PeriodType::Quarterly, periods: 8 })
                    .await
                    .ok()
            } else {
                None
            }
        },
        async { if is_company { engine.router().dividends(&key).await.ok() } else { None } },
    );

    let mut s = Screen::new("DES", format!("{} — {}", inst.name, TITLE), Some(ks.clone()));
    let dec = inst.price_decimals;
    if let Some(p) = engine.quote_provenance(&key) {
        s.source(&p);
    }

    // Header block: identity.
    let mut ident = vec![
        Field::text("Name", &inst.name).styled(Style::Emphasis),
        Field::text("Ticker", &ks),
        Field::text("Type", inst.asset_class.label()),
        Field::text("Currency", &inst.currency),
    ];
    if let Some(ex) = inst.exchange_name.as_ref().or(inst.exchange_mic.as_ref()) {
        ident.push(Field::text("Exchange", ex));
    }
    if let Some(sec) = &inst.sector {
        ident.push(Field::text("Sector", sec));
    }
    if let Some(ind) = &inst.industry {
        ident.push(Field::text("Industry", ind));
    }
    if let Some(c) = &inst.country {
        ident.push(Field::text("Country", c));
    }
    if let Some(f) = &inst.figi {
        ident.push(Field::text("FIGI", f));
    }
    s.push(Block::Fields { title: None, columns: 2, fields: ident });

    // Price block.
    let mut price = Vec::new();
    if let Some(q) = &quote {
        price.push(Field::num("Last", finite(q.last), Format::Price { decimals: dec }).styled(Style::Emphasis));
        price.push(Field::num("Net Chg", finite(q.net_change), Format::Change { decimals: dec }));
        price.push(Field::num("% Chg", finite(q.pct_change), Format::ChangePercent { decimals: 2 }));
        price.push(Field::num("Bid", finite(q.bid), Format::Price { decimals: dec }));
        price.push(Field::num("Ask", finite(q.ask), Format::Price { decimals: dec }));
        price.push(Field::num("Open", finite(q.open), Format::Price { decimals: dec }));
        price.push(Field::num("Day High", finite(q.high), Format::Price { decimals: dec }));
        price.push(Field::num("Day Low", finite(q.low), Format::Price { decimals: dec }));
        price.push(Field::num("Prev Close", finite(q.prev_close), Format::Price { decimals: dec }));
        price.push(Field::num("Volume", finite(q.volume), Format::Large { decimals: 2 }));
    }
    let last_px = quote.as_ref().and_then(|q| finite(q.last));
    if let Ok(b) = &bars
        && !b.value.is_empty()
    {
        let hi = b.value.high.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let lo = b.value.low.iter().copied().fold(f64::INFINITY, f64::min);
        price.push(Field::num("52 Wk High", Some(hi), Format::Price { decimals: dec }));
        price.push(Field::num("52 Wk Low", Some(lo), Format::Price { decimals: dec }));
        if let (Some(first), Some(last)) = (b.value.close.first(), last_px.or(b.value.close.last().copied())) {
            price.push(Field::num("1 Yr Chg %", Some((last / first - 1.0) * 100.0), Format::ChangePercent { decimals: 2 }));
        }
        if b.stale {
            s.push(Block::Notice { level: crate::screen::NoticeLevel::Warning, text: "History from offline cache".into() });
        }
    }

    // Valuation from fundamentals (TTM from the last four quarters).
    let mut val = Vec::new();
    let shares = profile.as_ref().and_then(|p| p.value.shares_outstanding);
    if let Some(f) = &fundamentals {
        s.source(&f.value.provenance);
        let mut income: Vec<&meridian_types::Statement> =
            f.value.statements.iter().filter(|st| st.kind == meridian_types::StatementKind::Income).collect();
        income.sort_by_key(|st| st.period_end);
        let last4: Vec<_> = income.iter().rev().take(4).collect();
        if last4.len() == 4 {
            let sum = |code: &str| -> Option<f64> { last4.iter().map(|st| st.value(code)).sum::<Option<f64>>() };
            let eps = sum("eps_diluted");
            let rev = sum("revenue");
            let ni = sum("net_income");
            val.push(Field::num("EPS (TTM)", eps, Format::Number { decimals: 2 }));
            if let (Some(px), Some(e)) = (last_px, eps)
                && e > 0.0
            {
                val.push(Field::num("P/E (TTM)", Some(px / e), Format::Number { decimals: 2 }));
            }
            val.push(Field::num("Revenue (TTM)", rev, Format::Large { decimals: 2 }));
            val.push(Field::num("Net Income (TTM)", ni, Format::Large { decimals: 2 }));
            if let (Some(r), Some(n)) = (rev, ni)
                && r != 0.0
            {
                val.push(Field::num("Net Margin %", Some(n / r * 100.0), Format::Percent { decimals: 2 }));
            }
        }
    }
    if let (Some(sh), Some(px)) = (shares, last_px) {
        val.push(Field::num("Shares Out", Some(sh), Format::Large { decimals: 2 }));
        val.push(Field::num("Market Cap", Some(sh * px), Format::Large { decimals: 2 }));
    }
    if let Some(d) = &dividends {
        s.source(&d.value.provenance);
        let year_ago = meridian_types::nanos_to_date(now - 365 * NANOS_PER_DAY);
        let ttm: f64 = d
            .value
            .dividends
            .iter()
            .filter(|x| x.ex_date > year_ago && !matches!(x.kind, meridian_types::DividendKind::Split))
            .map(|x| x.amount)
            .sum();
        if ttm > 0.0 {
            val.push(Field::num("Div (TTM)", Some(ttm), Format::Number { decimals: 2 }));
            if let Some(px) = last_px {
                val.push(Field::num("Div Yield %", Some(ttm / px * 100.0), Format::Percent { decimals: 2 }));
            }
        }
    }
    if !price.is_empty() || !val.is_empty() {
        let mut fields = price;
        fields.extend(val);
        s.push(Block::Fields { title: Some("Price & Valuation".into()), columns: 3, fields });
    }

    s.push(Block::Chart(ChartSpec {
        security: ks.clone(),
        interval: "1d".into(),
        range: "1Y".into(),
        style: ChartStyle::Mountain,
        indicators: vec![],
        height_rows: 10,
    }));

    if let Some(p) = &profile {
        s.source(&meridian_types::Provenance { as_of: now, ..p_provenance(&engine, &p.provider) });
        let pr = &p.value;
        if let Some(d) = &pr.description {
            s.push(Block::Text { title: Some("Business Description".into()), body: d.clone() });
        }
        let mut info = Vec::new();
        info.push(Field::opt_text("Headquarters", pr.headquarters.clone()));
        info.push(Field::opt_text("Website", pr.website.clone()));
        if let Some(e) = pr.employees {
            info.push(Field::num("Employees", Some(e as f64), Format::Integer));
        }
        info.push(Field::opt_text("CEO", pr.ceo.clone()));
        info.push(Field::opt_text("Founded", pr.founded.clone()));
        info.push(Field::opt_text("Fiscal Year End", pr.fiscal_year_end.clone()));
        info.push(Field::opt_text("IPO Date", pr.ipo_date.clone()));
        if let (Some(code), Some(desc)) = (&pr.sic_code, &pr.sic_description) {
            info.push(Field::text("SIC", format!("{code} {desc}")));
        }
        info.retain(|f| f.text.is_some() || f.value.is_some());
        if !info.is_empty() {
            s.push(Block::Fields { title: Some("Company Info".into()), columns: 2, fields: info });
        }
    }

    // Related functions, numbered like the incumbent's DES pages.
    let related: &[(&str, &str)] = if is_company {
        &[
            ("Price Graph", "GP"),
            ("Historical Prices", "HP"),
            ("Financials", "FA"),
            ("Earnings Estimates", "EE"),
            ("Earnings History", "ERN"),
            ("Analyst Recs", "ANR"),
            ("Holders", "HDS"),
            ("Dividends", "DVD"),
            ("Company News", "CN"),
            ("Filings", "CF"),
            ("Options", "OMON"),
        ]
    } else {
        &[("Price Graph", "GP"), ("Intraday Graph", "GIP"), ("Historical Prices", "HP"), ("News", "CN")]
    };
    for (label, f) in related {
        s.menu_item(label, Action::new(f, Some(&ks)), false);
    }
    s
}

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

fn p_provenance(engine: &Engine, provider: &meridian_types::ProviderId) -> meridian_types::Provenance {
    let caps = engine.router().capabilities();
    let (delay, source, attribution) = caps
        .iter()
        .find(|(id, _)| id == provider)
        .and_then(|(_, c)| c.entry(meridian_provider::Capability::Profile, None).map(|e| (e.delay, e.source.clone(), c.attribution.clone())))
        .unwrap_or((meridian_types::DataDelay::EndOfDay, meridian_types::FeedSource::Official, None));
    meridian_types::Provenance {
        provider: provider.clone(),
        synthetic: delay == meridian_types::DataDelay::Synthetic,
        delay,
        source,
        as_of: 0,
        source_ref: None,
        attribution,
    }
}
