//! Command-line parser.
//!
//! ```text
//! [<SECURITY> [<EXCHANGE>] <SECTOR>] [<FUNCTION>] [<ARGS>...]
//! ```
//!
//! The Swift shell calls [`parse`] when the user presses GO.

use meridian_types::{MarketSector, SecurityKey};
use serde::{Deserialize, Serialize};

use crate::error::CommandError;
use crate::registry::{FunctionSpec, SecurityNeed, lookup};

/// Largest number accepted by `<n> <GO>`.
pub const MAX_MENU_ITEM: u32 = 9999;

/// Panel state that commands are interpreted against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseContext {
    /// The panel's currently loaded security.
    pub loaded: Option<SecurityKey>,
}

/// Result of parsing one command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParsedCommand {
    /// Nothing typed.
    Empty,
    /// A security, optionally followed by a function and its arguments.
    /// `function: None` loads the security's main menu (or passes `args` to it).
    Security {
        security: SecurityKey,
        /// Canonical upper-case mnemonic.
        function: Option<String>,
        args: Vec<String>,
    },
    /// A function applied to the panel's loaded security, or to none.
    Function {
        /// Canonical upper-case mnemonic.
        function: String,
        args: Vec<String>,
    },
    /// `<n> <GO>`: select numbered menu item `n`.
    MenuItem(u32),
    /// Text that is neither a function nor a security key. The shell opens
    /// the security finder with it. Holds the trimmed input, case preserved.
    Search(String),
}

impl ParsedCommand {
    /// The security the command acts on: the typed one, or, for a bare
    /// function that takes a security, the panel's loaded one.
    #[must_use]
    pub fn target_security<'a>(&'a self, ctx: &'a ParseContext) -> Option<&'a SecurityKey> {
        match self {
            Self::Security { security, .. } => Some(security),
            Self::Function { function, .. } => lookup(function)
                .filter(|spec| spec.takes_security())
                .and(ctx.loaded.as_ref()),
            Self::Empty | Self::MenuItem(_) | Self::Search(_) => None,
        }
    }
}

/// Parses a command line.
///
/// Rules:
/// - Tokens are separated by whitespace and by `<` / `>`, so `<EQUITY>`,
///   `EQUITY`, and `Equity` are the same token. Tokens are upper-cased.
/// - A sector token (any spelling [`MarketSector::parse_label`] accepts) in
///   position 1 or 2 ends a security key: `SYMBOL [EXCHANGE] SECTOR`. More
///   than two tokens before the first sector token is a [`ParsedCommand::Search`].
///   Equities without an exchange get `US`.
/// - After the sector, a registered mnemonic is the function and the rest
///   are its arguments; otherwise everything after the sector is arguments.
/// - Without a sector token: a lone integer in `1..=9999` is a menu item; a
///   leading registered mnemonic is a function (so `CN` alone is Company
///   News, and ticker CN needs `CN US <EQUITY>`); anything else is a search.
///
/// The grammar is context-free today; `_ctx` is part of the signature so
/// context-dependent rules can be added without changing callers.
#[must_use]
pub fn parse(input: &str, _ctx: &ParseContext) -> ParsedCommand {
    let tokens: Vec<String> = raw_tokens(input).map(str::to_uppercase).collect();
    if tokens.is_empty() {
        return ParsedCommand::Empty;
    }

    if let Some((at, sector)) = find_sector(&tokens) {
        let Some(security) = security_key(&tokens[..at], sector) else {
            return ParsedCommand::Search(input.trim().to_owned());
        };
        let rest = &tokens[at + 1..];
        let (function, args) = match rest.split_first() {
            Some((first, args)) => match lookup(first) {
                Some(spec) => (Some(spec.mnemonic.to_owned()), args.to_vec()),
                None => (None, rest.to_vec()),
            },
            None => (None, Vec::new()),
        };
        return ParsedCommand::Security {
            security,
            function,
            args,
        };
    }

    if let [only] = tokens.as_slice()
        && let Some(n) = menu_item(only)
    {
        return ParsedCommand::MenuItem(n);
    }

    if let Some((first, args)) = tokens.split_first()
        && let Some(spec) = lookup(first)
    {
        return ParsedCommand::Function {
            function: spec.mnemonic.to_owned(),
            args: args.to_vec(),
        };
    }

    ParsedCommand::Search(input.trim().to_owned())
}

