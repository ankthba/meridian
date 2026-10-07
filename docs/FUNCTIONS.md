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

## 1.1: Plain-word functions

New features get plain-word ids, not new mnemonics (CLAUDE.md). These are Meridian's own; no incumbent mnemonic is claimed. Screens are built in the engine (`screens/today.rs`, `calendar.rs`, `inbox.rs`); the command-registry entries land separately.

| Id | Function | Status | Notes |
|---|---|---|---|
| TODAY | Home: portfolio value and today's change, market strip (SPY, QQQ, 10-year Treasury, BTC), holdings with live prices, coming up (next 10 days), new filings (5 unread), news on holdings (8) | **Ours** | Without positions it offers IMPORT and shows the first watchlist. Sections load concurrently; each degrades on its own. |
| CALENDAR | Earnings, ex-dividend dates and macro releases in a window for holdings and watchlists (or all securities) | **Ours** | `range` Next 10 days / Today / This week / Next week / This month; `scope`; `kind`; macro `importance` (high by default). Rows open ERN, DVD or ECO. |
| FILINGS | Inbox of recent SEC filings for holdings and watchlists, newest first, read/unread | **Ours** | 8-K items by their official Form 8-K titles; opening a filing in CF marks it read; periodic reports and proxies open on CF's diff versus the prior filing of the same form. `Mark all read` (by time, idempotent), `unread=<accession>`. Read state in SQLite (`filing_reads`). |

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
| CALENDAR | Earnings: Finnhub `/calendar/earnings` (free tier: upcoming releases plus 1 month back; date, before open / after close, consensus EPS). Up to 3 stocks are asked one by one; more cost one request for the window. Dividends: Alpaca corporate actions for up to 100 symbols per request, filtered by ex-date. Macro: FRED release dates, high-importance releases unless `importance=All`. A missing source leaves only its kind NOT AVAILABLE. |
| FILINGS | SEC EDGAR submissions per company (the CF list cache, 15 minutes). A company that fails is named in a notice; if every one fails, the screen is NOT AVAILABLE with the reason (e.g. no contact email set). No AI calls; CF summarizes only on request. |
| TODAY | Quotes from the stream when subscribed, else one REST batch; the 10-year yield from the Treasury par curve (`UST:10 Yr`, keyless), else FRED `DGS10`; calendar and filings as above; company news from Alpaca and Finnhub for the 10 largest positions by cost, de-duplicated by headline, reused for five minutes. |

### Conflicts with the brief

1. **Input-field colour**: the brief says inputs are white and yellow. Public sources say editable fields are **amber**. References will settle it; we'll extract exact colours from them.
2. **HELP**: the brief binds it to ⌘?, while the standard keyboard uses F1. Proposal: bind both.
3. **MENU** and **PANEL** have no standard Mac-keyboard keys; bindings needed (see open questions in `ARCHITECTURE.md` §16).
