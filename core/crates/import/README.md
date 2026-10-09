# meridian-import

Reads a broker's CSV export into normalized portfolio transactions. Pure: no I/O, no clock (the caller passes the date a positions snapshot is recorded on).

- `parse(text, mapping, opts)` detects the format from its header row, or reads the file through a user-chosen `ColumnMapping`, and returns `ParsedImport { format, header_line, headers, transactions, warnings, snapshot }`.
- `detect(text)` and `suggest_mapping(header_line, headers)` feed the app's column-mapping UI when detection fails.
- Each `ImportedTx` has trade date, settle date, symbol, kind (`meridian_types::TransactionKind`), signed quantity (shares in +, out −), price, signed amount (cash in +, out −), fees, cost basis (snapshots), currency, description, the broker's action text, account, and a fingerprint.
- **Rows are never dropped silently or guessed.** A row that can't be read, has an unknown code, or is unsupported (options, short sales, mergers, bonds) becomes an `ImportWarning` with its 1-based line number. Skipped bookkeeping rows (Vanguard settlement-fund sweeps, Fidelity core money-market moves, Vanguard's holdings summary) are reported as one note each, with their line numbers.
- **Fingerprints** (`fingerprint.rs`) are FNV-1a 128 over the format, trade date, the broker's action text, symbol, quantity, price, amount and cost basis, plus an occurrence number for identical rows in one file. Meridian's own classification is not an input, so improving a code table later doesn't re-import old rows. Snapshot rows leave the date out, so importing the same holdings again is a duplicate.
- **The account is not in the fingerprint** (it stays on the row for display). The same Fidelity trade has no account in a single-account `History_for_Account_X.csv`, the account number in an `Accounts_History.csv`, and an account name in 2024 all-accounts files, so hashing it made overlapping downloads import twice. Identical rows in different accounts of one file still differ by occurrence number (`-0`, `-1`). Trade-off: two separate single-account files that each contain an identical trade on the same day (same action text, symbol, quantity, price and amount) de-duplicate against each other, so the second one is not imported; the import result reports it among the duplicates, and the trade can be added by hand in PORT. The FNV parameters and test vectors come from <http://www.isthe.com/chongo/tech/comp/fnv/index.html> and Zig's `std.hash.Fnv1a_128` tests (<https://raw.githubusercontent.com/ziglang/zig/master/lib/std/hash/fnv.zig>).
- Output is in trade-date order. Files that list newest first are reversed before a stable sort, so same-day rows keep their real order. Split rows for the same account, symbol and date are netted into one change, so a reverse split's "remove old shares" and "add new shares" legs never pass through an empty position.

## Confidence

None of these brokers publishes a specification for its CSV. Formats were reconstructed (2026-10-07) from broker help pages where they exist, open-source parsers built against real exports, and the structure (never the values) of real exports that people published. **Confirmed** means a broker page, real parser code or a real export shows it; **unconfirmed** means one secondary source or an inference, and the code says so where it relies on one. No real account data is in this repository; the fixtures are hand-built (`tests/fixtures/SOURCES.md`).

| Format | Confidence | Notes |
|---|---|---|
| Robinhood account activity report | High | One stable 9-column header since at least 2020; codes from real rows. No official code list exists. |
| Fidelity account history | Medium-high | More than ten header layouts 2019–2026; columns are bound by name, so new orders work, but a renamed column would not. |
| Charles Schwab transactions | High for layouts, medium for rare actions | Three layouts, all read. Action list from parsers; Schwab publishes none. |
| Vanguard `OfxDownload.csv` | High for the brokerage section | Legacy mutual-fund-only and retirement-plan sections are not imported (noted in warnings). |
| Positions snapshot | High for Fidelity and Schwab headers, medium for others | Any CSV with `Symbol` and `Quantity`/`Qty (Quantity)`/`Shares` and no date column. |
| Generic mapping | n/a | User chooses the columns; kinds come from the keyword table in `formats/mapped.rs`, shown in the preview before saving. |

## Robinhood (`formats/robinhood.rs`)

How to get it (official): Account → Reports and statements → Account activity report → Generate new report (pick account type and dates); it arrives as a CSV under Reports within about 2 to 24 hours. It excludes crypto, futures and spending activity. <https://robinhood.com/us/en/support/articles/finding-your-reports-and-statements/>

