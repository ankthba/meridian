//! Command-line autocomplete for the grouped completion popover.
//!
//! Each row has a group heading, a plain title and subtitle, a right-side
//! hint (the mnemonic; empty for securities, where the app shows the live
//! price), the text Tab puts in the bar (`completion`) and, when the row
//! names one, the fully resolved [`Action`] that Return runs.
//!
//! # Groups
//!
//! | Group | Rows |
//! |---|---|
//! | `Compare AAPL with` | securities to add while typing `aapl vs …` |
//! | `Security` | securities matching the input; for `aapl fil`, the typed security |
//! | `On AAPL` | what to open for a typed security (`aapl fil` → Filings, `aapl 5` → 5-day and 5-year charts), or for the panel's security when a topic is typed alone (`chart`) |
//! | `Commands` | commands that need no security: `earnings this week`, `cpi`, `settings`, `ask …` |
//! | `Functions` | registry functions by mnemonic, title or keyword |
//!
//! The result is in display order: groups in the order above, best first
//! within each group. `best` marks the single best row overall, which Return
//! runs while the user hasn't moved the highlight. When [`interpret`] makes
//! a command of the input, that command's row is the best one, so Return on
//! the highlight and GO on the typed text agree.
//!
//! A security typed with nothing after it (`aapl `) lists what can be opened
//! on it; the full key (`AAPL US <EQUITY> `) does the same with mnemonic
//! completions. A sector typed as a plain word may belong to a name
//! (`nvidia corp`, `s&p index`); unless the key is in the index or the sector
//! key itself was used (`<CORP>`), name matches are merged in. When the panel
//! has a security loaded, functions that can't run on it lose 150 points.
//!
//! # Ranking
//!
//! Each match falls into a tier with a base score. Popularity adds up to
//! [`POPULARITY_WEIGHT`], which is smaller than the gap between tiers, so it
//! orders matches within a tier and never lifts a match into a higher one.
//!
//! | Tier | Base |
//! |---|---|
//! | exact mnemonic (what GO would run), exact phrase (`aapl filings`, `cpi`) | 1200 |
//! | exact ticker (`AAPL`, `AAPL US`, `AAPL US EQU`) | 1000 |
//! | a command's phrase starts with the input (`earn` → earnings this week) | 850, minus 0.5 per table position |
//! | mnemonic, ticker, topic or range prefix, minus 2 per missing character | 800 |
//! | the typed security above its `On` rows | 700 |
//! | every query word prefixes a word of the name or function title | 600 (+40 if the first words match, +10 if equal) |
//! | every query word prefixes a title or keyword word | 500 |
//! | the whole query is a substring of the name (3+ characters) | 400 |
//! | the query is a subsequence of a topic's title, same first letter (`fil` → Financials) | 300, minus gaps |
//! | the query is a subsequence of the ticker, same first letter | 200, minus gaps |
//! | listing for a security with nothing typed after it | 100, minus 1 per row |
//!
//! Functions, topics and commands rank as if their popularity were 0.5;
//! topics for the panel's loaded security lose 20. Ties break on the shorter
//! ticker or mnemonic, then alphabetically. Rows that run the same action
//! are merged, keeping the better score.
//!
//! # Performance
//!
//! Tickers and name words are kept in sorted vectors, so the prefix tiers are
//! binary searches. The substring and subsequence tiers need a linear scan,
//! which is skipped when the indexed tiers already fill `limit` with scores
//! the scan can't beat. Plain-language rows scan the fixed phrase tables
//! (about 150 phrases). The budget is 5 ms per call over the full universe;
//! `benches/suggest.rs` measures it.

use std::cmp::Ordering;
use std::collections::HashMap;

use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Serialize};

use crate::parse::{
    ParseContext, ParsedCommand, find_sector, format_security, raw_tokens, security_key,
    token_is_bracketed,
};
use crate::plain::{
    self, Action, COMMANDS, MAX_COMPARE, RANGES, SecurityResolver, TOPICS, Topic, compare_keys_of,
    compare_of, hint_for, interpret, is_joiner, is_phrase, is_vs, leading_security, range_title,
    resolve_security,
};
use crate::registry::{FunctionSpec, lookup, registry};

/// Most popularity can add to a score.
pub const POPULARITY_WEIGHT: f32 = 100.0;

const EXACT_MNEMONIC: f32 = 1200.0;
const EXACT_PHRASE: f32 = EXACT_MNEMONIC;
const EXACT_TICKER: f32 = 1000.0;
const COMMAND_PREFIX: f32 = 850.0;
const COMMAND_ORDER_STEP: f32 = 0.5;
const PREFIX: f32 = 800.0;
const PREFIX_LEN_PENALTY: f32 = 2.0;
/// Caps the prefix length penalty so long tickers stay above the next tier.
const MAX_PREFIX_PENALTY_CHARS: usize = 20;
const CONTEXT_SECURITY: f32 = 700.0;
const WORD_PREFIX: f32 = 600.0;
const FIRST_WORD_BONUS: f32 = 40.0;
const EXACT_FIRST_WORD_BONUS: f32 = 10.0;
const KEYWORD_PREFIX: f32 = 500.0;
const NAME_SUBSTRING: f32 = 400.0;
const TOPIC_FUZZY: f32 = 300.0;
const TICKER_SUBSEQUENCE: f32 = 200.0;
const SUBSEQUENCE_GAP_PENALTY: f32 = 5.0;
/// Highest score the linear-scan tiers can produce.
const SCAN_TIER_MAX: f32 = NAME_SUBSTRING + POPULARITY_WEIGHT;
/// Functions, topics and commands rank as if they had this popularity.
const FUNCTION_POPULARITY: f32 = 0.5;
const FUNCTION_BOOST: f32 = FUNCTION_POPULARITY * POPULARITY_WEIGHT;
/// Subtracted from functions that can't run on the loaded security.
const INCOMPATIBLE_PENALTY: f32 = 150.0;
/// Subtracted from topics offered for the panel's loaded security.
const LOADED_PENALTY: f32 = 20.0;
/// Score of the first row when listing everything for a security.
const LIST_SCORE: f32 = 100.0;
const MIN_SUBSTRING_LEN: usize = 3;
const MIN_SUBSEQUENCE_LEN: usize = 2;
/// Plain commands and loaded-security topics need this many typed letters.
const MIN_PLAIN_CHARS: usize = 2;
/// Name words beyond this many aren't indexed.
const MAX_NAME_WORDS: usize = 32;
const NO_SLOT: u32 = u32::MAX;

/// One security in the autocomplete index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedInstrument {
    pub key: SecurityKey,
    /// Display name, e.g. `Apple Inc.`.
    pub name: String,
    /// Relative popularity in `[0, 1]`. Values outside are clamped and NaN
    /// counts as 0.
    pub popularity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuggestionKind {
    /// A security (its row opens the security).
    Security,
    /// A function, topic or command.
    Function,
}

/// The heading a row is listed under.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuggestionGroup {
    /// Securities to add to a comparison with the named security.
    Compare(String),
    Security,
    /// Things to open for the named security.
    On(String),
    Commands,
    Functions,
}

impl SuggestionGroup {
    /// Sentence-case heading: `Security`, `On AAPL`, `Compare AAPL with`.
    #[must_use]
    pub fn heading(&self) -> String {
        match self {
            Self::Compare(s) => format!("Compare {s} with"),
            Self::Security => "Security".into(),
            Self::On(s) => format!("On {s}"),
            Self::Commands => "Commands".into(),
            Self::Functions => "Functions".into(),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Compare(_) => 0,
            Self::Security => 1,
            Self::On(_) => 2,
            Self::Commands => 3,
            Self::Functions => 4,
        }
    }
}

/// One autocomplete row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    /// `AAPL US Equity` or `DES` (the function id for plain rows).
    pub display: String,
    /// Security name or function title.
    pub detail: String,
    /// Replacement for the whole command line, ending in a space, e.g.
    /// `aapl `, `aapl filings ` or `AAPL US <EQUITY> GP `.
    pub completion: String,
    /// Higher is better. Comparable only within one call.
    pub score: f32,
    pub group: SuggestionGroup,
    /// `AAPL`, `Filings`, `Earnings this week`.
    pub title: String,
    /// The security's name or a plain description.
    pub subtitle: String,
    /// Mnemonic or shortcut shown on the right; empty for securities.
    pub hint: String,
    /// What Return runs on this row; `None` when it needs more input (a
    /// function that needs a security, with none loaded).
    pub action: Option<Action>,
    /// The single best row: what Return runs before the highlight moves.
    pub best: bool,
}

/// In-memory autocomplete index over instruments and the function registry.
#[derive(Debug, Clone)]
pub struct SuggestIndex {
    entries: Vec<Entry>,
    by_key: HashMap<SecurityKey, u32>,
    /// Entry indices sorted by (ticker, display).
    by_ticker: Vec<u32>,
    /// Every indexed name word, sorted by text.
    by_word: Vec<WordRef>,
    functions: Vec<FunctionEntry>,
}

#[derive(Debug, Clone)]
struct Entry {
    inst: IndexedInstrument,
    /// Upper-case symbol.
    ticker: String,
    /// Upper-case exchange, or empty.
    exchange: String,
    name_lower: String,
    /// Lower-case alphanumeric words of the name, in order.
    words: Vec<String>,
    display: String,
    /// Popularity contribution to the score.
    boost: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WordRef {
    entry: u32,
    word: u16,
}

#[derive(Debug, Clone)]
struct FunctionEntry {
    spec: &'static FunctionSpec,
    title_words: Vec<String>,
    keyword_words: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    score: f32,
    kind: SuggestionKind,
    /// Entry index for securities, registry index for functions.
    idx: u32,
}

/// How a typed query matches a phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhraseMatch {
    Exact,
    /// The phrase continues for this many more characters.
    Prefix(usize),
}

impl PhraseMatch {
    fn score(self, penalty: f32) -> f32 {
        match self {
            Self::Exact => EXACT_PHRASE + FUNCTION_BOOST - penalty,
            Self::Prefix(missing) => prefix_score(missing) + FUNCTION_BOOST - penalty,
        }
    }
}

