//! Command-line autocomplete.
//!
//! As in the incumbent, typing shows matching securities and functions
//! together: `EA` lists the Electronic Arts stock and the earnings
//! functions. Once the line holds a complete security (`AAPL US <EQUITY> `),
//! only functions that accept that security are suggested.
//!
//! A sector typed as a plain word may belong to a name (`nvidia corp`,
//! `s&p index`). Unless the key is in the index or the sector key itself was
//! used (`<CORP>`), name matches are merged into the function suggestions.
//! When the panel has a security loaded, functions that can't run on it lose
//! 150 points.
//!
//! # Ranking
//!
//! Each match falls into a tier with a base score. Popularity adds up to
//! [`POPULARITY_WEIGHT`], which is smaller than the gap between tiers, so it
//! orders matches within a tier and never lifts a match into a higher one.
//!
//! | Tier | Base |
//! |---|---|
//! | exact mnemonic (what GO would run) | 1200 |
//! | exact ticker (`AAPL`, `AAPL US`, `AAPL US EQU`) | 1000 |
//! | mnemonic or ticker prefix, minus 2 per missing character | 800 |
//! | every query word prefixes a word of the name or function title | 600 (+40 if the first words match, +10 if equal) |
//! | every query word prefixes a title or keyword word | 500 |
//! | the whole query is a substring of the name (3+ characters) | 400 |
//! | the query is a subsequence of the ticker, same first letter | 200, minus gaps |
//!
//! Functions rank as if their popularity were 0.5. Ties break on the shorter
//! ticker or mnemonic, then alphabetically.
//!
//! # Performance
//!
//! Tickers and name words are kept in sorted vectors, so the prefix tiers are
//! binary searches. The substring and subsequence tiers need a linear scan,
//! which is skipped when the indexed tiers already fill `limit` with scores
//! the scan can't beat.

use std::cmp::Ordering;
use std::collections::HashMap;

use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Serialize};

use crate::parse::{
    ParseContext, find_sector, format_security, raw_tokens, security_key, token_is_bracketed,
};
use crate::registry::{FunctionSpec, lookup, registry};

/// Most popularity can add to a score.
pub const POPULARITY_WEIGHT: f32 = 100.0;

const EXACT_MNEMONIC: f32 = 1200.0;
const EXACT_TICKER: f32 = 1000.0;
const PREFIX: f32 = 800.0;
const PREFIX_LEN_PENALTY: f32 = 2.0;
/// Caps the prefix length penalty so long tickers stay above the next tier.
const MAX_PREFIX_PENALTY_CHARS: usize = 20;
const WORD_PREFIX: f32 = 600.0;
const FIRST_WORD_BONUS: f32 = 40.0;
const EXACT_FIRST_WORD_BONUS: f32 = 10.0;
const KEYWORD_PREFIX: f32 = 500.0;
const NAME_SUBSTRING: f32 = 400.0;
const TICKER_SUBSEQUENCE: f32 = 200.0;
const SUBSEQUENCE_GAP_PENALTY: f32 = 5.0;
/// Highest score the linear-scan tiers can produce.
const SCAN_TIER_MAX: f32 = NAME_SUBSTRING + POPULARITY_WEIGHT;
/// Functions rank as if they had this popularity.
const FUNCTION_POPULARITY: f32 = 0.5;
/// Subtracted from functions that can't run on the loaded security.
const INCOMPATIBLE_PENALTY: f32 = 150.0;
/// Score of the first function when listing everything for a security.
const LIST_SCORE: f32 = 100.0;
const MIN_SUBSTRING_LEN: usize = 3;
const MIN_SUBSEQUENCE_LEN: usize = 2;
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
    Security,
    Function,
}

/// One autocomplete row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    /// `AAPL US Equity` or `DES`.
    pub display: String,
    /// Security name or function title.
    pub detail: String,
    /// Replacement for the whole command line, ending in a space, e.g.
    /// `AAPL US <EQUITY> ` or `AAPL US <EQUITY> GP `.
    pub completion: String,
    /// Higher is better. Comparable only within one call.
    pub score: f32,
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
    completion: String,
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