Format (confirmed from real exports and parsers):
- Header `"Activity Date","Process Date","Settle Date","Instrument","Description","Trans Code","Quantity","Price","Amount"`, every cell quoted, LF line endings, no BOM, newest first. An older 11-column variant (adds `Account Type`, `Suppressed`) is unconfirmed; columns are bound by name so it would also read.
- Dates `M/D/YYYY`. Money `$1,234.56`, negatives `($1,234.56)` (a `-$` form is accepted too). Quantities plain, up to 6 decimals; a trailing `S` marks shares leaving (old leg of `SPR`/`SXCH`, long `OEXP`).
- Descriptions span lines: name, `CUSIP: …`, optional third line (`Dividend Reinvestment`, `Recurring`, …). A dividend reinvestment is a `Buy` with `Dividend Reinvestment` on that line, after the `CDIV` row.
- Footer: a `""` row, then a row of nine empty cells and a disclaimer beginning "The data provided is for informational purposes only." (skipped only when the data columns are empty).
- `SPL` quantity is the additional shares; `SPR` is two rows (old `NS` out, new `N` in).

Code table (`Buy`, `Sell`, `BTO`… confirmed in real rows unless marked):

| Codes | Imported as |
|---|---|
| `Buy` (`Dividend Reinvestment` on the description → reinvested dividend) | buy |
| `Sell`, `BCXL` (buy cancellation: shares returned for a refund) | sell |
| `SPL`, `SPR` | split (netted) |
| `SOFF` (spinoff), `REC` (shares received, meaning from one parser, unconfirmed) | transfer in, no cost basis |
| `CDIV`, `SCAP`, `MDIV` (unconfirmed) | dividend |
| `INT`, `SLIP` (stock lending), `MTCH` (IRA match), `GDBP` (deposit boost), `MINT` (margin interest, negative) | interest |
| `GOLD`, `GMPC` (plan credit, positive), `DTAX`, `DFEE`, `AFEE` (unconfirmed) | fee |
| `MISC` | other (listed, not P&L) |
| `ACH`, `RTP`, `DCF`, `WIRE`, `ITRF`, `CFIR`, `XENT`, `XENT_RWR`, `XENT_RWM`, `XENT_CC`, `FUTSWP` | deposit / withdrawal by sign; a negative row whose description mentions a fee is a fee |
| `ACATI` / `ACATO` (unconfirmed) | transfer in/out when shares move, else deposit/withdrawal |
| `BTO`, `STC`, `STO`, `BTC`, `OEXP`, `OASGN`, `OEXCS`, `OCC`, `OCA` | warning: options not supported |
| `SXCH`, `MRGS`, `MRGC`, `MRGR`, `CONV` | warning: exchanges and mergers not supported |
| anything else (`T/A`, `CIL`, `ROC`, `JNLC`, …) | warning: unknown code |

Sources: <https://github.com/GrantBirki/ghostfolio/blob/main/docs/imports/robinhood.md>, <https://github.com/mitdesai/ghostfolio-importer/blob/HEAD/app/robinhood.py>, <https://github.com/akourk/finledger/blob/HEAD/src/parsers/robinhood.py> (and its `tests/test_robinhood_footer.py`), <https://github.com/medloh/stockpile/blob/34bf6cf8101e54620ea1e2bda940f0a3aa88e791/shared/stocks_shared/parsers/robinhood.py>, <https://github.com/nathancheek/robinhood_capital_gains_estimator> (SPL/SPR 4-decimal note), <https://github.com/qmyhd/LLM-portfolio-project/blob/HEAD/scripts/validate_robinhood.py>, <https://github.com/yaitskov/RobinHood-pr0fit/blob/HEAD/src/RobinHood/RobinRow.hs>, <https://gist.github.com/robodhruv/3eedd86d91fa89d86d896248e2337358> (2021 11-column variant), <https://trademetria.com/integrations/robinhood>.

## Fidelity (`formats/fidelity.rs`)