/// Command-line text for a security, e.g. `AAPL US <EQUITY>` or
/// `EURUSD <CRNCY>`. Parsing it yields the same key.
#[must_use]
pub fn format_security(key: &SecurityKey) -> String {
    let cap = key.sector.key_cap();
    match &key.exchange {
        Some(exchange) => format!("{} {exchange} <{cap}>", key.symbol),
        None => format!("{} <{cap}>", key.symbol),
    }
}

/// Checks that `spec` can run against `security` (the typed or loaded one).
///
/// Functions that never take a security accept anything; a security typed
/// with one of them is ignored.
pub fn check_security_need(
    spec: &FunctionSpec,
    security: Option<&SecurityKey>,
) -> Result<(), CommandError> {
    match (spec.needs_security, security) {
        (SecurityNeed::None, _) | (SecurityNeed::Optional, None) => Ok(()),
        (SecurityNeed::Required, None) => Err(CommandError::SecurityRequired {
            function: spec.mnemonic.to_owned(),
            example: example_security(spec),
        }),
        (SecurityNeed::Optional | SecurityNeed::Required, Some(sec)) => {
            if spec.accepts_sector(sec.sector) {
                Ok(())
            } else {
                Err(CommandError::SectorMismatch {
                    function: spec.mnemonic.to_owned(),
                    security: sec.clone(),
                    sectors: spec.sectors.to_vec(),
                })
            }
        }
    }
}

/// Checks a parsed command against its function's security needs, using the
/// panel's loaded security for bare functions. Commands without a function
/// always pass.
pub fn validate(command: &ParsedCommand, ctx: &ParseContext) -> Result<(), CommandError> {
    let (function, security) = match command {
        ParsedCommand::Security {
            security,
            function: Some(function),
            ..
        } => (function, Some(security)),
        ParsedCommand::Function { function, .. } => (function, ctx.loaded.as_ref()),
        ParsedCommand::Security { function: None, .. }
        | ParsedCommand::Empty
        | ParsedCommand::MenuItem(_)
        | ParsedCommand::Search(_) => return Ok(()),
    };
    let spec = lookup(function).ok_or_else(|| CommandError::UnknownFunction(function.clone()))?;
    check_security_need(spec, security)
}

/// Splits on whitespace and on `<` / `>`, dropping empty pieces.
pub(crate) fn raw_tokens(input: &str) -> impl Iterator<Item = &str> {
    input
        .split(|c: char| c.is_whitespace() || c == '<' || c == '>')
        .filter(|t| !t.is_empty())
}

/// Whether token `index` of `input` was typed as a bracketed sector key
/// (`<CORP>`, as the sector keys insert) rather than as a plain word.
pub(crate) fn token_is_bracketed(input: &str, index: usize) -> bool {
    raw_tokens(input).nth(index).is_some_and(|token| {
        let start = token.as_ptr().addr() - input.as_ptr().addr();
        input[..start].ends_with('<')
    })
}

/// Position and value of the sector token that ends a security key: the
/// first sector token after position 0 (position 0 is always a symbol, which
/// lets tickers such as `EQ` or `PFD` be loaded).
pub(crate) fn find_sector(tokens: &[String]) -> Option<(usize, MarketSector)> {
    tokens
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(i, t)| MarketSector::parse_label(t).map(|sector| (i, sector)))
}

/// Builds a key from the one or two tokens before the sector.
pub(crate) fn security_key(head: &[String], sector: MarketSector) -> Option<SecurityKey> {
    match head {
        [symbol] => Some(SecurityKey::new(
            symbol.as_str(),
            default_exchange(sector),
            sector,
        )),
        [symbol, exchange] => Some(SecurityKey::new(symbol.as_str(), Some(exchange), sector)),
        _ => None,
    }
}

fn default_exchange(sector: MarketSector) -> Option<&'static str> {
    (sector == MarketSector::Equity).then_some("US")
}

