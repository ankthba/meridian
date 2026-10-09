# Meridian design system: Instrument

Meridian's interface is original. It does not imitate any other terminal's look: no black-and-amber palette, no colored function bars, no yellow-key chrome. The reference for the direction is the approved mockup (`docs/design/instrument.png`).

## Principles

1. **Typography does the work.** Hierarchy comes from size, weight and color, not from boxes, cards or fills.
2. **One neutral palette, one signal pair.** Everything is graphite and bone; green and red mean only up and down.
3. **Numbers line up.** Every number is monospaced with tabular figures and right-aligned on its decimal.
4. **Keyboard first.** The command bar is the front door; every pane shows its shortcut.
5. **Nothing generic.** No gradients (except a chart's faint area fill), no glass or blur, no purple, no emoji, no pill badges on every value, no drop shadows except on popovers.

## Tokens

| Token | Value | Use |
|---|---|---|
| `bg` | `#121212` | Window and pane background |
| `header` | `#181818` | Pane headers, command bar background |
| `raised` | `#1B1B1B` | Popovers, menus |
| `selected` | `#1E1E1E` | Selected row |
| `hover` | `#2A2A2A` | Highlighted option in a popover |
| `line` | `#2A2A2A` | Pane borders, header rules |
| `hairline` | `#1F1F1F` | Row separators, chart grid |
| `text` | `#E8E6E1` | Primary text, the chart's price line |
| `text2` | `#BEBBB3` | Secondary text |
| `muted` | `#8A877F` | Labels, axes, timestamps, sources |
| `up` | `#5BC98A` | Positive change |
| `down` | `#EF6F66` | Negative change |
| `upTint` / `downTint` | `up`/`down` at 10% | Diff lines, tick flash |
| `warn` | `#E0B25C` | NOT AVAILABLE reasons, notices |

There is no accent hue. Emphasis is `text` against `text2`/`muted`; the selected tab gets a 1.5 pt `text` underline.

## Type

- **Interface text:** SF Pro (system font). Body 13, secondary 12, small 11.
- **Data:** SF Mono (`.monospaced` system design) with tabular figures, 12.5 in tables and 13 in fields. Prices in headers 15 semibold.
- **Pane labels:** SF Pro 10.5 bold, uppercase, tracking 1.4, `muted`.
- **Wordmark:** "MERIDIAN", SF Pro 11 bold, tracking 3.

## Layout

- **Top bar** (40 pt): window controls, wordmark, then right-aligned feed status (`● Live`, the sources, the clock in mono).
- **Command bar** (44 pt): `›` prompt and input at 15 pt. Completions open as a popover (560 pt wide, `raised`, 8 pt corner radius, the only shadow in the app), grouped under small uppercase headings: SECURITY, ON <SYMBOL>, FUNCTIONS, RECENT. Each option shows its name, a description in `text2`, and on the right its mnemonic or value in mono `muted`. ↑↓ move, Return opens, Tab completes.
- **Panes:** a 2 × 2 grid with 1 pt `line` borders and no gaps, rounding or cards. Column split 1.45 : 1.
- **Pane header** (34 pt, `header`): the uppercase label (function name), the security in semibold, then the screen's sections as text tabs (`muted`; selected `text` with an underline), and the shortcut (`⌘1`…`⌘4`) right-aligned in mono `muted`.
- **Pane content:** 10–12 pt padding.

## Components

- **Tables:** header row 11 pt `muted`, no fill. Rows 5 pt vertical padding with a `hairline` bottom border. The selected row gets `selected` fill. Numbers right-aligned mono. Changes are colored text with a sign (`+1.49`, `−0.19`); no pills.
- **Fields (key/value):** label `muted` on the left, value mono `text` on the right, `hairline` separator.
- **Tick flash:** the changed cell's background flashes `upTint`/`downTint` for 400 ms. No other animation.
- **Charts:** price line in `text` at 1.8 pt on `bg`, grid in `hairline`, axes in `muted` 11 pt. The last-price tag is a `text` box with `bg` text. Studies are 1.2 pt `muted` lines, labeled directly at their right end (no legend). Volume bars in `#232323`. The crosshair is dotted `muted` with a `raised` tooltip.
- **Diffs:** removed lines `downTint` background with `#F2A29C` text; added lines `upTint` with `#9BE0B6`; context in `muted`. Mono 11.5.
- **Notices:** NOT AVAILABLE and other notices are `warn` text with no box.
- **Inputs inside screens:** underlined text fields (1 pt `line` underline, `text` on focus); no filled boxes.
- **Sources:** each pane shows its sources as `muted` 11 pt text (e.g. `iex · rt`), never as colored badges.

## Accessibility

VoiceOver reaches every screen without any change to how it looks.

- **Speak what is drawn.** Accessibility text comes from the strings the views draw (`TerminalFormatter`). `SpokenText` only turns symbols into words (`+1.74%` → "up 1.74 percent", `3.39T` → "3.39 trillion", `% Chg` → "percent change"); it never formats numbers itself.
- **Custom-drawn views are real elements.** The grid is a table of rows, cells and column headers. A row reads as one sentence: numbers carry their column name, words and dates read alone. VO-Space on a row does what a click does. The price chart is an image whose label summarizes the visible bars (first and last close, change, high, low). Swift Charts get axis names and a summary, heat maps read one line per row.
- **Nothing on the hot path.** Grid elements are made the first time an assistive app asks, hold only row and column indices, and read values when asked. Drawing and live ticks do no accessibility work. Notifications go out only for table shape and selection changes, only after VoiceOver has looked at that grid, never per tick.
- **Structure.** Each pane is a container named "Pane 1, Today"; pane labels and section titles are headings; section tabs are buttons with a selected state; text that acts on a tap gets the button trait and an action; decoration (`›`, menu chevrons) is hidden; the feed dot's color is also said in words; inputs take their visible label.
- **Announce only what focus can't follow:** the highlighted suggestion while focus stays in the command field, and the chart's new window after keyboard zoom or pan. Only when VoiceOver is running.
- **Reduce Motion** turns off the tick flash, the only animation. Values and colors still update.

## Words

- Sentence case everywhere except pane labels and the wordmark.
- Name things plainly: "Filings", "What changed", "Coming up". Mnemonics appear only as hints.
- Command hints teach plain language: `aapl`, `aapl 5y`, `aapl filings`, `earnings this week`.