impl Default for SuggestIndex {
    fn default() -> Self {
        Self::new(Vec::new())
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

    /// Suggestions for the current command-line text, best first, at most
    /// `limit` of them.
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

        if let Some((at, sector)) = find_sector(&upper)
            && let Some(security) = security_key(&upper[..at], sector)
        {
            let functions =
                self.functions_for(&security, &upper[at + 1..], &lower[at + 1..], limit);
            // A sector word typed as plain text may be part of a name
            // ("nvidia corp", "s&p index"). Unless the key is known or the
            // sector key itself was used, also search names.
            if token_is_bracketed(input, at) || self.by_key.contains_key(&security) {
                return functions;
            }
            let general = self.suggest_all(&upper, &lower, ctx, limit);
            return merge(functions, general, limit);
        }
        self.suggest_all(&upper, &lower, ctx, limit)
    }

    /// Functions that accept `security`, matched against the text after it.
    fn functions_for(
        &self,
        security: &SecurityKey,
        upper: &[String],
        lower: &[String],
        limit: usize,
    ) -> Vec<Suggestion> {
        // `AAPL US <EQUITY> GP 1Y`: the function is chosen; the user is typing arguments.
        if upper.len() >= 2 && lookup(&upper[0]).is_some() {
            return Vec::new();
        }
        let qwords = query_words(lower);
        let mut scored: Vec<(f32, &FunctionEntry)> = self
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.spec.takes_security() && f.spec.accepts_sector(security.sector))
            .filter_map(|(i, f)| {
                if upper.is_empty() {
                    Some((LIST_SCORE - i as f32, f))
                } else {
                    f.score(upper, &qwords).map(|s| (s, f))
                }
            })
            .collect();
        scored.sort_unstable_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.spec.mnemonic.len().cmp(&b.1.spec.mnemonic.len()))
                .then_with(|| a.1.spec.mnemonic.cmp(b.1.spec.mnemonic))
        });
        let prefix = format_security(security);
        scored
            .into_iter()
            .take(limit)
            .map(|(score, f)| Suggestion {
                kind: SuggestionKind::Function,
                display: f.spec.mnemonic.to_owned(),
                detail: f.spec.title.to_owned(),
                completion: format!("{prefix} {} ", f.spec.mnemonic),
                score,
            })
            .collect()
    }

    /// Securities and functions matching the whole input.
    fn suggest_all(
        &self,
        upper: &[String],
        lower: &[String],
        ctx: &ParseContext,
        limit: usize,
    ) -> Vec<Suggestion> {
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
                upsert(&mut cands, &mut slots, e, base + entry.boost);
            }
        }

        // Name words: every query word must prefix some word of the name.
        if let Some(anchor) = qwords.iter().max_by_key(|w| w.len()) {
            for r in self.word_range(anchor) {
                let entry = &self.entries[r.entry as usize];
                if all_prefix(&qwords, &entry.words) {
                    let score = WORD_PREFIX + first_word_bonus(&qwords, &entry.words) + entry.boost;
                    upsert(&mut cands, &mut slots, r.entry, score);
                }
            }
        }

        let strong = cands.iter().filter(|c| c.score > SCAN_TIER_MAX).count();
        if strong < limit {
            self.scan(upper, lower, &mut cands, &mut slots);
        }
        self.finish(cands, limit)
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

    fn finish(&self, mut cands: Vec<Candidate>, limit: usize) -> Vec<Suggestion> {
        let cmp = |a: &Candidate, b: &Candidate| self.candidate_cmp(a, b);
        if cands.len() > limit {
            cands.select_nth_unstable_by(limit - 1, cmp);
            cands.truncate(limit);
        }
        cands.sort_unstable_by(cmp);
        cands.iter().map(|c| self.suggestion(c)).collect()
    }

    fn suggestion(&self, c: &Candidate) -> Suggestion {
        match c.kind {
            SuggestionKind::Security => {
                let entry = &self.entries[c.idx as usize];
                Suggestion {
                    kind: SuggestionKind::Security,
                    display: entry.display.clone(),
                    detail: entry.inst.name.clone(),
                    completion: entry.completion.clone(),
                    score: c.score,
                }
            }
            SuggestionKind::Function => {
                let spec = self.functions[c.idx as usize].spec;
                Suggestion {
                    kind: SuggestionKind::Function,
                    display: spec.mnemonic.to_owned(),
                    detail: spec.title.to_owned(),
                    completion: format!("{} ", spec.mnemonic),
                    score: c.score,
                }
            }
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
            completion: format!("{} ", format_security(&key)),
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
        best.map(|s| s + FUNCTION_POPULARITY * POPULARITY_WEIGHT)
    }
}

