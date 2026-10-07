# Fixtures

**Every file here is hand-built.** None is a real export and none contains real account data: account numbers (`Z00000001`, `12345678`, `XXXX-1234`, `...000`), quantities, prices, amounts and dates are invented, and `ABCD` (with CUSIPs `000000AA3`/`000000AB1`) is a made-up security used for corporate-action cases. Real tickers appear only as symbols. Layouts follow the sourced format descriptions in `../../README.md`; disclaimer and footer text is paraphrased except for the opening words the README quotes.

| File | Layout it follows | Exercises |
|---|---|---|
| `robinhood_activity.csv` | Robinhood account activity report: 9 quoted columns, LF, newest first, multi-line descriptions, `($…)` negatives, disclaimer footer row | buys, sell, `CDIV` plus dividend-reinvestment `Buy`, `SPL`, `SPR` with `100S`, `INT`, `GOLD`, `ACH` in/out, an option (`BTO`) and an unknown code as warnings |
| `robinhood_activity_later.csv` | Same | a later report overlapping the first one (re-import de-duplication) |
| `fidelity_history_2025.csv` | Fidelity single-account history, 2024–2025: BOM, two empty lines, ` ($)` names with `Cash Balance ($)`, leading spaces, disclaimer block | buys, sell with fees, dividend and reinvestment, core (`SPAXX`) reinvestment skipped, `FOREIGN TAX PAID`, `DISTRIBUTION` split with a market-value Amount, EFT in/out, option and unknown action as warnings |
| `fidelity_all_accounts_2026q1.csv` | Fidelity all-accounts history, January–April 2026: `Account` and `Account Number`, Price before Quantity | identical buys in one account and in two accounts, reverse split with a CUSIP leg, `IN LIEU OF`, `JOURNALED` pair, ACAT shares without cost |
| `fidelity_history_2026q3.csv` | Fidelity single-account history from July 2026: header on line 1, empty line after it, `MM-DD-YYYY`, `""` cells | `as of YYYY-MM-DD` event date, settlement date |
| `schwab_2020.csv` | Schwab up to ~2021: banner, trailing commas, `Transactions Total` footer, CRLF | buy, `as of` bank interest, MoneyLink; amounts sum to the footer total |
| `schwab_2024.csv` | Schwab since late 2023: header only, newest first | stock split `as of`, sell with fees, reverse split with the old CUSIP, reinvestment pair, foreign tax, journaled shares, interest, MoneyLink, option and unknown action as warnings |
| `vanguard_ofx.csv` | Vanguard `OfxDownload.csv`: holdings section, three empty lines, transactions with `Commissions and Fees`, ISO dates, trailing commas, rows grouped by holding | buy, dividend and reinvestment, sell with fee, VMFXX sweeps and zero-share reinvestment skipped, reverse split pair, cash in lieu, deposit, withdrawal, option and unknown type as warnings |
| `positions_fidelity.csv` | Fidelity `Portfolio_Positions` 2026: BOM, 16 columns, data rows with a trailing comma, footer | `SPAXX**` core, `--` cost basis, CUSIP bond, `BRK.B`, `Pending Activity` |
| `positions_schwab.csv` | Schwab positions 2024–2025: banner, `""` row, `Qty (Quantity)`, quoted `$1,234.00` | `Incomplete` cost basis, option row, `Cash & Cash Investments`, `Account Total` |
| `generic_mapped.csv` | No broker: a custom export for the column-mapping path, `DD/MM/YYYY` | positive buy totals, unknown action, missing quantity, unreadable date |

The files were written by a short throwaway script so quoting, BOMs and line endings are exact. Edit them in a text editor, not a spreadsheet (a spreadsheet re-save changes quoting and dates). Expected line numbers in `../formats.rs` were cross-checked with Python's `csv` module.
