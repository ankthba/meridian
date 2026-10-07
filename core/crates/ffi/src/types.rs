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
    /// Bar charts: label for x value `i`, shown instead of numeric ticks.
    pub x_categories: Option<Vec<String>>,
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
                    x_categories: x.x_categories,
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
    /// ASK asked to show a function in another panel.
    Show { function: String, security: Option<String>, args: Vec<KeyValue> },
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

// --- charts -------------------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChartStudyFfi {
    pub name: String,
    /// 0 = price overlay; 1.. = lower panes.
    pub pane: u8,
    /// f32 little-endian; overlays relative to `origin`, panes absolute.
    pub values: Vec<u8>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChartDataFfi {
    pub security: String,
    pub interval: String,
    pub count: u32,
    pub origin: f64,
    /// `[n: u32][origin: f64]` then ts i64[n], open/high/low/close f32[n]
    /// (relative to origin), volume f32[n]; little-endian.
    pub bars: Vec<u8>,
    pub studies: Vec<ChartStudyFfi>,
    pub price_decimals: u8,
    pub sources: Vec<SourceBadgeFfi>,
    pub stale: bool,
}

// --- ASK ------------------------------------------------------------------

#[derive(Debug, Clone, uniffi::Record)]
pub struct AskToolFfi {
    pub tool_use_id: String,
    pub tool: String,
    pub input_json: String,
    pub sql: Option<String>,
    /// `provider (MOCK|RT…)` per source.
    pub sources: Vec<String>,
    pub rows: Option<u64>,
    pub duration_ms: u64,
    pub is_error: bool,
}

