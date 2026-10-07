//! HELP — command syntax, the key map, and a function directory built from
//! the command registry; `topic=<MNEMONIC>` explains one function.
//!
//! The key map mirrors the Swift shell (`Shell/KeyRouter.swift` and the
//! `KeyboardOverlay` rows in `Shell/MainWindow.swift`); a test reads both
//! files so the lists can't drift apart silently.

use std::sync::Arc;

use meridian_command::{FunctionCategory, FunctionSpec, SecurityNeed, lookup, registry};
use meridian_types::SecurityKey;

use super::ScreenRequest;
use crate::core::Engine;
use crate::screen::{Action, Block, Cell, Column, Field, Input, InputKind, Row, Screen, Style, Table};

const TITLE: &str = "Help";
/// The `topic` value (and first choice) meaning the overview page.
const OVERVIEW: &str = "Overview";

/// The `KeyboardOverlay` rows, verbatim and in order.
const OVERLAY_KEYS: &[(&str, &str)] = &[
    ("GO", "Return"),
    ("CANCEL", "Esc"),
    ("MENU (back)", "⌘[  ·  End  ·  Delete on empty line"),
    ("HELP", "F1  ·  ⌘?   (twice: function directory)"),
    ("PAGE FWD / BACK", "PgDn / PgUp  ·  ⌘↓ / ⌘↑"),
    ("PANEL (next / prev)", "⌃Tab / ⌃⇧Tab  ·  ⌘1–⌘4"),
    ("Autocomplete", "↑ ↓ to choose · Tab to accept"),
    ("Numbered item", "<n> Return"),
    ("GOVT", "F2 · ⌥1"),
    ("CORP", "F3 · ⌥2"),
    ("MTGE", "F4 · ⌥3"),
    ("M-MKT", "F5 · ⌥4"),
    ("MUNI", "F6 · ⌥5"),
    ("PFD", "F7 · ⌥6"),
    ("EQUITY", "F8 · ⌥7"),
    ("CMDTY", "F9 · ⌥8"),
    ("INDEX", "F10 · ⌥9"),
    ("CRNCY", "F11 · ⌥0"),
    ("Launchpad", "BLP <GO>"),
    ("Settings / API keys", "⌘,"),
];

/// Bound in `KeyRouter` (⌘/ → keyboard overlay) but not listed in the overlay.
const ROUTER_ONLY_KEYS: &[(&str, &str)] = &[("Key reference overlay", "⌘/")];

const SYNTAX: &str = "\
<security> <yellow key> <function> <GO>     AAPL US <EQUITY> DES <GO>
<function> <GO>                              runs on the panel's loaded security: DES <GO>
<security> <yellow key> <GO>                 loads the security and opens its function menu
<n> <GO>                                     selects numbered item n
any other text <GO>                          searches for securities (SECF)

Yellow keys: GOVT  CORP  MTGE  M-MKT  MUNI  PFD  EQUITY  CMDTY  INDEX  CRNCY
A security is SYMBOL [EXCHANGE] <yellow key>; equities without an exchange are US.
HELP <GO> opens this page; pick a function under \"Help on\" for its own page.";

/// Where a function's data comes from in LIVE mode, named as in
/// Settings → Setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Src {
    Alpaca,
    Edgar,
    Fred,
    Finnhub,
    Anthropic,
    /// Coinbase, Kraken, Frankfurter (ECB), US Treasury, RSS press releases.
    Keyless,
}

impl Src {
    fn label(self) -> &'static str {
        match self {
            Src::Alpaca => "Alpaca",
            Src::Edgar => "SEC EDGAR",
            Src::Fred => "FRED",
            Src::Finnhub => "Finnhub",
            Src::Anthropic => "Anthropic",
            Src::Keyless => "Keyless",
        }
    }
}

/// Help content the registry doesn't hold.
struct Topic {
    mnemonic: &'static str,
    /// (argument, values and meaning).
    args: &'static [(&'static str, &'static str)],
    /// A command line that runs the function (without `<GO>`).
    example: &'static str,
    /// Extra usage note (typed shortcuts), or "".
    usage: &'static str,
    /// LIVE-mode sources and what each provides; empty when none applies.
    live: &'static [(Src, &'static str)],
    /// Shown when `live` is empty, or as a caveat.
    live_note: &'static str,
    related: &'static [&'static str],
}

