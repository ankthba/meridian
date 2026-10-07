//! Deterministic synthetic instrument universe shared by the perf smoke test
//! and the criterion bench (`benches/suggest.rs` includes this file by path).

use meridian_command::IndexedInstrument;
use meridian_types::{MarketSector, SecurityKey};

const LEADS: [&str; 24] = [
    "American", "Global", "United", "First", "Pacific", "Northern", "Southern", "Atlantic",
    "Advanced", "Applied", "Digital", "General", "National", "Premier", "Summit", "Pioneer",
    "Liberty", "Frontier", "Golden", "Silver", "Coastal", "Western", "Eastern", "Central",
];
const CORES: [&str; 20] = [
    "Energy",
    "Health",
    "Bio",
    "Software",
    "Semiconductor",
    "Financial",
    "Insurance",
    "Retail",
    "Foods",
    "Mining",
    "Logistics",
    "Realty",
    "Media",
    "Telecom",
    "Motors",
    "Aerospace",
    "Water",
    "Steel",
    "Chemical",
    "Capital",
];
const SUFFIXES: [&str; 8] = [
    "Inc.", "Corp", "Holdings", "Group", "Ltd", "PLC", "Co", "Trust",
];
const EXCHANGES: [&str; 6] = ["US", "LN", "GR", "JP", "HK", "CN"];

/// Base-26 letters for `n`, e.g. 0 → `A`, 26 → `BA`.
fn letters(mut n: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// `n` synthetic instruments with unique keys, plus a few real-looking ones
/// (Apple, Amazon, Bank of America) so queries have known targets.
pub fn universe(n: usize) -> Vec<IndexedInstrument> {
    let mut out: Vec<IndexedInstrument> = (0..n)
        .map(|i| {
            let sector = match i % 50 {
                0 => MarketSector::Index,
                1 => MarketSector::Curncy,
                2 => MarketSector::Cmdty,
                _ => MarketSector::Equity,
            };
            let exchange =
                (sector == MarketSector::Equity).then_some(EXCHANGES[i % EXCHANGES.len()]);
            let name = format!(
                "{} {} {}",
                LEADS[i % LEADS.len()],
                CORES[(i / LEADS.len()) % CORES.len()],
                SUFFIXES[(i / (LEADS.len() * CORES.len())) % SUFFIXES.len()]
            );
            // Knuth multiplicative hash: a fixed, well-spread popularity.
            let popularity = ((i as u64 * 2_654_435_761) % 1000) as f32 / 1000.0;
            IndexedInstrument {
                key: SecurityKey::new(letters(i + 26), exchange, sector),
                name,
                popularity,
            }
        })
        .collect();
    for (symbol, name) in [
        ("AAPL", "Apple Inc."),
        ("AMZN", "Amazon.com Inc."),
        ("BAC", "Bank of America Corp"),
        ("MSFT", "Microsoft Corp"),
    ] {
        out.push(IndexedInstrument {
            key: SecurityKey::new(symbol, Some("US"), MarketSector::Equity),
            name: name.to_owned(),
            popularity: 1.0,
        });
    }
    out.push(IndexedInstrument {
        key: SecurityKey::currency("BTCUSD"),
        name: "Bitcoin / USD".to_owned(),
        popularity: 0.6,
    });
    out
}

/// Queries covering every ranking tier, including ones that force the
/// linear scan (`mazon`, `zzzzq`, `QXJ`).
pub const QUERIES: [&str; 12] = [
    "A",
    "AA",
    "AAP",
    "apple",
    "american energy",
    "bank of",
    "mazon",
    "zzzzq",
    "QXJ",
    "fin",
    "AAPL US",
    "AAPL US <EQUITY> G",
];

/// Plain-language queries: a ticker with a topic, a range, a listing, a
/// comparison, commands, a crypto shorthand and a question.
pub const PLAIN_QUERIES: [&str; 12] = [
    "aapl fil",
    "aapl ",
    "aapl 5y",
    "aapl vs ms",
    "aapl vs msft 5y",
    "earn",
    "earnings this week",
    "appl",
    "cpi",
    "btc",
    "ask what moved apple",
    "AAPL US <EQUITY> fil",
];
