# meridian-provider-mock

Deterministic, synthetic data for every `Provider` capability, so UI work can proceed without API keys.
No network access, no external docs, no rate limits. Capabilities declare `DataDelay::Synthetic`,
`FeedSource::Synthetic`, and `CachePolicy::NoStore` (mock data is never persisted).

Everything returned is labeled: `Provenance::synthetic`, `Instrument::is_synthetic`, the `SYNTHETIC` quote flag,
`[MOCK] ` headlines, `SYNTHETIC FILING — MOCK DATA` on every filing document, `[MOCK TRANSCRIPT]` in transcripts,
and a `SYNTHETIC` note on economic series. Tickers and company names are real; every number is generated.
Firm, fund, analyst, executive, and insider names are invented.

## Determinism

Every series is a function of `MockConfig::seed`, an FNV-1a hash of the security key, and the injected clock.
Price paths start at a fixed epoch (1999-01-04) and are pinned to each symbol's reference price on a fixed anchor
date (2026-01-02), so history does not change as the clock advances. Daily OHLCV and intraday paths use per-day
counter-based generators, so a bar never depends on the requested range.

## Models

- **Daily prices**: GBM; shocks mix a shared factor (GARCH(1,1) with crash jumps; Europe/Asia load on the US
  factor) with idiosyncratic GARCH(1,1) and Poisson jumps. Currencies, rate futures, commodities, and TLT
  mean-revert. VIX is driven by the US factor's conditional variance. SPY/QQQ/IWM/DIA, ES1/NQ1/YM1, and GLD track
  their underlyings. Volume depends on recent shock size. Calendars: NYSE holidays with observance rules and
  unscheduled closures; weekdays for FX and non-US indices; every day for crypto. Splits (public events) are applied
  for `Adjustment::None`; `SplitsAndDividends` back-adjusts for the synthetic dividends.
- **Intraday**: one-minute Brownian bridge from the daily open to close with a U-shaped volatility profile, mapped
  monotonically onto the daily high/low, so minute bars aggregate exactly to the daily bar. U-shaped volume with
  auction spikes. Sessions in exchange-local time with US/EU/AU DST rules implemented here. FX pairs are ratios of
  currency paths, so crosses triangulate.
- **Quotes / stream**: snapshot from the session so far. The stream is one tokio task with a 10 ms tick, Poisson
  arrivals per symbol, factor-correlated random walks, tick rounding, and bid/ask bounce. It ticks around the clock.
- **Options**: Black-Scholes on an SSVI surface (`eta * (1 + |rho|) <= 2`, no butterfly arbitrage), monthly plus
  weekly expiries, strike grid by price level. Greeks are `Computed { model: "mock-bs" }`.
- **Fundamentals**: quarterly simulation from 2003 with sector templates; the balance sheet rolls forward from the
  cash-flow statement, all amounts rounded to whole reporting units, so `total_assets == total_liabilities +
  total_equity` exactly and annual flows equal the sum of their quarters.
- **Macro**: policy rate stepped at rule-based FOMC dates, Nelson-Siegel curve (DGS2/10/30 and the UST curve come
  from the same model), monthly activity/price indices with recession and inflation regimes, and a calendar whose
  actuals and release dates come from the same series.

## Performance (release, M5 Pro)

`cargo bench -p meridian-provider-mock`: 20 years of daily bars ≈ 0.44 ms; one month of one-minute bars ≈ 0.51 ms;
cached quote ≈ 13 µs. The 2,000-symbol stream test sustains ~6,000 events/s at 3 updates/s/symbol.