How to get it: Fidelity's FAQ says the History page's Download button exports account history as CSV (<http://web.archive.org/web/20251215105509/https://www.fidelity.com/customer-service/faqs-exporting-account-information>, official; the live URL now redirects). Current path (confirmed by several users): Accounts & Trade → Portfolio → an account or All accounts → Activity & Orders → time period → download icon → Download as CSV. Files: `History_for_Account_<acct>.csv` or `Accounts_History.csv`. A per-download range limit (365 days) is reported but unconfirmed.

Format (confirmed across about twenty real files):
- Header found by its `Run Date`, `Action`, `Symbol`, `Quantity` and `Amount ($)`/`Amount` columns; every other column is looked up by name with or without ` ($)`, and `Security Description` is accepted for `Description`. Layouts seen: 2019–2023 (`Security Description`, `Security Type`), 2024–2025 (`Description`, `Type`, `Cash Balance ($)`), foreign-currency variants with `Exchange Quantity`/`Currency`/`Exchange Rate`, all-accounts files with `Account` and (from 2025) `Account Number`, and 2026 files with Price before Quantity.
- BOM, two empty lines, header (until mid-2026); from July 2026 the header is on line 1 followed by an empty line, dates are `MM-DD-YYYY` and empty cells are `""`. Cells often had a leading space before mid-2025.
- A row is data only when its first cell is a date; reading stops at the disclaimer ("The data and information in this spreadsheet…", "Brokerage services are provided…", "Date downloaded …").
- Numbers are plain signed decimals. Quantity + is shares in, − out; Amount + is cash in. Commission and fees are already in Amount. Rows of Type `Shares` (splits, mergers, ACAT share legs) carry a market value in Amount, which is ignored.
- `as of` dates inside the Action (`as of 02/15/2019`, `as of Jan-30-2026`, `as of 2026-07-15`, `AS OF 01-16-24`) are used as the trade date; Run Date is the posting date.
- Options have symbols like `-XLE260731C55`; bonds and T-bills use CUSIPs; core money markets (`SPAXX`, `FDRXX`, `FZFXX`, `FCASH`, `CORE`) appear without `**` in history files.

Action table (prefixes, case-insensitive, in this order):

| Action | Imported as |
|---|---|
| `… OPENING TRANSACTION …`, `… CLOSING TRANSACTION …`, `EXPIRED`, `ASSIGNED`, `EXERCISED` (unconfirmed) | warning: options |
| `BUY CANCEL`, `SELL CANCEL`, `… CXL …` (unconfirmed) | warning: cancelled trades |
| `YOU BOUGHT` (an `RSU` vest posted for $0 → transfer in at the row's price, unconfirmed) | buy |
| `YOU SOLD` | sell |
| `FOREIGN TAX PAID`, `ADJ FOREIGN TAX`, `NON-RESIDENT TAX`, `FED TAX W/H` (unconfirmed), `FEE CHARGED`, `ADVISOR FEE`, `ADJUST FEE` | fee |
| `DIVIDEND RECEIVED`, `DIVIDEND ADJUSTMENT`, `SHORT-TERM CAP GAIN`, `LONG-TERM CAP GAIN`, `RETURN OF CAPITAL` | dividend |
| `REINVESTMENT` | reinvested dividend (core funds skipped) |
| `INTEREST EARNED`, `MUNI EXEMPT INT`, `INTEREST FULLY PAID`, `INTEREST…` | interest |
| `DISTRIBUTION SPINOFF` (unconfirmed) | transfer in, no cost basis |
| `DISTRIBUTION` (split: additional shares), `REVERSE SPLIT` (two legs, paired onto the ticker) | split |
| `IN LIEU OF` (cash for fractional shares) | other |
| `MERGER`, `NAME CHANGED` | warning: mergers |
| `REDEMPTION PAYOUT` | warning: bond redemptions |
| `REDEMPTION FROM CORE`, `PURCHASE INTO CORE` (full text unconfirmed), `EXCHANGED TO` | skipped (core sweep) |
| `… ACAT …`, `ROLLOVER SHARES`, `TRANSFERRED FROM/TO` | transfer in/out when shares move, else deposit/withdrawal |
| `Electronic Funds Transfer`, `WIRE TRANSFER`, `DIRECT DEPOSIT/DEBIT`, `CHECK RECEIVED/PAID`, `BILL PAYMENT`, `DEBIT CARD`, `CASH ADVANCE`, `JOURNALED`, `CASH CONTRIBUTION`, `CONTRIBUTION`, `PARTIC CONTR`, `CO CONTR`, `ROLLOVER CASH`, `NORMAL DISTR`, `EARLY DIST` | deposit / withdrawal by sign |
| Workplace-plan rows (`Contributions`, `Dividend`, `Transfer`, `Exchange In/Out`, `Investment Gain/Loss`, `REVENUE CREDIT`, `RECORDKEEPING FEE`) | warning: 401(k) rows not supported |

Sources: hledger <https://github.com/hledgerorg/hledger/issues/2082>, <https://github.com/hledgerorg/hledger/blob/4522ec442ccaf87057623e161b58318c4b40441c/examples/csv/investment/fidelity.csv.rules>; beancount_reds_importers <https://github.com/redstreet/beancount_reds_importers/blob/102b5375691f058a990a9959ae66f44aaaa29a32/beancount_reds_importers/importers/fidelity/fidelity_brokerage_csv.py>, <https://github.com/redstreet/beancount_reds_importers/blob/102b5375691f058a990a9959ae66f44aaaa29a32/beancount_reds_importers/importers/fidelity/fidelity_all_accounts_csv.py>, issues <https://github.com/redstreet/beancount_reds_importers/issues/170>, <https://github.com/redstreet/beancount_reds_importers/issues/176>, <https://github.com/redstreet/beancount_reds_importers/issues/181>, <https://github.com/redstreet/beancount_reds_importers/issues/182>; <https://github.com/LaunchPlatform/beanhub-extract/blob/5faba21faab6cb004bbc92f62ac90bc716a66f1c/beanhub_extract/extractors/fidelity.py>; <https://github.com/TheInfiniteKind/moneydance_open/blob/f6e7a4b2a4fd28b1794173230a3dc5372e726b9b/python_scripts/import_fidelity/import_fidelity.py>; <https://github.com/esalino/wealthogic/blob/0059564f7bb9ee2ebb5478958ca91c1a0bd313c7/apps/api/internal/uploads/fidelity_transactions.go>; <https://github.com/brooksbol/options-prototype/blob/bf61b206f9881b43ddd833d0305ac9e9bdd01c6b/docs/engineering-spikes/fidelity-activity-history.md>; Fund Manager <https://www.fundmanagersoftware.com/help/fidelity_csv.html>, <https://fundmanagersoftware.com/forum/viewtopic.php?p=28259>, <https://fundmanagersoftware.com/forum/viewtopic.php?p=28957>; <https://groups.google.com/g/microsoft-money/c/2qsJbndxTBo> (July 2026 date change).

## Charles Schwab (`formats/schwab.rs`)

How to get it: Accounts → History → Transactions → date range → Export → CSV (steps from third-party guides; no Schwab help page found). Schwab states up to four years of transaction history under Accounts > History (official, <https://www.schwab.com/td-ameritrade>). Nothing in the current file identifies the account (it is only in the file name).

Format (confirmed):
- Up to ~2021: banner `"Transactions  for account … as of … ET"`, header `"Date","Action","Symbol","Description","Quantity","Price","Fees & Comm","Amount",` with a trailing comma on every row, footer `Transactions Total,…`. ~2022–mid 2023: banner and footer kept, no trailing comma on rows. Late 2023 onward: header on line 1, no banner or footer. All are read; the banner sits above the header and the footer is skipped by its first cell.
- Date `MM/DD/YYYY` or `MM/DD/YYYY as of MM/DD/YYYY`; the second date is when the event happened (interest period end, split date, assignment) and is used as the trade date. Rows are newest first.
- Money `$1234.56` / `-$1234.56` (thousands separators appear in footers and some files; parentheses accepted, unconfirmed). Trade quantities are unsigned; share movements (`Journaled Shares`, `Security Transfer`, splits) are signed. `Fees & Comm` is already netted into Amount.
- Reinvestment is a pair: an income row (`Reinvest Dividend`, `Qual Div Reinvest`, …) with Amount + and a `Reinvest Shares` row with shares and Amount −.
- `Stock Split` quantity is the additional shares; `Reverse Split` is two rows, the old leg often under the old CUSIP (paired onto the ticker).

Action table (exact names, case-insensitive): `Buy` → buy; `Sell` → sell; `Reinvest Shares` → reinvested dividend; `Reinvest Dividend`, `Qual Div Reinvest`, `Qual Div Reinvest Adj`, `Non-Qualified Div`, `Short/Long Term Cap Gain Reinvest`, `Pr Yr Div Reinvest`, `Cash Dividend`, `Qualified Dividend`, `Special Dividend`, `Special Qual Div`, `Special Non Qual Div`, `Pr Yr Cash Div`, `Pr Yr Special Div`, `Div Adjustment`, `Long/Short Term Cap Gain`, `Return Of Capital` → dividend; `Bank Interest`, `Credit Interest`, `Bond Interest`, `Margin Interest`, `Interest Adj`, `Promotional Award` → interest; `NRA Tax Adj`, `NRA Withholding`, `NRA Withhold`, `Pr Yr NRA Tax`, `Foreign Tax Paid`, `Foreign Tax Reclaim (Adj)`, `IRS Withhold Adj`, `ADR Mgmt Fee`, `Service Fee`, `Advisor Fee` → fee; `Cash In Lieu`, `Misc Cash Entry`, `Adjustment` → other; `Stock Split`, `Reverse Split`, `Stock Div Dist` → split; `Spin-off`, `Stock Plan Activity` (price, when given, is the value at vest) → transfer in; `MoneyLink Transfer/Deposit/Adj`, `Wire Sent/Received`, `Wire Funds (Received/Adj)`, `Funds Received/Paid`, `Bank Transfer`, `Journal`, `Internal Transfer`, `Futures MM Sweep`, `Visa Purchase` → deposit/withdrawal by sign; `Journaled Shares`, `Security Transfer` → transfer in/out by quantity sign (valuation price dropped), else cash by sign; options (`Buy/Sell to Open/Close`, `Expired`, `Assigned`, `Exchange or Exercise`, `Options Frwd Split (Adj)`), `Sell Short`, `Buy to Cover` (unconfirmed), `Cancel Buy/Sell` (sign unconfirmed), mergers and name changes, `Full Redemption (Adj)` → warnings; anything else → warning.

Sources: <https://github.com/redstreet/beancount_reds_importers/blob/main/beancount_reds_importers/importers/schwab/schwab_csv_brokerage.py> (and its test CSV), <https://github.com/redstreet/beancount_reds_importers/commit/bee9b44758fb37d1ccac5a94f98b96e25825aa5c>, <https://github.com/jbms/beancount-import/blob/master/beancount_import/source/schwab_csv.py> (and `testdata/source/schwab_csv`), <https://github.com/cgt-calc/capital-gains-calculator/blob/main/docs/brokers/schwab.md>, <https://github.com/cgt-calc/capital-gains-calculator/blob/main/cgt_calc/parsers/schwab.py>, cgt-calc commits <https://github.com/cgt-calc/capital-gains-calculator/commit/5c181963964745f24a35d7b1b124c2ef2b10f953>, <https://github.com/cgt-calc/capital-gains-calculator/commit/828c83a41ecf74f7ee0c6427d68f4164490b25b9>, <https://github.com/cgt-calc/capital-gains-calculator/commit/d1864b5d81bdbb99d1cf8549b91858a240e71060> and issues 407/451/464, <https://github.com/vroonhof/opensteuerauszug/blob/main/src/opensteuerauszug/importers/schwab/formats.md>, <https://github.com/dickwolff/Export-To-Ghostfolio/blob/main/samples/schwab-export.csv>, <https://github.com/ewmailing/Schwab2ofx/blob/master/schwab2ofx.lua>, <https://github.com/coffee-cpu/capital-gains-uk-101/blob/main/public/examples/schwab-transactions-example.csv>, <https://github.com/reubano/csv2ofx/issues/77>, <https://github.com/sinhong2011/TraderMemos/issues/291>.

## Vanguard (`formats/vanguard.rs`)

How to get it: Vanguard's technical-support page links the "CSV (spreadsheet)" download to the Download center (<https://investor.vanguard.com/technical-support>, official, linking <https://personal.vanguard.com/us/OfxWelcome>). Users report: Transaction history → Download → "A spreadsheet-compatible CSV file" → date range → accounts; the file is always `OfxDownload.csv`, and ranges stop at 18 months (unconfirmed).

Format (confirmed from real 2023–2026 files and parsers):
- Holdings section first: `Account Number,Investment Name,Symbol,Shares,Share Price,Total Value,` (not imported; a note says so, since the transactions only cover the downloaded range). Then the transactions section: `Account Number,Trade Date,Settlement Date,Transaction Type,Transaction Description,Investment Name,Symbol,Shares,Share Price,Principal Amount,Commissions and Fees,Net Amount,Accrued Interest,Account Type,` (older files say `Commission Fees`; both accepted). Every row ends with a comma. Optional retirement-plan sections follow (`Plan Number,…` or `Account Number,Trade Date,Run Date,Transaction Activity,…`); reading stops there with a note.
- Dates ISO `YYYY-MM-DD` (older files `MM/DD/YYYY`; both accepted). Numbers are plain signed decimals. Net Amount is the signed cash effect with commissions already deducted (Principal equals Net on trades). Share Price is meaningful only on trades.
- Rows are grouped by account and holding, not date; output is sorted.
- The settlement fund (VMFXX) generates `Sweep in`/`Sweep out` rows and zero-share `Reinvestment` rows: skipped with a note (its `Dividend` rows are income).
- Reverse splits are two `Stock split` rows (−old, +new), netted; forward splits as a single row of additional shares are reported but unconfirmed.

Type table: `Buy` → buy; `Sell` → sell; `Dividend`, `Capital gain (LT)`, `Capital gain (ST)` → dividend; `Reinvestment`, `Reinvestment (LT gain)`, `Reinvestment (ST gain)` → reinvested dividend (zero shares → skipped); `Interest`, `Interest charge` → interest; `Fee`, `Withholding` → fee; `Stock split` → split; `Corp Action (Spinoff)` (unconfirmed) → transfer in; `Corp Action (Cash in Lieu)` → other; `Funds Received`, `Contribution`, `Rollover (incoming)` → deposit; `Withdrawal`, `Distribution` → withdrawal; `Transfer (incoming)`, `Transfer (outgoing)`, `Transfer`, `Conversion (incoming/outgoing)` → transfer in/out when shares move, else deposit/withdrawal; `Wire in/out` (unconfirmed) → by sign; `Sweep in`/`Sweep out` → skipped; option types, `Corp Action (Sec Exchange)`, `Corp Action (Merger)`, `Corp Action (Exchange)`, `Corp Action (Redemption)`, `Sell short`, `Buy to cover` → warnings; anything else → warning.

Sources: <https://github.com/hledgerorg/hledger/blob/HEAD/examples/csv/investment/vanguard.csv.rules>, <https://github.com/beancount/beanbuff/blob/HEAD/beanbuff/vanguard/vanguard_csv.py>, <https://github.com/edelgm6/ledger/blob/HEAD/api/tests/models/test_csvprofile.py>, <https://github.com/hansonmatt/finance/blob/HEAD/vanguard_transaction_row_processor.py>, <https://github.com/itsme188/vanguard-skin/blob/HEAD/lib/import/parsers/vanguard-export.ts>, <https://github.com/sulrich/vanguard-analysis/blob/HEAD/tests/conftest.py>, <https://github.com/Ulthran/ctbus_finance/blob/HEAD/ctbus_finance/importers/vanguard.py>, <https://github.com/Roco-scientist/VAnguard-POrtfolio-REbalance/blob/HEAD/README.md>, <https://github.com/multifol-io/financial-variables/blob/HEAD/data/custodians/vanguard.csv>, <https://support.portseido.com/export-trades/vanguard/>, <https://wealthfolio.app/docs/guide/csv-import/>, <https://fundmanagersoftware.com/forum/viewtopic.php?p=9450> (forward split as one row, unconfirmed).

## Positions snapshot (`formats/positions.rs`)

Detected when a header has a symbol column (`Symbol`/`Ticker`), a quantity column (`Quantity`, `Qty (Quantity)`, `Qty`, `Shares`) and nothing that belongs to a transaction history. Header names are compared lower-cased with spaces and punctuation removed, so a history is recognized whether it writes `Trade Date`, `TransactionDate` (E*TRADE style) or `Date/Time` (IBKR style): any name containing `date` (except Merrill's `COB Date`, and `update`), `time`/`when` or a transaction time (`Trade Time`, `ExecTime`), or a type column (`Action`, `Transaction Type`/`TransactionType`, `Activity`, `Trans Code`, `Txn Type`, `Buy/Sell`) sends the file to the column-mapping path instead. `Type` alone does not: Fidelity's positions file uses it for the account type. Each holding becomes a transfer in on the import day with its cost basis: `Cost Basis Total`, `Cost Basis`, `Total Cost Basis` or `Total Cost`; else average cost (`Average Cost Basis`, `Cost Basis Per Share`, `Cost/Share`, `Price Paid $`, `Average Cost`, `Avg Cost`, `Cost Per Share`) × quantity; else none (PORT shows the cost as unknown). Not imported, with a warning: summary rows (`Cash & Cash Investments`, `Account Total`, `Positions Total`, `Futures Cash`, `TOTAL`, `CASH`, `Pending Activity` anywhere in the row), Fidelity core positions (`SPAXX**` etc.), options (by `Security Type`/`Asset Type` or the symbol's shape), CUSIPs (bonds, CDs), E*TRADE lot rows (a date in the symbol column), zero or short quantities. Schwab's `Incomplete` cost basis reads as unknown. Fidelity's disclaimer footer and E*TRADE's `Generated at` line are skipped; Schwab's all-accounts export (account-name row, repeated header per account) is followed.

Confirmed headers: Fidelity `Portfolio_Positions_*.csv` (2020–2026 variants, sentence-case 2026), Schwab positions (2023, 2024–2025, 2026 `Asset Type`), Vanguard holdings section, E*TRADE `PortfolioDownload.csv`, Merrill (no cost column). Sources: <https://github.com/istota-project/istota/blob/HEAD/src/istota/money/core/importers/fidelity_positions.py>, <https://github.com/multifol-io/site/blob/HEAD/library/Models/Importer.cs>, <https://github.com/NimbleEngineer21/greenlight/blob/HEAD/src/lib/parsers/fidelity.js>, <https://github.com/vroonhof/opensteuerauszug/blob/HEAD/src/opensteuerauszug/importers/schwab/position_extractor.py>, <https://github.com/redstreet/beancount_reds_importers/blob/HEAD/beancount_reds_importers/importers/schwab/schwab_csv_positions.py>, <https://github.com/redfish64/themetrack/blob/HEAD/schwab_parser.py>, <https://github.com/multifol-io/financial-variables/blob/HEAD/data/custodians/etrade-PortfolioDownload.csv>, <https://github.com/multifol-io/financial-variables/blob/HEAD/data/custodians/merillEdge.csv>, <https://www.fidelity.com/webxpress/help/topics/learn_portfolio_positions.shtml> (official, download steps only). Robinhood has no holdings export (<https://robinhood.com/us/en/support/articles/finding-your-account-documents>).

## Generic mapping (`formats/mapped.rs`)

The user maps column numbers to fields (`ColumnMapping`). With a date column each row is a transaction: kind from the action column by keyword (split → split; reinvest → reinvested dividend when shares move, else dividend; fee/commission/tax → fee; dividend/div/distribution/cap gain → dividend; interest → interest; buy/bought/purchase → buy; sell/sold/sale/redemption → sell; transfer → transfer in/out by quantity sign, else deposit/withdrawal by amount sign; deposit/contribution → deposit; withdraw/disbursement → withdrawal; anything else → warning), or, without an action column, by the quantity's sign (+ buy, − sell). Without a date column the file is a positions snapshot. Dates are `MM/DD/YYYY` unless `day_first`; ISO dates always work. The delimiter is sniffed (comma, tab or semicolon). The suggested mapping matches header names ignoring case, spaces and punctuation (`TransactionDate` finds the date column).

## Not supported

Options, short sales, cancelled/corrected trades, mergers, name changes and exchanges, bonds/CDs/T-bills, workplace 401(k) rows, Schwab Equity Awards exports, Vanguard legacy mutual-fund and retirement sections, crypto. Each appears as a warning on its line so the user can adjust by hand. No FX conversion: each row keeps its currency.