fn menu_item(token: &str) -> Option<u32> {
    // At most 4 digits after leading zeros keeps `parse` from overflowing.
    let digits = token.trim_start_matches('0');
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 4 {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (1..=MAX_MENU_ITEM).contains(&n).then_some(n)
}

/// A security the function accepts, in command-line form.
fn example_security(spec: &FunctionSpec) -> String {
    let sector = spec
        .sectors
        .first()
        .copied()
        .unwrap_or(MarketSector::Equity);
    let key = match sector {
        MarketSector::Equity => SecurityKey::equity("AAPL"),
        MarketSector::Index => SecurityKey::index("SPX"),
        MarketSector::Curncy => SecurityKey::currency("EURUSD"),
        MarketSector::Cmdty => SecurityKey::new("CLZ6", None, MarketSector::Cmdty),
        // No familiar single-token example; show the shape instead.
        MarketSector::Govt
        | MarketSector::Corp
        | MarketSector::Mtge
        | MarketSector::MMkt
        | MarketSector::Muni
        | MarketSector::Pfd => SecurityKey::new("TICKER", None, sector),
    };
    format_security(&key)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn p(input: &str) -> ParsedCommand {
        parse(input, &ParseContext::default())
    }

    fn sec(key: SecurityKey, function: Option<&str>, args: &[&str]) -> ParsedCommand {
        ParsedCommand::Security {
            security: key,
            function: function.map(str::to_owned),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    fn func(function: &str, args: &[&str]) -> ParsedCommand {
        ParsedCommand::Function {
            function: function.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    #[test]
    fn security_with_function() {
        let aapl = SecurityKey::equity("AAPL");
        assert_eq!(
            p("AAPL US <EQUITY> DES"),
            sec(aapl.clone(), Some("DES"), &[])
        );
        assert_eq!(p("aapl us equity des"), sec(aapl.clone(), Some("DES"), &[]));
        assert_eq!(
            p("AAPL EQUITY GP 1Y"),
            sec(aapl.clone(), Some("GP"), &["1Y"])
        );
        assert_eq!(
            p("  AAPL   US <Equity>  DES  "),
            sec(aapl.clone(), Some("DES"), &[])
        );
        // Sector keys typed without surrounding spaces.
        assert_eq!(p("AAPL US<EQUITY>DES"), sec(aapl, Some("DES"), &[]));
    }

    #[test]
    fn security_alone() {
        assert_eq!(
            p("IBM US Equity"),
            sec(SecurityKey::equity("IBM"), None, &[])
        );
        assert_eq!(p("SPX INDEX"), sec(SecurityKey::index("SPX"), None, &[]));
        assert_eq!(
            p("CLZ6 COMDTY"),
            sec(
                SecurityKey::new("CLZ6", None, MarketSector::Cmdty),
                None,
                &[]
            )
        );
        assert_eq!(
            p("BMW GR <EQUITY>"),
            sec(
                SecurityKey::new("BMW", Some("GR"), MarketSector::Equity),
                None,
                &[]
            )
        );
    }

    #[test]
    fn non_equity_sectors_have_no_default_exchange() {
        assert_eq!(
            p("EURUSD CURNCY GP"),
            sec(SecurityKey::currency("EURUSD"), Some("GP"), &[])
        );
        assert_eq!(
            p("EURUSD <CRNCY>"),
            sec(SecurityKey::currency("EURUSD"), None, &[])
        );
        assert_eq!(
            p("CLZ6 CMDTY GP"),
            sec(
                SecurityKey::new("CLZ6", None, MarketSector::Cmdty),
                Some("GP"),
                &[]
            )
        );
        assert_eq!(
            p("XYZ M-MKT"),
            sec(SecurityKey::new("XYZ", None, MarketSector::MMkt), None, &[])
        );
    }

    #[test]
    fn args_without_function() {
        assert_eq!(
            p("AAPL US <EQUITY> 1Y"),
            sec(SecurityKey::equity("AAPL"), None, &["1Y"])
        );
    }

    #[test]
    fn mnemonic_takes_precedence_over_ticker() {
        assert_eq!(p("CN"), func("CN", &[]));
        assert_eq!(
            p("CN US <EQUITY> CN"),
            sec(
                SecurityKey::new("CN", Some("US"), MarketSector::Equity),
                Some("CN"),
                &[]
            )
        );
    }

    #[test]
    fn sector_alias_tickers_can_be_loaded() {
        assert_eq!(p("EQ US EQUITY"), sec(SecurityKey::equity("EQ"), None, &[]));
        assert_eq!(
            p("PFD <EQUITY> DES"),
            sec(SecurityKey::equity("PFD"), Some("DES"), &[])
        );
    }

    #[test]
    fn bare_functions() {
        assert_eq!(p("DES"), func("DES", &[]));
        assert_eq!(p("des"), func("DES", &[]));
        assert_eq!(p("TOP"), func("TOP", &[]));
        assert_eq!(p("GP 1Y"), func("GP", &["1Y"]));
        assert_eq!(p("  omon  "), func("OMON", &[]));
    }

    #[test]
    fn menu_items() {
        assert_eq!(p("12"), ParsedCommand::MenuItem(12));
        assert_eq!(p("1"), ParsedCommand::MenuItem(1));
        assert_eq!(p("9999"), ParsedCommand::MenuItem(9999));
        assert_eq!(p("0012"), ParsedCommand::MenuItem(12));
        assert_eq!(p("0"), ParsedCommand::Search("0".to_owned()));
        assert_eq!(p("10000"), ParsedCommand::Search("10000".to_owned()));
        assert_eq!(p("+5"), ParsedCommand::Search("+5".to_owned()));
        assert_eq!(p("12 13"), ParsedCommand::Search("12 13".to_owned()));
        assert_eq!(
            p("99999999999999999999"),
            ParsedCommand::Search("99999999999999999999".to_owned())
        );
    }

    #[test]
    fn searches_and_empty() {
        assert_eq!(p("apple"), ParsedCommand::Search("apple".to_owned()));
        assert_eq!(
            p("  bank of america "),
            ParsedCommand::Search("bank of america".to_owned())
        );
        assert_eq!(p(""), ParsedCommand::Empty);
        assert_eq!(p("   "), ParsedCommand::Empty);
        assert_eq!(p("<>"), ParsedCommand::Empty);
        // A sector key alone is not a security.
        assert_eq!(p("<EQUITY>"), ParsedCommand::Search("<EQUITY>".to_owned()));
        // Multi-word symbols are not allowed.
        assert_eq!(
            p("BANK OF AMERICA <EQUITY>"),
            ParsedCommand::Search("BANK OF AMERICA <EQUITY>".to_owned())
        );
    }

    #[test]
    fn format_security_round_trips() {
        let keys = [
            SecurityKey::equity("AAPL"),
            SecurityKey::currency("EURUSD"),
            SecurityKey::index("SPX"),
            SecurityKey::new("CLZ6", None, MarketSector::Cmdty),
            SecurityKey::new("XYZ", None, MarketSector::MMkt),
        ];
        assert_eq!(format_security(&keys[0]), "AAPL US <EQUITY>");
        assert_eq!(format_security(&keys[1]), "EURUSD <CRNCY>");
        for key in keys {
            assert_eq!(p(&format_security(&key)), sec(key, None, &[]));
        }
    }

    #[test]
    fn security_need_messages() {
        let des = lookup("DES").unwrap();
        let err = check_security_need(des, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "DES requires a security. Load one first, e.g. AAPL US <EQUITY> DES <GO>"
        );

        let fa = lookup("FA").unwrap();
        let err = check_security_need(fa, Some(&SecurityKey::currency("EURUSD"))).unwrap_err();
        assert_eq!(
            err.to_string(),
            "FA does not apply to EURUSD Curncy. FA accepts Equity securities only."
        );

        let omon = lookup("OMON").unwrap();
        let err = check_security_need(omon, Some(&SecurityKey::currency("EURUSD"))).unwrap_err();
        assert_eq!(
            err.to_string(),
            "OMON does not apply to EURUSD Curncy. OMON accepts Equity or Index securities only."
        );
        assert!(
            matches!(err, CommandError::SectorMismatch { ref sectors, .. } if sectors.len() == 2)
        );
        let err = check_security_need(omon, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "OMON requires a security. Load one first, e.g. AAPL US <EQUITY> OMON <GO>"
        );
    }

    #[test]
    fn security_need_passes() {
        let aapl = SecurityKey::equity("AAPL");
        let spx = SecurityKey::index("SPX");
        let check = |m: &str, s: Option<&SecurityKey>| check_security_need(lookup(m).unwrap(), s);
        assert!(check("DES", Some(&aapl)).is_ok());
        assert!(check("GP", Some(&SecurityKey::currency("EURUSD"))).is_ok());
        assert!(check("OMON", Some(&spx)).is_ok());
        assert!(check("FA", Some(&spx)).is_err());
        assert!(check("TOP", None).is_ok());
        assert!(check("TOP", Some(&aapl)).is_ok());
        assert!(check("CN", None).is_ok());
        assert!(check("CN", Some(&spx)).is_ok());
        assert!(check("CORR", None).is_ok());
    }

    #[test]
    fn validate_uses_context() {
        let none = ParseContext::default();
        let loaded = ParseContext {
            loaded: Some(SecurityKey::equity("AAPL")),
        };
        let fx = ParseContext {
            loaded: Some(SecurityKey::currency("EURUSD")),
        };

        let des = parse("DES", &none);
        assert!(matches!(
            validate(&des, &none),
            Err(CommandError::SecurityRequired { .. })
        ));
        assert_eq!(validate(&des, &loaded), Ok(()));
        assert!(matches!(
            validate(&parse("FA", &fx), &fx),
            Err(CommandError::SectorMismatch { .. })
        ));
        // A typed security wins over the loaded one.
        assert_eq!(validate(&parse("AAPL US <EQUITY> FA", &fx), &fx), Ok(()));
        assert_eq!(validate(&parse("apple", &none), &none), Ok(()));
        assert_eq!(validate(&parse("IBM US <EQUITY>", &none), &none), Ok(()));
        let bogus = ParsedCommand::Function {
            function: "ZZZZ".to_owned(),
            args: vec![],
        };
        assert_eq!(
            validate(&bogus, &none),
            Err(CommandError::UnknownFunction("ZZZZ".to_owned()))
        );
    }

    #[test]
    fn target_security() {
        let aapl = SecurityKey::equity("AAPL");
        let ctx = ParseContext {
            loaded: Some(aapl.clone()),
        };
        assert_eq!(parse("DES", &ctx).target_security(&ctx), Some(&aapl));
        assert_eq!(parse("TOP", &ctx).target_security(&ctx), None);
        let ibm = parse("IBM US <EQUITY> DES", &ctx);
        assert_eq!(ibm.target_security(&ctx), Some(&SecurityKey::equity("IBM")));
        assert_eq!(parse("12", &ctx).target_security(&ctx), None);
    }

    #[test]
    fn serde_round_trip() {
        let ctx = ParseContext {
            loaded: Some(SecurityKey::equity("AAPL")),
        };
        for input in ["AAPL US <EQUITY> GP 1Y", "DES", "12", "apple", ""] {
            let cmd = parse(input, &ctx);
            let json = serde_json::to_string(&cmd).unwrap();
            assert_eq!(serde_json::from_str::<ParsedCommand>(&json).unwrap(), cmd);
        }
        let json = serde_json::to_string(&ctx).unwrap();
        assert_eq!(serde_json::from_str::<ParseContext>(&json).unwrap(), ctx);
        let err = check_security_need(lookup("FA").unwrap(), Some(&SecurityKey::index("SPX")))
            .unwrap_err();
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(serde_json::from_str::<CommandError>(&json).unwrap(), err);
    }

    fn vocab_token() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("AAPL".to_owned()),
            Just("US".to_owned()),
            Just("<EQUITY>".to_owned()),
            Just("equity".to_owned()),
            Just("<CRNCY>".to_owned()),
            Just("M-MKT".to_owned()),
            Just("DES".to_owned()),
            Just("CN".to_owned()),
            Just("12".to_owned()),
            Just("<".to_owned()),
            Just(">".to_owned()),
            Just("<>".to_owned()),
            "[0-9]{1,12}",
            "\\PC{0,6}",
            ".{0,4}",
        ]
    }

    fn key_strategy() -> impl Strategy<Value = SecurityKey> {
        let sector = prop::sample::select(MarketSector::ALL.to_vec());
        ("[A-Z][A-Z0-9]{0,5}", prop::option::of("[A-Z]{2}"), sector).prop_map(
            |(symbol, exchange, sector)| {
                // Equities always carry an exchange once parsed.
                let exchange = match (sector, exchange) {
                    (MarketSector::Equity, None) => Some("US".to_owned()),
                    (_, e) => e,
                };
                SecurityKey::new(symbol, exchange.as_deref(), sector)
            },
        )
    }

    proptest! {
        #[test]
        fn parse_never_panics(input in ".*") {
            let _ = p(&input);
        }

        #[test]
        fn parse_never_panics_on_command_like_input(
            tokens in prop::collection::vec(vocab_token(), 0..8),
        ) {
            let input = tokens.join(" ");
            let ctx = ParseContext { loaded: Some(SecurityKey::equity("AAPL")) };
            let cmd = parse(&input, &ctx);
            let _ = validate(&cmd, &ctx);
            match cmd {
                ParsedCommand::Empty => prop_assert!(raw_tokens(&input).next().is_none()),
                ParsedCommand::MenuItem(n) => prop_assert!((1..=MAX_MENU_ITEM).contains(&n)),
                ParsedCommand::Search(s) => prop_assert_eq!(s, input.trim()),
                ParsedCommand::Security { security, .. } => {
                    prop_assert!(!security.symbol.is_empty());
                }
                ParsedCommand::Function { function, .. } => {
                    prop_assert!(lookup(&function).is_some());
                }
            }
        }

        #[test]
        fn formatted_keys_parse_back(key in key_strategy()) {
            // An exchange code that is itself a sector label would end the key early.
            prop_assume!(key.exchange.as_deref().and_then(MarketSector::parse_label).is_none());
            prop_assert_eq!(p(&format_security(&key)), sec(key, None, &[]));
        }
    }
}