/// Merges two best-first lists, keeping the better-scored copy of a
/// function that appears in both.
fn merge(a: Vec<Suggestion>, b: Vec<Suggestion>, limit: usize) -> Vec<Suggestion> {
    let mut all: Vec<Suggestion> = a.into_iter().chain(b).collect();
    // Stable: on equal scores, suggestions for the typed security come first.
    all.sort_by(|x, y| y.score.total_cmp(&x.score));
    let mut out: Vec<Suggestion> = Vec::with_capacity(limit.min(all.len()));
    for s in all {
        if out.len() == limit {
            break;
        }
        if !out
            .iter()
            .any(|o| o.kind == s.kind && o.display == s.display)
        {
            out.push(s);
        }
    }
    out
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

/// Scores `needle` as a subsequence of `hay` that starts at the same first
/// letter; compact matches and shorter tickers score higher.
fn subsequence_score(hay: &[u8], needle: &[u8]) -> Option<f32> {
    if needle.len() >= hay.len() || hay.first() != needle.first() {
        return None;
    }
    let mut at = 0;
    let mut gaps = 0;
    for &c in needle {
        let skip = hay[at..].iter().position(|&b| b == c)?;
        gaps += skip;
        at += skip + 1;
    }
    let trailing = hay.len() - at;
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

    fn run(index: &SuggestIndex, input: &str) -> Vec<Suggestion> {
        index.suggest(input, &ParseContext::default(), 12)
    }

    fn displays(s: &[Suggestion]) -> Vec<&str> {
        s.iter().map(|s| s.display.as_str()).collect()
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

    #[test]
    fn ticker_prefix_ranks_popular_first() {
        let s = run(&fixture(), "AAP");
        assert_eq!(s[0].display, "AAPL US Equity");
        assert_eq!(s[0].detail, "Apple Inc.");
        assert_eq!(s[0].completion, "AAPL US <EQUITY> ");
        assert_eq!(s[0].kind, SuggestionKind::Security);
        let secs = securities(&s);
        assert!(secs.contains(&"AAPB US Equity") && secs.contains(&"AAPU US Equity"));
    }

    #[test]
    fn exact_ticker_beats_more_popular_prefix() {
        let s = run(&fixture(), "AA");
        assert_eq!(securities(&s)[..2], ["AA US Equity", "AAPL US Equity"]);
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
        let des = s.iter().find(|s| s.display == "DES").unwrap();
        assert_eq!(des.detail, "Security Description");
        assert_eq!(des.completion, "DES ");
    }

    #[test]
    fn exact_mnemonic_ranks_first() {
        let s = run(&fixture(), "des");
        assert_eq!(s[0].display, "DES");
        // CN alone is Company News, as GO would run it; ticker CN follows.
        let s = run(&fixture(), "CN");
        assert_eq!(displays(&s)[..2], ["CN", "CN US Equity"]);
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

        // Lower case, no exchange, no brackets: canonical completion.
        let s = run(&fixture(), "aapl equity gi");
        assert_eq!(s[0].completion, "AAPL US <EQUITY> GIP ");
    }

    #[test]
    fn functions_after_a_security_respect_sector() {
        let s = run(&fixture(), "EURUSD CURNCY ");
        let fns = functions(&s);
        assert_eq!(fns[..4], ["DES", "GP", "GIP", "HP"]);
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
        assert_eq!(all[..5], top[..]);
        assert!(all.windows(2).all(|w| w[0].score >= w[1].score));
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
    }

    fn index_all(index: &SuggestIndex, input: &str) -> Vec<Suggestion> {
        index.suggest(input, &ParseContext::default(), 100)
    }

    proptest! {
        #[test]
        fn suggest_never_panics_and_is_sorted(input in "\\PC{0,24}", limit in 0usize..20) {
            let index = fixture();
            let ctx = ParseContext { loaded: Some(SecurityKey::equity("AAPL")) };
            let s = index.suggest(&input, &ctx, limit);
            prop_assert!(s.len() <= limit);
            prop_assert!(s.windows(2).all(|w| w[0].score >= w[1].score));
            prop_assert!(s.iter().all(|s| s.completion.ends_with(' ')));
        }

        #[test]
        fn suggest_never_panics_on_arbitrary_bytes(input in ".*") {
            let _ = fixture().suggest(&input, &ParseContext::default(), 12);
        }
    }
}