impl Default for SuggestIndex {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl SecurityResolver for SuggestIndex {
    fn exact(&self, symbol: &str, exchange: Option<&str>) -> Option<SecurityKey> {
        self.ticker_range(symbol)
            .iter()
            .map(|&e| &self.entries[e as usize])
            .filter(|e| e.ticker == symbol && exchange.is_none_or(|x| e.exchange == x))
            .max_by(|a, b| {
                resolve_rank(a)
                    .cmp(&resolve_rank(b))
                    .then_with(|| a.boost.total_cmp(&b.boost))
            })
            .map(|e| e.inst.key.clone())
    }
}

/// Which of several securities sharing a symbol a bare ticker means: a US
/// listing, then an index, a currency or crypto pair, anything else.
fn resolve_rank(e: &Entry) -> u8 {
    match e.inst.key.sector {
        MarketSector::Equity if e.exchange == "US" => 4,
        MarketSector::Index => 3,
        MarketSector::Curncy => 2,
        MarketSector::Equity => 0,
        _ => 1,
    }
}

impl SuggestIndex {
    /// Builds the index. When two instruments share a key, the later one wins.
    #[must_use]
    pub fn new(instruments: Vec<IndexedInstrument>) -> Self {
        let mut index = Self {
            entries: Vec::with_capacity(instruments.len()),
            by_key: HashMap::with_capacity(instruments.len()),
            by_ticker: Vec::new(),
            by_word: Vec::new(),
            functions: registry().iter().map(FunctionEntry::new).collect(),
        };
        for inst in instruments {
            let entry = Entry::new(inst);
            if let Some(&i) = index.by_key.get(&entry.inst.key) {
                index.entries[i as usize] = entry;
            } else {
                let Ok(i) = u32::try_from(index.entries.len()) else {
                    break;
                };
                index.by_key.insert(entry.inst.key.clone(), i);
                index.entries.push(entry);
            }
        }

        let mut by_ticker: Vec<u32> = (0..index.entries.len() as u32).collect();
        by_ticker.sort_unstable_by(|&a, &b| index.ticker_cmp(a, b));
        let mut by_word: Vec<WordRef> = index
            .entries
            .iter()
            .enumerate()
            .flat_map(|(e, entry)| {
                (0..entry.words.len()).map(move |w| WordRef {
                    entry: e as u32,
                    word: w as u16,
                })
            })
            .collect();
        by_word.sort_unstable_by(|&a, &b| index.word_cmp(a, b));
        index.by_ticker = by_ticker;
        index.by_word = by_word;
        index
    }

    /// Adds an instrument, or replaces the one with the same key.
    /// Costs O(n); use [`SuggestIndex::new`] for bulk loads.
    pub fn add_or_update(&mut self, inst: IndexedInstrument) {
        let entry = Entry::new(inst);
        if let Some(&i) = self.by_key.get(&entry.inst.key) {
            // Same key means same ticker and display, so `by_ticker` stays sorted.
            self.by_word.retain(|r| r.entry != i);
            self.entries[i as usize] = entry;
            self.insert_words(i);
            return;
        }
        let Ok(i) = u32::try_from(self.entries.len()) else {
            return;
        };
        self.by_key.insert(entry.inst.key.clone(), i);
        self.entries.push(entry);
        let pos = self
            .by_ticker
            .partition_point(|&e| self.ticker_cmp(e, i) == Ordering::Less);
        self.by_ticker.insert(pos, i);
        self.insert_words(i);
    }

    /// Number of indexed instruments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What the input means against this index: see [`interpret`].
    #[must_use]
    pub fn interpret(&self, input: &str, ctx: &ParseContext) -> ParsedCommand {
        interpret(input, ctx, self)
    }

    /// The shortest command-line text that names `key`: the ticker (`aapl`)
    /// when it resolves back to `key`, then ticker and exchange (`bmw gr`),
    /// else the full key (`XYZ <CORP>`).
    #[must_use]
    pub fn plain_text(&self, key: &SecurityKey) -> String {
        let symbol = key.symbol.to_ascii_lowercase();
        if resolve_security(&symbol, self).as_ref() == Some(key) {
            return symbol;
        }
        if let Some(exchange) = &key.exchange
            && exchange.len() == 2
            && self
                .exact(&key.symbol.to_ascii_uppercase(), Some(exchange.as_str()))
                .as_ref()
                == Some(key)
        {
            return format!("{symbol} {}", exchange.to_ascii_lowercase());
        }
        format_security(key)
    }

    /// Suggestions for the current command-line text, at most `limit` of
    /// them, in display order (see the module docs).
    #[must_use]
    pub fn suggest(&self, input: &str, ctx: &ParseContext, limit: usize) -> Vec<Suggestion> {
        if limit == 0 {
            return Vec::new();
        }
        let upper: Vec<String> = raw_tokens(input).map(str::to_uppercase).collect();
        if upper.is_empty() {
            return Vec::new();
        }
        let lower: Vec<String> = raw_tokens(input).map(str::to_lowercase).collect();
        let open_end = input.ends_with(char::is_whitespace);
        let mut rows = Rows::default();

        if let Some((at, sector)) = find_sector(&upper)
            && let Some(security) = security_key(&upper[..at], sector)
        {
            let tail = plain::words(&lower[at + 1..].join(" "));
            if !tail.first().is_some_and(|w| is_vs(w)) {
                let prefix = format_security(&security);
                self.on_rows(&security, &prefix, &tail, open_end, true, &mut rows);
            }
            // A sector word typed as plain text may be part of a name
            // ("nvidia corp", "s&p index"). Unless the key is known or the
            // sector key itself was used, also search names.
            if !(token_is_bracketed(input, at) || self.by_key.contains_key(&security)) {
                self.general_rows(&upper, &lower, ctx, limit, &mut rows);
            }
        } else {
            self.plain_rows(input, &upper, &lower, open_end, ctx, limit, &mut rows);
        }
        rows.finish(&interpret(input, ctx, self), input, limit)
    }

    /// Rows for input that holds no full security key.
    fn plain_rows(
        &self,
        input: &str,
        upper: &[String],
        lower: &[String],
        open_end: bool,
        ctx: &ParseContext,
        limit: usize,
        rows: &mut Rows,
    ) {
        let words = plain::words(input);
        if words.is_empty() {
            return;
        }
        if self.compare_rows(&words, open_end, limit, rows) {
            return;
        }
        Self::ask_rows(input, &words, rows);
        // A typed security: `aapl `, `aapl fil`, `bmw gr news`.
        let lead = if words.len() >= 2 || open_end {
            leading_security(&words, self)
        } else {
            None
        };
        if let Some((key, used)) = &lead {
            let tail = &words[*used..];
            let prefix = self.plain_text(key);
            if tail.is_empty() {
                if open_end {
                    rows.push(self.security_row(key, EXACT_TICKER));
                    self.on_rows(key, &prefix, tail, open_end, false, rows);
                    return;
                }
            } else {
                let before = rows.len();
                self.on_rows(key, &prefix, tail, open_end, false, rows);
                if rows.len() > before {
                    rows.push(self.security_row(key, CONTEXT_SECURITY));
                }
            }
        }
        Self::command_rows(&words, open_end, rows);
        if lead.is_none()
            && let Some(loaded) = &ctx.loaded
            && typed_len(&words) >= MIN_PLAIN_CHARS
        {
            let mut list = Vec::new();
            Self::topic_rows(loaded, None, &words, open_end, LOADED_PENALTY, &mut list);
            rows.extend(sorted(list));
        }
        self.general_rows(upper, lower, ctx, limit, rows);
    }

    /// Things to open on `key`, matched against the words typed after it.
    /// `prefix` is the security's text in completions; `key_form` means it
    /// was typed as a full key, so listings complete with mnemonics.
    fn on_rows(
        &self,
        key: &SecurityKey,
        prefix: &str,
        tail: &[String],
        open_end: bool,
        key_form: bool,
        rows: &mut Rows,
    ) {
        // `AAPL US <EQUITY> GP 1Y`: the function is chosen; the user is typing arguments.
        if tail.len() >= 2 && lookup(&tail[0]).is_some() {
            return;
        }
        let group = SuggestionGroup::On(short_name(key));
        let mut list = Vec::new();
        if tail.is_empty() {
            let mut score = LIST_SCORE;
            for t in TOPICS.iter().filter(|t| t.listed && t.accepts(key.sector)) {
                let word = if key_form { t.function } else { t.phrases[0] };
                list.push(topic_row(t, key, score, format!("{prefix} {word} ")));
                score -= 1.0;
            }
            for f in &self.functions {
                let spec = f.spec;
                let listed = TOPICS
                    .iter()
                    .any(|t| t.listed && t.function == spec.mnemonic && t.args.is_empty());
                if spec.takes_security() && spec.accepts_sector(key.sector) && !listed {
                    list.push(function_row(
                        spec,
                        score,
                        group.clone(),
                        format!("{prefix} {} ", spec.mnemonic),
                        Some(Action::new(spec.mnemonic, Some(key.clone()))),
                    ));
                    score -= 1.0;
                }
            }
        } else {
            Self::topic_rows(key, Some(prefix), tail, open_end, 0.0, &mut list);
            let upper: Vec<String> = tail.iter().map(|w| w.to_uppercase()).collect();
            let qwords = query_words(tail);
            for f in &self.functions {
                if !(f.spec.takes_security() && f.spec.accepts_sector(key.sector)) {
                    continue;
                }
                if let Some(score) = f.score(&upper, &qwords) {
                    list.push(function_row(
                        f.spec,
                        score,
                        group.clone(),
                        format!("{prefix} {} ", f.spec.mnemonic),
                        Some(Action::new(f.spec.mnemonic, Some(key.clone()))),
                    ));
                }
            }
        }
        rows.extend(sorted(list));
    }