const BARS_LIVE: &[(Src, &str)] = &[
    (Src::Alpaca, "US stocks and ETFs (consolidated history; on the free plan it ends 15 minutes before now)"),
    (Src::Keyless, "crypto from Coinbase and Kraken; FX daily reference rates from Frankfurter (ECB)"),
];
const QUOTES_LIVE: &[(Src, &str)] = &[
    (Src::Alpaca, "US stock and ETF quotes (IEX only on the free plan, streamed for up to 30 symbols)"),
    (Src::Keyless, "crypto from Coinbase and Kraken; FX reference rates from Frankfurter (ECB)"),
];

const TOPICS: &[Topic] = &[
    Topic {
        mnemonic: "DES",
        args: &[],
        example: "AAPL US <EQUITY> DES",
        usage: "",
        live: &[
            (Src::Alpaca, "price, change, volume and 52-week range for US stocks and ETFs; dividends for yield"),
            (Src::Edgar, "company name, industry, address and financial figures"),
            (Src::Finnhub, "company profile when SEC EDGAR has none"),
            (Src::Keyless, "quotes for crypto and FX securities"),
        ],
        live_note: "",
        related: &["GP", "FA", "CN", "DVD", "ANR"],
    },
    Topic {
        mnemonic: "GP",
        args: &[
            ("range", "1D 5D 1M 3M 6M YTD 1Y 2Y 3Y 5Y 10Y 20Y MAX (default 1Y)"),
            ("interval", "bar size: 1m 5m 15m 30m 1h 1d 1w 1mo (default depends on the range)"),
            ("style", "Candles, Line, Mountain or Bars (default Candles)"),
            ("studies", "comma list: SMA:n EMA:n BB:n:k VWAP RSI:n MACD:a:b:c ATR:n STOCH:k:d OBV (default SMA:50,SMA:200)"),
        ],
        example: "AAPL US <EQUITY> GP",
        usage: "Type a range after the mnemonic: AAPL US <EQUITY> GP 5Y <GO>",
        live: BARS_LIVE,
        live_note: "",
        related: &["GIP", "HP", "DES", "BTST"],
    },
    Topic {
        mnemonic: "GIP",
        args: &[
            ("range", "1D 5D 1M (default 1D)"),
            ("interval", "bar size: 1m 5m 15m 30m 1h (default depends on the range)"),
            ("style", "Candles, Line, Mountain or Bars (default Candles)"),
            ("studies", "comma list as for GP (default VWAP)"),
        ],
        example: "AAPL US <EQUITY> GIP",
        usage: "Type a range after the mnemonic: AAPL US <EQUITY> GIP 5D <GO>",
        live: BARS_LIVE,
        live_note: "",
        related: &["GP", "HP", "W"],
    },
    Topic {
        mnemonic: "HP",
        args: &[
            ("period", "Daily, Weekly or Monthly (default Daily)"),
            ("start", "first date, MM/DD/YYYY or YYYY-MM-DD (default: 3 months, 1 year or 5 years back by period)"),
            ("end", "last date (default today)"),
        ],
        example: "AAPL US <EQUITY> HP",
        usage: "Type the period after the mnemonic: AAPL US <EQUITY> HP WEEKLY <GO>",
        live: BARS_LIVE,
        live_note: "",
        related: &["GP", "GIP", "DVD"],
    },
    Topic {
        mnemonic: "W",
        args: &[
            ("list", "watchlist to show (chosen from the menu)"),
            ("add", "security to add, e.g. MSFT US Equity"),
            ("remove", "security to remove"),
            ("new", "name of a new watchlist"),
        ],
        example: "W",
        usage: "",
        live: QUOTES_LIVE,
        live_note: "",
        related: &["MOST", "WEI", "ALRT", "BLP"],
    },
    Topic {
        mnemonic: "BLP",
        args: &[],
        example: "BLP",
        usage: "",
        live: &[],
        live_note: "No data of its own: each Launchpad component uses its function's sources.",
        related: &["W", "GP", "TOP"],
    },
    Topic {
        mnemonic: "SECF",
        args: &[("q", "name or ticker text"), ("sector", "yellow key to filter by, e.g. EQUITY")],
        example: "SECF APPLE",
        usage: "Text typed after SECF is the search: SECF APPLE <GO>",
        live: &[
            (Src::Edgar, "US companies from the SEC ticker list"),
            (Src::Keyless, "crypto pairs from Coinbase and Kraken"),
        ],
        live_note: "",
        related: &["DES", "W"],
    },
    Topic {
        mnemonic: "MOST",
        args: &[("by", "Volume, Gainers or Losers (default Volume)")],
        example: "MOST",
        usage: "",
        live: &[],
        live_note: "NOT AVAILABLE in LIVE mode: it needs a market-wide quote feed, and the free sources quote only requested symbols.",
        related: &["W", "WEI"],
    },
    Topic {
        mnemonic: "N",
        args: &[
            ("scope", "press for company press releases (default: market news)"),
            ("q", "text filter"),
            ("story", "id of a story to open (rows set it)"),
        ],
        example: "N",
        usage: "Text typed after N filters headlines: N EARNINGS <GO>",
        live: &[
            (Src::Alpaca, "market news (Benzinga)"),
            (Src::Finnhub, "market headlines"),
            (Src::Keyless, "company press releases from wire RSS feeds"),
        ],
        live_note: "",
        related: &["TOP", "CN"],
    },
    Topic {
        mnemonic: "CN",
        args: &[("q", "text filter"), ("story", "id of a story to open (rows set it)")],
        example: "AAPL US <EQUITY> CN",
        usage: "",
        live: &[
            (Src::Alpaca, "company news (Benzinga)"),
            (Src::Finnhub, "company news"),
            (Src::Keyless, "the company's press releases from wire RSS feeds"),
        ],
        live_note: "",
        related: &["N", "TOP", "CF", "DES"],
    },
    Topic {
        mnemonic: "TOP",
        args: &[("q", "text filter"), ("story", "id of a story to open (rows set it)")],
        example: "TOP",
        usage: "",
        live: &[(Src::Alpaca, "latest market-wide headlines"), (Src::Finnhub, "market headlines")],
        live_note: "No free source curates top stories; TOP shows the latest market headlines.",
        related: &["N", "CN"],
    },
    Topic {
        mnemonic: "FA",
        args: &[
            ("stmt", "IS (income), BS (balance sheet), CF (cash flow) or RATIOS (default IS)"),
            ("per", "Annual or Quarterly (default Annual)"),
        ],
        example: "MSFT US <EQUITY> FA",
        usage: "Type the statement after the mnemonic: MSFT US <EQUITY> FA BS <GO>",
        live: &[(Src::Edgar, "as-reported XBRL financial statements; restated values from later filings replace originals")],
        live_note: "",
        related: &["EE", "ERN", "DVD", "RV", "CF"],
    },
    Topic {
        mnemonic: "EE",
        args: &[("metric", "EPS, REV or EBITDA (default EPS)")],
        example: "NVDA US <EQUITY> EE",
        usage: "",
        live: &[],
        live_note: "NOT AVAILABLE in LIVE mode: no free source provides consensus estimates.",
        related: &["ERN", "FA", "ANR"],
    },
    Topic {
        mnemonic: "ERN",
        args: &[],
        example: "NVDA US <EQUITY> ERN",
        usage: "",
        live: &[(Src::Finnhub, "earnings history")],
        live_note: "",
        related: &["EE", "FA", "ANR"],
    },
    Topic {
        mnemonic: "ANR",
        args: &[],
        example: "AAPL US <EQUITY> ANR",
        usage: "",
        live: &[(Src::Finnhub, "analyst rating counts"), (Src::Alpaca, "last price for the implied return")],
        live_note: "",
        related: &["EE", "ERN", "DES"],
    },
    Topic {
        mnemonic: "HDS",
        args: &[("type", "All, Institutions, Funds or Insiders (default All)")],
        example: "AAPL US <EQUITY> HDS",
        usage: "",
        live: &[],
        live_note: "NOT AVAILABLE in LIVE mode: no free source provides holders.",
        related: &["DES", "CF"],
    },
    Topic {
        mnemonic: "DVD",
        args: &[],
        example: "KO US <EQUITY> DVD",
        usage: "",
        live: &[
            (Src::Alpaca, "corporate actions: ex-, record and pay dates, cash amounts and splits"),
            (Src::Edgar, "dividends per share by fiscal period and splits disclosed in filings"),
        ],
        live_note: "",
        related: &["FA", "DES", "HP"],
    },
    Topic {
        mnemonic: "CF",
        args: &[
            ("form", "All, 10-K, 10-Q, 8-K, Proxy or Ownership (default All)"),
            ("doc", "accession number of a filing to open (rows set it)"),
            ("section", "section number within the open filing"),
            ("diff", "1 = compare with the prior filing of the same form"),
            ("summary", "1 = AI summary of the open filing"),
        ],
        example: "AAPL US <EQUITY> CF",
        usage: "",
        live: &[(Src::Edgar, "filing lists and documents"), (Src::Anthropic, "AI summaries")],
        live_note: "",
        related: &["FA", "CN", "DES"],
    },
    Topic {
        mnemonic: "OMON",
        args: &[
            ("expiry", "expiration date (default: nearest)"),
            ("strikes", "number of strikes around the money (default 20)"),
        ],
        example: "AAPL US <EQUITY> OMON",
        usage: "",
        live: &[
            (Src::Alpaca, "option chain (on the free plan the indicative feed: derived quotes, trades delayed 15 minutes), underlying price, dividends"),
            (Src::Keyless, "US Treasury 3-month yield as the rate"),
        ],
        live_note: "",
        related: &["OVDV", "OVME", "GP"],
    },
    Topic {
        mnemonic: "OVDV",
        args: &[("expiry", "expiration for the smile (default: first about a month out)")],
        example: "SPY US <EQUITY> OVDV",
        usage: "",
        live: &[
            (Src::Alpaca, "option chain implied volatilities, underlying price, dividends"),
            (Src::Keyless, "US Treasury 3-month yield as the rate"),
        ],
        live_note: "",
        related: &["OMON", "OVME"],
    },
    Topic {
        mnemonic: "OVME",
        args: &[
            ("strategy", "Long Call, Long Put, Covered Call, Bull Call Spread, Bear Put Spread, Straddle, Strangle, Iron Condor, Butterfly"),
            ("expiry", "expiration date (default 30 days out)"),
            ("model", "Black-Scholes or Binomial (default Black-Scholes)"),
            ("spot, vol, rate, div", "overrides; vol, rate and div in percent"),
            ("k1 … k4", "strikes for the strategy legs"),
        ],
        example: "AAPL US <EQUITY> OVME",
        usage: "",
        live: &[
            (Src::Alpaca, "spot, price history for 30-day volatility, dividends"),
            (Src::Keyless, "US Treasury 3-month yield as the rate"),
        ],
        live_note: "",
        related: &["OMON", "OVDV"],
    },
    Topic {
        mnemonic: "EQS",
        args: &[
            ("sector", "sector filter (default All)"),
            ("min_cap_b", "minimum market cap, $ billions"),
            ("max_pe", "maximum P/E"),
            ("min_growth", "minimum revenue growth %"),
            ("min_margin", "minimum net margin %"),
        ],
        example: "EQS",
        usage: "",
        live: &[(Src::Edgar, "financial statements"), (Src::Alpaca, "prices and one-year history")],
        live_note: "",
        related: &["RV", "FA"],
    },
    Topic {
        mnemonic: "RV",
        args: &[],
        example: "MSFT US <EQUITY> RV",
        usage: "",
        live: &[(Src::Edgar, "financial statements and industry"), (Src::Alpaca, "prices")],
        live_note: "",
        related: &["EQS", "FA"],
    },
    Topic {
        mnemonic: "CORR",
        args: &[
            ("securities", "comma-separated security keys (default: a built-in list)"),
            ("range", "history window (default 1Y)"),
        ],
        example: "CORR",
        usage: "",
        live: BARS_LIVE,
        live_note: "",
        related: &["PORT", "GP"],
    },
    Topic {
        mnemonic: "PORT",
        args: &[
            ("portfolio", "portfolio to show (chosen from the menu)"),
            ("add", "1 = add a transaction from security, qty, price, date and fees"),
            ("delete_tx", "transaction id to delete (rows set it)"),
        ],
        example: "PORT",
        usage: "",
        live: QUOTES_LIVE,
        live_note: "Positions and transactions are stored on this Mac.",
        related: &["CORR", "W"],
    },
    Topic {
        mnemonic: "BTST",
        args: &[
            ("strategy", "SMA Cross, RSI Reversion, Breakout, MACD Cross or Buy & Hold (default SMA Cross)"),
            ("range", "history window (default 5Y)"),
            ("short", "Yes allows short positions (default No)"),
        ],
        example: "SPY US <EQUITY> BTST",
        usage: "",
        live: BARS_LIVE,
        live_note: "",
        related: &["GP", "PORT"],
    },
    Topic {
        mnemonic: "ALRT",
        args: &[
            ("create", "1 = create from security, condition, value, repeat and note"),
            ("condition", "Price Above, Price Below, % Chg Above, % Chg Below, Volume Above or News Keyword"),
            ("repeat", "Once, Every Cross or Every 15 Min"),
            ("toggle, delete", "alert id (rows set them)"),
        ],
        example: "ALRT",
        usage: "",
        live: &[
            (Src::Alpaca, "prices; news for keyword alerts"),
            (Src::Finnhub, "news for keyword alerts"),
            (Src::Keyless, "crypto and FX prices"),
        ],
        live_note: "",
        related: &["W", "CN"],
    },
    Topic {
        mnemonic: "WEI",
        args: &[],
        example: "WEI",
        usage: "",
        live: &[(Src::Alpaca, "quotes for US-listed ETFs used as index proxies")],
        live_note: "Index levels aren't available from free sources; rows show ETF proxies, whose % change approximates the index's.",
        related: &["W", "GP", "ECO"],
    },
    Topic {
        mnemonic: "ECO",
        args: &[
            ("series", "FRED series id to chart, e.g. CPIAUCSL"),
            ("view", "curve = US Treasury yield curve"),
            ("from, to", "calendar dates (default 3 days back to 7 days ahead)"),
            ("importance", "High, Medium or Low (default Low = all)"),
        ],
        example: "ECO",
        usage: "",
        live: &[(Src::Fred, "release calendar and economic series"), (Src::Keyless, "US Treasury par yield curve")],
        live_note: "",
        related: &["WEI", "FXC"],
    },
    Topic {
        mnemonic: "FXC",
        args: &[],
        example: "FXC",
        usage: "",
        live: &[(Src::Keyless, "ECB reference rates via Frankfurter (daily)")],
        live_note: "",
        related: &["CRYP", "ECO"],
    },
    Topic {
        mnemonic: "CRYP",
        args: &[],
        example: "CRYP",
        usage: "",
        live: &[(Src::Keyless, "Coinbase and Kraken quotes and streams")],
        live_note: "",
        related: &["FXC", "W"],
    },
    Topic {
        mnemonic: "ASK",
        args: &[],
        example: "ASK",
        usage: "Type the question in the ASK panel.",
        live: &[(Src::Anthropic, "the model")],
        live_note: "Its tools read the other sources; data whose terms don't clearly allow AI use is left out.",
        related: &["DES", "FA", "CN"],
    },
    Topic {
        mnemonic: "HELP",
        args: &[("topic", "function mnemonic to explain (default: this overview)")],
        example: "HELP",
        usage: "",
        live: &[],
        live_note: "Built in; uses no market data.",
        related: &["SECF", "BLP", "ASK"],
    },
];

