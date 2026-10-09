# Privacy

Meridian has no accounts, no analytics and no telemetry. It runs on your Mac and talks only to the services below. This page lists every one.

## What leaves your Mac

| When | Where | What is sent |
|---|---|---|
| You use a data source you set up | That source: Alpaca, Finnhub, FRED, SEC EDGAR | Your request (e.g. a ticker or a date range) and your key for that source. SEC EDGAR receives the contact name and email you entered, which its fair-access policy requires in every request. |
| Always, without a key | Coinbase, Kraken, the ECB via Frankfurter, the U.S. Treasury, press-release feeds (GlobeNewswire, PR Newswire, Business Wire) | Standard requests for public data. |
| You ask ASK a question, or ask for a filing summary | Anthropic, with your own API key | Your question and the data Meridian retrieved to answer it, or the filing text. Sources whose terms don't allow AI use are never sent. Nothing is sent unless you ask. |
| At launch and once a day (Settings → General → Check for updates; on by default) | api.github.com | One request for the latest release. No identifiers. |
| You choose Help → Report an Issue | GitHub, in your browser | A new issue form with the app and macOS version filled in. You decide what to post; crash reports are only shown to you in Finder, never sent. |

Nothing else is sent anywhere. Broker CSV files you import are read on your Mac and never uploaded.

## What stays on your Mac

- **Keys** are stored only in the macOS Keychain (login keychain, service `meridian.provider.<source>`).
- **Your portfolio, watchlists, alerts, settings and layout** are in `~/Library/Application Support/Meridian/app.sqlite`.
- **Cached market data** is in `~/Library/Application Support/Meridian/` (`market.duckdb` and `cache/`), kept according to each source's terms. Settings → Storage deletes it per source.
- **Crash reports** from the core are in `~/Library/Application Support/Meridian/crashes/`; macOS keeps its own in `~/Library/Logs/DiagnosticReports/`.

To remove everything, see [Uninstalling](README.md#uninstalling).

## The data you fetch

Market data belongs to its providers, and each provider's terms apply to what you fetch with your own keys. Meridian is for personal use and doesn't redistribute data.