impl From<&meridian_ask::ToolAudit> for AskToolFfi {
    fn from(a: &meridian_ask::ToolAudit) -> Self {
        Self {
            tool_use_id: a.tool_use_id.clone(),
            tool: a.tool.clone(),
            input_json: a.input_json.clone(),
            sql: a.sql.clone(),
            sources: a.sources.iter().map(|s| format!("{}{}", s.provider, if s.synthetic { " (MOCK)" } else { "" })).collect(),
            rows: a.rows,
            duration_ms: a.duration_ms,
            is_error: a.is_error,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NumberCheckFfi {
    pub text: String,
    /// UTF-16 offsets into `answer` (for NSString/AttributedString ranges).
    pub start: u32,
    pub end: u32,
    pub verified: bool,
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AskTurnFfi {
    pub question: String,
    pub answer: String,
    pub tools: Vec<AskToolFfi>,
    pub checks: Vec<NumberCheckFfi>,
    pub model: String,
    pub served_by_fallback: bool,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
}

impl From<&meridian_ask::AskTurn> for AskTurnFfi {
    fn from(t: &meridian_ask::AskTurn) -> Self {
        let utf16 = |byte: usize| -> u32 {
            let b = byte.min(t.answer.len());
            let b = (0..=b).rev().find(|i| t.answer.is_char_boundary(*i)).unwrap_or(0);
            t.answer[..b].encode_utf16().count() as u32
        };
        Self {
            question: t.question.clone(),
            answer: t.answer.clone(),
            tools: t.tool_calls.iter().map(Into::into).collect(),
            checks: t
                .number_checks
                .iter()
                .map(|c| NumberCheckFfi {
                    text: c.text.clone(),
                    start: utf16(c.span.0),
                    end: utf16(c.span.1),
                    verified: c.verified,
                    tool_use_id: c.matched_tool_call.clone(),
                })
                .collect(),
            model: t.model.clone(),
            served_by_fallback: t.served_by_fallback,
            stop_reason: t.stop_reason.clone(),
            error: t.error.clone(),
            input_tokens: t.usage.input_tokens,
            output_tokens: t.usage.output_tokens,
            cache_read_tokens: t.usage.cache_read_input_tokens,
        }
    }
}

// --- broker CSV import ---------------------------------------------------------

/// Most preview rows and warnings sent to Swift (FFI rule: no unbounded
/// record lists). Totals are always exact.
const PREVIEW_ROWS: usize = 50;
const MAX_WARNINGS: usize = 200;

/// What an imported row does (`meridian_types::TransactionKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ImportKindFfi {
    Buy,
    Sell,
    Dividend,
    ReinvestedDividend,
    Interest,
    Fee,
    Split,
    TransferIn,
    TransferOut,
    Deposit,
    Withdrawal,
    Other,
}

impl From<meridian_types::TransactionKind> for ImportKindFfi {
    fn from(k: meridian_types::TransactionKind) -> Self {
        use meridian_types::TransactionKind as K;
        match k {
            K::Buy => Self::Buy,
            K::Sell => Self::Sell,
            K::Dividend => Self::Dividend,
            K::ReinvestedDividend => Self::ReinvestedDividend,
            K::Interest => Self::Interest,
            K::Fee => Self::Fee,
            K::Split => Self::Split,
            K::TransferIn => Self::TransferIn,
            K::TransferOut => Self::TransferOut,
            K::Deposit => Self::Deposit,
            K::Withdrawal => Self::Withdrawal,
            K::Other => Self::Other,
        }
    }
}

/// The user's column mapping for a file whose format wasn't recognized.
/// Columns are 0-based positions in `ImportPreviewFfi.headers`. Without a
/// date column the file is read as a positions snapshot (symbol and
/// quantity required). Start from `ImportPreviewFfi.suggested_mapping`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportMappingFfi {
    /// 1-based line of the header row (`ImportPreviewFfi.header_line`).
    pub header_line: u32,
    pub trade_date: Option<u32>,
    pub settle_date: Option<u32>,
    pub symbol: Option<u32>,
    /// Transaction type column (`Buy`, `Dividend` …).
    pub action: Option<u32>,
    pub quantity: Option<u32>,
    pub price: Option<u32>,
    pub amount: Option<u32>,
    pub fees: Option<u32>,
    pub currency: Option<u32>,
    pub description: Option<u32>,
    /// Total cost basis (positions).
    pub cost_basis: Option<u32>,
    /// Average cost per share (positions).
    pub average_cost: Option<u32>,
    /// Currency when there is no currency column; empty means USD.
    pub default_currency: String,
    /// Dates are DD/MM/YYYY rather than MM/DD/YYYY.
    pub day_first: bool,
}

impl From<ImportMappingFfi> for meridian_import::ColumnMapping {
    fn from(m: ImportMappingFfi) -> Self {
        let c = |v: Option<u32>| v.map(|x| x as usize);
        Self {
            header_line: m.header_line,
            trade_date: c(m.trade_date),
            settle_date: c(m.settle_date),
            symbol: c(m.symbol),
            action: c(m.action),
            quantity: c(m.quantity),
            price: c(m.price),
            amount: c(m.amount),
            fees: c(m.fees),
            currency: c(m.currency),
            description: c(m.description),
            cost_basis: c(m.cost_basis),
            average_cost: c(m.average_cost),
            default_currency: m.default_currency,
            day_first: m.day_first,
        }
    }
}

impl From<&meridian_import::ColumnMapping> for ImportMappingFfi {
    fn from(m: &meridian_import::ColumnMapping) -> Self {
        let c = |v: Option<usize>| v.map(|x| u32::try_from(x).unwrap_or(u32::MAX));
        Self {
            header_line: m.header_line,
            trade_date: c(m.trade_date),
            settle_date: c(m.settle_date),
            symbol: c(m.symbol),
            action: c(m.action),
            quantity: c(m.quantity),
            price: c(m.price),
            amount: c(m.amount),
            fees: c(m.fees),
            currency: c(m.currency),
            description: c(m.description),
            cost_basis: c(m.cost_basis),
            average_cost: c(m.average_cost),
            default_currency: m.default_currency.clone(),
            day_first: m.day_first,
        }
    }
}

/// One row as it would be imported. Signs: `quantity` is the change in
/// shares (+ in, − out), `amount` the cash flow (+ in, − out), `fees` a
/// positive cost.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportRowFfi {
    /// 1-based line in the file.
    pub line: u32,
    /// `YYYY-MM-DD`.
    pub trade_date: String,
    pub settle_date: Option<String>,
    /// Ticker as the broker wrote it.
    pub symbol: Option<String>,
    /// The security key it is stored under (`BRK/B US Equity`).
    pub security: Option<String>,
    pub kind: ImportKindFfi,
    /// Sentence-case label for `kind`.
    pub kind_label: String,
    pub quantity: Option<f64>,
    pub price: Option<f64>,
    pub amount: Option<f64>,
    pub fees: Option<f64>,
    /// Total cost basis (positions snapshots).
    pub cost_basis: Option<f64>,
    pub currency: String,
    pub description: String,
    /// The broker's own action or code text.
    pub action: String,
    pub account: Option<String>,
}

/// A row that was not imported, or a note about the file (line 0 = whole
/// file).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportWarningFfi {
    pub line: u32,
    pub message: String,
    /// The row's text, truncated.
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportKindCountFfi {
    pub kind: ImportKindFfi,
    pub label: String,
    pub count: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportPreviewFfi {
    /// `robinhood`, `fidelity`, `schwab`, `vanguard`, `positions` or
    /// `mapped`; `None` when not recognized (offer the mapping UI).
    pub format: Option<String>,
    /// Display name ("Robinhood account activity" …, or "Not recognized").
    pub format_name: String,
    pub recognized: bool,
    /// Holdings snapshot: rows are recorded as transfers in on the import day.
    pub snapshot: bool,
    /// 1-based line of the header row.
    pub header_line: u32,
    pub headers: Vec<String>,
    /// The first rows (at most 50), in trade-date order.
    pub rows: Vec<ImportRowFfi>,
    /// Rows that would be imported.
    pub total_rows: u32,
    /// Rows by kind (non-zero only).
    pub counts: Vec<ImportKindCountFfi>,
    /// At most 200; `warning_count` is the total.
    pub warnings: Vec<ImportWarningFfi>,
    pub warning_count: u32,
    /// Date range of the rows, `YYYY-MM-DD`.
    pub first_date: Option<String>,
    pub last_date: Option<String>,
    /// Starting point for the column-mapping UI (the mapping used, when one
    /// was given).
    pub suggested_mapping: ImportMappingFfi,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ImportResultFfi {
    pub portfolio_id: i64,
    pub portfolio_name: String,
    /// Format id, as in `ImportPreviewFfi.format`.
    pub format: String,
    pub imported: u32,
    /// Rows already in the portfolio from an earlier import.
    pub duplicates: u32,
    /// At most 200; `warning_count` is the total.
    pub warnings: Vec<ImportWarningFfi>,
    pub warning_count: u32,
}

fn import_warnings(w: &[meridian_import::ImportWarning]) -> (Vec<ImportWarningFfi>, u32) {
    let list = w.iter().take(MAX_WARNINGS).map(|w| ImportWarningFfi { line: w.line, message: w.message.clone(), text: w.text.clone() }).collect();
    (list, u32::try_from(w.len()).unwrap_or(u32::MAX))
}

fn import_row(t: &meridian_import::ImportedTx) -> ImportRowFfi {
    ImportRowFfi {
        line: t.line,
        trade_date: t.trade_date.format("%Y-%m-%d").to_string(),
        settle_date: t.settle_date.map(|d| d.format("%Y-%m-%d").to_string()),
        symbol: t.symbol.clone(),
        security: t.symbol.as_deref().and_then(meridian_engine::import::security_for),
        kind: t.kind.into(),
        kind_label: t.kind.label().into(),
        quantity: t.quantity,
        price: t.price,
        amount: t.amount,
        fees: t.fees,
        cost_basis: t.cost_basis,
        currency: t.currency.clone(),
        description: t.description.clone(),
        action: t.action.clone(),
        account: t.account.clone(),
    }
}

pub(crate) fn import_preview(p: &meridian_engine::import::ImportPreview) -> ImportPreviewFfi {
    let n = |x: usize| u32::try_from(x).unwrap_or(u32::MAX);
    let suggested_mapping = (&p.suggested_mapping).into();
    if let Some(parsed) = &p.parsed {
        let (warnings, warning_count) = import_warnings(&parsed.warnings);
        let date = |t: Option<&meridian_import::ImportedTx>| t.map(|t| t.trade_date.format("%Y-%m-%d").to_string());
        ImportPreviewFfi {
            format: Some(parsed.format.id().into()),
            format_name: parsed.format.name().into(),
            recognized: true,
            snapshot: parsed.snapshot,
            header_line: parsed.header_line,
            headers: parsed.headers.clone(),
            rows: parsed.transactions.iter().take(PREVIEW_ROWS).map(import_row).collect(),
            total_rows: n(parsed.transactions.len()),
            counts: parsed.counts().into_iter().map(|(k, c)| ImportKindCountFfi { kind: k.into(), label: k.label().into(), count: n(c) }).collect(),
            warnings,
            warning_count,
            first_date: date(parsed.transactions.first()),
            last_date: date(parsed.transactions.last()),
            suggested_mapping,
        }
    } else {
        let (warnings, warning_count) = import_warnings(&p.warnings);
        ImportPreviewFfi {
            format: None,
            format_name: "Not recognized".into(),
            recognized: false,
            snapshot: false,
            header_line: p.header_line,
            headers: p.headers.clone(),
            rows: Vec::new(),
            total_rows: 0,
            counts: Vec::new(),
            warnings,
            warning_count,
            first_date: None,
            last_date: None,
            suggested_mapping,
        }
    }
}

pub(crate) fn import_result(o: &meridian_engine::import::ImportOutcome) -> ImportResultFfi {
    let (warnings, warning_count) = import_warnings(&o.warnings);
    ImportResultFfi {
        portfolio_id: o.portfolio_id,
        portfolio_name: o.portfolio_name.clone(),
        format: o.format.id().into(),
        imported: u32::try_from(o.imported).unwrap_or(u32::MAX),
        duplicates: u32::try_from(o.duplicates).unwrap_or(u32::MAX),
        warnings,
        warning_count,
    }
}
