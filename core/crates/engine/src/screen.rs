//! Generic screen model. Every function screen is built in Rust as a list of
//! blocks the Swift side renders with one terminal renderer. Values are
//! typed with a display format; Swift only formats, never computes.

use serde::{Deserialize, Serialize};

/// How to display a numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// Fixed decimals.
    Number { decimals: u8 },
    /// Price with instrument decimals.
    Price { decimals: u8 },
    /// Value is already in percent units (12.3 means 12.3%).
    Percent { decimals: u8 },
    /// Signed change, colored up/down.
    Change { decimals: u8 },
    /// Signed percent change, colored up/down.
    ChangePercent { decimals: u8 },
    /// Abbreviated: 1.23K / 4.56M / 7.89B / 1.02T.
    Large { decimals: u8 },
    /// Whole number with thousands separators.
    Integer,
    /// Value is Unix nanos; date only.
    Date,
    /// Value is Unix nanos; date and time.
    DateTime,
    /// Value is Unix nanos; time of day (exchange-agnostic, local).
    Time,
    /// Text cell.
    Text,
}

/// Semantic style; the Swift theme maps these to colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Style {
    #[default]
    Normal,
    /// White header/emphasis text.
    Emphasis,
    Up,
    Down,
    Muted,
    /// Highlighted editable cell.
    Input,
    Warning,
    /// Security/function link (selectable).
    Link,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    pub value: Option<f64>,
    pub text: Option<String>,
    pub format: Format,
    pub style: Style,
}

impl Field {
    #[must_use]
    pub fn num(label: &str, value: Option<f64>, format: Format) -> Self {
        Self { label: label.into(), value, text: None, format, style: Style::Normal }
    }

    #[must_use]
    pub fn text(label: &str, text: impl Into<String>) -> Self {
        Self { label: label.into(), value: None, text: Some(text.into()), format: Format::Text, style: Style::Normal }
    }

    #[must_use]
    pub fn opt_text(label: &str, text: Option<impl Into<String>>) -> Self {
        Self { label: label.into(), value: None, text: text.map(Into::into), format: Format::Text, style: Style::Normal }
    }

    #[must_use]
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

/// Hot-row field a live column binds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveField {
    Last,
    Bid,
    Ask,
    NetChange,
    PctChange,
    Volume,
    Open,
    High,
    Low,
    PrevClose,
    Time,
    BidSize,
    AskSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub title: String,
    pub format: Format,
    pub align: Align,
    /// Width in characters (monospace grid).
    pub width: u16,
    /// If set, the cell shows the live value for the row's security.
    pub live: Option<LiveField>,
}

impl Column {
    #[must_use]
    pub fn text(title: &str, width: u16) -> Self {
        Self { title: title.into(), format: Format::Text, align: Align::Left, width, live: None }
    }

    #[must_use]
    pub fn num(title: &str, format: Format, width: u16) -> Self {
        Self { title: title.into(), format, align: Align::Right, width, live: None }
    }

