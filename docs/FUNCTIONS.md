# Function registry

The terminal functions Meridian will implement, the mnemonic each uses, and whether that mnemonic is a verified standard. Verified means a public source (university library guides, function lists) confirms the incumbent terminal uses that mnemonic for that purpose. Research date: 2026-10-05.

**Rule:** a function ships under a mnemonic only if it is **Verified** or the user has approved it. Rows marked **ASK USER** are blocked until answered.

## Phase 3: Core market

| Mnemonic | Function | Status | Source |
|---|---|---|---|
| DES | Security description | Verified | [CFI list](https://corporatefinanceinstitute.com/resources/equities/bloomberg-functions-shortcuts-list/) |
| GP | Historical price chart with indicators and drawing tools | Verified | CFI; [ISEG](https://iseg.libguides.com/c.php?g=706923&p=5094213) |
| GIP | Intraday price chart | Verified | CFI |
| HP | Historical price table | Verified | CFI; [UDel](https://lerner.udel.edu/seeing-opportunity/bloomberg-functions-list/) |
| W | Security worksheet: real-time multi-security watchlist | Verified as the current worksheet function | [Cranfield](https://blogs.cranfield.ac.uk/library/bloomberg-w/) |
| NW | Legacy custom market monitor | Verified, legacy | UDel |
| (Launchpad Monitor) | Monitor *component* inside Launchpad (BLP), up to 2,000 securities | Verified as a component; it is not a command-line function | [Lippincott](https://lippincottlibrary.wordpress.com/2013/03/11/bloomberg-launchpad-part-one/) |
| BLP | Launchpad workspace | Verified | Wikipedia; Lippincott |
| SECF | Security finder (filtered search by asset class) | Verified | [UF](https://businesslibrary.uflib.ufl.edu/c.php?g=114612&p=746555) |
| MOST | Most active securities | Verified | CFI; UDel |

**ASK USER:** The quote monitor in the brief has no single "MON" mnemonic. Should it be `W` (worksheet), a Launchpad Monitor component, or both?

## Phase 4: News, fundamentals, filings

| Mnemonic | Function | Status | Source |
|---|---|---|---|
| N | News main menu | Verified | CFI; UDel |
| CN | Company news | Verified | CFI; ISEG |
| TOP | Top news | Verified | CFI; ISEG |
| FA | Financial analysis (statements) | Verified | CFI; ISEG |
| EE | Earnings estimates | Verified | CFI; UDel |
| ERN | Earnings history vs consensus | Verified | CFI; UDel |
| ANR | Analyst recommendations | Verified | CFI; ISEG |
| HDS | Holders | Verified | UDel |
| DVD | Dividends and splits | Verified | CFI; UDel |
| CF | Company filings | Verified | CFI; UDel |

The brief's **AI summary and diff vs prior filing** are our own additions. They will appear as numbered actions inside CF, not as a new mnemonic, unless you'd prefer a separate one.

## Phase 5: Derivatives

| Mnemonic | Function | Status | Source |
|---|---|---|---|
| OMON | Option monitor (chain, Greeks, IV) | Verified | ISEG; [HBS](https://www.library.hbs.edu/services/help-center/bloomberg-options) |
| OVDV | Volatility surface | Verified | Vendor webinar page (search snippet only) |
| OVME | Option valuation / strategy payoff | Verified | ISEG; UDel |

## Phase 6: Analytics

| Mnemonic | Function | Status | Source |
|---|---|---|---|
| EQS | Equity screener | Verified | ISEG; York |
| RV | Relative valuation / comps | Verified | ISEG; CFI |
| CORR | Correlation matrix | Verified | UDel |
| PORT | Portfolio and risk analytics | Verified | ISEG |
| BTST | Backtester | Verified, weak source (video title) | Bloomberg Pro Tips short |
| ALRT | Alerts | Verified | UDel; Stanford |

## Phase 7: Macro and cross-asset

| Mnemonic | Function | Status | Source |
|---|---|---|---|
| WEI | World equity indices | Verified | ISEG; UDel |
| ECO | Economic calendar | Verified | ISEG; UDel |
| FXC | FX cross-rate matrix | Verified | ISEG; [UMich](https://kresgeguides.bus.umich.edu/bloomberg/Currencies) |
| FXIP | FX information portal | Verified (optional, not in brief) | Lippincott; UDel |
| WCRS | World currency ranker | Verified (optional, not in brief) | ISEG; UMich |
| CRYP | Crypto monitor | Verified | UMich; vendor press release |

**FRED series browsing** (part of ECO in the brief): the incumbent's economic data lives under ECO and related functions. Unless you want a separate function, our FRED browser will be a numbered page inside ECO.

## Phase 8: ASK + alerts

| Mnemonic | Function | Status | Notes |
|---|---|---|---|
| ASK | Our AI analyst | **Ours (not standard)** | Approved by the brief. The incumbent has a similar function, `ASKB`. `ASK` itself has no evidence of being an existing mnemonic, so there's no collision. |

## 1.1: plain-word functions

New functions get plain-word ids instead of new mnemonics (CLAUDE.md, 2026-10-07). They are Meridian's own and make no claim to be standard.

| Id | Function | Arguments | Notes |
|---|---|---|---|
| TODAY | Home: your markets, holdings and what's coming up | none | Registered so `today` and `home` resolve and autocomplete; the screen is built separately |
| CALENDAR | Upcoming earnings reports and dividend dates | `kind` = `earnings` or `dividends` (absent: both); `range` = `today`, `this-week` or `next-week` (absent: this week) | `earnings this week` → `kind=earnings range=this-week`; the screen is built separately |
| FILINGS | Filings inbox: new filings from the companies you follow | none | `filings` alone; `aapl filings` is still CF for one company. The screen is built separately |
| COMPARE | Securities side by side: performance rebased to 100 and a table of last price, 1D/1M/YTD/1Y change, market cap, P/E, net margin and dividend yield | `securities` = comma-separated keys (up to 8); `range` = a chart preset (default 1Y) | `aapl vs msft [vs …] [range]`, `compare aapl with msft`. Changes are measured from the last close on or before the period's start date (YTD from the prior year-end), and the chart is rebased at the same close, so a 1Y chart ends at the 1Y change |

`SETTINGS` and `IMPORT` are app actions, not registry functions: `settings` and `import` resolve to them and the app opens Settings or the CSV import.

## Plain-language commands (1.1)

Case-insensitive. A security is a ticker (`aapl`, `brk.b`, `brk/b`), a ticker and exchange (`bmw gr`), a coin (`btc`, `eth`) or a currency pair (`eurusd`, `eur/usd`). The vocabulary lives in `core/crates/command/src/plain.rs`.

| Typed | Runs |
|---|---|
| `<sec>` | DES |
| `<sec> 1d · 5d · 1m · 3m · 6m · ytd · 1y · 2y · 3y · 5y · 10y · 20y · max` (also `5 years`, `1 week`, `year to date`) | GP with `range` |
| `<sec> chart · graph [range]`, `<sec> 5y chart` | GP |
| `<sec> intraday [range]`, `<sec> today` | GIP |
| `<sec> news` | CN |
| `<sec> filings`; `10-k · 10-q · 8-k` (also `10k`, `annual report`) | CF; with `form` |
| `<sec> financials · fundamentals`; `income [statement] · balance sheet · cash flow · ratios [quarterly]` | FA; with `stmt` (and `per`) |
| `<sec> earnings` · `estimates` · `analysts · ratings` · `dividends · splits` · `holders` | ERN · EE · ANR · DVD · HDS |
| `<sec> options · chain` · `volatility · vol · smile` · `option valuation` | OMON · OVDV · OVME |
| `<sec> backtest [range]` · `peers · comps` · `history · prices` · `description · profile · overview` · `correlation` | BTST · RV · HP · DES · CORR |
| `<sec> <MNEMONIC> [args]` | as `SYMBOL US <EQUITY> <MNEMONIC> [args]` |
| `<topic> <sec>` (`filings aapl`, `5y msft`) | as `<sec> <topic>` |
| `<topic>` alone (`chart`, `options`, `5y`) | that topic on the panel's loaded security |
| `<sec> vs <sec2> [vs …] [range]`, `compare <sec> [with] <sec2> …` | COMPARE |
| `today · home` · `calendar` · `filings` · `portfolio` · `import` · `settings` | TODAY · CALENDAR · FILINGS · PORT · IMPORT · SETTINGS |
| `earnings [this week · next week · today]`, `dividends [this week · next week · today]` | CALENDAR with `kind` (and `range`) |
| `news · headlines` · `crypto` · `fx · currencies` · `indices · world` · `screener` · `alerts` · `watchlist` · `most active` · `gainers · losers` · `help` | TOP · CRYP · FXC · WEI · EQS · ALRT · W · MOST · HELP |
| `rates · yield curve · treasury` | ECO `view=curve` |
| `cpi · inflation` · `unemployment` · `jobs · payrolls` · `gdp` · `real gdp` · `fed funds` | ECO `series=` CPIAUCSL · UNRATE · PAYEMS · GDP · GDPC1 · FEDFUNDS |
| `ask <question>`, or two or more words ending in `?` | ASK with `q=<question>` as typed |
| anything else | security search (SECF), as before |

**Ambiguity rules** (decided 2026-10-07; tests in `plain.rs` and `suggest.rs`):

1. **Mnemonics keep precedence.** A typed key or a leading mnemonic means what it always did (`CN` alone is Company News; `W` is the worksheet). Exceptions: `ask <question>` keeps the question's case, and a plain-word id followed by more words is read as a phrase (`compare aapl msft`, `filings aapl`, `AAPL US <EQUITY> filings`).
2. **Data words yield to an exact ticker.** `cpi`, `inflation`, `unemployment`, `jobs`, `payrolls` and `gdp` open the FRED series unless the index lists a ticker with exactly that symbol, which then opens instead: a listed ticker is the more specific match, and the series stays the next row in the suggestions. Navigation words (`news`, `crypto`, `fx`, `world`, `today`, `settings`, …) never yield; the ticker is reachable as `news us` or with its key.
3. **Major coins beat same-named funds.** `btc`, `eth`, `sol`, `xrp`, `ada`, `doge`, `avax`, `link`, `dot`, `ltc`, `bch`, `xlm`, `uni`, `atom`, `aave` (the CRYP list) mean the coin even where a fund trades under the ticker (`btc us` reaches the fund). Other coin shorthands apply only when no ticker matches.
4. **A ticker before a range word is a security.** `aapl vs max` compares with the ticker MAX; `aapl max` is the all-time chart.
5. **Currency pairs of two ISO codes resolve without an index entry** (`usdjpy`), since the pair itself names the instrument; a screen with no source for it says NOT AVAILABLE.
6. **Unresolved securities don't guess.** `zzzz 5y`, `aapl vs zzzz` and `aapl qqqq` stay security searches.

## Shell-level keys and screens (Phase 2)

| Item | Finding | Source |
|---|---|---|
| Sector keys | F2 GOVT, F3 CORP, F4 MTGE, F5 M-MKT, F6 MUNI, F7 PFD, F8 EQUITY, F9 CMDTY, F10 INDEX, F11 CRNCY. F12's label varies by keyboard generation (CLIENT / PORT / ALPHA). | [Illinois](https://guides.library.illinois.edu/bloomberg_user_guide/the_bloomberg_keyboard), [Seton Hall](https://library.shu.edu/c.php?g=351647&p=2373722), Wikipedia |
| HELP | F1. On a function, it opens that function's help. After typed words, it searches. HELP HELP opens live help chat. | UCD, UT Tampa, SMU |
| HELP (function) | Registered in the function registry (2026-10-07) so `HELP <GO>` opens a screen: command syntax, the key map (kept identical to `KeyRouter.swift` and the ⌘/ overlay by a test), and a directory of every function grouped by area. `topic=<MNEMONIC>` (the "Help on" input) explains one function: arguments, an example, which Settings → Setup source it needs in LIVE mode, related functions. The F1 key itself still shows the key overlay (twice: MENU); pointing it at this screen is a pending app change. | Same sources as the key |
| GO / CANCEL | Enter / Esc | Wikipedia, Pace |
| MENU | Back / related-functions menu. No standard-keyboard equivalent found. | Pace, BU |
| END/BACK | End key: back to the previous screen (newer keyboards) | [UPenn](https://guides.library.upenn.edu/bloomberg/keyboard) |
| PAGE FWD / BACK | PgDn / PgUp. `<n> PAGE FWD` jumps *n* pages. | BU, FGCU |
| PANEL | Cycles focus between panels. No standard-keyboard equivalent found. | SMU, BU |
| PRINT | Prints the page (`<n> PRINT` prints n pages) | BU |
| Numbered items | `<n> <GO>` selects; 95) Settings and 96) News are conventional | ISEG, Investopedia mirror |
| Autocomplete | Matching functions and securities appear as you type | UF, Imperial |
| Panel anatomy | Toolbar (menu + recent securities), command line, red function bar with drop-downs, function area | Imperial, NYU Law, ISEG |
| Linking | Launchpad Group Manager; groups are labeled by **letter** (A, B, C…), with security groups and monitor groups. Colour-group linking was not found. | Lippincott Part III |

## LIVE data notes (free tier)

| Mnemonic | What LIVE mode shows |
|---|---|
| DVD | Alpaca corporate actions (ex-, record and pay dates, cash amounts, splits) merged with SEC EDGAR dividends per share by fiscal period and splits disclosed in filings. A source that fails leaves only its part NOT AVAILABLE. |
| WEI | Index levels aren't available from free sources, so each index row shows a labeled US-listed ETF proxy (e.g. S&P 500 → SPY, Nikkei 225 → EWJ) quoted through Alpaca. |
| FA | As-reported EDGAR statements; a value restated in a later filing wins, even under a different concept. A split disclosed inside the displayed window adds a notice that per-share values aren't adjusted. |

### Conflicts with the brief

1. **Input-field colour**: the brief says inputs are white and yellow. Public sources say editable fields are **amber**. References will settle it; we'll extract exact colours from them.
2. **HELP**: the brief binds it to ⌘?, while the standard keyboard uses F1. Proposal: bind both.
3. **MENU** and **PANEL** have no standard Mac-keyboard keys; bindings needed (see open questions in `ARCHITECTURE.md` §16).
