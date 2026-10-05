//! Alert rules (ALRT) and their evaluation.
//!
//! Conditions are edge-triggered: an alert fires when its condition goes
//! from false to true (or is true on the first observation), not on every
//! tick while it stays true. `repeat` controls whether it re-arms.

use std::collections::HashMap;

use meridian_types::{NewsItem, SecurityKey, UnixNanos, NANOS_PER_SEC};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AlertCondition {
    PriceAbove { level: f64 },
    PriceBelow { level: f64 },
    /// Daily % change ≥ `pct` (use negative for drops with `PctChangeBelow`).
    PctChangeAbove { pct: f64 },
    PctChangeBelow { pct: f64 },
    VolumeAbove { volume: f64 },
    /// Headline or summary mentions `keyword` (case-insensitive), for news
    /// tagged with the rule's security.
    NewsKeyword { keyword: String },
}

impl AlertCondition {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            AlertCondition::PriceAbove { level } => format!("Last ≥ {level}"),
            AlertCondition::PriceBelow { level } => format!("Last ≤ {level}"),
            AlertCondition::PctChangeAbove { pct } => format!("Chg % ≥ {pct}"),
            AlertCondition::PctChangeBelow { pct } => format!("Chg % ≤ {pct}"),
            AlertCondition::VolumeAbove { volume } => format!("Volume ≥ {volume}"),
            AlertCondition::NewsKeyword { keyword } => format!("News mentions \"{keyword}\""),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AlertRepeat {
    /// Fire once, then disable.
    Once,
    /// Re-arm each time the condition becomes false again.
    EveryCross,
    /// Re-arm after a cooldown even if the condition stays true.
    Cooldown { seconds: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: i64,
    pub security: SecurityKey,
    pub condition: AlertCondition,
    pub repeat: AlertRepeat,
    pub note: Option<String>,
    pub enabled: bool,
}

/// Quote fields alerts look at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteView {
    pub last: f64,
    pub pct_change: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiredAlert {
    pub rule_id: i64,
    pub security: SecurityKey,
    pub message: String,
    pub value: Option<f64>,
    pub at: UnixNanos,
    /// Whether the rule should now be disabled (`Once`).
    pub disable: bool,
}

#[derive(Debug, Default, Clone)]
struct RuleState {
    was_true: Option<bool>,
    last_fired: Option<UnixNanos>,
    done: bool,
}

#[derive(Debug, Default)]
pub struct AlertEngine {
    rules: Vec<AlertRule>,
    state: HashMap<i64, RuleState>,
}

impl AlertEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the rule set, keeping state for rules that still exist.
    pub fn set_rules(&mut self, rules: Vec<AlertRule>) {
        self.state.retain(|id, _| rules.iter().any(|r| r.id == *id));
        self.rules = rules;
    }

    #[must_use]
    pub fn rules(&self) -> &[AlertRule] {
        &self.rules
    }

    /// Securities with enabled quote-based rules (to subscribe).
    #[must_use]
    pub fn watched_securities(&self) -> Vec<SecurityKey> {
        let mut v: Vec<SecurityKey> = self
            .rules
            .iter()
            .filter(|r| r.enabled && !matches!(r.condition, AlertCondition::NewsKeyword { .. }))
            .map(|r| r.security.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    fn step(&mut self, rule: &AlertRule, is_true: bool, value: Option<f64>, now: UnixNanos, what: &str) -> Option<FiredAlert> {
        let st = self.state.entry(rule.id).or_default();
        if st.done {
            return None;
        }
        let rising = is_true && st.was_true != Some(true);
        let cooled = match (rule.repeat, st.last_fired) {
            (AlertRepeat::Cooldown { seconds }, Some(t)) => {
                is_true && now - t >= i64::try_from(seconds).unwrap_or(i64::MAX).saturating_mul(NANOS_PER_SEC)
            }
            _ => false,
        };
        st.was_true = Some(is_true);
        if !(rising || cooled) {
            return None;
        }
        st.last_fired = Some(now);
        let disable = rule.repeat == AlertRepeat::Once;
        if disable {
            st.done = true;
        }
        let note = rule.note.as_deref().map(|n| format!(" — {n}")).unwrap_or_default();
        Some(FiredAlert {
            rule_id: rule.id,
            security: rule.security.clone(),
            message: format!("{}: {} ({what}){note}", rule.security, rule.condition.describe()),
            value,
            at: now,
            disable,
        })
    }

    /// Evaluates quote rules for `key`.
    pub fn on_quote(&mut self, key: &SecurityKey, q: QuoteView, now: UnixNanos) -> Vec<FiredAlert> {
        let rules: Vec<AlertRule> =
            self.rules.iter().filter(|r| r.enabled && &r.security == key).cloned().collect();
        let mut out = Vec::new();
        for r in &rules {
            let (is_true, value, what) = match &r.condition {
                AlertCondition::PriceAbove { level } => (q.last >= *level, q.last, format!("last {}", q.last)),
                AlertCondition::PriceBelow { level } => (q.last <= *level, q.last, format!("last {}", q.last)),
                AlertCondition::PctChangeAbove { pct } => (q.pct_change >= *pct, q.pct_change, format!("{:+.2}%", q.pct_change)),
                AlertCondition::PctChangeBelow { pct } => (q.pct_change <= *pct, q.pct_change, format!("{:+.2}%", q.pct_change)),
                AlertCondition::VolumeAbove { volume } => (q.volume >= *volume, q.volume, format!("vol {}", q.volume)),
                AlertCondition::NewsKeyword { .. } => continue,
            };
            if value.is_nan() {
                continue;
            }
            if let Some(f) = self.step(r, is_true, Some(value), now, &what) {
                out.push(f);
            }
        }
        out
    }

    /// Evaluates news-keyword rules against one item.
    pub fn on_news(&mut self, item: &NewsItem, now: UnixNanos) -> Vec<FiredAlert> {
        let rules: Vec<AlertRule> = self
            .rules
            .iter()
            .filter(|r| r.enabled && matches!(r.condition, AlertCondition::NewsKeyword { .. }))
            .cloned()
            .collect();
        let text = format!("{} {}", item.headline, item.summary.as_deref().unwrap_or("")).to_lowercase();
        let mut out = Vec::new();
        for r in &rules {
            let AlertCondition::NewsKeyword { keyword } = &r.condition else { continue };
            let tagged = item.tickers.iter().any(|t| t.eq_ignore_ascii_case(&r.security.symbol));
            if !tagged || !text.contains(&keyword.to_lowercase()) {
                continue;
            }
            // Each matching story is a new event: reset the edge.
            self.state.entry(r.id).or_default().was_true = Some(false);
            if let Some(mut f) = self.step(r, true, None, now, &item.headline) {
                f.message = format!("{}: news \"{}\" — {}", r.security, keyword, item.headline);
                out.push(f);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use meridian_types::Provenance;

    use super::*;

    fn rule(id: i64, cond: AlertCondition, repeat: AlertRepeat) -> AlertRule {
        AlertRule { id, security: SecurityKey::equity("AAPL"), condition: cond, repeat, note: None, enabled: true }
    }

    fn q(last: f64) -> QuoteView {
        QuoteView { last, pct_change: 0.0, volume: 0.0 }
    }

    #[test]
    fn fires_on_crossing_only() {
        let mut e = AlertEngine::new();
        e.set_rules(vec![rule(1, AlertCondition::PriceAbove { level: 100.0 }, AlertRepeat::EveryCross)]);
        let k = SecurityKey::equity("AAPL");
        assert!(e.on_quote(&k, q(99.0), 0).is_empty());
        assert_eq!(e.on_quote(&k, q(101.0), 1).len(), 1);
        assert!(e.on_quote(&k, q(102.0), 2).is_empty(), "still above: no repeat");
        assert!(e.on_quote(&k, q(98.0), 3).is_empty());
        assert_eq!(e.on_quote(&k, q(100.5), 4).len(), 1, "re-armed after falling back");
    }

    #[test]
    fn once_disables() {
        let mut e = AlertEngine::new();
        e.set_rules(vec![rule(1, AlertCondition::PriceBelow { level: 50.0 }, AlertRepeat::Once)]);
        let k = SecurityKey::equity("AAPL");
        let f = e.on_quote(&k, q(40.0), 0);
        assert!(f[0].disable);
        e.on_quote(&k, q(60.0), 1);
        assert!(e.on_quote(&k, q(40.0), 2).is_empty());
    }

    #[test]
    fn cooldown_refires_while_true() {
        let mut e = AlertEngine::new();
        e.set_rules(vec![rule(1, AlertCondition::PriceAbove { level: 1.0 }, AlertRepeat::Cooldown { seconds: 60 })]);
        let k = SecurityKey::equity("AAPL");
        assert_eq!(e.on_quote(&k, q(2.0), 0).len(), 1);
        assert!(e.on_quote(&k, q(2.0), 30 * NANOS_PER_SEC).is_empty());
        assert_eq!(e.on_quote(&k, q(2.0), 61 * NANOS_PER_SEC).len(), 1);
    }

    #[test]
    fn nan_values_are_ignored() {
        let mut e = AlertEngine::new();
        e.set_rules(vec![rule(1, AlertCondition::PriceAbove { level: 1.0 }, AlertRepeat::EveryCross)]);
        assert!(e.on_quote(&SecurityKey::equity("AAPL"), q(f64::NAN), 0).is_empty());
    }

    #[test]
    fn news_keyword() {
        let mut e = AlertEngine::new();
        e.set_rules(vec![rule(1, AlertCondition::NewsKeyword { keyword: "guidance".into() }, AlertRepeat::EveryCross)]);
        let item = NewsItem {
            id: "1".into(),
            source: "Wire".into(),
            headline: "Apple raises guidance".into(),
            summary: None,
            body: None,
            url: None,
            published_at: 0,
            received_at: 0,
            tickers: vec!["AAPL".into()],
            topics: vec![],
            provenance: Provenance::synthetic(0),
        };
        assert_eq!(e.on_news(&item, 0).len(), 1);
        assert_eq!(e.on_news(&item, 1).len(), 1, "each story fires");
        let mut other = item.clone();
        other.tickers = vec!["MSFT".into()];
        assert!(e.on_news(&other, 2).is_empty());
    }

    #[test]
    fn serde_round_trip() {
        let r = rule(3, AlertCondition::PctChangeBelow { pct: -5.0 }, AlertRepeat::Cooldown { seconds: 300 });
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<AlertRule>(&j).unwrap(), r);
    }
}
