//! Plain-language commands: `aapl`, `aapl 5y`, `aapl filings`, `aapl vs msft`,
//! `earnings this week`, `cpi`, `ask …`.
//!
//! [`interpret`] layers plain phrases over the mnemonic grammar
//! ([`parse()`](crate::parse())). Input the grammar already gives a meaning
//! keeps it (`AAPL US <EQUITY> GP 5Y`, `GP 5Y`, `12`); what it would send to
//! the security finder is read as plain language first. Bare tickers need
//! the instrument index, which the caller passes in as a
//! [`SecurityResolver`], so this module stays pure.
//!
//! # Precedence
//!
//! 1. The mnemonic grammar: a typed security key, a leading mnemonic (`CN`
//!    alone is still Company News), a menu number. Exceptions: `ask
//!    <question>` keeps the question's case, and the plain-word ids
//!    (`COMPARE`, `FILINGS`, `CALENDAR`, `TODAY`) followed by more words are
//!    read as phrases (`compare aapl msft`, `filings aapl`), as is a key
//!    followed by plain words (`AAPL US <EQUITY> filings`).
//! 2. Comparisons: `aapl vs msft [vs …] [range]`, `compare aapl with msft`.
//! 3. Whole-input commands (`today`, `earnings this week`, `rates`, `cpi`).
//!    Data words (`cpi`, `inflation`, `gdp`, `jobs`, `payrolls`,
//!    `unemployment`) yield to an exact ticker of the same name, because a
//!    listed ticker is the more specific match and the series stays one row
//!    away in the suggestions. Navigation words (`news`, `crypto`, `fx`,
//!    `settings`, …) never yield: they are Meridian's own vocabulary.
//! 4. A security followed by what to open: `aapl`, `aapl 5y`, `aapl
//!    filings`, `aapl gp 5y` (a mnemonic after a bare ticker works like the
//!    full key).
//! 5. What to open followed by a security: `filings aapl`, `5y msft`.
//! 6. What to open alone applies to the panel's loaded security: `chart`,
//!    `options`, `5y`.
//! 7. Two or more words ending in `?` go to ASK as a question.
//! 8. Anything else stays a security search, as before.
//!
//! # Securities
//!
//! A security is a ticker (`aapl`, `brk.b`, `brk/b`), a ticker and a
//! two-letter exchange (`bmw gr`), a crypto shorthand (`btc` →
//! `BTCUSD Curncy`) or a currency pair (`eurusd`, `eur/usd`). The major
//! coins in [`MAJOR_COINS`] mean the coin even where a fund trades under the
//! same ticker (`BTC`, `ETH`); other shorthands apply only when no ticker
//! matches. A pair of two ISO currency codes resolves even when the index
//! doesn't list it, since the pair itself names the instrument.

use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Serialize};

use crate::parse::{ParseContext, ParsedCommand, parse, raw_tokens};
use crate::registry::{FunctionSpec, lookup};

/// Most securities one comparison takes.
pub const MAX_COMPARE: usize = 8;

/// Actions the app handles itself; they have no engine screen.
pub const APP_ACTIONS: [&str; 2] = ["SETTINGS", "IMPORT"];

/// Registered ids that are plain words; followed by more words they are
/// read as phrases (`compare aapl msft`, `filings aapl`).
const PLAIN_WORD_IDS: [&str; 4] = ["TODAY", "CALENDAR", "FILINGS", "COMPARE"];

/// Coins whose shorthand means the coin even where a fund trades under the
/// same ticker. Kept equal to the CRYP list in the engine by a test there.
pub const MAJOR_COINS: [&str; 15] = [
    "BTC", "ETH", "SOL", "XRP", "ADA", "DOGE", "AVAX", "LINK", "DOT", "LTC", "BCH", "XLM", "UNI",
    "ATOM", "AAVE",
];

/// Major-coin shorthand that is also an operating company's US ticker
/// (LTC Properties, Atomera, Interlink, Emeren, Banco de Chile). Where that
/// stock is listed it wins; the coin is still `ltcusd`. Funds that hold the
/// coin (BTC, ETH) don't count: there the coin wins.
const COIN_TICKERS_OF_COMPANIES: [&str; 5] = ["LTC", "ATOM", "LINK", "SOL", "BCH"];

/// ISO 4217 codes accepted in currency pairs that aren't in the index.
const CURRENCIES: [&str; 30] = [
    "USD", "EUR", "JPY", "GBP", "CHF", "CAD", "AUD", "NZD", "CNH", "CNY", "HKD", "SGD", "SEK",
    "NOK", "DKK", "MXN", "BRL", "INR", "KRW", "ZAR", "TRY", "PLN", "CZK", "HUF", "ILS", "THB",
    "TWD", "IDR", "PHP", "MYR",
];

/// A fully resolved command: what Return runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Action {
    /// A registered function id, or one of [`APP_ACTIONS`].
    pub function: String,
    pub security: Option<SecurityKey>,
    /// Named screen arguments, e.g. `range=5Y`.
    pub args: Vec<(String, String)>,
}

impl Action {
    #[must_use]
    pub fn new(function: &str, security: Option<SecurityKey>) -> Self {
        Self {
            function: function.to_owned(),
            security,
            args: Vec::new(),
        }
    }

    /// Sets argument `key`, replacing an earlier value.
    #[must_use]
    pub fn arg(mut self, key: &str, value: impl Into<String>) -> Self {
        self.args.retain(|(k, _)| k != key);
        self.args.push((key.to_owned(), value.into()));
        self
    }

    #[must_use]
    pub fn arg_value(&self, key: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the app handles this action itself rather than asking the
    /// engine for a screen.
    #[must_use]
    pub fn is_app_action(&self) -> bool {
        APP_ACTIONS.contains(&self.function.as_str())
    }
}

/// Looks securities up by symbol in the instrument index.
pub trait SecurityResolver {
    /// The indexed security whose symbol is exactly `symbol` (upper case),
    /// listed on `exchange` when one is given. When several match, the most
    /// likely one: a US listing first, then the most popular.
    fn exact(&self, symbol: &str, exchange: Option<&str>) -> Option<SecurityKey>;
}

/// Something to open for a security, and the words that ask for it.
#[derive(Debug)]
pub(crate) struct Topic {
    /// Lower-case phrases; the first is the canonical one used in completions.
    pub phrases: &'static [&'static str],
    pub function: &'static str,
    pub args: &'static [(&'static str, &'static str)],
    /// Popover title; empty means the function's plain name.
    pub title: &'static str,
    /// Popover subtitle; empty means the function's summary.
    pub subtitle: &'static str,
    /// Listed when a security is typed with nothing after it.
    pub listed: bool,
}

const fn topic(
    phrases: &'static [&'static str],
    function: &'static str,
    args: &'static [(&'static str, &'static str)],
    title: &'static str,
    subtitle: &'static str,
    listed: bool,
) -> Topic {
    Topic {
        phrases,
        function,
        args,
        title,
        subtitle,
        listed,
    }
}

