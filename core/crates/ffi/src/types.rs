//! FFI records mirroring the engine's screen model and other low-frequency
//! payloads. Conversions are mechanical; keep them that way.

use meridian_engine::screen as s;

#[derive(Debug, Clone, uniffi::Record)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

fn kv(v: &[(String, String)]) -> Vec<KeyValue> {
    v.iter().map(|(k, v)| KeyValue { key: k.clone(), value: v.clone() }).collect()
}

pub(crate) fn from_kv(v: &[KeyValue]) -> Vec<(String, String)> {
    v.iter().map(|x| (x.key.clone(), x.value.clone())).collect()
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum FormatFfi {
    Number { decimals: u8 },
    Price { decimals: u8 },
    Percent { decimals: u8 },
    Change { decimals: u8 },
    ChangePercent { decimals: u8 },
    Large { decimals: u8 },
    Integer,
    Date,
    DateTime,
    Time,
    Text,
}

impl From<s::Format> for FormatFfi {
    fn from(f: s::Format) -> Self {
        match f {
            s::Format::Number { decimals } => Self::Number { decimals },
            s::Format::Price { decimals } => Self::Price { decimals },
            s::Format::Percent { decimals } => Self::Percent { decimals },
            s::Format::Change { decimals } => Self::Change { decimals },
            s::Format::ChangePercent { decimals } => Self::ChangePercent { decimals },
            s::Format::Large { decimals } => Self::Large { decimals },
            s::Format::Integer => Self::Integer,
            s::Format::Date => Self::Date,
            s::Format::DateTime => Self::DateTime,
            s::Format::Time => Self::Time,
            s::Format::Text => Self::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StyleFfi {
    Normal,
    Emphasis,
    Up,
    Down,
    Muted,
    Input,
    Warning,
    Link,
}

impl From<s::Style> for StyleFfi {
    fn from(v: s::Style) -> Self {
        match v {
            s::Style::Normal => Self::Normal,
            s::Style::Emphasis => Self::Emphasis,
            s::Style::Up => Self::Up,
            s::Style::Down => Self::Down,
            s::Style::Muted => Self::Muted,
            s::Style::Input => Self::Input,
            s::Style::Warning => Self::Warning,
            s::Style::Link => Self::Link,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FieldFfi {
    pub label: String,
    pub value: Option<f64>,
    pub text: Option<String>,
    pub format: FormatFfi,
    pub style: StyleFfi,
}

impl From<s::Field> for FieldFfi {
    fn from(f: s::Field) -> Self {
        Self { label: f.label, value: f.value, text: f.text, format: f.format.into(), style: f.style.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LiveFieldFfi {
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

impl From<s::LiveField> for LiveFieldFfi {
    fn from(v: s::LiveField) -> Self {
        match v {
            s::LiveField::Last => Self::Last,
            s::LiveField::Bid => Self::Bid,
            s::LiveField::Ask => Self::Ask,
            s::LiveField::NetChange => Self::NetChange,
            s::LiveField::PctChange => Self::PctChange,
            s::LiveField::Volume => Self::Volume,
            s::LiveField::Open => Self::Open,
            s::LiveField::High => Self::High,
            s::LiveField::Low => Self::Low,
            s::LiveField::PrevClose => Self::PrevClose,
            s::LiveField::Time => Self::Time,
            s::LiveField::BidSize => Self::BidSize,
            s::LiveField::AskSize => Self::AskSize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AlignFfi {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ColumnFfi {
    pub title: String,
    pub format: FormatFfi,
    pub align: AlignFfi,
    pub width: u16,
    pub live: Option<LiveFieldFfi>,
}

impl From<s::Column> for ColumnFfi {
    fn from(c: s::Column) -> Self {
        Self {
            title: c.title,
            format: c.format.into(),
            align: match c.align {
                s::Align::Left => AlignFfi::Left,
                s::Align::Right => AlignFfi::Right,
                s::Align::Center => AlignFfi::Center,
            },
            width: c.width,
            live: c.live.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CellFfi {
    pub value: Option<f64>,
    pub text: Option<String>,
    pub style: StyleFfi,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ActionFfi {
    pub function: String,
    pub security: Option<String>,
    pub args: Vec<KeyValue>,
}

impl From<s::Action> for ActionFfi {
    fn from(a: s::Action) -> Self {
        Self { function: a.function, security: a.security, args: kv(&a.args) }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RowFfi {
    pub cells: Vec<CellFfi>,
    pub depth: u8,
    pub security: Option<String>,
    pub action: Option<ActionFfi>,
    pub emphasis: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TableFfi {
    pub title: Option<String>,
    pub columns: Vec<ColumnFfi>,
    pub rows: Vec<RowFfi>,
    pub page_size: Option<u32>,
    pub numbered: bool,
}

impl From<s::Table> for TableFfi {
    fn from(t: s::Table) -> Self {
        Self {
            title: t.title,
            columns: t.columns.into_iter().map(Into::into).collect(),
            rows: t
                .rows
                .into_iter()
                .map(|r| RowFfi {
                    cells: r.cells.into_iter().map(|c| CellFfi { value: c.value, text: c.text, style: c.style.into() }).collect(),
                    depth: r.depth,
                    security: r.security,
                    action: r.action.map(Into::into),
                    emphasis: r.emphasis,
                })
                .collect(),
            page_size: t.page_size,
            numbered: t.numbered,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum InputKindFfi {
    Number,
    Text,
    Date,
    Choice,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct InputFfi {
    pub id: String,
    pub label: String,
    pub value: String,
    pub kind: InputKindFfi,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NoticeLevelFfi {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DiffKindFfi {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct DiffLineFfi {
    pub kind: DiffKindFfi,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChartStyleFfi {
    Line,
    Candles,
    Bars,
    Mountain,
}

impl From<s::ChartStyle> for ChartStyleFfi {
    fn from(v: s::ChartStyle) -> Self {
        match v {
            s::ChartStyle::Line => Self::Line,
            s::ChartStyle::Candles => Self::Candles,
            s::ChartStyle::Bars => Self::Bars,
            s::ChartStyle::Mountain => Self::Mountain,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChartSpecFfi {
    pub security: String,
    pub interval: String,
    pub range: String,
    pub style: ChartStyleFfi,
    pub indicators: Vec<String>,
    pub height_rows: u16,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct XySeriesFfi {
    pub name: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub style: StyleFfi,
    pub bars: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct XyChartFfi {
    pub title: String,
    pub x_label: String,
    pub y_label: String,
    pub series: Vec<XySeriesFfi>,
    pub x_marker: Option<f64>,
    pub height_rows: u16,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct HeatMapFfi {
    pub title: String,
    pub row_labels: Vec<String>,
    pub col_labels: Vec<String>,
    pub values: Vec<f64>,
    pub format: FormatFfi,
    pub diverging: bool,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum BlockFfi {
    Fields { title: Option<String>, columns: u8, fields: Vec<FieldFfi> },
    Table { table: TableFfi },
    Text { title: Option<String>, body: String },
    Inputs { title: Option<String>, inputs: Vec<InputFfi> },
    Notice { level: NoticeLevelFfi, text: String },
    Chart { spec: ChartSpecFfi },
    Xy { chart: XyChartFfi },
    Heat { map: HeatMapFfi },
    Diff { title: String, lines: Vec<DiffLineFfi> },
}

impl From<s::Block> for BlockFfi {
    fn from(b: s::Block) -> Self {
        match b {
            s::Block::Fields { title, columns, fields } => Self::Fields { title, columns, fields: fields.into_iter().map(Into::into).collect() },
            s::Block::Table(t) => Self::Table { table: t.into() },
            s::Block::Text { title, body } => Self::Text { title, body },
            s::Block::Inputs { title, inputs } => Self::Inputs {
                title,
                inputs: inputs
                    .into_iter()
                    .map(|i| InputFfi {
                        id: i.id,
                        label: i.label,
                        value: i.value,
                        kind: match i.kind {
                            s::InputKind::Number => InputKindFfi::Number,
                            s::InputKind::Text => InputKindFfi::Text,
                            s::InputKind::Date => InputKindFfi::Date,
                            s::InputKind::Choice => InputKindFfi::Choice,
                        },
                        options: i.options,
                    })
                    .collect(),
            },
            s::Block::Notice { level, text } => Self::Notice {
                level: match level {
                    s::NoticeLevel::Info => NoticeLevelFfi::Info,
                    s::NoticeLevel::Warning => NoticeLevelFfi::Warning,
                    s::NoticeLevel::Error => NoticeLevelFfi::Error,
                },
                text,
            },
            s::Block::Chart(c) => Self::Chart {
                spec: ChartSpecFfi {
                    security: c.security,
                    interval: c.interval,
                    range: c.range,
                    style: c.style.into(),
                    indicators: c.indicators,
                    height_rows: c.height_rows,
                },
            },
            s::Block::Xy(x) => Self::Xy {
                chart: XyChartFfi {
                    title: x.title,
                    x_label: x.x_label,
                    y_label: x.y_label,
                    series: x
                        .series
                        .into_iter()
                        .map(|se| XySeriesFfi { name: se.name, x: se.x, y: se.y, style: se.style.into(), bars: se.bars })
                        .collect(),
                    x_marker: x.x_marker,
                    height_rows: x.height_rows,
                },
            },
            s::Block::Heat(h) => Self::Heat {
                map: HeatMapFfi {
                    title: h.title,
                    row_labels: h.row_labels,
                    col_labels: h.col_labels,
                    values: h.values,
                    format: h.format.into(),
                    diverging: h.diverging,
                },
            },
            s::Block::Diff { title, lines } => Self::Diff {
                title,
                lines: lines
                    .into_iter()
                    .map(|l| DiffLineFfi {
                        kind: match l.kind {
                            s::DiffKind::Same => DiffKindFfi::Same,
                            s::DiffKind::Added => DiffKindFfi::Added,
                            s::DiffKind::Removed => DiffKindFfi::Removed,
                        },
                        text: l.text,
                    })
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MenuItemFfi {
    pub number: u32,
    pub label: String,
    pub action: ActionFfi,
    pub selected: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SourceBadgeFfi {
    pub provider: String,
    pub delay: String,
    pub source: String,
    pub synthetic: bool,
    pub as_of: i64,
    pub attribution: Option<String>,
}

impl From<s::SourceBadge> for SourceBadgeFfi {
    fn from(b: s::SourceBadge) -> Self {
        Self { provider: b.provider, delay: b.delay, source: b.source, synthetic: b.synthetic, as_of: b.as_of, attribution: b.attribution }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum ScreenStatusFfi {
    Ok,
    NotAvailable { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScreenFfi {
    pub function: String,
    pub title: String,
    pub security: Option<String>,
    pub menu: Vec<MenuItemFfi>,
    pub blocks: Vec<BlockFfi>,
    pub status: ScreenStatusFfi,
    pub sources: Vec<SourceBadgeFfi>,
    pub args: Vec<KeyValue>,
    pub refresh_ms: Option<u32>,
}

impl From<s::Screen> for ScreenFfi {
    fn from(x: s::Screen) -> Self {
        Self {
            function: x.function,
            title: x.title,
            security: x.security,
            menu: x
                .menu
                .into_iter()
                .map(|m| MenuItemFfi { number: m.number, label: m.label, action: m.action.into(), selected: m.selected })
                .collect(),
            blocks: x.blocks.into_iter().map(Into::into).collect(),
            status: match x.status {
                s::ScreenStatus::Ok => ScreenStatusFfi::Ok,
                s::ScreenStatus::NotAvailable { reason } => ScreenStatusFfi::NotAvailable { reason },
                s::ScreenStatus::Error { message } => ScreenStatusFfi::Error { message },
            },
            sources: x.sources.into_iter().map(Into::into).collect(),
            args: kv(&x.args),
            refresh_ms: x.refresh_ms,
        }
    }
}

// --- command line ---------------------------------------------------------

#[derive(Debug, Clone, uniffi::Enum)]
pub enum ParsedCommandFfi {
    Empty,
    Security { security: String, function: Option<String>, args: Vec<String> },
    Function { function: String, args: Vec<String> },
    MenuItem { number: u32 },
    Search { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SuggestionKindFfi {
    Security,
    Function,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SuggestionFfi {
    pub kind: SuggestionKindFfi,
    pub display: String,
    pub detail: String,
    pub completion: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FunctionInfoFfi {
    pub mnemonic: String,
    pub title: String,
    pub description: String,
    pub needs_security: bool,
    pub category: String,
}

// --- data sources -----------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct CapabilityRowFfi {
    pub capability: String,
    pub asset_classes: String,
    pub delay: String,
    pub source: String,
    pub history: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct DataSourceFfi {
    pub provider: String,
    pub capabilities: Vec<CapabilityRowFfi>,
    pub terms_note: String,
    pub docs_url: String,
    pub attribution: Option<String>,
    pub requires_credentials: bool,
    pub cache_policy: String,
    pub ai_policy: String,
    pub connected: Option<bool>,
}

// --- events -----------------------------------------------------------------

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CoreEventFfi {
    FeedStatus { provider: String, connected: bool, message: String },
    AlertFired { rule_id: i64, security: String, message: String, at: i64 },
    UniverseLoaded { instruments: u64 },
    Status { message: String },
}

// --- hot rows -----------------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct LayoutFieldFfi {
    pub name: String,
    pub offset: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PollResultFfi {
    pub seq: u64,
    /// Packed rows; layout from `hot_row_layout`.
    pub rows: Vec<u8>,
}

// --- watchlists & workspaces ---------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct WatchlistFfi {
    pub id: i64,
    pub name: String,
    pub securities: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct WorkspaceSummaryFfi {
    pub id: String,
    pub name: String,
    pub updated_at: i64,
}
