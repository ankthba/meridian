//! The function registry: metadata for every function Meridian ships.
//!
//! Every mnemonic here is either verified as a standard terminal mnemonic or
//! approved in `docs/FUNCTIONS.md`. Swift maps each mnemonic to a view
//! factory, and ARCHITECTURE §9.2 requires a test that keeps the Rust and
//! Swift sets identical.

use meridian_types::MarketSector;
use serde::{Deserialize, Serialize};

/// Whether a function operates on a security.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SecurityNeed {
    /// Never takes a security (`TOP`, `WEI`). A typed security is ignored.
    None,
    /// Uses the security when there is one and works without it (`CN`).
    Optional,
    /// Can't run without a security (`DES`).
    Required,
}

/// Grouping used by menus and help screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionCategory {
    Security,
    Charts,
    News,
    Fundamentals,
    Filings,
    Derivatives,
    Analytics,
    Macro,
    Monitors,
    Workspace,
    Ai,
}

/// Static metadata for one function.
///
/// Serialize-only: the registry is compiled in, so there's nothing to
/// deserialize into `&'static` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FunctionSpec {
    /// Upper-case mnemonic typed on the command line, e.g. `DES`.
    pub mnemonic: &'static str,
    /// Short title for menus and autocomplete.
    pub title: &'static str,
    /// One-sentence description for help screens.
    pub description: &'static str,
    /// Lower-case search words for autocomplete.
    pub keywords: &'static [&'static str],
    pub needs_security: SecurityNeed,
    /// Sectors the function accepts. Empty means any sector.
    pub sectors: &'static [MarketSector],
    pub category: FunctionCategory,
}

impl FunctionSpec {
    /// Whether the function uses a security at all.
    #[must_use]
    pub fn takes_security(&self) -> bool {
        self.needs_security != SecurityNeed::None
    }

    /// Whether the function accepts a security of `sector`.
    #[must_use]
    pub fn accepts_sector(&self, sector: MarketSector) -> bool {
        self.sectors.is_empty() || self.sectors.contains(&sector)
    }
}

/// All registered functions, in menu order.
#[must_use]
pub fn registry() -> &'static [FunctionSpec] {
    &REGISTRY
}

/// Finds a function by mnemonic, ignoring ASCII case.
#[must_use]
pub fn lookup(mnemonic: &str) -> Option<&'static FunctionSpec> {
    REGISTRY
        .iter()
        .find(|spec| spec.mnemonic.eq_ignore_ascii_case(mnemonic))
}

const ANY: &[MarketSector] = &[];
const EQUITY: &[MarketSector] = &[MarketSector::Equity];
const EQUITY_INDEX: &[MarketSector] = &[MarketSector::Equity, MarketSector::Index];

use FunctionCategory as C;
use SecurityNeed as S;