/// Security topics; listed ones appear in this order under `aapl `.
pub(crate) const TOPICS: &[Topic] = &[
    topic(
        &["overview", "description", "profile", "about"],
        "DES",
        &[],
        "",
        "",
        true,
    ),
    topic(&["chart", "graph", "price chart"], "GP", &[], "", "", true),
    topic(&["news", "headlines"], "CN", &[], "", "", true),
    topic(&["filings", "sec filings"], "CF", &[], "", "", true),
    topic(
        &[
            "financials",
            "fundamentals",
            "statements",
            "financial statements",
        ],
        "FA",
        &[],
        "",
        "",
        true,
    ),
    topic(&["earnings", "results"], "ERN", &[], "", "", true),
    topic(
        &["options", "chain", "option chain", "options chain"],
        "OMON",
        &[],
        "",
        "",
        true,
    ),
    topic(
        &["dividends", "dividend", "splits"],
        "DVD",
        &[],
        "",
        "",
        true,
    ),
    topic(
        &["analysts", "ratings", "analyst ratings", "recommendations"],
        "ANR",
        &[],
        "",
        "",
        true,
    ),
    topic(&["peers", "comps", "comparables"], "RV", &[], "", "", true),
    topic(
        &["history", "prices", "price history", "historical prices"],
        "HP",
        &[],
        "",
        "",
        true,
    ),
    topic(
        &["intraday", "today", "intraday chart"],
        "GIP",
        &[],
        "",
        "",
        true,
    ),
    topic(
        &[
            "volatility",
            "vol",
            "smile",
            "vol surface",
            "volatility surface",
            "iv",
        ],
        "OVDV",
        &[],
        "",
        "",
        true,
    ),
    topic(&["estimates", "consensus"], "EE", &[], "", "", true),
    topic(
        &["holders", "ownership", "shareholders"],
        "HDS",
        &[],
        "",
        "",
        true,
    ),
    topic(&["backtest"], "BTST", &[], "", "", true),
    topic(
        &["10-k", "10k", "annual report", "annual reports"],
        "CF",
        &[("form", "10-K")],
        "Annual reports (10-K)",
        "10-K filings, with summaries and what changed",
        false,
    ),
    topic(
        &["10-q", "10q", "quarterly report", "quarterly reports"],
        "CF",
        &[("form", "10-Q")],
        "Quarterly reports (10-Q)",
        "10-Q filings, with summaries and what changed",
        false,
    ),
    topic(
        &["8-k", "8k", "current report", "current reports"],
        "CF",
        &[("form", "8-K")],
        "Current reports (8-K)",
        "8-K filings: material events as they happen",
        false,
    ),
    topic(
        &["income statement", "income"],
        "FA",
        &[("stmt", "IS")],
        "Income statement",
        "Revenue, margins and earnings by year",
        false,
    ),
    topic(
        &["balance sheet", "balance"],
        "FA",
        &[("stmt", "BS")],
        "Balance sheet",
        "Assets, liabilities and equity",
        false,
    ),
    topic(
        &["cash flow", "cashflow", "cash flows"],
        "FA",
        &[("stmt", "CF")],
        "Cash flow",
        "Operating, investing and financing cash flows",
        false,
    ),
    topic(
        &["ratios"],
        "FA",
        &[("stmt", "RATIOS")],
        "Ratios",
        "Margins, returns and valuation ratios",
        false,
    ),
    topic(&["option valuation", "payoff"], "OVME", &[], "", "", false),
    topic(&["correlation"], "CORR", &[], "", "", false),
];

impl Topic {
    /// Takes a trailing range: `aapl chart 5y`, `aapl backtest 10y`.
    pub(crate) fn ranged(&self) -> bool {
        matches!(self.function, "GP" | "GIP" | "BTST")
    }

    pub(crate) fn spec(&self) -> Option<&'static FunctionSpec> {
        lookup(self.function)
    }

    pub(crate) fn title(&self) -> &'static str {
        if self.title.is_empty() {
            self.spec().map_or(self.function, |s| s.name)
        } else {
            self.title
        }
    }

    pub(crate) fn subtitle(&self) -> &'static str {
        if self.subtitle.is_empty() {
            self.spec().map_or("", |s| s.summary)
        } else {
            self.subtitle
        }
    }

    /// Whether the topic's function accepts a security of `sector`.
    pub(crate) fn accepts(&self, sector: MarketSector) -> bool {
        self.spec().is_some_and(|s| s.accepts_sector(sector))
    }

    pub(crate) fn action(&self, key: &SecurityKey) -> Action {
        self.args.iter().fold(
            Action::new(self.function, Some(key.clone())),
            |a, (k, v)| a.arg(k, *v),
        )
    }

    /// The action with the words after the phrase applied: a range for
    /// charts and backtests, a period for financials.
    fn with_modifiers(&self, key: &SecurityKey, rest: &[String]) -> Option<Action> {
        let action = self.action(key);
        if rest.is_empty() {
            return Some(action);
        }
        if self.ranged()
            && let Some(r) = range_of(rest)
        {
            return Some(action.arg("range", r.code));
        }
        match (self.function, words_str(rest).as_slice()) {
            ("FA", ["quarterly"]) => Some(action.arg("per", "Quarterly")),
            ("FA", ["annual"]) => Some(action.arg("per", "Annual")),
            _ => None,
        }
    }
}

/// A chart range preset and the words for it.
#[derive(Debug)]
pub(crate) struct RangeDef {
    /// The `range` argument value.
    pub code: &'static str,
    pub phrases: &'static [&'static str],
    /// `5 years`, for titles such as `Backtest, 5 years`.
    pub label: &'static str,
    /// `5-year chart`.
    pub chart_title: &'static str,
}

const fn range(
    code: &'static str,
    phrases: &'static [&'static str],
    label: &'static str,
    chart_title: &'static str,
) -> RangeDef {
    RangeDef {
        code,
        phrases,
        label,
        chart_title,
    }
}

pub(crate) const RANGES: &[RangeDef] = &[
    range("1D", &["1d", "1 day"], "1 day", "1-day chart"),
    range(
        "5D",
        &["5d", "5 days", "1w", "1 week"],
        "5 days",
        "5-day chart",
    ),
    range("1M", &["1m", "1mo", "1 month"], "1 month", "1-month chart"),
    range(
        "3M",
        &["3m", "3mo", "3 months"],
        "3 months",
        "3-month chart",
    ),
    range(
        "6M",
        &["6m", "6mo", "6 months"],
        "6 months",
        "6-month chart",
    ),
    range(
        "YTD",
        &["ytd", "year to date"],
        "year to date",
        "Year-to-date chart",
    ),
    range("1Y", &["1y", "1yr", "1 year"], "1 year", "1-year chart"),
    range("2Y", &["2y", "2yr", "2 years"], "2 years", "2-year chart"),
    range("3Y", &["3y", "3yr", "3 years"], "3 years", "3-year chart"),
    range("5Y", &["5y", "5yr", "5 years"], "5 years", "5-year chart"),
    range(
        "10Y",
        &["10y", "10yr", "10 years"],
        "10 years",
        "10-year chart",
    ),
    range(
        "20Y",
        &["20y", "20yr", "20 years"],
        "20 years",
        "20-year chart",
    ),
    range("MAX", &["max", "all time"], "all time", "All-time chart"),
];