    #[must_use]
    pub fn live(mut self, f: LiveField) -> Self {
        self.live = Some(f);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Cell {
    pub value: Option<f64>,
    pub text: Option<String>,
    pub style: Style,
}

impl Cell {
    #[must_use]
    pub fn num(v: Option<f64>) -> Self {
        Self { value: v.filter(|x| x.is_finite()), text: None, style: Style::Normal }
    }

    #[must_use]
    pub fn text(t: impl Into<String>) -> Self {
        Self { value: None, text: Some(t.into()), style: Style::Normal }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Numeric cell styled up/down by sign.
    #[must_use]
    pub fn signed(v: Option<f64>) -> Self {
        let style = match v {
            Some(x) if x > 0.0 => Style::Up,
            Some(x) if x < 0.0 => Style::Down,
            _ => Style::Normal,
        };
        Self { value: v.filter(|x| x.is_finite()), text: None, style }
    }

    #[must_use]
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Indentation for hierarchical tables (financial statements).
    pub depth: u8,
    /// Security for live columns and for linking.
    pub security: Option<String>,
    /// What selecting the row does.
    pub action: Option<Action>,
    /// Header/subtotal rows render emphasized.
    pub emphasis: bool,
}

impl Row {
    #[must_use]
    pub fn new(cells: Vec<Cell>) -> Self {
        Self { cells, depth: 0, security: None, action: None, emphasis: false }
    }

    #[must_use]
    pub fn security(mut self, key: &str) -> Self {
        self.security = Some(key.into());
        self
    }

    #[must_use]
    pub fn action(mut self, a: Action) -> Self {
        self.action = Some(a);
        self
    }

    #[must_use]
    pub fn depth(mut self, d: u8) -> Self {
        self.depth = d;
        self
    }

    #[must_use]
    pub fn emphasis(mut self) -> Self {
        self.emphasis = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub title: Option<String>,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Rows per page; the renderer pages with PAGE FWD/BACK.
    pub page_size: Option<u32>,
    /// Number the rows (`1)`, `2)` …) for `<n> <GO>` selection.
    pub numbered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputKind {
    Number,
    Text,
    Date,
    /// One of `options`.
    Choice,
}

/// Editable cell. Changing it reloads the screen with `args[id] = value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub id: String,
    pub label: String,
    pub value: String,
    pub kind: InputKind,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Chart embedded in a screen; Swift fetches the series with
/// `chart_data` using this spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub security: String,
    pub interval: String,
    pub range: String,
    pub style: ChartStyle,
    pub indicators: Vec<String>,
    pub height_rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartStyle {
    Line,
    Candles,
    Bars,
    Mountain,
}

/// Small inline chart with data embedded (payoff diagrams, smiles, curves).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XyChart {
    pub title: String,
    pub x_label: String,
    pub y_label: String,
    pub series: Vec<XySeries>,
    /// Vertical reference line (e.g. spot).
    pub x_marker: Option<f64>,
    pub height_rows: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XySeries {
    pub name: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub style: Style,
    pub bars: bool,
}

/// Heat map (correlation matrix, vol surface).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeatMap {
    pub title: String,
    pub row_labels: Vec<String>,
    pub col_labels: Vec<String>,
    /// Row-major; NaN = missing.
    pub values: Vec<f64>,
    pub format: Format,
    /// Diverging around zero (correlation) or sequential (vol).
    pub diverging: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Fields { title: Option<String>, columns: u8, fields: Vec<Field> },
    Table(Table),
    Text { title: Option<String>, body: String },
    Inputs { title: Option<String>, inputs: Vec<Input> },
    Notice { level: NoticeLevel, text: String },
    Chart(ChartSpec),
    Xy(XyChart),
    Heat(HeatMap),
    Diff { title: String, lines: Vec<DiffLine> },
}

/// A navigation target: function + security + arguments. Rows and menu
/// items carry these so selection never round-trips through the parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub function: String,
    pub security: Option<String>,
    pub args: Vec<(String, String)>,
}

impl Action {
    #[must_use]
    pub fn new(function: &str, security: Option<&str>) -> Self {
        Self { function: function.into(), security: security.map(Into::into), args: Vec::new() }
    }

    #[must_use]
    pub fn arg(mut self, k: &str, v: impl Into<String>) -> Self {
        self.args.retain(|(key, _)| key != k);
        self.args.push((k.into(), v.into()));
        self
    }

    /// Copy of `self` with every `(k, v)` from `args` applied.
    #[must_use]
    pub fn with_args(mut self, args: &[(String, String)]) -> Self {
        for (k, v) in args {
            self = self.arg(k, v.clone());
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    pub number: u32,
    pub label: String,
    pub action: Action,
    pub selected: bool,
}

/// Where a screen's data came from, shown as badges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBadge {
    pub provider: String,
    /// `RT`, `DLY 15`, `EOD`, `MOCK`.
    pub delay: String,
    /// `CONSOL`, `IEX`, `OFFICIAL`, `MOCK` …
    pub source: String,
    pub synthetic: bool,
    pub as_of: i64,
    pub attribution: Option<String>,
}

impl SourceBadge {
    #[must_use]
    pub fn from_provenance(p: &meridian_types::Provenance) -> Self {
        Self {
            provider: p.provider.to_string(),
            delay: p.delay.badge(),
            source: p.source.label(),
            synthetic: p.synthetic,
            as_of: p.as_of,
            attribution: p.attribution.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScreenStatus {
    Ok,
    /// Capability missing for a stated reason (no provider, licensing, …).
    NotAvailable { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub function: String,
    pub title: String,
    pub security: Option<String>,
    /// Numbered actions shown in the red function bar.
    pub menu: Vec<MenuItem>,
    pub blocks: Vec<Block>,
    pub status: ScreenStatus,
    pub sources: Vec<SourceBadge>,
    /// Arguments that produced this screen (echoed for reloads).
    pub args: Vec<(String, String)>,
    /// Re-request the screen this often (monitors without live columns).
    pub refresh_ms: Option<u32>,
}

impl Screen {
    #[must_use]
    pub fn new(function: &str, title: impl Into<String>, security: Option<String>) -> Self {
        Self {
            function: function.into(),
            title: title.into(),
            security,
            menu: Vec::new(),
            blocks: Vec::new(),
            status: ScreenStatus::Ok,
            sources: Vec::new(),
            args: Vec::new(),
            refresh_ms: None,
        }
    }

    #[must_use]
    pub fn not_available(function: &str, title: &str, security: Option<String>, reason: impl Into<String>) -> Self {
        let mut s = Self::new(function, title, security);
        s.status = ScreenStatus::NotAvailable { reason: reason.into() };
        s
    }

    pub fn push(&mut self, b: Block) {
        self.blocks.push(b);
    }

    pub fn source(&mut self, p: &meridian_types::Provenance) {
        let b = SourceBadge::from_provenance(p);
        if !self.sources.contains(&b) {
            self.sources.push(b);
        }
    }

    pub fn menu_item(&mut self, label: &str, action: Action, selected: bool) {
        let number = u32::try_from(self.menu.len() + 1).unwrap_or(u32::MAX);
        self.menu.push(MenuItem { number, label: label.into(), action, selected });
    }
}