    /// Topic and range rows for `key` matched against `tail`. Without a
    /// `prefix` the rows are for the panel's loaded security.
    fn topic_rows(
        key: &SecurityKey,
        prefix: Option<&str>,
        tail: &[String],
        open_end: bool,
        penalty: f32,
        out: &mut Vec<Suggestion>,
    ) {
        let completion = |phrase: &str| match prefix {
            Some(p) => format!("{p} {phrase} "),
            None => format!("{phrase} "),
        };
        // Ranges alone (`5`, `5y`) or after a ranged topic (`chart 5`).
        let heads = std::iter::once((None, "")).chain(
            TOPICS
                .iter()
                .filter(|t| t.ranged())
                .flat_map(|t| t.phrases.iter().map(move |p| (Some(t), *p))),
        );
        for (topic, head) in heads {
            let n = if head.is_empty() {
                0
            } else {
                head.split(' ').count()
            };
            if tail.len() <= n || (n > 0 && !is_phrase(&tail[..n], head)) {
                continue;
            }
            let function = topic.map_or("GP", |t| t.function);
            if lookup(function).is_none_or(|s| !s.accepts_sector(key.sector)) {
                continue;
            }
            for r in RANGES {
                let Some((m, phrase)) = best_phrase(&tail[n..], r.phrases, open_end) else {
                    continue;
                };
                let base =
                    topic.map_or_else(|| Action::new("GP", Some(key.clone())), |t| t.action(key));
                let typed = if head.is_empty() {
                    phrase.to_owned()
                } else {
                    format!("{head} {phrase}")
                };
                out.push(plain_row(
                    range_title(function, r),
                    lookup(function).map_or("", |s| s.summary),
                    function,
                    m.score(penalty),
                    SuggestionGroup::On(short_name(key)),
                    completion(&typed),
                    base.arg("range", r.code),
                ));
            }
        }
        // Topics by phrase.
        for t in TOPICS.iter().filter(|t| t.accepts(key.sector)) {
            if let Some((m, phrase)) = best_phrase(tail, t.phrases, open_end) {
                out.push(topic_row(t, key, m.score(penalty), completion(phrase)));
            }
        }
        // Loosely: `fil` → Financials.
        if prefix.is_some()
            && !open_end
            && let [word] = tail
            && word.len() >= MIN_SUBSEQUENCE_LEN
        {
            for t in TOPICS.iter().filter(|t| t.listed && t.accepts(key.sector)) {
                let title = t.title().to_lowercase();
                if let Some(gaps) = subsequence_gaps(title.as_bytes(), word.as_bytes()) {
                    let score =
                        TOPIC_FUZZY + FUNCTION_BOOST - SUBSEQUENCE_GAP_PENALTY * gaps as f32;
                    out.push(topic_row(t, key, score.max(0.0), completion(t.phrases[0])));
                }
            }
        }
    }

    /// Commands whose phrase starts with the input.
    fn command_rows(words: &[String], open_end: bool, rows: &mut Rows) {
        if typed_len(words) < MIN_PLAIN_CHARS {
            return;
        }
        let mut list = Vec::new();
        for (i, c) in COMMANDS.iter().enumerate() {
            let Some((m, phrase)) = best_phrase(words, c.phrases, open_end) else {
                continue;
            };
            let score = match m {
                PhraseMatch::Exact => m.score(0.0),
                PhraseMatch::Prefix(_) => COMMAND_PREFIX - COMMAND_ORDER_STEP * i as f32,
            };
            list.push(plain_row(
                c.title().to_owned(),
                c.subtitle(),
                c.function,
                score,
                SuggestionGroup::Commands,
                format!("{phrase} "),
                c.action(),
            ));
        }
        rows.extend(list);
    }

    /// `ask <question>`, or a question ending in `?`.
    fn ask_rows(input: &str, words: &[String], rows: &mut Rows) {
        let action = if words.len() > 1 && words[0] == "ask" {
            plain::ask(input)
        } else if words.len() >= 2 && input.trim_end().ends_with('?') {
            Some(Action::new("ASK", None).arg("q", input.trim()))
        } else {
            None
        };
        if let Some(a) = action {
            let question = a.arg_value("q").unwrap_or_default().to_owned();
            rows.push(plain_row(
                "Ask".into(),
                &question,
                "ASK",
                EXACT_PHRASE + FUNCTION_BOOST,
                SuggestionGroup::Commands,
                format!("{} ", input.trim()),
                a,
            ));
        }
    }

    /// Rows while typing `aapl vs …` or `compare aapl with …`: securities to
    /// add. Returns whether the input is a comparison at all.
    fn compare_rows(
        &self,
        words: &[String],
        open_end: bool,
        limit: usize,
        rows: &mut Rows,
    ) -> bool {
        let (body, compare_form) = if words[0] == "compare" {
            (&words[1..], true)
        } else if words.iter().skip(1).any(|w| is_vs(w)) {
            (words, false)
        } else {
            return false;
        };
        let separator = |w: &str| is_vs(w) || (compare_form && is_joiner(w));
        let (done, partial) = match body.split_last() {
            Some((last, init)) if !open_end => (init, Some(last)),
            _ => (body, None),
        };
        let done: Vec<String> = done.iter().filter(|w| !separator(w)).cloned().collect();
        let mut keys: Vec<SecurityKey> = Vec::new();
        let mut i = 0;
        while i < done.len() {
            let Some((key, used)) = leading_security(&done[i..], self) else {
                return false;
            };
            if !keys.contains(&key) {
                keys.push(key);
            }
            i += used;
        }
        let Some(first) = keys.first() else {
            return false;
        };
        let Some(partial) = partial.filter(|p| !separator(p)) else {
            return true;
        };
        if keys.len() >= MAX_COMPARE {
            return true;
        }
        let group = SuggestionGroup::Compare(short_name(first));
        let typed = keys
            .iter()
            .map(|k| self.plain_text(k))
            .collect::<Vec<_>>()
            .join(" vs ");
        for (e, score) in self.security_matches(partial, limit) {
            let entry = &self.entries[e as usize];
            let key = &entry.inst.key;
            if keys.contains(key) {
                continue;
            }
            let mut all = keys.clone();
            all.push(key.clone());
            rows.push(Suggestion {
                completion: format!("{typed} vs {} ", self.plain_text(key)),
                group: group.clone(),
                action: Some(compare_of(&all, None)),
                ..self.entry_row(entry, score)
            });
        }
        true
    }

    /// Securities and functions matching the whole input (the tiers in the
    /// module docs).
    fn general_rows(
        &self,
        upper: &[String],
        lower: &[String],
        ctx: &ParseContext,
        limit: usize,
        rows: &mut Rows,
    ) {
        let qwords = query_words(lower);
        let loaded_sector = ctx.loaded.as_ref().map(|k| k.sector);
        let mut cands: Vec<Candidate> = Vec::new();

        for (i, f) in self.functions.iter().enumerate() {
            let Some(mut score) = f.score(upper, &qwords) else {
                continue;
            };
            if let Some(sector) = loaded_sector
                && f.spec.takes_security()
                && !f.spec.accepts_sector(sector)
            {
                score -= INCOMPATIBLE_PENALTY;
            }
            cands.push(Candidate {
                score,
                kind: SuggestionKind::Function,
                idx: i as u32,
            });
        }
        self.security_candidates(upper, lower, limit, &mut cands);
        for c in self.top(cands, limit) {
            rows.push(self.candidate_row(&c, ctx));
        }
    }

    /// Securities matching one typed word, best first.
    fn security_matches(&self, word: &str, limit: usize) -> Vec<(u32, f32)> {
        let upper = [word.to_uppercase()];
        let lower = [word.to_lowercase()];
        let mut cands = Vec::new();
        self.security_candidates(&upper, &lower, limit, &mut cands);
        self.top(cands, limit)
            .into_iter()
            .map(|c| (c.idx, c.score))
            .collect()
    }

    /// Adds security candidates for the ticker, name-word and scan tiers.
    fn security_candidates(
        &self,
        upper: &[String],
        lower: &[String],
        limit: usize,
        cands: &mut Vec<Candidate>,
    ) {
        let qwords = query_words(lower);
        let mut slots = vec![NO_SLOT; self.entries.len()];

        // Tickers: `AAP`, or `AAPL US`, `AAPL US EQU`, `EURUSD CUR`.
        if let Some((ticker, tail)) = upper.split_first()
            && tail.len() <= 2
        {
            for &e in self.ticker_range(ticker) {
                let entry = &self.entries[e as usize];
                let base = if entry.ticker == *ticker {
                    if !tail_matches(entry, tail) {
                        continue;
                    }
                    EXACT_TICKER
                } else if tail.is_empty() {
                    prefix_score(entry.ticker.len() - ticker.len())
                } else {
                    continue;
                };
                upsert(cands, &mut slots, e, base + entry.boost);
            }
        }

        // Name words: every query word must prefix some word of the name.
        if let Some(anchor) = qwords.iter().max_by_key(|w| w.len()) {
            for r in self.word_range(anchor) {
                let entry = &self.entries[r.entry as usize];
                if all_prefix(&qwords, &entry.words) {
                    let score = WORD_PREFIX + first_word_bonus(&qwords, &entry.words) + entry.boost;
                    upsert(cands, &mut slots, r.entry, score);
                }
            }
        }

        let strong = cands.iter().filter(|c| c.score > SCAN_TIER_MAX).count();
        if strong < limit {
            self.scan(upper, lower, cands, &mut slots);
        }
    }

    /// Linear scan for the name-substring and ticker-subsequence tiers,
    /// skipping entries that already matched a higher tier.
    fn scan(
        &self,
        upper: &[String],
        lower: &[String],
        cands: &mut Vec<Candidate>,
        slots: &mut [u32],
    ) {
        let phrase = lower.join(" ");
        let substring = (phrase.chars().count() >= MIN_SUBSTRING_LEN).then_some(phrase.as_bytes());
        let subsequence = match upper {
            [q] if q.chars().count() >= MIN_SUBSEQUENCE_LEN => Some(q.as_bytes()),
            _ => None,
        };
        if substring.is_none() && subsequence.is_none() {
            return;
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if slots[i] != NO_SLOT {
                continue;
            }
            let base = if substring.is_some_and(|p| contains(entry.name_lower.as_bytes(), p)) {
                Some(NAME_SUBSTRING)
            } else {
                subsequence.and_then(|q| subsequence_score(entry.ticker.as_bytes(), q))
            };
            if let Some(base) = base {
                upsert(cands, slots, i as u32, base + entry.boost);
            }
        }
    }

    /// The best `limit` candidates, best first.
    fn top(&self, mut cands: Vec<Candidate>, limit: usize) -> Vec<Candidate> {
        let cmp = |a: &Candidate, b: &Candidate| self.candidate_cmp(a, b);
        if cands.len() > limit {
            cands.select_nth_unstable_by(limit - 1, cmp);
            cands.truncate(limit);
        }
        cands.sort_unstable_by(cmp);
        cands
    }

    fn candidate_row(&self, c: &Candidate, ctx: &ParseContext) -> Suggestion {
        match c.kind {
            SuggestionKind::Security => self.entry_row(&self.entries[c.idx as usize], c.score),
            SuggestionKind::Function => {
                let spec = self.functions[c.idx as usize].spec;
                function_row(
                    spec,
                    c.score,
                    SuggestionGroup::Functions,
                    format!("{} ", spec.mnemonic),
                    function_action(spec, ctx.loaded.as_ref()),
                )
            }
        }
    }