/// Plain words for a range preset: `5Y` → `5 years`, `YTD` → `year to date`.
#[must_use]
pub fn range_label(code: &str) -> Option<&'static str> {
    RANGES
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(code))
        .map(|r| r.label)
}

/// Title for `function` run over range `r`: `5-year chart`, `Backtest, 5 years`.
pub(crate) fn range_title(function: &str, r: &RangeDef) -> String {
    match function {
        "GP" => r.chart_title.to_owned(),
        other => {
            let name = lookup(other).map_or(other, |s| s.name);
            format!("{name}, {}", r.label)
        }
    }
}

/// A command that needs no security.
#[derive(Debug)]
pub(crate) struct CommandDef {
    /// Lower-case phrases; the first is canonical.
    pub phrases: &'static [&'static str],
    pub function: &'static str,
    pub args: &'static [(&'static str, &'static str)],
    /// Empty means the function's plain name.
    pub title: &'static str,
    /// Empty means the function's summary.
    pub subtitle: &'static str,
    /// A data word (`gdp`) that an exact ticker of the same name outranks.
    pub data_word: bool,
}

const fn command(
    phrases: &'static [&'static str],
    function: &'static str,
    args: &'static [(&'static str, &'static str)],
    title: &'static str,
    subtitle: &'static str,
) -> CommandDef {
    CommandDef {
        phrases,
        function,
        args,
        title,
        subtitle,
        data_word: false,
    }
}

/// An economic series by FRED id: `args` is `&[("series", id)]`.
const fn data(
    phrases: &'static [&'static str],
    args: &'static [(&'static str, &'static str)],
    title: &'static str,
    subtitle: &'static str,
) -> CommandDef {
    CommandDef {
        phrases,
        function: "ECO",
        args,
        title,
        subtitle,
        data_word: true,
    }
}

/// Commands, most useful first: suggestion order follows this table.
#[rustfmt::skip]
pub(crate) const COMMANDS: &[CommandDef] = &[
    command(&["today", "home"], "TODAY", &[], "", ""),
    command(&["earnings this week"], "CALENDAR", &[("kind", "earnings"), ("range", "this-week")], "Earnings this week", "Companies reporting this week"),
    command(&["earnings next week"], "CALENDAR", &[("kind", "earnings"), ("range", "next-week")], "Earnings next week", "Companies reporting next week"),
    command(&["earnings today"], "CALENDAR", &[("kind", "earnings"), ("range", "today")], "Earnings today", "Companies reporting today"),
    command(&["dividends this week"], "CALENDAR", &[("kind", "dividends"), ("range", "this-week")], "Dividends this week", "Dividend dates this week"),
    command(&["dividends next week"], "CALENDAR", &[("kind", "dividends"), ("range", "next-week")], "Dividends next week", "Dividend dates next week"),
    command(&["dividends today"], "CALENDAR", &[("kind", "dividends"), ("range", "today")], "Dividends today", "Dividend dates today"),
    command(&["earnings", "earnings calendar"], "CALENDAR", &[("kind", "earnings")], "Earnings calendar", "Upcoming earnings reports"),
    command(&["dividends", "dividend calendar"], "CALENDAR", &[("kind", "dividends")], "Dividend calendar", "Upcoming dividend dates"),
    command(&["calendar"], "CALENDAR", &[], "", ""),
    command(&["news", "headlines", "top news"], "TOP", &[], "", ""),
    command(&["filings", "filings inbox"], "FILINGS", &[], "", ""),
    command(&["portfolio", "holdings", "positions"], "PORT", &[], "", ""),
    command(&["watchlist", "watchlists"], "W", &[], "", ""),
    command(&["import", "import trades", "import csv"], "IMPORT", &[], "Import trades", "Add positions from a broker CSV file"),
    command(&["crypto", "cryptocurrencies"], "CRYP", &[], "", ""),
    command(&["fx", "currencies", "forex"], "FXC", &[], "", ""),
    command(&["indices", "world", "world indices", "markets"], "WEI", &[], "", ""),
    command(&["rates", "yield curve", "treasury", "treasuries", "yields", "treasury curve"], "ECO", &[("view", "curve")], "Treasury yield curve", "US Treasury par yields by maturity"),
    data(&["cpi", "inflation"], &[("series", "CPIAUCSL")], "Inflation (CPI)", "Consumer prices, all urban consumers · FRED CPIAUCSL"),
    data(&["unemployment", "unemployment rate", "jobless rate"], &[("series", "UNRATE")], "Unemployment rate", "US unemployment rate · FRED UNRATE"),
    data(&["jobs", "payrolls", "nonfarm payrolls", "jobs report"], &[("series", "PAYEMS")], "Payrolls", "Nonfarm payroll employment · FRED PAYEMS"),
    data(&["gdp"], &[("series", "GDP")], "GDP", "US gross domestic product · FRED GDP"),
    data(&["real gdp"], &[("series", "GDPC1")], "Real GDP", "Inflation-adjusted US output · FRED GDPC1"),
    data(&["fed funds", "fed funds rate", "fedfunds", "fed rate"], &[("series", "FEDFUNDS")], "Fed funds rate", "Effective federal funds rate · FRED FEDFUNDS"),
    command(&["economic calendar", "economy", "macro"], "ECO", &[], "", ""),
    command(&["screener", "stock screener", "screen"], "EQS", &[], "", ""),
    command(&["most active", "movers"], "MOST", &[], "", ""),
    command(&["gainers", "top gainers"], "MOST", &[("by", "Gainers")], "Top gainers", "Biggest gainers today"),
    command(&["losers", "top losers"], "MOST", &[("by", "Losers")], "Top losers", "Biggest losers today"),
    command(&["alerts"], "ALRT", &[], "", ""),
    command(&["compare"], "COMPARE", &[], "", ""),
    command(&["ask"], "ASK", &[], "", ""),
    command(&["settings", "preferences", "api keys"], "SETTINGS", &[], "Settings", "Data sources, API keys and setup"),
    command(&["help"], "HELP", &[], "", ""),
    command(&["launchpad"], "BLP", &[], "", ""),
];

impl CommandDef {
    pub(crate) fn action(&self) -> Action {
        self.args
            .iter()
            .fold(Action::new(self.function, None), |a, (k, v)| a.arg(k, *v))
    }

    pub(crate) fn title(&self) -> &'static str {
        if self.title.is_empty() {
            lookup(self.function).map_or(self.function, |s| s.name)
        } else {
            self.title
        }
    }

    pub(crate) fn subtitle(&self) -> &'static str {
        if self.subtitle.is_empty() {
            lookup(self.function).map_or("", |s| s.summary)
        } else {
            self.subtitle
        }
    }
}

/// The right-side hint for an action: its function id, or the shortcut for
/// app actions that have one.
#[must_use]
pub fn hint_for(function: &str) -> &str {
    match function {
        "SETTINGS" => "⌘,",
        other => other,
    }
}

