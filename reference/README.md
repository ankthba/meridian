# Reference screenshots

These screenshots are the visual spec. Each function screen is built to match its references; differences may only be in data, not layout, typography, or color.

## Layout

```
reference/
  <FUNCTION>/
    01-<state>.png      e.g. 01-default.png, 02-indicators.png, 03-menu-open.png
    notes.md            optional: what each image shows, window size, anything cropped
  compare/
    <FUNCTION>-<state>.png   generated: reference | ours | diff (committed)
```

- Use full-resolution, uncropped captures of a single panel where possible, and note the original window size in `notes.md`. Color extraction needs uncompressed PNGs: no JPEG, and no screenshots of screenshots.
- More states per screen means better fidelity: default, menus open, scrolled, error/empty.

## Inventory (2026-10-05)

**No reference screenshots exist yet.** The directory was empty when the project started. Every screen below is **MISSING**.

### Shell (needed for Phase 2)

| Folder | What to capture | Status |
|---|---|---|
| `SHELL/` | Full 4-panel window at login; one panel close-up showing toolbar, command line, red function bar | MISSING |
| `SHELL/` | Command line mid-typing with autocomplete dropdown (security matches and function matches) | MISSING |
| `MENU/` | A security's main menu after `AAPL US <EQUITY> <GO>` (numbered items) | MISSING |
| `HELP/` | Function help page; help search results | MISSING |
| `BLP/` | Launchpad with several components, a monitor, linked groups (group letters visible), docked components | MISSING |
| `SECF/` | Security finder | MISSING |

### Functions

| Phase | Folder | Status |
|---|---|---|
| 3 | `DES/` | MISSING |
| 3 | `GP/` (with indicators, drawing tools, settings menu) | MISSING |
| 3 | `GIP/` | MISSING |
| 3 | `HP/` | MISSING |
| 3 | `W/` and/or Launchpad Monitor (pending decision) | MISSING |
| 3 | `MOST/` (optional) | MISSING |
| 4 | `N/`, `CN/`, `TOP/` (list view and story view) | MISSING |
| 4 | `FA/` (income, balance, cash flow tabs) | MISSING |
| 4 | `EE/`, `ERN/`, `ANR/`, `HDS/`, `DVD/` | MISSING |
| 4 | `CF/` (filing list and filing view) | MISSING |
| 5 | `OMON/`, `OVDV/`, `OVME/` | MISSING |
| 6 | `EQS/`, `RV/`, `CORR/`, `PORT/`, `BTST/`, `ALRT/` | MISSING |
| 7 | `WEI/`, `ECO/`, `FXC/`, `CRYP/` | MISSING |
| 8 | `ASK/` | No incumbent equivalent to copy. It will be styled from the shared visual system (SHELL + a list/detail screen such as CN), so it doesn't need its own reference. |

Our own screens with no incumbent counterpart (keyboard overlay, settings/API keys, data-mode banner) also follow the shared visual system and need no reference.
