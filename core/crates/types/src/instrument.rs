use serde::{Deserialize, Serialize};

use crate::key::SecurityKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetClass {
    Equity,
    Etf,
    Index,
    Option,
    Future,
    Crypto,
    Fx,
    Bond,
    Rate,
    Economic,
}

impl AssetClass {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            AssetClass::Equity => "Common Stock",
            AssetClass::Etf => "ETF",
            AssetClass::Index => "Index",
            AssetClass::Option => "Option",
            AssetClass::Future => "Future",
            AssetClass::Crypto => "Crypto",
            AssetClass::Fx => "Currency",
            AssetClass::Bond => "Bond",
            AssetClass::Rate => "Rate",
            AssetClass::Economic => "Economic",
        }
    }
}

/// Reference data for one security.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instrument {
    pub key: SecurityKey,
    pub name: String,
    pub asset_class: AssetClass,
    pub currency: String,
    /// ISO 10383 MIC of the primary listing, if known.
    pub exchange_mic: Option<String>,
    pub exchange_name: Option<String>,
    pub figi: Option<String>,
    /// SEC Central Index Key for US issuers.
    pub cik: Option<u64>,
    pub tick_size: f64,
    /// Decimal places used to display prices.
    pub price_decimals: u8,
    pub multiplier: f64,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub country: Option<String>,
    pub is_synthetic: bool,
}

impl Instrument {
    /// Minimal instrument with sensible defaults; providers fill in more.
    #[must_use]
    pub fn basic(key: SecurityKey, name: impl Into<String>, asset_class: AssetClass, currency: &str) -> Self {
        Self {
            key,
            name: name.into(),
            asset_class,
            currency: currency.to_owned(),
            exchange_mic: None,
            exchange_name: None,
            figi: None,
            cik: None,
            tick_size: 0.01,
            price_decimals: 2,
            multiplier: 1.0,
            sector: None,
            industry: None,
            country: None,
            is_synthetic: false,
        }
    }
}

/// Extended descriptive data for DES.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CompanyProfile {
    pub description: Option<String>,
    pub website: Option<String>,
    pub headquarters: Option<String>,
    pub employees: Option<u64>,
    pub ceo: Option<String>,
    pub founded: Option<String>,
    pub fiscal_year_end: Option<String>,
    pub ipo_date: Option<String>,
    pub shares_outstanding: Option<f64>,
    pub market_cap: Option<f64>,
    pub sic_code: Option<String>,
    pub sic_description: Option<String>,
}