/// What the input means, given the securities `resolver` knows. See the
/// module docs for the precedence.
#[must_use]
pub fn interpret<R: SecurityResolver + ?Sized>(
    input: &str,
    ctx: &ParseContext,
    resolver: &R,
) -> ParsedCommand {
    let parsed = parse(input, ctx);
    match &parsed {
        ParsedCommand::Function { function, args } if !args.is_empty() => {
            if function == "ASK" {
                return ask(input).map_or(parsed, ParsedCommand::Run);
            }
            if PLAIN_WORD_IDS.contains(&function.as_str()) {
                return plain(input, ctx, resolver).unwrap_or(parsed);
            }
            parsed
        }
        ParsedCommand::Security {
            security,
            function,
            args,
        } => {
            // `AAPL US <EQUITY> filings`, `AAPL US <EQUITY> 5y`, `… vs MSFT`.
            let tail = match function {
                None if !args.is_empty() => normalize(args.iter().map(String::as_str)),
                Some(f) if PLAIN_WORD_IDS.contains(&f.as_str()) => {
                    normalize(std::iter::once(f.as_str()).chain(args.iter().map(String::as_str)))
                }
                _ => return parsed,
            };
            match on_security(security.clone(), &tail, resolver) {
                Some(run @ ParsedCommand::Run(_)) => run,
                _ => parsed,
            }
        }
        ParsedCommand::Search(_) => plain(input, ctx, resolver).unwrap_or(parsed),
        ParsedCommand::Function { .. }
        | ParsedCommand::Empty
        | ParsedCommand::MenuItem(_)
        | ParsedCommand::Run(_) => parsed,
    }
}

fn plain<R: SecurityResolver + ?Sized>(
    input: &str,
    ctx: &ParseContext,
    resolver: &R,
) -> Option<ParsedCommand> {
    let words = words(input);
    if words.is_empty() {
        return None;
    }
    if let Some(a) = compare_action(&words, resolver) {
        return Some(ParsedCommand::Run(a));
    }
    if let Some(c) = command_exact(&words) {
        if c.data_word
            && let [w] = words.as_slice()
            && let Some(key) = resolver.exact(&w.to_ascii_uppercase(), None)
        {
            return Some(ParsedCommand::Run(Action::new("DES", Some(key))));
        }
        return Some(ParsedCommand::Run(c.action()));
    }
    if let Some((key, used)) = leading_security(&words, resolver)
        && let Some(cmd) = on_security(key, &words[used..], resolver)
    {
        return Some(cmd);
    }
    if let Some(a) = topic_then_security(&words, resolver) {
        return Some(ParsedCommand::Run(a));
    }
    if let Some(loaded) = &ctx.loaded
        && let Some(a) = topic_action(loaded, &words)
    {
        return Some(ParsedCommand::Run(a));
    }
    if words.len() >= 2 && input.trim_end().ends_with('?') {
        return Some(ParsedCommand::Run(
            Action::new("ASK", None).arg("q", input.trim()),
        ));
    }
    None
}

/// `ask <question>`, keeping the question as typed.
pub(crate) fn ask(input: &str) -> Option<Action> {
    let first = raw_tokens(input).next()?;
    if !first.eq_ignore_ascii_case("ask") {
        return None;
    }
    let end = first.as_ptr().addr() - input.as_ptr().addr() + first.len();
    let question = input[end..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '<' || c == '>')
        .trim_end();
    (!question.is_empty()).then(|| Action::new("ASK", None).arg("q", question))
}

/// What follows a resolved security: nothing (overview), `vs …`, a topic,
/// a range, or a mnemonic with its arguments.
fn on_security<R: SecurityResolver + ?Sized>(
    key: SecurityKey,
    tail: &[String],
    resolver: &R,
) -> Option<ParsedCommand> {
    let Some(first) = tail.first() else {
        return Some(ParsedCommand::Run(Action::new("DES", Some(key))));
    };
    if is_vs(first) {
        return compare_list(vec![key], &tail[1..], resolver).map(ParsedCommand::Run);
    }
    if let Some(a) = topic_action(&key, tail) {
        return Some(ParsedCommand::Run(a));
    }
    let spec = lookup(first)?;
    Some(ParsedCommand::Security {
        security: key,
        function: Some(spec.mnemonic.to_owned()),
        args: tail[1..].iter().map(|w| w.to_uppercase()).collect(),
    })
}

/// The action for `words` said about `key`: a range (`5y`), a topic with
/// optional modifiers (`chart 5y`, `income quarterly`) or a range then a
/// topic (`5y chart`). The longest matching phrase wins.
pub(crate) fn topic_action(key: &SecurityKey, words: &[String]) -> Option<Action> {
    if words.is_empty() {
        return None;
    }
    if let Some(r) = range_of(words) {
        return Some(Action::new("GP", Some(key.clone())).arg("range", r.code));
    }
    let mut best: Option<(usize, Action)> = None;
    let mut consider = |len: usize, action: Action| {
        if best.as_ref().is_none_or(|(l, _)| len > *l) {
            best = Some((len, action));
        }
    };
    for t in TOPICS {
        for phrase in t.phrases {
            let n = phrase.split(' ').count();
            if words.len() < n {
                continue;
            }
            if is_phrase(&words[..n], phrase)
                && let Some(a) = t.with_modifiers(key, &words[n..])
            {
                consider(n, a);
            }
            if t.ranged()
                && words.len() > n
                && is_phrase(&words[words.len() - n..], phrase)
                && let Some(r) = range_of(&words[..words.len() - n])
            {
                consider(n, t.action(key).arg("range", r.code));
            }
        }
    }
    best.map(|(_, a)| a)
}

/// `filings aapl`, `5y msft`, `news bmw gr`.
fn topic_then_security<R: SecurityResolver + ?Sized>(
    words: &[String],
    resolver: &R,
) -> Option<Action> {
    for used in [2, 1] {
        if words.len() <= used {
            continue;
        }
        let split = words.len() - used;
        if let Some((key, n)) = leading_security(&words[split..], resolver)
            && n == used
            && let Some(a) = topic_action(&key, &words[..split])
        {
            return Some(a);
        }
    }
    None
}

pub(crate) fn is_vs(word: &str) -> bool {
    matches!(word, "vs" | "vs." | "versus")
}

/// Words between securities in `compare aapl with msft`.
pub(crate) fn is_joiner(word: &str) -> bool {
    is_vs(word) || matches!(word, "with" | "and" | "to" | "against")
}

/// `aapl vs msft [vs …] [range]` or `compare aapl [with] msft [range]`.
pub(crate) fn compare_action<R: SecurityResolver + ?Sized>(
    words: &[String],
    resolver: &R,
) -> Option<Action> {
    if words.first().is_some_and(|w| w == "compare") {
        return compare_list(Vec::new(), &words[1..], resolver);
    }
    let at = words.iter().position(|w| is_vs(w))?;
    let (first, used) = leading_security(&words[..at], resolver)?;
    if used != at {
        return None;
    }
    compare_list(vec![first], &words[at + 1..], resolver)
}

/// Resolves the securities in `rest` (after any already in `keys`), with an
/// optional range at the end.
fn compare_list<R: SecurityResolver + ?Sized>(
    keys: Vec<SecurityKey>,
    rest: &[String],
    resolver: &R,
) -> Option<Action> {
    // A trailing range, unless the words only make sense as securities
    // (`aapl vs max` compares with MediaAlpha).
    for len in (1..=3).rev() {
        if rest.len() > len
            && let Some(r) = range_of(&rest[rest.len() - len..])
            && let Some(a) = compare_keys(keys.clone(), &rest[..rest.len() - len], resolver)
        {
            return Some(a.arg("range", r.code));
        }
    }
    compare_keys(keys, rest, resolver)
}