    fn entry_row(&self, entry: &Entry, score: f32) -> Suggestion {
        let key = &entry.inst.key;
        Suggestion {
            kind: SuggestionKind::Security,
            display: entry.display.clone(),
            detail: entry.inst.name.clone(),
            completion: format!("{} ", self.plain_text(key)),
            score,
            group: SuggestionGroup::Security,
            title: short_name(key),
            subtitle: entry.inst.name.clone(),
            hint: String::new(),
            action: Some(Action::new("DES", Some(key.clone()))),
            best: false,
        }
    }

    /// The row for `key`, which may be outside the index (a currency pair).
    fn security_row(&self, key: &SecurityKey, score: f32) -> Suggestion {
        match self.by_key.get(key) {
            Some(&i) => self.entry_row(&self.entries[i as usize], score),
            None => Suggestion {
                kind: SuggestionKind::Security,
                display: key.to_string(),
                detail: key.to_string(),
                completion: format!("{} ", self.plain_text(key)),
                score,
                group: SuggestionGroup::Security,
                title: short_name(key),
                subtitle: key.to_string(),
                hint: String::new(),
                action: Some(Action::new("DES", Some(key.clone()))),
                best: false,
            },
        }
    }

    /// Best first; ties go to the shorter ticker or mnemonic, then
    /// alphabetical display text.
    fn candidate_cmp(&self, a: &Candidate, b: &Candidate) -> Ordering {
        b.score
            .total_cmp(&a.score)
            .then_with(|| self.primary(a).len().cmp(&self.primary(b).len()))
            .then_with(|| self.display(a).cmp(self.display(b)))
            .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
            .then_with(|| a.idx.cmp(&b.idx))
    }

    /// Ticker or mnemonic.
    fn primary(&self, c: &Candidate) -> &str {
        match c.kind {
            SuggestionKind::Security => &self.entries[c.idx as usize].ticker,
            SuggestionKind::Function => self.functions[c.idx as usize].spec.mnemonic,
        }
    }

    fn display(&self, c: &Candidate) -> &str {
        match c.kind {
            SuggestionKind::Security => &self.entries[c.idx as usize].display,
            SuggestionKind::Function => self.functions[c.idx as usize].spec.mnemonic,
        }
    }

    fn ticker_cmp(&self, a: u32, b: u32) -> Ordering {
        let (ea, eb) = (&self.entries[a as usize], &self.entries[b as usize]);
        ea.ticker
            .cmp(&eb.ticker)
            .then_with(|| ea.display.cmp(&eb.display))
    }

    fn word(&self, r: WordRef) -> &str {
        &self.entries[r.entry as usize].words[r.word as usize]
    }

    fn word_cmp(&self, a: WordRef, b: WordRef) -> Ordering {
        self.word(a)
            .cmp(self.word(b))
            .then_with(|| a.entry.cmp(&b.entry))
            .then_with(|| a.word.cmp(&b.word))
    }

    /// Entries whose ticker starts with `prefix`, in ticker order.
    fn ticker_range(&self, prefix: &str) -> &[u32] {
        let ticker = |e: u32| self.entries[e as usize].ticker.as_str();
        let start = self.by_ticker.partition_point(|&e| ticker(e) < prefix);
        let len = self.by_ticker[start..].partition_point(|&e| ticker(e).starts_with(prefix));
        &self.by_ticker[start..start + len]
    }

    /// Name words that start with `prefix`.
    fn word_range(&self, prefix: &str) -> &[WordRef] {
        let start = self.by_word.partition_point(|&r| self.word(r) < prefix);
        let len = self.by_word[start..].partition_point(|&r| self.word(r).starts_with(prefix));
        &self.by_word[start..start + len]
    }

    fn insert_words(&mut self, entry: u32) {
        for w in 0..self.entries[entry as usize].words.len() {
            let r = WordRef {
                entry,
                word: w as u16,
            };
            let pos = self
                .by_word
                .partition_point(|&x| self.word_cmp(x, r) == Ordering::Less);
            self.by_word.insert(pos, r);
        }
    }
}

/// Rows collected for one call, merged by action.
#[derive(Debug, Default)]
struct Rows(Vec<Suggestion>);

impl Rows {
    fn len(&self) -> usize {
        self.0.len()
    }

    /// Adds `s`, or keeps the better of it and a row that runs the same
    /// action (or, without actions, shows the same thing in the same group).
    fn push(&mut self, s: Suggestion) {
        let same = |r: &Suggestion| match (&r.action, &s.action) {
            (Some(a), Some(b)) => a == b,
            (None, None) => r.kind == s.kind && r.display == s.display && r.group == s.group,
            _ => false,
        };
        if let Some(r) = self.0.iter_mut().find(|r| same(r)) {
            if s.score > r.score {
                *r = s;
            }
        } else {
            self.0.push(s);
        }
    }

    fn extend(&mut self, rows: impl IntoIterator<Item = Suggestion>) {
        for s in rows {
            self.push(s);
        }
    }

    /// Puts what GO would run first, keeps the best `limit`, marks the best
    /// and orders the rest for display.
    fn finish(mut self, interpreted: &ParsedCommand, input: &str, limit: usize) -> Vec<Suggestion> {
        let promoted = self.promote(interpreted, input);
        // When the input is a definite command (a function with arguments,
        // a security and a function) and no row stands for it, no row is
        // best: Return then does what GO does with the text, instead of
        // running a look-alike row ("GP 5Y" is the chart over five years on
        // the loaded security, not the ticker GP).
        let definite = matches!(
            interpreted,
            ParsedCommand::Function { .. } | ParsedCommand::Security { function: Some(_), .. } | ParsedCommand::Run(_)
        );
        let mut rows = self.0;
        // Stable: equal scores keep the order they were found in.
        rows.sort_by(|a, b| b.score.total_cmp(&a.score));
        rows.truncate(limit);
        if (promoted || !definite)
            && let Some(first) = rows.first_mut()
        {
            first.best = true;
        }
        rows.sort_by_key(|s| s.group.rank());
        rows
    }

    /// Lifts the row for the interpreted command above every other row,
    /// adding one when a resolved action has no row yet. Returns whether
    /// a row now stands for what GO would run.
    fn promote(&mut self, interpreted: &ParsedCommand, input: &str) -> bool {
        let rows = &mut self.0;
        let target = match interpreted {
            ParsedCommand::Run(a) => Some(
                rows.iter()
                    .position(|s| s.action.as_ref() == Some(a))
                    .unwrap_or_else(|| {
                        rows.push(describe(a, input));
                        rows.len() - 1
                    }),
            ),
            ParsedCommand::Function { function, args } => rows.iter().position(|s| {
                (s.kind == SuggestionKind::Function
                    && s.display == *function
                    && matches!(
                        s.group,
                        SuggestionGroup::Commands | SuggestionGroup::Functions
                    ))
                    // A command row that runs the bare function ("ask").
                    || (args.is_empty()
                        && s.action.as_ref().is_some_and(|a| {
                            a.function == *function && a.args.is_empty() && a.security.is_none()
                        }))
            }),
            ParsedCommand::Security {
                security,
                function: Some(f),
                ..
            } => rows.iter().position(|s| {
                s.action.as_ref().is_some_and(|a| {
                    a.function == *f && a.args.is_empty() && a.security.as_ref() == Some(security)
                })
            }),
            ParsedCommand::Security { function: None, .. }
            | ParsedCommand::Empty
            | ParsedCommand::MenuItem(_)
            | ParsedCommand::Search(_) => None,
        };
        if let Some(i) = target {
            let top = rows
                .iter()
                .map(|s| s.score)
                .fold(f32::NEG_INFINITY, f32::max);
            rows[i].score = top + 1.0;
        }
        target.is_some()
    }
}

/// A row for a resolved action no other row covers, e.g. `aapl vs msft 5y`.
fn describe(action: &Action, input: &str) -> Suggestion {
    let spec = lookup(&action.function);
    let summary = spec.map_or("", |s| s.summary);
    let (group, title, subtitle) = if action.function == "COMPARE" {
        let keys = compare_keys_of(action);
        let first = keys
            .first()
            .or(action.security.as_ref())
            .map(short_name)
            .unwrap_or_default();
        let others = keys
            .iter()
            .skip(1)
            .map(short_name)
            .collect::<Vec<_>>()
            .join(", ");
        let range = action
            .arg_value("range")
            .and_then(|c| RANGES.iter().find(|r| r.code == c));
        let subtitle = match range {
            Some(r) => format!("{summary}, {}", r.label),
            None => summary.to_owned(),
        };
        (SuggestionGroup::Compare(first), others, subtitle)
    } else {
        let group = action
            .security
            .as_ref()
            .map_or(SuggestionGroup::Commands, |k| {
                SuggestionGroup::On(short_name(k))
            });
        (group, action_title(action), summary.to_owned())
    };
    Suggestion {
        kind: SuggestionKind::Function,
        display: action.function.clone(),
        detail: title.clone(),
        completion: format!("{} ", input.trim()),
        score: 0.0,
        group,
        title,
        subtitle,
        hint: hint_for(&action.function).to_owned(),
        action: Some(action.clone()),
        best: false,
    }
}

/// Plain title for an action: its range chart, topic or command title, or
/// the function's name.
fn action_title(action: &Action) -> String {
    let f = action.function.as_str();
    if f == "ASK" {
        return "Ask".into();
    }
    if let Some(r) = action
        .arg_value("range")
        .and_then(|c| RANGES.iter().find(|r| r.code == c))
    {
        return range_title(f, r);
    }
    let has = |args: &[(&str, &str)]| args.iter().all(|(k, v)| action.arg_value(k) == Some(*v));
    if action.security.is_some()
        && let Some(t) = TOPICS
            .iter()
            .filter(|t| t.function == f && has(t.args))
            .max_by_key(|t| t.args.len())
    {
        return t.title().to_owned();
    }
    if let Some(c) = COMMANDS
        .iter()
        .filter(|c| c.function == f && has(c.args))
        .max_by_key(|c| c.args.len())
    {
        return c.title().to_owned();
    }
    lookup(f).map_or_else(|| f.to_owned(), |s| s.name.to_owned())
}

/// The action a function row runs: on the loaded security when it takes
/// one, `None` when it needs a security and none fits.
fn function_action(spec: &FunctionSpec, loaded: Option<&SecurityKey>) -> Option<Action> {
    match loaded {
        _ if !spec.takes_security() => Some(Action::new(spec.mnemonic, None)),
        Some(k) if spec.accepts_sector(k.sector) => {
            Some(Action::new(spec.mnemonic, Some(k.clone())))
        }
        _ if spec.needs_security == crate::registry::SecurityNeed::Optional => {
            Some(Action::new(spec.mnemonic, None))
        }
        _ => None,
    }
}

fn function_row(
    spec: &FunctionSpec,
    score: f32,
    group: SuggestionGroup,
    completion: String,
    action: Option<Action>,
) -> Suggestion {
    Suggestion {
        kind: SuggestionKind::Function,
        display: spec.mnemonic.to_owned(),
        detail: spec.title.to_owned(),
        completion,
        score,
        group,
        title: spec.name.to_owned(),
        subtitle: spec.summary.to_owned(),
        hint: hint_for(spec.mnemonic).to_owned(),
        action,
        best: false,
    }
}

fn topic_row(t: &Topic, key: &SecurityKey, score: f32, completion: String) -> Suggestion {
    plain_row(
        t.title().to_owned(),
        t.subtitle(),
        t.function,
        score,
        SuggestionGroup::On(short_name(key)),
        completion,
        t.action(key),
    )
}

fn plain_row(
    title: String,
    subtitle: &str,
    function: &str,
    score: f32,
    group: SuggestionGroup,
    completion: String,
    action: Action,
) -> Suggestion {
    Suggestion {
        kind: SuggestionKind::Function,
        display: function.to_owned(),
        detail: title.clone(),
        completion,
        score,
        group,
        title,
        subtitle: subtitle.to_owned(),
        hint: hint_for(function).to_owned(),
        action: Some(action),
        best: false,
    }
}

/// `AAPL`, or `BMW GR` for an equity listed outside the US.
fn short_name(key: &SecurityKey) -> String {
    match &key.exchange {
        Some(x) if key.sector == MarketSector::Equity && x != "US" => format!("{} {x}", key.symbol),
        _ => key.symbol.clone(),
    }
}

/// Best first, ties on the shorter then alphabetical id.
fn sorted(mut list: Vec<Suggestion>) -> Vec<Suggestion> {
    list.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.display.len().cmp(&b.display.len()))
            .then_with(|| a.display.cmp(&b.display))
    });
    list
}