fn topic_of(mnemonic: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.mnemonic == mnemonic)
}

/// Directory groups, in display order.
const AREAS: &[(FunctionCategory, &str)] = &[
    (FunctionCategory::Security, "Security"),
    (FunctionCategory::Charts, "Charts & Prices"),
    (FunctionCategory::News, "News"),
    (FunctionCategory::Fundamentals, "Company Fundamentals"),
    (FunctionCategory::Filings, "Filings"),
    (FunctionCategory::Derivatives, "Derivatives"),
    (FunctionCategory::Analytics, "Analytics"),
    (FunctionCategory::Macro, "Macro & Cross-Asset"),
    (FunctionCategory::Monitors, "Monitors"),
    (FunctionCategory::Workspace, "Workspace"),
    (FunctionCategory::Ai, "AI"),
];

fn area_label(c: FunctionCategory) -> &'static str {
    AREAS.iter().find(|(a, _)| *a == c).map_or("Other", |(_, l)| *l)
}

fn security_text(spec: &FunctionSpec) -> String {
    let sectors = || spec.sectors.iter().map(|s| s.key_cap()).collect::<Vec<_>>().join(", ");
    match (spec.needs_security, spec.sectors.is_empty()) {
        (SecurityNeed::Required, true) => "Required".into(),
        (SecurityNeed::Required, false) => format!("Required ({})", sectors()),
        (SecurityNeed::Optional, true) => "Optional".into(),
        (SecurityNeed::Optional, false) => format!("Optional ({})", sectors()),
        (SecurityNeed::None, _) => "—".into(),
    }
}