fn compare_keys<R: SecurityResolver + ?Sized>(
    mut keys: Vec<SecurityKey>,
    rest: &[String],
    resolver: &R,
) -> Option<Action> {
    let words: Vec<String> = rest.iter().filter(|w| !is_joiner(w)).cloned().collect();
    let mut i = 0;
    while i < words.len() {
        let (key, used) = leading_security(&words[i..], resolver)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
        i += used;
    }
    (2..=MAX_COMPARE)
        .contains(&keys.len())
        .then(|| compare_of(&keys, None))
}

/// The COMPARE action for `keys` over `range` (the screen's default when
/// `None`).
#[must_use]
pub fn compare_of(keys: &[SecurityKey], range: Option<&str>) -> Action {
    let list = keys
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let a = Action::new("COMPARE", keys.first().cloned()).arg("securities", list);
    match range {
        Some(r) => a.arg("range", r),
        None => a,
    }
}

/// The securities a COMPARE action lists, in order.
#[must_use]
pub fn compare_keys_of(action: &Action) -> Vec<SecurityKey> {
    action
        .arg_value("securities")
        .map(|s| s.split(',').filter_map(|k| k.trim().parse().ok()).collect())
        .unwrap_or_default()
}

/// The command whose phrase is exactly `words`.
pub(crate) fn command_exact(words: &[String]) -> Option<&'static CommandDef> {
    COMMANDS
        .iter()
        .find(|c| c.phrases.iter().any(|p| is_phrase(words, p)))
}

pub(crate) fn range_of(words: &[String]) -> Option<&'static RangeDef> {
    RANGES
        .iter()
        .find(|r| r.phrases.iter().any(|p| is_phrase(words, p)))
}

/// Whether `words` spell `phrase` exactly.
pub(crate) fn is_phrase(words: &[String], phrase: &str) -> bool {
    words.iter().map(String::as_str).eq(phrase.split(' '))
}

fn words_str(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).collect()
}

/// Lower-case words of `input`: split on whitespace, `<`, `>` and commas,
/// with trailing `?` and `!` dropped.
#[must_use]
pub fn words(input: &str) -> Vec<String> {
    normalize(raw_tokens(input))
}