fn typed_len(words: &[String]) -> usize {
    words.iter().map(String::len).sum()
}

/// The best way `query` matches one of `phrases`, and that phrase.
fn best_phrase<'a>(
    query: &[String],
    phrases: &'a [&'a str],
    open_end: bool,
) -> Option<(PhraseMatch, &'a str)> {
    phrases
        .iter()
        .filter_map(|p| phrase_match(query, p, open_end).map(|m| (m, *p)))
        .min_by_key(|(m, _)| match m {
            PhraseMatch::Exact => 0,
            PhraseMatch::Prefix(missing) => missing + 1,
        })
}

/// Whether the typed words are `phrase` or the start of it: every word but
/// the last equal, the last a prefix of its counterpart (equal when the
/// input ends with a space).
fn phrase_match(query: &[String], phrase: &str, open_end: bool) -> Option<PhraseMatch> {
    let words: Vec<&str> = phrase.split(' ').collect();
    let n = query.len();
    if n == 0 || n > words.len() {
        return None;
    }
    if !query[..n - 1].iter().zip(&words).all(|(q, w)| q == w) {
        return None;
    }
    let last = query[n - 1].as_str();
    let target = words[n - 1];
    let after: usize = words[n..].iter().map(|w| w.len() + 1).sum();
    if last == target {
        Some(if n == words.len() {
            PhraseMatch::Exact
        } else {
            PhraseMatch::Prefix(after)
        })
    } else if !open_end && target.starts_with(last) {
        Some(PhraseMatch::Prefix(target.len() - last.len() + after))
    } else {
        None
    }
}

impl Entry {
    fn new(inst: IndexedInstrument) -> Self {
        // Canonical key, so lookups by key and display are case-stable.
        let key = SecurityKey::new(
            inst.key.symbol.as_str(),
            inst.key.exchange.as_deref(),
            inst.key.sector,
        );
        let popularity = if inst.popularity.is_nan() {
            0.0
        } else {
            inst.popularity.clamp(0.0, 1.0)
        };
        let name_lower = inst.name.to_lowercase();
        let words = words_of(&name_lower)
            .take(MAX_NAME_WORDS)
            .map(str::to_owned)
            .collect();
        Self {
            ticker: key.symbol.to_uppercase(),
            exchange: key.exchange.clone().unwrap_or_default(),
            display: key.to_string(),
            boost: popularity * POPULARITY_WEIGHT,
            name_lower,
            words,
            inst: IndexedInstrument {
                key,
                name: inst.name,
                popularity,
            },
        }
    }
}

impl FunctionEntry {
    fn new(spec: &'static FunctionSpec) -> Self {
        let lower_words = |s: &str| {
            words_of(&s.to_lowercase())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        Self {
            spec,
            title_words: lower_words(spec.title),
            keyword_words: spec.keywords.iter().flat_map(|k| lower_words(k)).collect(),
        }
    }

    /// Best score of this function for the query, if it matches at all.
    fn score(&self, upper: &[String], qwords: &[&str]) -> Option<f32> {
        let mut best = None;
        if let [only] = upper {
            let m = self.spec.mnemonic;
            if m == only {
                best = Some(max_score(best, EXACT_MNEMONIC));
            } else if m.starts_with(only.as_str()) {
                best = Some(max_score(best, prefix_score(m.len() - only.len())));
            }
        }
        if !qwords.is_empty() {
            if all_prefix(qwords, &self.title_words) {
                let title = WORD_PREFIX + first_word_bonus(qwords, &self.title_words);
                best = Some(max_score(best, title));
            } else if qwords.iter().all(|q| {
                self.title_words
                    .iter()
                    .chain(&self.keyword_words)
                    .any(|w| w.starts_with(q))
            }) {
                best = Some(max_score(best, KEYWORD_PREFIX));
            }
        }
        best.map(|s| s + FUNCTION_BOOST)
    }
}

fn max_score(best: Option<f32>, score: f32) -> f32 {
    best.map_or(score, |b| b.max(score))
}

fn prefix_score(missing_chars: usize) -> f32 {
    PREFIX - PREFIX_LEN_PENALTY * missing_chars.min(MAX_PREFIX_PENALTY_CHARS) as f32
}

/// Lower-case alphanumeric runs: `at&t inc.` → `at`, `t`, `inc`.
fn words_of(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

fn query_words(lower: &[String]) -> Vec<&str> {
    lower.iter().flat_map(|t| words_of(t)).collect()
}

fn all_prefix(qwords: &[&str], words: &[String]) -> bool {
    qwords
        .iter()
        .all(|q| words.iter().any(|w| w.starts_with(q)))
}

fn first_word_bonus(qwords: &[&str], words: &[String]) -> f32 {
    match (qwords.first(), words.first()) {
        (Some(q), Some(w)) if w == q => FIRST_WORD_BONUS + EXACT_FIRST_WORD_BONUS,
        (Some(q), Some(w)) if w.starts_with(q) => FIRST_WORD_BONUS,
        _ => 0.0,
    }
}

/// For an exact ticker, checks the typed exchange and/or partial sector:
/// `[]`, `[US]`, `[EQU]`, `[US, EQU]`.
fn tail_matches(entry: &Entry, tail: &[String]) -> bool {
    let sector = entry.inst.key.sector;
    match tail {
        [] => true,
        [x] => entry.exchange.starts_with(x.as_str()) || sector_prefix(sector, x),
        [exchange, x] => entry.exchange == *exchange && sector_prefix(sector, x),
        _ => false,
    }
}

fn sector_prefix(sector: MarketSector, typed: &str) -> bool {
    sector.key_cap().starts_with(typed)
        || sector.key_label().to_ascii_uppercase().starts_with(typed)
}

fn upsert(cands: &mut Vec<Candidate>, slots: &mut [u32], entry: u32, score: f32) {
    let slot = &mut slots[entry as usize];
    if *slot == NO_SLOT {
        *slot = cands.len() as u32;
        cands.push(Candidate {
            score,
            kind: SuggestionKind::Security,
            idx: entry,
        });
    } else {
        let c = &mut cands[*slot as usize];
        c.score = c.score.max(score);
    }
}

fn kind_rank(kind: SuggestionKind) -> u8 {
    match kind {
        SuggestionKind::Function => 0,
        SuggestionKind::Security => 1,
    }
}

/// Byte substring search; fast for the short haystacks of security names.
/// A byte match of valid UTF-8 in valid UTF-8 is always a character match.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    if needle.len() > hay.len() {
        return false;
    }
    let last_start = hay.len() - needle.len();
    hay[..=last_start]
        .iter()
        .enumerate()
        .any(|(i, &b)| b == first && hay[i + 1..i + needle.len()] == *rest)
}

/// Gaps skipped when `needle` is a subsequence of `hay` starting at the
/// same first byte.
fn subsequence_gaps(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() || hay.first() != needle.first() {
        return None;
    }
    let mut at = 0;
    let mut gaps = 0;
    for &c in needle {
        let skip = hay[at..].iter().position(|&b| b == c)?;
        gaps += skip;
        at += skip + 1;
    }
    Some(gaps)
}