/// Action that opens `spec`, carrying the panel's security when it applies.
fn open(spec: &FunctionSpec, security: Option<&SecurityKey>) -> Action {
    let sec = security.filter(|k| spec.takes_security() && spec.accepts_sector(k.sector)).map(ToString::to_string);
    Action::new(spec.mnemonic, sec.as_deref())
}

fn topic_input(current: &str) -> Block {
    let mut options = vec![OVERVIEW.to_owned()];
    options.extend(registry().iter().map(|f| f.mnemonic.to_owned()));
    Block::Inputs {
        title: None,
        inputs: vec![Input { id: "topic".into(), label: "Help on".into(), value: current.into(), kind: InputKind::Choice, options }],
    }
}

pub(crate) async fn help(_engine: Arc<Engine>, req: ScreenRequest) -> Screen {
    let topic = req.arg("topic").map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case(OVERVIEW));
    let Some(t) = topic else { return overview(&req) };
    if let Some(spec) = lookup(&t) {
        return topic_screen(spec, &req);
    }
    let mut s = Screen::not_available("HELP", TITLE, None, format!("{t} is not a Meridian function; HELP <GO> lists every function"));
    s.push(topic_input(OVERVIEW));
    s
}

fn overview(req: &ScreenRequest) -> Screen {
    let mut s = Screen::new("HELP", TITLE, None);
    s.push(topic_input(OVERVIEW));
    s.push(Block::Text { title: Some("Command Syntax".into()), body: SYNTAX.into() });
    let keys = OVERLAY_KEYS
        .iter()
        .chain(ROUTER_ONLY_KEYS)
        .map(|(k, v)| Row::new(vec![Cell::text(*k).styled(Style::Emphasis), Cell::text(*v)]))
        .collect();
    s.push(Block::Table(Table {
        title: Some("Keys".into()),
        columns: vec![Column::text("Key", 22), Column::text("Binding", 44)],
        rows: keys,
        page_size: None,
        numbered: false,
    }));
    let mut rows = Vec::new();
    for (area, label) in AREAS {
        let specs: Vec<&FunctionSpec> = registry().iter().filter(|f| f.category == *area).collect();
        if specs.is_empty() {
            continue;
        }
        rows.push(Row::new(vec![Cell::text(*label).styled(Style::Emphasis), Cell::empty(), Cell::empty(), Cell::empty()]).emphasis());
        for f in specs {
            rows.push(
                Row::new(vec![
                    Cell::text(f.mnemonic).styled(Style::Link),
                    Cell::text(f.title),
                    Cell::text(security_text(f)),
                    Cell::text(f.description).styled(Style::Muted),
                ])
                .action(open(f, req.security.as_ref()))
                .depth(1),
            );
        }
    }
    s.push(Block::Table(Table {
        title: Some("Function Directory (select a row to open it)".into()),
        columns: vec![Column::text("Function", 9), Column::text("Title", 28), Column::text("Security", 20), Column::text("Description", 90)],
        rows,
        page_size: None,
        numbered: false,
    }));
    s
}