static REGISTRY: [FunctionSpec; 32] = [
    // Core market
    FunctionSpec {
        mnemonic: "DES",
        title: "Security Description",
        description: "Profile of the security: business summary, key statistics, and identifiers.",
        keywords: &[
            "description",
            "profile",
            "overview",
            "company",
            "summary",
            "business",
            "statistics",
            "identifiers",
        ],
        needs_security: S::Required,
        sectors: ANY,
        category: C::Security,
    },
    FunctionSpec {
        mnemonic: "GP",
        title: "Historical Price Graph",
        description: "Historical price chart with technical indicators and drawing tools.",
        keywords: &[
            "graph",
            "chart",
            "price",
            "history",
            "historical",
            "candlestick",
            "line",
            "technical",
            "indicators",
            "moving",
            "average",
        ],
        needs_security: S::Required,
        sectors: ANY,
        category: C::Charts,
    },
    FunctionSpec {
        mnemonic: "GIP",
        title: "Intraday Price Graph",
        description: "Intraday chart of the current or a recent trading session.",
        keywords: &[
            "intraday", "graph", "chart", "price", "today", "session", "minute", "tick", "bars",
        ],
        needs_security: S::Required,
        sectors: ANY,
        category: C::Charts,
    },
    FunctionSpec {
        mnemonic: "HP",
        title: "Historical Prices",
        description: "Table of historical open, high, low, close, and volume.",
        keywords: &[
            "historical",
            "prices",
            "history",
            "table",
            "ohlc",
            "open",
            "high",
            "low",
            "close",
            "volume",
            "daily",
        ],
        needs_security: S::Required,
        sectors: ANY,
        category: C::Charts,
    },
    FunctionSpec {
        mnemonic: "W",
        title: "Worksheet",
        description: "Real-time worksheet of securities with configurable columns.",
        keywords: &[
            "worksheet",
            "watchlist",
            "monitor",
            "quotes",
            "list",
            "securities",
            "realtime",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Monitors,
    },
    FunctionSpec {
        mnemonic: "BLP",
        title: "Launchpad",
        description: "Workspace of monitors, charts, news, and function panels across pages and displays.",
        keywords: &[
            "launchpad",
            "workspace",
            "layout",
            "desktop",
            "pages",
            "components",
            "monitor",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Workspace,
    },
    FunctionSpec {
        mnemonic: "SECF",
        title: "Security Finder",
        description: "Search for securities by name, ticker, or asset class.",
        keywords: &[
            "security",
            "finder",
            "search",
            "lookup",
            "find",
            "ticker",
            "symbol",
            "instrument",
            "asset",
            "class",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Security,
    },
    FunctionSpec {
        mnemonic: "MOST",
        title: "Most Active",
        description: "Most active securities ranked by volume, value traded, and price change.",
        keywords: &[
            "most", "active", "volume", "gainers", "losers", "movers", "leaders", "traded",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Monitors,
    },
    // News, fundamentals, filings
    FunctionSpec {
        mnemonic: "N",
        title: "News",
        description: "News main menu: top stories, sources, and topics.",
        keywords: &[
            "news",
            "headlines",
            "stories",
            "menu",
            "sources",
            "topics",
            "wire",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::News,
    },
    FunctionSpec {
        mnemonic: "CN",
        title: "Company News",
        description: "News for the loaded security, or general market news when none is loaded.",
        keywords: &["company", "news", "headlines", "stories", "security"],
        needs_security: S::Optional,
        sectors: ANY,
        category: C::News,
    },
    FunctionSpec {
        mnemonic: "TOP",
        title: "Top News",
        description: "Top headlines across markets.",
        keywords: &["top", "news", "headlines", "stories", "breaking", "market"],
        needs_security: S::None,
        sectors: ANY,
        category: C::News,
    },
    FunctionSpec {
        mnemonic: "FA",
        title: "Financial Analysis",
        description: "Financial statements and ratios: income statement, balance sheet, and cash flow.",
        keywords: &[
            "financial",
            "statements",
            "income",
            "balance",
            "sheet",
            "cash",
            "flow",
            "fundamentals",
            "ratios",
            "revenue",
            "earnings",
            "margins",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "EE",
        title: "Earnings Estimates",
        description: "Consensus analyst estimates for EPS, revenue, and other metrics.",
        keywords: &[
            "earnings",
            "estimates",
            "consensus",
            "eps",
            "forecast",
            "revenue",
            "analysts",
            "guidance",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "ERN",
        title: "Earnings History",
        description: "Reported earnings against consensus, with surprises and price reaction.",
        keywords: &[
            "earnings",
            "history",
            "reported",
            "actual",
            "surprise",
            "beat",
            "miss",
            "consensus",
            "eps",
            "results",
            "quarterly",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "ANR",
        title: "Analyst Recommendations",
        description: "Analyst ratings, price targets, and the consensus recommendation.",
        keywords: &[
            "analyst",
            "recommendations",
            "ratings",
            "buy",
            "sell",
            "hold",
            "target",
            "consensus",
            "upgrade",
            "downgrade",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "HDS",
        title: "Holders",
        description: "Institutional and insider holders of the security.",
        keywords: &[
            "holders",
            "ownership",
            "institutional",
            "insiders",
            "shareholders",
            "funds",
            "stake",
            "13f",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "DVD",
        title: "Dividends and Splits",
        description: "Dividend history and projections, and stock splits.",
        keywords: &[
            "dividends",
            "splits",
            "yield",
            "payout",
            "distributions",
            "ex-date",
            "history",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Fundamentals,
    },
    FunctionSpec {
        mnemonic: "CF",
        title: "Company Filings",
        description: "Regulatory filings such as 10-K, 10-Q, and 8-K, with summaries and changes from the prior filing.",
        keywords: &[
            "company",
            "filings",
            "sec",
            "edgar",
            "10-k",
            "10-q",
            "8-k",
            "annual",
            "quarterly",
            "report",
            "regulatory",
            "documents",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Filings,
    },
    // Derivatives
    FunctionSpec {
        mnemonic: "OMON",
        title: "Option Monitor",
        description: "Option chain with prices, implied volatility, and Greeks.",
        keywords: &[
            "options",
            "chain",
            "monitor",
            "calls",
            "puts",
            "strikes",
            "expiry",
            "implied",
            "volatility",
            "greeks",
            "delta",
            "gamma",
        ],
        needs_security: S::Required,
        sectors: EQUITY_INDEX,
        category: C::Derivatives,
    },
    FunctionSpec {
        mnemonic: "OVDV",
        title: "Volatility Surface",
        description: "Implied volatility surface and skew across strikes and expiries.",
        keywords: &[
            "volatility",
            "surface",
            "implied",
            "skew",
            "smile",
            "term",
            "structure",
            "options",
            "strikes",
            "expiries",
        ],
        needs_security: S::Required,
        sectors: EQUITY_INDEX,
        category: C::Derivatives,
    },
    FunctionSpec {
        mnemonic: "OVME",
        title: "Option Valuation",
        description: "Value options and multi-leg strategies, with payoff and scenario analysis.",
        keywords: &[
            "option",
            "valuation",
            "pricing",
            "calculator",
            "black",
            "scholes",
            "binomial",
            "strategy",
            "payoff",
            "scenario",
            "greeks",
        ],
        needs_security: S::Required,
        sectors: EQUITY_INDEX,
        category: C::Derivatives,
    },
    // Analytics
    FunctionSpec {
        mnemonic: "EQS",
        title: "Equity Screener",
        description: "Screen equities by fundamental and market criteria.",
        keywords: &[
            "equity", "screener", "screen", "filter", "stocks", "criteria", "search",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Analytics,
    },
    FunctionSpec {
        mnemonic: "RV",
        title: "Relative Valuation",
        description: "Compare the security with its peers on valuation and performance.",
        keywords: &[
            "relative",
            "valuation",
            "comps",
            "comparables",
            "peers",
            "multiples",
            "pe",
            "ebitda",
        ],
        needs_security: S::Required,
        sectors: EQUITY,
        category: C::Analytics,
    },
    FunctionSpec {
        mnemonic: "CORR",
        title: "Correlation Matrix",
        description: "Correlation of returns across a set of securities.",
        keywords: &[
            "correlation",
            "matrix",
            "returns",
            "covariance",
            "diversification",
        ],
        needs_security: S::Optional,
        sectors: ANY,
        category: C::Analytics,
    },
    FunctionSpec {
        mnemonic: "PORT",
        title: "Portfolio Analytics",
        description: "Portfolio holdings, performance, and risk.",
        keywords: &[
            "portfolio",
            "holdings",
            "positions",
            "performance",
            "risk",
            "var",
            "attribution",
            "exposure",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Analytics,
    },
    FunctionSpec {
        mnemonic: "BTST",
        title: "Backtester",
        description: "Backtest a trading strategy on the security's price history.",
        keywords: &[
            "backtest",
            "backtester",
            "strategy",
            "simulation",
            "trading",
            "rules",
            "historical",
            "performance",
        ],
        needs_security: S::Required,
        sectors: ANY,
        category: C::Analytics,
    },
    FunctionSpec {
        mnemonic: "ALRT",
        title: "Alerts",
        description: "Create and manage price and event alerts.",
        keywords: &[
            "alerts",
            "alarm",
            "notification",
            "trigger",
            "price",
            "watch",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Monitors,
    },
    // Macro and cross-asset
    FunctionSpec {
        mnemonic: "WEI",
        title: "World Equity Indices",
        description: "Major equity indices worldwide with price and performance.",
        keywords: &[
            "world",
            "equity",
            "indices",
            "indexes",
            "global",
            "markets",
            "stock",
            "regions",
            "performance",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Macro,
    },
    FunctionSpec {
        mnemonic: "ECO",
        title: "Economic Calendar",
        description: "Economic releases with actual, consensus, and prior values, plus economic data series.",
        keywords: &[
            "economic",
            "calendar",
            "releases",
            "macro",
            "data",
            "gdp",
            "cpi",
            "inflation",
            "employment",
            "payrolls",
            "rates",
            "fred",
            "series",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Macro,
    },
    FunctionSpec {
        mnemonic: "FXC",
        title: "FX Cross Rates",
        description: "Matrix of cross exchange rates between major currencies.",
        keywords: &[
            "fx",
            "foreign",
            "exchange",
            "currency",
            "currencies",
            "cross",
            "rates",
            "matrix",
            "forex",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Macro,
    },
    FunctionSpec {
        mnemonic: "CRYP",
        title: "Crypto Monitor",
        description: "Prices and market data for cryptocurrencies.",
        keywords: &[
            "crypto",
            "cryptocurrency",
            "bitcoin",
            "ethereum",
            "digital",
            "assets",
            "coins",
            "monitor",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Monitors,
    },
    // AI
    FunctionSpec {
        mnemonic: "ASK",
        title: "AI Analyst",
        description: "Ask questions about markets and securities in plain language; numbers in answers are checked against the data used.",
        keywords: &[
            "ai",
            "analyst",
            "ask",
            "question",
            "assistant",
            "chat",
            "research",
        ],
        needs_security: S::None,
        sectors: ANY,
        category: C::Ai,
    },
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const EXPECTED: [&str; 32] = [
        "DES", "GP", "GIP", "HP", "W", "BLP", "SECF", "MOST", "N", "CN", "TOP", "FA", "EE", "ERN",
        "ANR", "HDS", "DVD", "CF", "OMON", "OVDV", "OVME", "EQS", "RV", "CORR", "PORT", "BTST",
        "ALRT", "WEI", "ECO", "FXC", "CRYP", "ASK",
    ];

    fn spec(m: &str) -> &'static FunctionSpec {
        lookup(m).unwrap_or_else(|| panic!("{m} missing"))
    }

    #[test]
    fn registry_has_exactly_the_approved_mnemonics() {
        let got: HashSet<&str> = registry().iter().map(|s| s.mnemonic).collect();
        let want: HashSet<&str> = EXPECTED.into_iter().collect();
        assert_eq!(got, want);
        assert_eq!(registry().len(), EXPECTED.len(), "duplicate mnemonic");
    }

    #[test]
    fn lookup_ignores_case() {
        assert_eq!(lookup("des").map(|s| s.mnemonic), Some("DES"));
        assert_eq!(lookup("Omon").map(|s| s.mnemonic), Some("OMON"));
        assert_eq!(lookup("w").map(|s| s.mnemonic), Some("W"));
        assert!(lookup("AAPL").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("DE").is_none());
    }

    #[test]
    fn specs_are_well_formed() {
        for s in registry() {
            assert!(
                !s.mnemonic.is_empty() && s.mnemonic.len() <= 4,
                "{}",
                s.mnemonic
            );
            assert!(
                s.mnemonic.bytes().all(|b| b.is_ascii_uppercase()),
                "{}",
                s.mnemonic
            );
            assert!(!s.title.is_empty(), "{}", s.mnemonic);
            assert!(s.description.ends_with('.'), "{}", s.mnemonic);
            assert!(!s.keywords.is_empty(), "{}", s.mnemonic);
            for k in s.keywords {
                assert_eq!(
                    *k,
                    k.to_lowercase(),
                    "{}: keyword {k} must be lower-case",
                    s.mnemonic
                );
                assert!(
                    !k.contains(' '),
                    "{}: keyword {k} must be one word",
                    s.mnemonic
                );
            }
            // A sector restriction only makes sense for functions that take a security.
            if s.needs_security == SecurityNeed::None {
                assert!(s.sectors.is_empty(), "{}", s.mnemonic);
            }
        }
    }

    #[test]
    fn titles_match_the_brief() {
        let titles = [
            ("GP", "Historical Price Graph"),
            ("GIP", "Intraday Price Graph"),
            ("HP", "Historical Prices"),
            ("W", "Worksheet"),
            ("OMON", "Option Monitor"),
            ("OVDV", "Volatility Surface"),
            ("OVME", "Option Valuation"),
            ("FXC", "FX Cross Rates"),
            ("CRYP", "Crypto Monitor"),
            ("ASK", "AI Analyst"),
        ];
        for (m, t) in titles {
            assert_eq!(spec(m).title, t);
        }
    }

    #[test]
    fn security_needs_and_sectors() {
        for m in ["GP", "GIP", "HP", "DES", "BTST"] {
            assert_eq!(spec(m).needs_security, SecurityNeed::Required, "{m}");
            assert!(spec(m).sectors.is_empty(), "{m}");
        }
        for m in ["FA", "EE", "ERN", "ANR", "HDS", "DVD", "CF", "RV"] {
            assert_eq!(spec(m).needs_security, SecurityNeed::Required, "{m}");
            assert_eq!(spec(m).sectors, &[MarketSector::Equity], "{m}");
        }
        assert_eq!(
            spec("OMON").sectors,
            &[MarketSector::Equity, MarketSector::Index]
        );
        for m in [
            "WEI", "ECO", "FXC", "CRYP", "TOP", "N", "EQS", "BLP", "W", "MOST", "PORT", "ALRT",
            "ASK", "SECF",
        ] {
            assert_eq!(spec(m).needs_security, SecurityNeed::None, "{m}");
        }
        for m in ["CN", "CORR"] {
            assert_eq!(spec(m).needs_security, SecurityNeed::Optional, "{m}");
        }
    }

    #[test]
    fn sector_acceptance() {
        assert!(spec("OMON").accepts_sector(MarketSector::Index));
        assert!(!spec("OMON").accepts_sector(MarketSector::Curncy));
        assert!(spec("GP").accepts_sector(MarketSector::Cmdty));
        assert!(!spec("FA").accepts_sector(MarketSector::Index));
        assert!(!spec("TOP").takes_security());
        assert!(spec("CN").takes_security());
    }

    #[test]
    fn categories() {
        assert_eq!(spec("FA").category, FunctionCategory::Fundamentals);
        assert_eq!(spec("CF").category, FunctionCategory::Filings);
        assert_eq!(spec("OMON").category, FunctionCategory::Derivatives);
        assert_eq!(spec("ECO").category, FunctionCategory::Macro);
        assert_eq!(spec("BLP").category, FunctionCategory::Workspace);
        assert_eq!(spec("ASK").category, FunctionCategory::Ai);
    }
}