/// Scores `needle` as a subsequence of `hay` that starts at the same first
/// letter; compact matches and shorter tickers score higher.
fn subsequence_score(hay: &[u8], needle: &[u8]) -> Option<f32> {
    if needle.len() >= hay.len() {
        return None;
    }
    let gaps = subsequence_gaps(hay, needle)?;
    // Every needle byte and gap consumed a hay byte; the rest trails.
    let trailing = hay.len() - needle.len() - gaps;
    let score = TICKER_SUBSEQUENCE
        - SUBSEQUENCE_GAP_PENALTY * gaps as f32
        - PREFIX_LEN_PENALTY * trailing as f32;
    Some(score.max(0.0))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn inst(key: SecurityKey, name: &str, popularity: f32) -> IndexedInstrument {
        IndexedInstrument {
            key,
            name: name.to_owned(),
            popularity,
        }
    }

    fn eq(symbol: &str, name: &str, popularity: f32) -> IndexedInstrument {
        inst(SecurityKey::equity(symbol), name, popularity)
    }

    fn fixture() -> SuggestIndex {
        SuggestIndex::new(vec![
            eq("AAPL", "Apple Inc.", 1.0),
            eq("AAPB", "GraniteShares 2x Long AAPL Daily ETF", 0.05),
            eq("AAPU", "Direxion Daily AAPL Bull 2X Shares", 0.05),
            eq("AA", "Alcoa Corp", 0.3),
            eq("AMAT", "Applied Materials Inc.", 0.6),
            eq("APLE", "Apple Hospitality REIT Inc.", 0.1),
            eq("MSFT", "Microsoft Corp", 1.0),
            eq("AMZN", "Amazon.com Inc.", 1.0),
            eq("GOOGL", "Alphabet Inc. Class A", 0.95),
            eq("EA", "Electronic Arts Inc.", 0.5),
            eq("EAT", "Brinker International Inc.", 0.2),
            eq("DE", "Deere & Co", 0.5),
            eq("IBM", "International Business Machines Corp", 0.7),
            eq("BAC", "Bank of America Corp", 0.8),
            eq("T", "AT&T Inc.", 0.7),
            eq("CN", "Xtrackers MSCI All China Equity ETF", 0.01),
            inst(
                SecurityKey::new("BMW", Some("GR"), MarketSector::Equity),
                "Bayerische Motoren Werke AG",
                0.4,
            ),
            inst(SecurityKey::index("SPX"), "S&P 500 Index", 0.9),
            inst(SecurityKey::currency("EURUSD"), "Euro / US Dollar", 0.9),
            inst(
                SecurityKey::new("CLZ6", None, MarketSector::Cmdty),
                "WTI Crude Oil Dec 2026",
                0.4,
            ),
        ])
    }

    /// The fixture plus crypto, a class share and a fund named like a coin.
    fn plain_fixture() -> SuggestIndex {
        let mut index = fixture();
        for i in [
            inst(SecurityKey::currency("BTCUSD"), "Bitcoin / USD", 0.6),
            inst(SecurityKey::currency("ETHUSD"), "Ethereum / USD", 0.6),
            eq("BTC", "Grayscale Bitcoin Mini Trust", 0.3),
            eq("BRK-B", "Berkshire Hathaway Inc. Class B", 0.9),
            eq("KO", "Coca-Cola Co", 0.8),
            eq("EARN", "Ellington Credit Co", 0.1),
        ] {
            index.add_or_update(i);
        }
        index
    }

    fn run(index: &SuggestIndex, input: &str) -> Vec<Suggestion> {
        index.suggest(input, &ParseContext::default(), 12)
    }

    fn displays(s: &[Suggestion]) -> Vec<&str> {
        s.iter().map(|s| s.display.as_str()).collect()
    }

    fn titles(s: &[Suggestion]) -> Vec<&str> {
        s.iter().map(|s| s.title.as_str()).collect()
    }

    fn securities(s: &[Suggestion]) -> Vec<&str> {
        s.iter()
            .filter(|s| s.kind == SuggestionKind::Security)
            .map(|s| s.display.as_str())
            .collect()
    }

    fn functions(s: &[Suggestion]) -> Vec<&str> {
        s.iter()
            .filter(|s| s.kind == SuggestionKind::Function)
            .map(|s| s.display.as_str())
            .collect()
    }

    fn best(s: &[Suggestion]) -> &Suggestion {
        let bests: Vec<&Suggestion> = s.iter().filter(|s| s.best).collect();
        assert_eq!(bests.len(), 1, "{:?}", titles(s));
        bests[0]
    }

    fn group(s: &[Suggestion], heading: &str) -> Vec<String> {
        s.iter()
            .filter(|s| s.group.heading() == heading)
            .map(|s| s.title.clone())
            .collect()
    }

    #[test]
    fn ticker_prefix_ranks_popular_first() {
        let s = run(&fixture(), "AAP");
        assert_eq!(s[0].display, "AAPL US Equity");
        assert_eq!(s[0].detail, "Apple Inc.");
        assert_eq!(s[0].completion, "aapl ");
        assert_eq!(s[0].kind, SuggestionKind::Security);
        assert_eq!(s[0].title, "AAPL");
        assert_eq!(s[0].subtitle, "Apple Inc.");
        assert_eq!(s[0].hint, "");
        assert_eq!(s[0].group, SuggestionGroup::Security);
        assert_eq!(
            s[0].action,
            Some(Action::new("DES", Some(SecurityKey::equity("AAPL"))))
        );
        let secs = securities(&s);
        assert!(secs.contains(&"AAPB US Equity") && secs.contains(&"AAPU US Equity"));
    }

    #[test]
    fn exact_ticker_beats_more_popular_prefix() {
        let s = run(&fixture(), "AA");
        assert_eq!(securities(&s)[..2], ["AA US Equity", "AAPL US Equity"]);
        assert_eq!(best(&s).display, "AA US Equity");
    }

    #[test]
    fn name_search() {
        let s = run(&fixture(), "apple");
        assert_eq!(securities(&s)[0], "AAPL US Equity");
        assert!(securities(&s).contains(&"APLE US Equity"));
        // Applied Materials doesn't match "apple".
        assert!(!securities(&s).contains(&"AMAT US Equity"));

        let s = run(&fixture(), "bank of am");
        assert_eq!(securities(&s), ["BAC US Equity"]);

        let s = run(&fixture(), "at&t");
        assert_eq!(securities(&s)[0], "T US Equity");
    }

    #[test]
    fn misspelled_names_still_find_the_company() {
        let s = run(&fixture(), "appl");
        assert_eq!(securities(&s)[0], "AAPL US Equity");
        assert_eq!(best(&s).title, "AAPL");
    }

    #[test]
    fn name_substring_and_ticker_subsequence() {
        let s = run(&fixture(), "mazon");
        assert_eq!(securities(&s), ["AMZN US Equity"]);

        let s = run(&fixture(), "GGL");
        assert_eq!(securities(&s), ["GOOGL US Equity"]);
    }

    #[test]
    fn functions_by_mnemonic_prefix() {
        let s = run(&fixture(), "DE");
        assert!(functions(&s).contains(&"DES"), "{:?}", displays(&s));
        // The exact ticker DE still ranks above the DES prefix match.
        assert_eq!(s[0].display, "DE US Equity");
        assert!(s[0].best);
        let des = s.iter().find(|s| s.display == "DES").unwrap();
        assert_eq!(des.detail, "Security Description");
        assert_eq!(des.title, "Overview");
        assert_eq!(des.hint, "DES");
        assert_eq!(des.group, SuggestionGroup::Functions);
        assert_eq!(des.completion, "DES ");
        // DES needs a security and none is loaded.
        assert_eq!(des.action, None);
    }

    #[test]
    fn exact_mnemonic_is_best() {
        let s = run(&fixture(), "des");
        assert_eq!(s[0].display, "DES");
        assert!(s[0].best);
        // CN alone is Company News, as GO would run it; ticker CN is listed
        // first under its own heading.
        let s = run(&fixture(), "CN");
        assert_eq!(displays(&s)[..2], ["CN US Equity", "CN"]);
        assert_eq!(best(&s).display, "CN");
        assert_eq!(best(&s).group, SuggestionGroup::Functions);
    }

    #[test]
    fn functions_by_keyword() {
        let s = run(&fixture(), "fin");
        assert_eq!(functions(&s)[0], "FA");
        let s = run(&fixture(), "cash flow");
        assert_eq!(functions(&s), ["FA"]);
        let s = run(&fixture(), "option chain");
        assert_eq!(functions(&s), ["OMON"]);
    }

    #[test]
    fn mixes_securities_and_functions() {
        let s = run(&fixture(), "EA");
        assert_eq!(s[0].display, "EA US Equity");
        let fns = functions(&s);
        assert!(fns.contains(&"EE") && fns.contains(&"ERN"), "{fns:?}");
        assert!(securities(&s).contains(&"EAT US Equity"));
    }

    #[test]
    fn functions_after_a_security() {
        let s = run(&fixture(), "AAPL US <EQUITY> G");
        assert!(s.iter().all(|s| s.kind == SuggestionKind::Function));
        assert_eq!(displays(&s)[..2], ["GP", "GIP"]);
        assert_eq!(s[0].completion, "AAPL US <EQUITY> GP ");
        assert_eq!(s[1].detail, "Intraday Price Graph");
        assert_eq!(s[0].group.heading(), "On AAPL");
        assert_eq!(s[0].title, "Chart");

        // Lower case, no exchange, no brackets: canonical completion.
        let s = run(&fixture(), "aapl equity gi");
        assert_eq!(s[0].completion, "AAPL US <EQUITY> GIP ");
    }

    #[test]
    fn functions_after_a_security_respect_sector() {
        let s = run(&fixture(), "EURUSD CURNCY ");
        let fns = functions(&s);
        assert_eq!(fns[..4], ["DES", "GP", "CN", "HP"]);
        for bad in ["FA", "OMON", "EE", "TOP", "W"] {
            assert!(!fns.contains(&bad), "{bad} in {fns:?}");
        }
        assert_eq!(s[0].completion, "EURUSD <CRNCY> DES ");

        let s = index_all(&fixture(), "SPX <INDEX> O");
        assert_eq!(functions(&s)[..3], ["OMON", "OVDV", "OVME"]);
        let s = index_all(&fixture(), "EURUSD <CRNCY> O");
        assert!(
            s.iter().all(|s| !s.display.starts_with('O')),
            "{:?}",
            displays(&s)
        );
        let s = index_all(&fixture(), "AAPL US <EQUITY> FA");
        assert_eq!(s[0].display, "FA");
        assert!(s[0].best);
    }

    #[test]
    fn sector_words_inside_names() {
        let index = fixture();
        // "corp" and "index" are sector labels, but these keys aren't known
        // and no sector key was pressed, so names are searched too.
        let s = run(&index, "microsoft corp");
        assert_eq!(s[0].display, "MSFT US Equity");
        let s = run(&index, "s&p index");
        assert_eq!(s[0].display, "SPX Index");
        // Function suggestions for the would-be key are still offered.
        assert!(functions(&run(&index, "s&p index")).contains(&"GP"));

        // A known key, or a pressed sector key, means functions only.
        let s = run(&index, "AAPL US EQUITY G");
        assert!(s.iter().all(|s| s.kind == SuggestionKind::Function));
        let s = run(&index, "microsoft <CORP> ");
        assert!(s.iter().all(|s| s.kind == SuggestionKind::Function));
        assert_eq!(s[0].completion, "MICROSOFT <CORP> DES ");
    }

    #[test]
    fn nothing_while_typing_arguments() {
        assert!(run(&fixture(), "AAPL US <EQUITY> GP 1Y").is_empty());
    }

    #[test]
    fn partial_security_keys() {
        let index = fixture();
        assert_eq!(run(&index, "AAPL US")[0].display, "AAPL US Equity");
        assert_eq!(run(&index, "AAPL U")[0].display, "AAPL US Equity");
        assert_eq!(run(&index, "AAPL US EQU")[0].display, "AAPL US Equity");
        assert_eq!(run(&index, "EURUSD CUR")[0].display, "EURUSD Curncy");
        assert_eq!(run(&index, "BMW GR")[0].display, "BMW GR Equity");
        assert!(securities(&run(&index, "AAPL LN")).is_empty());
    }

    #[test]
    fn loaded_security_demotes_incompatible_functions() {
        let index = fixture();
        let fx = ParseContext {
            loaded: Some(SecurityKey::currency("EURUSD")),
        };
        let plain = index.suggest("E", &ParseContext::default(), 50);
        let demoted = index.suggest("E", &fx, 50);
        let score = |s: &[Suggestion], m: &str| s.iter().find(|s| s.display == m).unwrap().score;
        assert!(score(&demoted, "EE") < score(&plain, "EE"));
        assert_eq!(score(&demoted, "ECO"), score(&plain, "ECO"));
    }

    #[test]
    fn deterministic_tie_breaks() {
        let index = SuggestIndex::new(vec![
            eq("ZZB", "Same", 0.5),
            eq("ZZAA", "Same", 0.5),
            eq("ZZA", "Same", 0.5),
        ]);
        let s = run(&index, "same");
        assert_eq!(
            displays(&s),
            ["ZZA US Equity", "ZZB US Equity", "ZZAA US Equity"]
        );
        assert_eq!(s, run(&index, "same"));
    }

    #[test]
    fn limits_and_empty_input() {
        let index = fixture();
        assert!(run(&index, "").is_empty());
        assert!(run(&index, "   ").is_empty());
        assert!(index.suggest("A", &ParseContext::default(), 0).is_empty());
        assert_eq!(index.suggest("A", &ParseContext::default(), 3).len(), 3);
        let all = index.suggest("A", &ParseContext::default(), 1000);
        let top = index.suggest("A", &ParseContext::default(), 5);
        // The five best of everything, in display order.
        let mut best5: Vec<&Suggestion> = all.iter().collect();
        best5.sort_by(|a, b| b.score.total_cmp(&a.score));
        for s in &top {
            assert!(
                best5[..5].iter().any(|b| b.display == s.display),
                "{}",
                s.display
            );
        }
        assert_well_ordered(&all);
        assert!(run(&index, "qqqqqq").is_empty());
    }

    #[test]
    fn add_or_update() {
        let mut index = fixture();
        let n = index.len();
        index.add_or_update(eq("NVDA", "NVIDIA Corp", 1.0));
        assert_eq!(index.len(), n + 1);
        assert_eq!(run(&index, "NVD")[0].display, "NVDA US Equity");
        assert_eq!(securities(&run(&index, "nvidia")), ["NVDA US Equity"]);

        // Same key (case-insensitive symbol): replaced, not duplicated.
        index.add_or_update(inst(
            SecurityKey {
                symbol: "nvda".to_owned(),
                exchange: Some("us".to_owned()),
                sector: MarketSector::Equity,
            },
            "Nvidia Corporation",
            1.0,
        ));
        assert_eq!(index.len(), n + 1);
        assert_eq!(run(&index, "corporation")[0].detail, "Nvidia Corporation");
        assert!(securities(&run(&index, "nvidia corp")).contains(&"NVDA US Equity"));
        assert!(
            run(&index, "NVIDIA Corp")
                .iter()
                .all(|s| s.detail != "NVIDIA Corp")
        );

        // Incremental inserts keep the ticker order binary-searchable.
        for t in ["AAPZ", "A", "AAP", "ZZZZ"] {
            index.add_or_update(eq(t, "Filler", 0.0));
        }
        assert_eq!(run(&index, "AAP")[0].display, "AAP US Equity");
        assert_eq!(run(&index, "AAPZ")[0].display, "AAPZ US Equity");
    }

    #[test]
    fn duplicate_keys_in_bulk_load_keep_the_last() {
        let index = SuggestIndex::new(vec![eq("AAPL", "Old", 1.0), eq("AAPL", "Apple Inc.", 1.0)]);
        assert_eq!(index.len(), 1);
        assert_eq!(run(&index, "AAPL")[0].detail, "Apple Inc.");
        assert!(securities(&run(&index, "old")).is_empty());
    }

    #[test]
    fn popularity_is_sanitized() {
        let index = SuggestIndex::new(vec![
            eq("XA", "X", f32::NAN),
            eq("XB", "X", f32::INFINITY),
            eq("XC", "X", -3.0),
        ]);
        let s = run(&index, "X");
        assert!(s.iter().all(|s| s.score.is_finite()));
        assert_eq!(securities(&s)[0], "XB US Equity");
    }

    #[test]
    fn empty_index_still_suggests_functions() {
        let index = SuggestIndex::default();
        assert!(index.is_empty());
        assert_eq!(run(&index, "OMO")[0].display, "OMON");
    }

    #[test]
    fn helpers() {
        assert!(contains(b"apple inc", b"pple"));
        assert!(contains(b"apple", b"apple"));
        assert!(!contains(b"app", b"apple"));
        assert!(contains(b"x", b""));
        assert!(subsequence_score(b"GOOGL", b"GGL").is_some());
        assert!(subsequence_score(b"GOOGL", b"OGL").is_none());
        assert!(subsequence_score(b"GOOGL", b"GOOGL").is_none());
        assert!(
            subsequence_score(b"AAPL", b"APL").unwrap()
                > subsequence_score(b"AXXPL", b"APL").unwrap()
        );
        assert_eq!(subsequence_gaps(b"financials", b"fil"), Some(6));
        let w = |s: &str| plain::words(s);
        assert_eq!(
            phrase_match(&w("earn"), "earnings this week", false),
            Some(PhraseMatch::Prefix(14))
        );
        assert_eq!(
            phrase_match(&w("earnings th"), "earnings this week", false),
            Some(PhraseMatch::Prefix(7))
        );
        assert_eq!(
            phrase_match(&w("earnings this week"), "earnings this week", false),
            Some(PhraseMatch::Exact)
        );
        assert_eq!(phrase_match(&w("earn"), "earnings this week", true), None);
        assert_eq!(
            phrase_match(&w("earnings"), "earnings this week", true),
            Some(PhraseMatch::Prefix(10))
        );
        assert_eq!(
            phrase_match(&w("earnings nx"), "earnings this week", false),
            None
        );
    }

    // --- plain language ---------------------------------------------------

    #[test]
    fn a_ticker_then_part_of_a_topic() {
        // The mockup: `aapl fil` offers Filings first under "On AAPL", with
        // the security above it.
        let index = plain_fixture();
        let s = run(&index, "aapl fil");
        assert_eq!(s[0].group, SuggestionGroup::Security);
        assert_eq!(s[0].title, "AAPL");
        assert_eq!(s[0].subtitle, "Apple Inc.");
        let on = group(&s, "On AAPL");
        assert_eq!(on[0], "Filings");
        assert!(on.contains(&"Financials".to_owned()), "{on:?}");
        let filings = best(&s);
        assert_eq!(filings.title, "Filings");
        assert_eq!(
            filings.subtitle,
            "10-K, 10-Q and 8-K, with summaries and what changed"
        );
        assert_eq!(filings.hint, "CF");
        assert_eq!(filings.completion, "aapl filings ");
        assert_eq!(
            filings.action,
            Some(Action::new("CF", Some(SecurityKey::equity("AAPL"))))
        );
        let financials = s.iter().find(|s| s.title == "Financials").unwrap();
        assert_eq!(
            financials.subtitle,
            "Income statement, balance sheet, cash flow"
        );
        assert_eq!(financials.hint, "FA");
    }

    #[test]
    fn a_ticker_and_a_space_lists_what_to_open() {
        let index = plain_fixture();
        let s = run(&index, "aapl ");
        assert_eq!(s[0].title, "AAPL");
        assert!(s[0].best, "Return opens the overview");
        let on = group(&s, "On AAPL");
        assert_eq!(on[..4], ["Chart", "News", "Filings", "Financials"]);
        assert!(
            !on.contains(&"Overview".to_owned()),
            "same action as the security row"
        );
        // No other securities once the ticker is committed.
        assert_eq!(securities(&s), ["AAPL US Equity"]);
        assert_eq!(s.len(), 12);
        let chart = s.iter().find(|s| s.title == "Chart").unwrap();
        assert_eq!(chart.completion, "aapl chart ");
        // Currencies get only what applies to them.
        let s = index_all(&index, "eurusd ");
        let on = group(&s, "On EURUSD");
        assert!(on.contains(&"Chart".to_owned()) && !on.contains(&"Financials".to_owned()));
    }

    #[test]
    fn ranges_after_a_ticker() {
        let index = plain_fixture();
        let s = run(&index, "aapl 5");
        let on = group(&s, "On AAPL");
        assert!(
            on.contains(&"5-day chart".to_owned()) && on.contains(&"5-year chart".to_owned()),
            "{on:?}"
        );
        let s = run(&index, "aapl 5y");
        let b = best(&s);
        assert_eq!(b.title, "5-year chart");
        assert_eq!(b.hint, "GP");
        assert_eq!(
            b.action,
            Some(Action::new("GP", Some(SecurityKey::equity("AAPL"))).arg("range", "5Y"))
        );
        let s = run(&index, "aapl chart 1");
        assert!(group(&s, "On AAPL").contains(&"1-year chart".to_owned()));
        let s = run(&index, "aapl backtest 1");
        let row = s.iter().find(|s| s.title == "Backtest, 10 years").unwrap();
        assert_eq!(row.completion, "aapl backtest 10y ");
    }

    #[test]
    fn exact_topics_are_what_return_runs() {
        let index = plain_fixture();
        let aapl = || Some(SecurityKey::equity("AAPL"));
        for (input, action) in [
            ("aapl filings", Action::new("CF", aapl())),
            ("aapl 10-k", Action::new("CF", aapl()).arg("form", "10-K")),
            (
                "aapl balance sheet",
                Action::new("FA", aapl()).arg("stmt", "BS"),
            ),
            ("aapl news", Action::new("CN", aapl())),
            ("aapl", Action::new("DES", aapl())),
            ("filings aapl", Action::new("CF", aapl())),
            (
                "aapl balance sheet quarterly",
                Action::new("FA", aapl())
                    .arg("stmt", "BS")
                    .arg("per", "Quarterly"),
            ),
        ] {
            let s = run(&index, input);
            assert_eq!(best(&s).action.as_ref(), Some(&action), "{input}");
        }
        let s = run(&index, "aapl balance s");
        assert_eq!(best(&s).title, "Balance sheet");
        assert_eq!(best(&s).completion, "aapl balance sheet ");
    }

    #[test]
    fn commands_by_prefix() {
        let index = fixture();
        let s = run(&index, "earn");
        let commands = group(&s, "Commands");
        assert_eq!(commands[0], "Earnings this week", "{commands:?}");
        assert_eq!(best(&s).title, "Earnings this week");
        let row = best(&s);
        assert_eq!(row.completion, "earnings this week ");
        assert_eq!(row.hint, "CALENDAR");
        assert_eq!(
            row.action,
            Some(
                Action::new("CALENDAR", None)
                    .arg("kind", "earnings")
                    .arg("range", "this-week")
            )
        );
        // The functions are still there.
        assert!(functions(&s).contains(&"ERN"));

        let s = run(&index, "earnings ne");
        assert_eq!(best(&s).title, "Earnings next week");
        let s = run(&index, "fed f");
        assert_eq!(best(&s).title, "Fed funds rate");
        let s = run(&index, "sett");
        assert_eq!(best(&s).title, "Settings");
        assert_eq!(best(&s).hint, "⌘,");
        let s = run(&index, "yield c");
        assert_eq!(best(&s).title, "Treasury yield curve");
        // One letter is too little for commands.
        assert!(group(&run(&index, "e"), "Commands").is_empty());
    }

    #[test]
    fn a_mnemonic_with_arguments_is_never_a_look_alike_ticker() {
        let mut index = plain_fixture();
        index.add_or_update(eq("GP", "GreenPower Motor Company", 0.1));
        index.add_or_update(eq("CF", "CF Industries", 0.1));
        for input in ["GP 5Y", "gp 5y", "CF 10-K"] {
            let s = run(&index, input);
            assert!(
                s.iter().all(|r| !r.best || r.kind == SuggestionKind::Function),
                "{input}: {:?}",
                s.iter().filter(|r| r.best).map(|r| (&r.title, &r.action)).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_ticker_of_the_same_name_is_best_but_the_command_stays() {
        let index = plain_fixture();
        let s = run(&index, "earn");
        assert_eq!(best(&s).display, "EARN US Equity");
        assert!(group(&s, "Commands").contains(&"Earnings this week".to_owned()));

        let mut index = plain_fixture();
        index.add_or_update(eq("GDP", "Goodrich Petroleum", 0.1));
        let s = run(&index, "gdp");
        assert_eq!(best(&s).display, "GDP US Equity");
        let cmd = s
            .iter()
            .find(|s| s.group == SuggestionGroup::Commands)
            .unwrap();
        assert_eq!(cmd.title, "GDP");
        assert_eq!(
            cmd.action,
            Some(Action::new("ECO", None).arg("series", "GDP"))
        );
        let s = run(&plain_fixture(), "gdp");
        assert_eq!(best(&s).title, "GDP");
    }

    #[test]
    fn registered_words_are_commands() {
        let s = run(&fixture(), "today");
        assert_eq!(best(&s).title, "Today");
        assert_eq!(best(&s).group, SuggestionGroup::Commands);
        assert_eq!(best(&s).action, Some(Action::new("TODAY", None)));
        let s = run(&fixture(), "fil");
        assert!(group(&s, "Commands").contains(&"Filings inbox".to_owned()));
    }

    #[test]
    fn topics_for_the_loaded_security() {
        let index = plain_fixture();
        let ctx = ParseContext {
            loaded: Some(SecurityKey::equity("MSFT")),
        };
        let s = index.suggest("char", &ctx, 12);
        let row = best(&s);
        assert_eq!(row.group.heading(), "On MSFT");
        assert_eq!(row.title, "Chart");
        assert_eq!(row.completion, "chart ");
        assert_eq!(
            row.action,
            Some(Action::new("GP", Some(SecurityKey::equity("MSFT"))))
        );
        // `news` alone is the market's news; the loaded security's is next to it.
        let s = index.suggest("news", &ctx, 12);
        assert_eq!(best(&s).title, "Top news");
        assert!(group(&s, "On MSFT").contains(&"News".to_owned()));
    }

    #[test]
    fn comparisons() {
        let index = plain_fixture();
        let s = run(&index, "aapl vs ms");
        assert_eq!(s[0].group.heading(), "Compare AAPL with");
        assert_eq!(s[0].title, "MSFT");
        assert_eq!(s[0].subtitle, "Microsoft Corp");
        assert_eq!(s[0].completion, "aapl vs msft ");
        let keys = compare_keys_of(s[0].action.as_ref().unwrap());
        assert_eq!(
            keys,
            [SecurityKey::equity("AAPL"), SecurityKey::equity("MSFT")]
        );
        // What GO runs is a row of its own when nothing else covers it.
        let s = run(&index, "aapl vs msft 5y");
        let b = best(&s);
        assert_eq!(b.group.heading(), "Compare AAPL with");
        assert_eq!(b.title, "MSFT");
        assert_eq!(
            b.subtitle,
            "Performance and key figures side by side, 5 years"
        );
        assert_eq!(b.hint, "COMPARE");
        assert_eq!(
            b.action.as_ref().and_then(|a| a.arg_value("range")),
            Some("5Y")
        );
        let s = run(&index, "compare aapl with goo");
        assert_eq!(s[0].title, "GOOGL");
        assert_eq!(s[0].completion, "aapl vs googl ");
        // Already compared securities aren't offered again.
        let s = run(&index, "aapl vs msft vs a");
        assert!(
            securities(&s)
                .iter()
                .all(|d| *d != "AAPL US Equity" && *d != "MSFT US Equity")
        );
    }

    #[test]
    fn crypto_and_class_shares() {
        let index = plain_fixture();
        let s = run(&index, "btc ");
        assert_eq!(s[0].display, "BTCUSD Curncy");
        assert!(s[0].best);
        let s = run(&index, "brk.b ");
        assert_eq!(s[0].display, "BRK-B US Equity");
        assert_eq!(index.plain_text(&SecurityKey::equity("BRK-B")), "brk-b");
        // The fund named BTC completes with its exchange so it resolves back.
        assert_eq!(index.plain_text(&SecurityKey::equity("BTC")), "btc us");
        assert_eq!(
            index.plain_text(&SecurityKey::new("BMW", Some("GR"), MarketSector::Equity)),
            "bmw"
        );
        assert_eq!(
            index.plain_text(&SecurityKey::new("XYZ", None, MarketSector::Corp)),
            "XYZ <CORP>"
        );
    }

    #[test]
    fn questions_go_to_ask() {
        let s = run(&fixture(), "ask what moved apple");
        let b = best(&s);
        assert_eq!(b.title, "Ask");
        assert_eq!(b.subtitle, "what moved apple");
        assert_eq!(
            b.action,
            Some(Action::new("ASK", None).arg("q", "what moved apple"))
        );
        let s = run(&fixture(), "why is msft down?");
        assert_eq!(best(&s).title, "Ask");
    }

    #[test]
    fn resolver_prefers_us_listings() {
        let index = SuggestIndex::new(vec![
            inst(
                SecurityKey::new("ABC", Some("LN"), MarketSector::Equity),
                "ABC London",
                0.9,
            ),
            eq("ABC", "ABC US", 0.1),
            inst(SecurityKey::index("ABC"), "ABC Index", 0.5),
        ]);
        assert_eq!(index.exact("ABC", None), Some(SecurityKey::equity("ABC")));
        assert_eq!(
            index.exact("ABC", Some("LN")),
            Some(SecurityKey::new("ABC", Some("LN"), MarketSector::Equity))
        );
        assert_eq!(index.exact("AB", None), None);
    }

    fn index_all(index: &SuggestIndex, input: &str) -> Vec<Suggestion> {
        index.suggest(input, &ParseContext::default(), 100)
    }

    /// Groups in display order, best first within each, one best row with
    /// the top score.
    fn assert_well_ordered(s: &[Suggestion]) {
        assert!(s.windows(2).all(|w| w[0].group.rank() <= w[1].group.rank()));
        assert!(
            s.windows(2)
                .all(|w| w[0].group.rank() != w[1].group.rank() || w[0].score >= w[1].score)
        );
        // At most one best row (none when nothing stands for a definite
        // command), and it scores highest.
        let bests: Vec<&Suggestion> = s.iter().filter(|x| x.best).collect();
        assert!(bests.len() <= 1, "{:?}", titles(s));
        if let Some(b) = bests.first() {
            assert!(s.iter().all(|x| x.score <= b.score));
        }
    }

    proptest! {
        #[test]
        fn suggest_never_panics_and_is_ordered(input in "\\PC{0,24}", limit in 0usize..20) {
            let index = plain_fixture();
            let ctx = ParseContext { loaded: Some(SecurityKey::equity("AAPL")) };
            let s = index.suggest(&input, &ctx, limit);
            prop_assert!(s.len() <= limit);
            assert_well_ordered(&s);
            prop_assert!(s.iter().all(|s| s.completion.ends_with(' ')));
        }

        #[test]
        fn suggest_never_panics_on_arbitrary_bytes(input in ".*") {
            let _ = fixture().suggest(&input, &ParseContext::default(), 12);
        }

        #[test]
        fn plain_inputs_are_ordered(words in prop::collection::vec(prop_oneof![
            Just("aapl"), Just("ms"), Just("vs"), Just("5"), Just("fil"), Just("chart"),
            Just("compare"), Just("with"), Just("ask"), Just("btc"), Just("earn"), Just("?"),
            Just("<EQUITY>"), Just("us"), Just("gp"),
        ], 0..5), trailing in any::<bool>()) {
            let mut input = words.join(" ");
            if trailing {
                input.push(' ');
            }
            let s = plain_fixture().suggest(&input, &ParseContext::default(), 12);
            assert_well_ordered(&s);
            for row in &s {
                prop_assert!(row.completion.ends_with(' '));
                prop_assert!(!row.title.is_empty());
                if let Some(a) = &row.action {
                    prop_assert!(lookup(&a.function).is_some() || a.is_app_action());
                }
            }
        }
    }
}