fn normalize<'a>(tokens: impl Iterator<Item = &'a str>) -> Vec<String> {
    tokens
        .flat_map(|t| t.split(','))
        .map(|w| w.trim_end_matches(['?', '!']).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The security named by the first one or two words (`aapl`, `bmw gr`),
/// and how many words it used.
pub(crate) fn leading_security<R: SecurityResolver + ?Sized>(
    words: &[String],
    resolver: &R,
) -> Option<(SecurityKey, usize)> {
    let first = words.first()?;
    if let Some(exchange) = words
        .get(1)
        .filter(|w| w.len() == 2 && w.bytes().all(|b| b.is_ascii_alphabetic()))
        && let Some(key) = resolver.exact(
            &first.to_ascii_uppercase(),
            Some(&exchange.to_ascii_uppercase()),
        )
    {
        return Some((key, 2));
    }
    resolve_security(first, resolver).map(|key| (key, 1))
}

/// The security one typed word names, if any. See the module docs.
#[must_use]
pub fn resolve_security<R: SecurityResolver + ?Sized>(
    word: &str,
    resolver: &R,
) -> Option<SecurityKey> {
    let up = word.to_ascii_uppercase();
    let symbol_like = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'/');
    if up.is_empty() || up.len() > 24 || !up.bytes().all(symbol_like) {
        return None;
    }
    let coin = |base: &str| {
        resolver
            .exact(&format!("{base}USD"), None)
            .filter(|k| k.sector == MarketSector::Curncy)
    };
    let company_listed = || {
        COIN_TICKERS_OF_COMPANIES.contains(&up.as_str())
            && resolver.exact(&up, None).is_some_and(|k| k.sector == MarketSector::Equity)
    };
    if MAJOR_COINS.contains(&up.as_str())
        && !company_listed()
        && let Some(key) = coin(&up)
    {
        return Some(key);
    }
    if let Some(key) = resolver.exact(&up, None) {
        return Some(key);
    }
    // Class shares and pairs typed with another separator: brk.b → BRK-B.
    let is_sep = |c: char| matches!(c, '.' | '/' | '-');
    if up.contains(is_sep) {
        for sep in ["-", ".", "/", ""] {
            let variant = up.replace(is_sep, sep);
            if variant != up
                && let Some(key) = resolver.exact(&variant, None)
            {
                return Some(key);
            }
        }
    }
    let joined: String = up.chars().filter(char::is_ascii_alphanumeric).collect();
    coin(&joined).or_else(|| currency_pair(&joined))
}

/// `EURUSD` → `EURUSD Curncy` when both halves are ISO currency codes.
fn currency_pair(s: &str) -> Option<SecurityKey> {
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let (base, quote) = s.split_at(3);
    (base != quote && CURRENCIES.contains(&base) && CURRENCIES.contains(&quote))
        .then(|| SecurityKey::currency(s))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use super::*;

    /// Symbol → keys, US listings first.
    struct Fixture(HashMap<String, Vec<SecurityKey>>);

    impl SecurityResolver for Fixture {
        fn exact(&self, symbol: &str, exchange: Option<&str>) -> Option<SecurityKey> {
            self.0
                .get(symbol)?
                .iter()
                .find(|k| exchange.is_none_or(|x| k.exchange.as_deref() == Some(x)))
                .cloned()
        }
    }

    fn fixture() -> Fixture {
        let keys = [
            SecurityKey::equity("AAPL"),
            SecurityKey::equity("MSFT"),
            SecurityKey::equity("GOOGL"),
            SecurityKey::equity("KO"),
            SecurityKey::equity("T"),
            SecurityKey::equity("V"),
            SecurityKey::equity("MAX"),
            SecurityKey::equity("BRK-B"),
            SecurityKey::equity("BTC"),
            SecurityKey::equity("LTC"),
            SecurityKey::equity("SPY"),
            SecurityKey::new("BMW", Some("GR"), MarketSector::Equity),
            SecurityKey::index("SPX"),
            SecurityKey::currency("BTCUSD"),
            SecurityKey::currency("ETHUSD"),
            SecurityKey::currency("PEPEUSD"),
            SecurityKey::currency("LTCUSD"),
            SecurityKey::currency("LINKUSD"),
            SecurityKey::currency("EURUSD"),
        ];
        let mut map: HashMap<String, Vec<SecurityKey>> = HashMap::new();
        for k in keys {
            map.entry(k.symbol.clone()).or_default().push(k);
        }
        Fixture(map)
    }

    /// Typed text, function, arguments.
    type Case = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );

    fn run(input: &str) -> ParsedCommand {
        interpret(input, &ParseContext::default(), &fixture())
    }

    fn run_loaded(input: &str, loaded: SecurityKey) -> ParsedCommand {
        let ctx = ParseContext {
            loaded: Some(loaded),
        };
        interpret(input, &ctx, &fixture())
    }

    fn action(f: &str, sec: Option<SecurityKey>, args: &[(&str, &str)]) -> ParsedCommand {
        ParsedCommand::Run(
            args.iter()
                .fold(Action::new(f, sec), |a, (k, v)| a.arg(k, *v)),
        )
    }

    #[allow(clippy::unnecessary_wraps)]
    fn aapl() -> Option<SecurityKey> {
        Some(SecurityKey::equity("AAPL"))
    }

    #[test]
    fn a_security_alone_opens_its_overview() {
        assert_eq!(run("aapl"), action("DES", aapl(), &[]));
        assert_eq!(run("  AAPL "), action("DES", aapl(), &[]));
        assert_eq!(
            run("brk.b"),
            action("DES", Some(SecurityKey::equity("BRK-B")), &[])
        );
        assert_eq!(
            run("brk/b"),
            action("DES", Some(SecurityKey::equity("BRK-B")), &[])
        );
        assert_eq!(
            run("eurusd"),
            action("DES", Some(SecurityKey::currency("EURUSD")), &[])
        );
        assert_eq!(
            run("eur/usd"),
            action("DES", Some(SecurityKey::currency("EURUSD")), &[])
        );
        // Not in the index, but two ISO codes name the pair.
        assert_eq!(
            run("usdjpy"),
            action("DES", Some(SecurityKey::currency("USDJPY")), &[])
        );
        assert_eq!(
            run("bmw gr"),
            action(
                "DES",
                Some(SecurityKey::new("BMW", Some("GR"), MarketSector::Equity)),
                &[]
            )
        );
        assert_eq!(
            run("aapl us"),
            action("DES", aapl(), &[]),
            "ticker and exchange"
        );
    }

    #[test]
    fn crypto_shorthand() {
        let btc = Some(SecurityKey::currency("BTCUSD"));
        // A major coin beats the fund that trades as BTC.
        assert_eq!(run("btc"), action("DES", btc.clone(), &[]));
        assert_eq!(run("BTC 5y"), action("GP", btc, &[("range", "5Y")]));
        assert_eq!(
            run("eth"),
            action("DES", Some(SecurityKey::currency("ETHUSD")), &[])
        );
        // A company listed under a coin's ticker wins; the coin is LTCUSD.
        assert_eq!(run("ltc"), action("DES", Some(SecurityKey::equity("LTC")), &[]));
        assert_eq!(run("ltcusd"), action("DES", Some(SecurityKey::currency("LTCUSD")), &[]));
        // Without that listing the shorthand is the coin.
        assert_eq!(run("link"), action("DES", Some(SecurityKey::currency("LINKUSD")), &[]));
        // Other coins resolve when no ticker matches.
        assert_eq!(
            run("pepe"),
            action("DES", Some(SecurityKey::currency("PEPEUSD")), &[])
        );
        // The fund is still reachable with its exchange.
        assert_eq!(
            run("btc us"),
            action("DES", Some(SecurityKey::equity("BTC")), &[])
        );
    }

    #[test]
    fn unknown_words_still_search() {
        assert_eq!(run("apple"), ParsedCommand::Search("apple".into()));
        assert_eq!(
            run("bank of america"),
            ParsedCommand::Search("bank of america".into())
        );
        assert_eq!(run("aapl qqqq"), ParsedCommand::Search("aapl qqqq".into()));
        assert_eq!(run("zzzz 5y"), ParsedCommand::Search("zzzz 5y".into()));
        // A topic alone needs a loaded security.
        assert_eq!(run("chart"), ParsedCommand::Search("chart".into()));
    }

    #[test]
    fn ranges_open_the_chart() {
        for (typed, code) in [
            ("1d", "1D"),
            ("5d", "5D"),
            ("1m", "1M"),
            ("3m", "3M"),
            ("6m", "6M"),
            ("ytd", "YTD"),
            ("1y", "1Y"),
            ("2y", "2Y"),
            ("5y", "5Y"),
            ("10y", "10Y"),
            ("max", "MAX"),
            ("5 years", "5Y"),
            ("1 week", "5D"),
            ("year to date", "YTD"),
        ] {
            assert_eq!(
                run(&format!("aapl {typed}")),
                action("GP", aapl(), &[("range", code)]),
                "{typed}"
            );
        }
        assert_eq!(run("aapl chart"), action("GP", aapl(), &[]));
        assert_eq!(run("aapl graph"), action("GP", aapl(), &[]));
        assert_eq!(
            run("aapl chart 5y"),
            action("GP", aapl(), &[("range", "5Y")])
        );
        assert_eq!(
            run("aapl 5y chart"),
            action("GP", aapl(), &[("range", "5Y")])
        );
        assert_eq!(run("aapl intraday"), action("GIP", aapl(), &[]));
        assert_eq!(
            run("aapl intraday 5d"),
            action("GIP", aapl(), &[("range", "5D")])
        );
        assert_eq!(run("aapl today"), action("GIP", aapl(), &[]));
    }

    #[test]
    fn security_topics() {
        let cases: &[Case] = &[
            ("news", "CN", &[]),
            ("filings", "CF", &[]),
            ("10-k", "CF", &[("form", "10-K")]),
            ("10-q", "CF", &[("form", "10-Q")]),
            ("8-k", "CF", &[("form", "8-K")]),
            ("10k", "CF", &[("form", "10-K")]),
            ("financials", "FA", &[]),
            ("income", "FA", &[("stmt", "IS")]),
            ("income statement", "FA", &[("stmt", "IS")]),
            ("balance sheet", "FA", &[("stmt", "BS")]),
            ("cash flow", "FA", &[("stmt", "CF")]),
            ("ratios", "FA", &[("stmt", "RATIOS")]),
            ("earnings", "ERN", &[]),
            ("analysts", "ANR", &[]),
            ("ratings", "ANR", &[]),
            ("dividends", "DVD", &[]),
            ("splits", "DVD", &[]),
            ("options", "OMON", &[]),
            ("chain", "OMON", &[]),
            ("volatility", "OVDV", &[]),
            ("vol", "OVDV", &[]),
            ("smile", "OVDV", &[]),
            ("backtest", "BTST", &[]),
            ("peers", "RV", &[]),
            ("comps", "RV", &[]),
            ("history", "HP", &[]),
            ("prices", "HP", &[]),
            ("description", "DES", &[]),
            ("profile", "DES", &[]),
            ("overview", "DES", &[]),
        ];
        for (tail, f, args) in cases {
            assert_eq!(
                run(&format!("aapl {tail}")),
                action(f, aapl(), args),
                "{tail}"
            );
            assert_eq!(
                run(&format!("AAPL {tail}")),
                action(f, aapl(), args),
                "{tail}"
            );
        }
        assert_eq!(
            run("aapl backtest 10y"),
            action("BTST", aapl(), &[("range", "10Y")])
        );
        assert_eq!(
            run("aapl balance sheet quarterly"),
            action("FA", aapl(), &[("stmt", "BS"), ("per", "Quarterly")])
        );
        // Words after a topic that don't modify it: not a command.
        assert_eq!(
            run("aapl news 5y"),
            ParsedCommand::Search("aapl news 5y".into())
        );
    }

    #[test]
    fn topic_before_the_security() {
        assert_eq!(run("filings aapl"), action("CF", aapl(), &[]));
        assert_eq!(
            run("news msft"),
            action("CN", Some(SecurityKey::equity("MSFT")), &[])
        );
        assert_eq!(run("5y aapl"), action("GP", aapl(), &[("range", "5Y")]));
        assert_eq!(
            run("balance sheet aapl"),
            action("FA", aapl(), &[("stmt", "BS")])
        );
        assert_eq!(
            run("chart bmw gr"),
            action(
                "GP",
                Some(SecurityKey::new("BMW", Some("GR"), MarketSector::Equity)),
                &[]
            )
        );
    }

    #[test]
    fn a_topic_alone_uses_the_loaded_security() {
        let msft = SecurityKey::equity("MSFT");
        assert_eq!(
            run_loaded("chart", msft.clone()),
            action("GP", Some(msft.clone()), &[])
        );
        assert_eq!(
            run_loaded("5y", msft.clone()),
            action("GP", Some(msft.clone()), &[("range", "5Y")])
        );
        assert_eq!(
            run_loaded("options", msft.clone()),
            action("OMON", Some(msft.clone()), &[])
        );
        // Plain commands stay global even with a security loaded.
        assert_eq!(run_loaded("news", msft.clone()), action("TOP", None, &[]));
        // A typed security wins over the loaded one.
        assert_eq!(run_loaded("aapl news", msft), action("CN", aapl(), &[]));
    }

    #[test]
    fn mnemonics_after_a_bare_ticker() {
        let sec = |f: &str, args: &[&str]| ParsedCommand::Security {
            security: SecurityKey::equity("AAPL"),
            function: Some(f.into()),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        };
        assert_eq!(run("aapl gp 5y"), sec("GP", &["5Y"]));
        assert_eq!(run("aapl des"), sec("DES", &[]));
        assert_eq!(run("aapl hp weekly"), sec("HP", &["WEEKLY"]));
        assert_eq!(run("aapl ovme"), sec("OVME", &[]));
    }

    #[test]
    fn mnemonic_syntax_is_unchanged() {
        let ctx = ParseContext::default();
        for input in [
            "AAPL US <EQUITY> GP 5Y",
            "AAPL US <EQUITY> DES",
            "AAPL US <EQUITY>",
            "GP 5Y",
            "DES",
            "CN",
            "W",
            "12",
            "",
            "HELP GP",
            "EURUSD <CRNCY> GP",
            "ask",
            "today",
            "filings",
            "calendar",
            "compare",
        ] {
            assert_eq!(run(input), parse(input, &ctx), "{input}");
        }
    }

    #[test]
    fn plain_words_after_a_full_key() {
        assert_eq!(run("AAPL US <EQUITY> filings"), action("CF", aapl(), &[]));
        assert_eq!(
            run("AAPL US <EQUITY> 1Y"),
            action("GP", aapl(), &[("range", "1Y")])
        );
        assert_eq!(
            run("aapl equity balance sheet"),
            action("FA", aapl(), &[("stmt", "BS")])
        );
        assert_eq!(
            run("AAPL US <EQUITY> vs MSFT"),
            ParsedCommand::Run(compare_of(
                &[SecurityKey::equity("AAPL"), SecurityKey::equity("MSFT")],
                None
            ))
        );
        // Unrecognized words after a key: the grammar's reading stands.
        assert_eq!(
            run("AAPL US <EQUITY> foo"),
            parse("AAPL US <EQUITY> foo", &ParseContext::default())
        );
    }

    #[test]
    fn comparisons() {
        let msft = SecurityKey::equity("MSFT");
        let googl = SecurityKey::equity("GOOGL");
        let a = SecurityKey::equity("AAPL");
        let two = compare_of(&[a.clone(), msft.clone()], None);
        assert_eq!(run("aapl vs msft"), ParsedCommand::Run(two.clone()));
        assert_eq!(run("AAPL versus MSFT"), ParsedCommand::Run(two.clone()));
        assert_eq!(run("compare aapl msft"), ParsedCommand::Run(two.clone()));
        assert_eq!(
            run("compare aapl with msft"),
            ParsedCommand::Run(two.clone())
        );
        assert_eq!(
            run("compare aapl and msft"),
            ParsedCommand::Run(two.clone())
        );
        assert_eq!(run("compare aapl, msft"), ParsedCommand::Run(two.clone()));
        assert_eq!(
            run("aapl vs msft 5y"),
            ParsedCommand::Run(compare_of(&[a.clone(), msft.clone()], Some("5Y")))
        );
        assert_eq!(
            run("aapl vs msft vs googl"),
            ParsedCommand::Run(compare_of(&[a.clone(), msft.clone(), googl.clone()], None))
        );
        assert_eq!(
            run("aapl vs msft vs googl 10 years"),
            ParsedCommand::Run(compare_of(&[a.clone(), msft, googl], Some("10Y")))
        );
        // MAX is a ticker here, not the range.
        assert_eq!(
            run("aapl vs max"),
            ParsedCommand::Run(compare_of(&[a.clone(), SecurityKey::equity("MAX")], None))
        );
        assert_eq!(
            run("aapl vs btc"),
            ParsedCommand::Run(compare_of(
                &[a.clone(), SecurityKey::currency("BTCUSD")],
                None
            ))
        );
        // V is Visa, not a separator.
        assert_eq!(
            run("aapl vs v"),
            ParsedCommand::Run(compare_of(&[a.clone(), SecurityKey::equity("V")], None))
        );
        // The args the screen reads.
        let ParsedCommand::Run(act) = run("aapl vs msft 5y") else {
            panic!()
        };
        assert_eq!(act.function, "COMPARE");
        assert_eq!(act.security, Some(a.clone()));
        assert_eq!(
            act.arg_value("securities"),
            Some("AAPL US Equity, MSFT US Equity")
        );
        assert_eq!(act.arg_value("range"), Some("5Y"));
        assert_eq!(compare_keys_of(&act).len(), 2);
        // Unresolvable or single operands: no comparison.
        assert_eq!(
            run("aapl vs zzzz"),
            ParsedCommand::Search("aapl vs zzzz".into())
        );
        assert_eq!(
            run("aapl vs aapl"),
            ParsedCommand::Search("aapl vs aapl".into())
        );
        assert_eq!(run("vs msft"), ParsedCommand::Search("vs msft".into()));
        assert!(matches!(
            run("compare zzzz msft"),
            ParsedCommand::Function { .. }
        ));
    }

    #[test]
    fn global_commands() {
        let cases: &[Case] = &[
            ("home", "TODAY", &[]),
            (
                "earnings this week",
                "CALENDAR",
                &[("kind", "earnings"), ("range", "this-week")],
            ),
            (
                "Earnings Next Week",
                "CALENDAR",
                &[("kind", "earnings"), ("range", "next-week")],
            ),
            (
                "earnings today",
                "CALENDAR",
                &[("kind", "earnings"), ("range", "today")],
            ),
            (
                "dividends this week",
                "CALENDAR",
                &[("kind", "dividends"), ("range", "this-week")],
            ),
            ("earnings", "CALENDAR", &[("kind", "earnings")]),
            ("portfolio", "PORT", &[]),
            ("import", "IMPORT", &[]),
            ("news", "TOP", &[]),
            ("headlines", "TOP", &[]),
            ("crypto", "CRYP", &[]),
            ("fx", "FXC", &[]),
            ("currencies", "FXC", &[]),
            ("indices", "WEI", &[]),
            ("world", "WEI", &[]),
            ("rates", "ECO", &[("view", "curve")]),
            ("yield curve", "ECO", &[("view", "curve")]),
            ("treasury", "ECO", &[("view", "curve")]),
            ("cpi", "ECO", &[("series", "CPIAUCSL")]),
            ("inflation", "ECO", &[("series", "CPIAUCSL")]),
            ("unemployment", "ECO", &[("series", "UNRATE")]),
            ("jobs", "ECO", &[("series", "PAYEMS")]),
            ("payrolls", "ECO", &[("series", "PAYEMS")]),
            ("gdp", "ECO", &[("series", "GDP")]),
            ("fed funds", "ECO", &[("series", "FEDFUNDS")]),
            ("screener", "EQS", &[]),
            ("alerts", "ALRT", &[]),
            ("settings", "SETTINGS", &[]),
        ];
        for (typed, f, args) in cases {
            assert_eq!(run(typed), action(f, None, args), "{typed}");
        }
        // Registered words run as functions through the grammar.
        let func = |f: &str| ParsedCommand::Function {
            function: f.into(),
            args: vec![],
        };
        assert_eq!(run("today"), func("TODAY"));
        assert_eq!(run("calendar"), func("CALENDAR"));
        assert_eq!(run("filings"), func("FILINGS"));
        assert_eq!(run("help"), func("HELP"));
        assert!(Action::new("SETTINGS", None).is_app_action());
        assert!(!Action::new("TODAY", None).is_app_action());
    }

    #[test]
    fn data_words_yield_to_an_exact_ticker() {
        let mut f = fixture();
        f.0.insert("GDP".into(), vec![SecurityKey::equity("GDP")]);
        f.0.insert("NEWS".into(), vec![SecurityKey::equity("NEWS")]);
        let ctx = ParseContext::default();
        assert_eq!(
            interpret("gdp", &ctx, &f),
            action("DES", Some(SecurityKey::equity("GDP")), &[])
        );
        // Without that ticker, the series.
        assert_eq!(run("gdp"), action("ECO", None, &[("series", "GDP")]));
        // Navigation words never yield.
        assert_eq!(interpret("news", &ctx, &f), action("TOP", None, &[]));
        // The ticker named NEWS is still reachable with its exchange.
        assert_eq!(
            interpret("news us", &ctx, &f),
            action("DES", Some(SecurityKey::equity("NEWS")), &[])
        );
    }

    #[test]
    fn ask_keeps_the_question() {
        assert_eq!(
            run("ask What drove Apple's margin?"),
            action("ASK", None, &[("q", "What drove Apple's margin?")])
        );
        assert_eq!(
            run("ASK  is <AAPL> cheap"),
            action("ASK", None, &[("q", "is <AAPL> cheap")])
        );
        assert_eq!(
            run("why did msft fall today?"),
            action("ASK", None, &[("q", "why did msft fall today?")])
        );
        // One word with a question mark is not a question.
        assert_eq!(run("aapl?"), action("DES", aapl(), &[]));
        assert_eq!(run("aapl news?"), action("CN", aapl(), &[]));
    }

    #[test]
    fn hints() {
        assert_eq!(hint_for("SETTINGS"), "⌘,");
        assert_eq!(hint_for("CF"), "CF");
        assert_eq!(range_label("5y"), Some("5 years"));
        assert_eq!(range_label("YTD"), Some("year to date"));
        assert_eq!(range_label("7Y"), None);
    }

    #[test]
    fn resolved_commands_validate_and_round_trip() {
        let ctx = ParseContext::default();
        for input in ["aapl filings", "aapl vs msft 5y", "settings", "cpi", "ask why?"] {
            let cmd = run(input);
            assert!(matches!(cmd, ParsedCommand::Run(_)), "{input}");
            assert_eq!(crate::validate(&cmd, &ctx), Ok(()), "{input}");
            let json = serde_json::to_string(&cmd).unwrap();
            assert_eq!(serde_json::from_str::<ParsedCommand>(&json).unwrap(), cmd);
        }
        // A topic on the loaded security still checks the sector.
        let fx = SecurityKey::currency("EURUSD");
        let cmd = run_loaded("financials", fx.clone());
        assert_eq!(cmd.target_security(&ParseContext::default()), Some(&fx));
        assert!(crate::validate(&cmd, &ParseContext::default()).is_err());
    }

    #[test]
    fn tables_are_well_formed() {
        for t in TOPICS {
            assert!(
                t.spec().is_some_and(FunctionSpec::takes_security),
                "{}",
                t.function
            );
            for p in t.phrases {
                assert_eq!(*p, p.to_lowercase());
                // A topic phrase that is also a mnemonic would be
                // unreachable, except the plain-word ids read as phrases.
                assert!(
                    lookup(p).is_none() || PLAIN_WORD_IDS.contains(&p.to_uppercase().as_str()),
                    "{p}"
                );
            }
        }
        for c in COMMANDS {
            assert!(
                lookup(c.function).is_some() || APP_ACTIONS.contains(&c.function),
                "{}",
                c.function
            );
            assert!(
                !c.title().is_empty() && !c.subtitle().is_empty(),
                "{}",
                c.function
            );
        }
        for r in RANGES {
            assert!(r.phrases.iter().all(|p| *p == p.to_lowercase()));
        }
        // Every phrase means one thing.
        let mut seen = std::collections::HashSet::new();
        for p in COMMANDS.iter().flat_map(|c| c.phrases.iter()) {
            assert!(seen.insert(*p), "duplicate command phrase {p}");
        }
        let mut seen = std::collections::HashSet::new();
        for p in TOPICS
            .iter()
            .flat_map(|t| t.phrases.iter())
            .chain(RANGES.iter().flat_map(|r| r.phrases.iter()))
        {
            assert!(seen.insert(*p), "duplicate topic phrase {p}");
        }
    }

    proptest! {
        #[test]
        fn interpret_never_panics(input in ".*") {
            let _ = run(&input);
        }

        #[test]
        fn plain_actions_are_resolved(words in prop::collection::vec(prop_oneof![
            Just("aapl"), Just("msft"), Just("vs"), Just("5y"), Just("news"), Just("chart"),
            Just("compare"), Just("with"), Just("ask"), Just("btc"), Just("max"), Just("?"),
            Just("earnings"), Just("this"), Just("week"), Just("GP"), Just("<EQUITY>"),
        ], 0..6)) {
            let input = words.join(" ");
            if let ParsedCommand::Run(a) = run(&input) {
                prop_assert!(lookup(&a.function).is_some() || a.is_app_action());
                if a.function == "COMPARE" {
                    // Bare `compare` opens the screen to pick securities;
                    // otherwise a comparison names 2 to MAX_COMPARE.
                    let keys = compare_keys_of(&a);
                    prop_assert!(keys.is_empty() || (2..=MAX_COMPARE).contains(&keys.len()));
                }
            }
        }
    }
}