fn topic_screen(spec: &'static FunctionSpec, req: &ScreenRequest) -> Screen {
    let t = topic_of(spec.mnemonic);
    let mut s = Screen::new("HELP", format!("{TITLE} — {} {}", spec.mnemonic, spec.title), None);
    s.menu_item(&format!("Open {}", spec.mnemonic), open(spec, req.security.as_ref()), false);
    // Merged into this screen's args by the shell, so it must replace `topic`.
    s.menu_item("Help Overview", Action::new("HELP", None).arg("topic", OVERVIEW), false);
    s.push(topic_input(spec.mnemonic));
    let mut fields = vec![
        Field::text("Function", spec.mnemonic).styled(Style::Emphasis),
        Field::text("Title", spec.title),
        Field::text("Area", area_label(spec.category)),
        Field::text("Security", security_text(spec)),
    ];
    if let Some(t) = t {
        fields.push(Field::text("Example", format!("{} <GO>", t.example)));
    }
    s.push(Block::Fields { title: None, columns: 2, fields });
    let mut body = spec.description.to_owned();
    if let Some(u) = t.map(|t| t.usage).filter(|u| !u.is_empty()) {
        body.push_str("\n\n");
        body.push_str(u);
    }
    s.push(Block::Text { title: Some("Description".into()), body });

    let args = t.map_or(&[][..], |t| t.args);
    if args.is_empty() {
        s.push(Block::Text { title: Some("Arguments".into()), body: "None.".into() });
    } else {
        s.push(Block::Table(Table {
            title: Some("Arguments (set by the screen's inputs and menus)".into()),
            columns: vec![Column::text("Argument", 22), Column::text("Values", 100)],
            rows: args.iter().map(|(a, v)| Row::new(vec![Cell::text(*a).styled(Style::Emphasis), Cell::text(*v)])).collect(),
            page_size: None,
            numbered: false,
        }));
    }

    let live = t.map_or(&[][..], |t| t.live);
    if !live.is_empty() {
        s.push(Block::Table(Table {
            title: Some("Data in LIVE Mode (Settings → Setup)".into()),
            columns: vec![Column::text("Source", 12), Column::text("Provides", 110)],
            rows: live.iter().map(|(src, what)| Row::new(vec![Cell::text(src.label()).styled(Style::Emphasis), Cell::text(*what)])).collect(),
            page_size: None,
            numbered: false,
        }));
    }
    if let Some(note) = t.map(|t| t.live_note).filter(|n| !n.is_empty()) {
        let title = if live.is_empty() { "Data in LIVE Mode" } else { "Note" };
        s.push(Block::Text { title: Some(title.into()), body: note.into() });
    }

    let related: Vec<Row> = t
        .map_or(&[][..], |t| t.related)
        .iter()
        .filter_map(|m| lookup(m))
        .map(|f| {
            Row::new(vec![Cell::text(f.mnemonic).styled(Style::Link), Cell::text(f.title), Cell::text(security_text(f))])
                .action(open(f, req.security.as_ref()))
        })
        .collect();
    if !related.is_empty() {
        s.push(Block::Table(Table {
            title: Some("Related Functions".into()),
            columns: vec![Column::text("Function", 9), Column::text("Title", 28), Column::text("Security", 20)],
            rows: related,
            page_size: None,
            numbered: true,
        }));
    }

    s
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use meridian_command::{ParseContext, ParsedCommand, parse};

    use super::*;

    #[test]
    fn every_registered_function_has_a_topic() {
        let registered: HashSet<&str> = registry().iter().map(|f| f.mnemonic).collect();
        let topics: HashSet<&str> = TOPICS.iter().map(|t| t.mnemonic).collect();
        assert_eq!(registered, topics);
        assert_eq!(TOPICS.len(), topics.len(), "duplicate topic");
        let areas: HashSet<FunctionCategory> = AREAS.iter().map(|(a, _)| *a).collect();
        assert!(registry().iter().all(|f| areas.contains(&f.category)), "every category has a directory group");
    }

    #[test]
    fn examples_parse_to_their_function_and_related_exist() {
        let ctx = ParseContext::default();
        for t in TOPICS {
            let spec = lookup(t.mnemonic).unwrap();
            let function = match parse(t.example, &ctx) {
                ParsedCommand::Security { function: Some(f), security, .. } => {
                    assert!(spec.takes_security(), "{}: example names a security", t.mnemonic);
                    assert!(spec.accepts_sector(security.sector), "{}", t.mnemonic);
                    f
                }
                ParsedCommand::Function { function, .. } => {
                    assert_ne!(spec.needs_security, SecurityNeed::Required, "{}: example needs a security", t.mnemonic);
                    function
                }
                other => panic!("{}: example parses as {other:?}", t.mnemonic),
            };
            assert_eq!(function, t.mnemonic);
            for r in t.related {
                assert!(lookup(r).is_some() && *r != t.mnemonic, "{}: related {r}", t.mnemonic);
            }
            assert!(!t.live.is_empty() || !t.live_note.is_empty(), "{}: say where LIVE data comes from", t.mnemonic);
        }
    }

    /// `(key, binding)` pairs in the `KeyboardOverlay.rows` literal.
    fn overlay_rows(swift: &str) -> Vec<(String, String)> {
        let start = swift.find("static let rows: [(String, String)] = [").expect("KeyboardOverlay.rows");
        let body = &swift[start..];
        let body = &body[..body.find("\n    ]").expect("end of rows")];
        body.split("(\"")
            .skip(1)
            .map(|piece| {
                let (key, rest) = piece.split_once("\", \"").expect("pair");
                let (binding, _) = rest.split_once("\")").expect("pair end");
                (key.to_owned(), binding.to_owned())
            })
            .collect()
    }

    #[test]
    fn key_map_matches_the_swift_shell() {
        let window = include_str!("../../../../../app/Meridian/Shell/MainWindow.swift");
        let ours: Vec<(String, String)> = OVERLAY_KEYS.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        assert_eq!(overlay_rows(window), ours, "HELP keys must equal KeyboardOverlay.rows");
        // Bindings stated above, as KeyRouter implements them.
        let router = include_str!("../../../../../app/Meridian/Shell/KeyRouter.swift");
        for needle in [
            "case 36, 76: return mods.isEmpty || mods == .shift ? .go : nil",
            "case 53: return .cancel",
            "case 122: return .help // F1",
            "case \"?\": return .help",
            "case \"[\": return .menu",
            "case 119: return .menu // End",
            "case 51 where commandEmpty && mods.isEmpty: return .menu // Delete on empty line",
            "case 121: return .pageForward // PgDn",
            "case 116: return .pageBack // PgUp",
            "case 125 where mods == .command: return .pageForward",
            "case 126 where mods == .command: return .pageBack",
            "case 48 where mods == .control: return .nextPanel",
            "case 48 where mods == [.control, .shift]: return .previousPanel",
            "case \"1\", \"2\", \"3\", \"4\": return .focusPanel(Int(ch)! - 1)",
            "case 48 where suggestionsOpen && mods.isEmpty: return .acceptSuggestion",
            "case \"/\": return .keyboardOverlay",
            "static let sectorOrder = [\"GOVT\", \"CORP\", \"MTGE\", \"M-MKT\", \"MUNI\", \"PFD\", \"EQUITY\", \"CMDTY\", \"INDEX\", \"CRNCY\"]",
            "120: \"GOVT\", 99: \"CORP\", 118: \"MTGE\", 96: \"M-MKT\", 97: \"MUNI\"",
            "98: \"PFD\", 100: \"EQUITY\", 101: \"CMDTY\", 109: \"INDEX\", 103: \"CRNCY\"",
            "let idx = d == 0 ? 9 : d - 1",
        ] {
            assert!(router.contains(needle), "KeyRouter.swift no longer contains: {needle}");
        }
    }

    #[test]
    fn security_column_and_actions() {
        let fa = lookup("FA").unwrap();
        assert_eq!(security_text(fa), "Required (EQUITY)");
        assert_eq!(security_text(lookup("GP").unwrap()), "Required");
        assert_eq!(security_text(lookup("CN").unwrap()), "Optional");
        assert_eq!(security_text(lookup("WEI").unwrap()), "—");
        let aapl = SecurityKey::equity("AAPL");
        assert_eq!(open(fa, Some(&aapl)).security.as_deref(), Some("AAPL US Equity"));
        assert_eq!(open(fa, Some(&SecurityKey::currency("EURUSD"))).security, None);
        assert_eq!(open(lookup("WEI").unwrap(), Some(&aapl)).security, None);
    }
}
