//! The mock universe: real tickers and names with synthetic parameters.
//!
//! Reference prices and share counts are rough, round levels from around the
//! anchor date so screens look plausible. They seed the generators; every
//! number the provider returns is synthetic and labeled as such.

use std::collections::HashMap;

use chrono::NaiveDate;
use meridian_types::{AssetClass, ExerciseStyle, Instrument, MarketSector, SecurityKey};

use crate::cal::{Calendar, Session, Tz, ymd};
use crate::hash::{Cell, key_hash, tag};

/// Date on which each symbol's price path is pinned to its reference price.
/// Fixed (not clock-relative) so history stays stable as the clock moves.
pub(crate) const ANCHOR: (i32, u32, u32) = (2026, 1, 2);

pub(crate) fn anchor_date() -> NaiveDate {
    ymd(ANCHOR.0, ANCHOR.1, ANCHOR.2)
}

/// GICS-style sectors used for fundamentals templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Sector {
    Tech,
    Comm,
    Disc,
    Staples,
    Health,
    Fin,
    Ind,
    Energy,
    Util,
    RealEstate,
    Materials,
}

impl Sector {
    pub(crate) const ALL: [Sector; 11] = [
        Sector::Tech,
        Sector::Comm,
        Sector::Disc,
        Sector::Staples,
        Sector::Health,
        Sector::Fin,
        Sector::Ind,
        Sector::Energy,
        Sector::Util,
        Sector::RealEstate,
        Sector::Materials,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Sector::Tech => "Information Technology",
            Sector::Comm => "Communication Services",
            Sector::Disc => "Consumer Discretionary",
            Sector::Staples => "Consumer Staples",
            Sector::Health => "Health Care",
            Sector::Fin => "Financials",
            Sector::Ind => "Industrials",
            Sector::Energy => "Energy",
            Sector::Util => "Utilities",
            Sector::RealEstate => "Real Estate",
            Sector::Materials => "Materials",
        }
    }

    fn filler_industry(self) -> &'static str {
        match self {
            Sector::Tech => "Application Software",
            Sector::Comm => "Interactive Media & Services",
            Sector::Disc => "Specialty Retail",
            Sector::Staples => "Packaged Foods & Meats",
            Sector::Health => "Health Care Equipment",
            Sector::Fin => "Regional Banks",
            Sector::Ind => "Industrial Machinery",
            Sector::Energy => "Oil & Gas Exploration & Production",
            Sector::Util => "Electric Utilities",
            Sector::RealEstate => "Industrial REITs",
            Sector::Materials => "Specialty Chemicals",
        }
    }
}

/// Common factors. Each symbol loads on one factor with correlation `rho`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Factor {
    UsEq = 0,
    Europe = 1,
    Asia = 2,
    Usd = 3,
    Crypto = 4,
    Commodity = 5,
    Rates = 6,
}

pub(crate) const N_FACTORS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Equity,
    Etf,
    Index,
    Fx,
    Crypto,
    Future,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Model {
    /// Own GBM path with GARCH volatility, factor loading, and jumps.
    Gbm,
    /// Scaled copy of another symbol's bars (ETF on an index, index future).
    Tracks { under: usize, ratio: f64 },
    /// Ratio of two currency paths (indices into [`CCYS`]).
    FxPair { base: usize, quote: usize },
    /// Volatility index driven by the US equity factor's variance.
    VolIndex,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DivSpec {
    /// Yield at the anchor price, percent.
    pub yield_pct: f64,
    pub start_year: i32,
    pub per_year: u32,
    /// Annual dividend growth.
    pub growth: f64,
    /// First ex-month of the yearly cycle (1..=12).
    pub first_month: u32,
    pub ex_day: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OptSpec {
    pub style: ExerciseStyle,
    pub root: String,
    pub weekly_root: String,
    pub weeklies: bool,
    /// Continuous dividend yield used for pricing.
    pub q: f64,
    /// Scales volume and open interest.
    pub activity: f64,
    pub index_like: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Params {
    pub kind: Kind,
    pub model: Model,
    pub ref_price: f64,
    pub mu: f64,
    pub sigma: f64,
    pub rho: f64,
    pub factor: Factor,
    pub jumps_per_year: f64,
    /// Jump size standard deviation in units of the daily sigma.
    pub jump_sd: f64,
    pub jump_mean: f64,
    /// Mean-reversion speed of the log price (per year; 0 = pure GBM).
    pub kappa: f64,
    /// Average daily volume (shares, contracts, coins).
    pub adv: f64,
    pub listed: NaiveDate,
    pub calendar: Calendar,
    pub session: Session,
    pub periods_per_year: f64,
    /// Typical full bid/ask spread in basis points (at least one tick).
    pub spread_bps: f64,
    /// Size unit for book sizes and trades.
    pub lot: f64,
    /// Decimal places of volume (0 for shares/contracts).
    pub vol_decimals: i32,
    /// Shares outstanding (equities/ETFs) or circulating supply (crypto).
    pub shares: f64,
    pub sector: Option<Sector>,
    pub fye_month: u32,
    pub div: Option<DivSpec>,
    pub options: Option<OptSpec>,
    /// Whether the instrument has a bid/ask book (indices don't).
    pub has_book: bool,
    pub region: Option<&'static str>,
    pub filler: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sym {
    pub inst: Instrument,
    pub p: Params,
    pub hash: u64,
}

impl Sym {
    pub(crate) fn key(&self) -> &SecurityKey {
        &self.inst.key
    }

    pub(crate) fn tick(&self) -> f64 {
        self.inst.tick_size
    }

    pub(crate) fn ticker(&self) -> &str {
        &self.inst.key.symbol
    }

    /// Market capitalization at the anchor price, USD.
    pub(crate) fn anchor_mcap(&self) -> f64 {
        self.p.ref_price * self.p.shares
    }
}

// ---------------------------------------------------------------------------
// Equities
// ---------------------------------------------------------------------------

struct E {
    t: &'static str,
    n: &'static str,
    s: Sector,
    ind: &'static str,
    nas: bool,
    cik: u64,
    px: f64,
    sh: f64,
    vol: f64,
    mu: f64,
    fye: u32,
    dy: f64,
    ds: i32,
    listed: u32,
}

#[allow(clippy::too_many_arguments)]
const fn e(
    t: &'static str,
    n: &'static str,
    s: Sector,
    ind: &'static str,
    nas: bool,
    cik: u64,
    px: f64,
    sh: f64,
    vol: f64,
    mu: f64,
    fye: u32,
    dy: f64,
    ds: i32,
    listed: u32,
) -> E {
    E { t, n, s, ind, nas, cik, px, sh, vol, mu, fye, dy, ds, listed }
}

use Sector::{Comm, Disc, Energy, Fin, Health, Ind, Materials, RealEstate, Staples, Tech, Util};

const Q: bool = true; // Nasdaq
const N: bool = false; // NYSE

/// ticker, name, sector, industry, nasdaq?, CIK (0 = unknown), ref price,
/// shares (bn), vol, drift, FYE month, dividend yield %, dividend start year,
/// listing date (yyyymmdd, 0 = before the path epoch).
#[rustfmt::skip]
const EQUITIES: &[E] = &[
    e("AAPL", "Apple Inc.", Tech, "Technology Hardware, Storage & Peripherals", Q, 320193, 255.0, 14.8, 0.27, 0.20, 9, 0.40, 2012, 0),
    e("MSFT", "Microsoft Corporation", Tech, "Systems Software", Q, 789019, 480.0, 7.43, 0.24, 0.16, 6, 0.75, 2003, 0),
    e("NVDA", "NVIDIA Corporation", Tech, "Semiconductors", Q, 1045810, 185.0, 24.3, 0.50, 0.33, 1, 0.02, 2012, 0),
    e("AVGO", "Broadcom Inc.", Tech, "Semiconductors", Q, 1730168, 345.0, 4.72, 0.40, 0.25, 10, 0.70, 2011, 20090806),
    e("AMD", "Advanced Micro Devices, Inc.", Tech, "Semiconductors", Q, 2488, 215.0, 1.62, 0.50, 0.18, 12, 0.0, 0, 0),
    e("INTC", "Intel Corporation", Tech, "Semiconductors", Q, 50863, 37.0, 4.7, 0.40, 0.02, 12, 0.0, 0, 0),
    e("CSCO", "Cisco Systems, Inc.", Tech, "Communications Equipment", Q, 858877, 77.0, 3.95, 0.22, 0.07, 7, 2.1, 2011, 0),
    e("ORCL", "Oracle Corporation", Tech, "Systems Software", N, 1341439, 195.0, 2.85, 0.40, 0.15, 5, 1.0, 2009, 0),
    e("CRM", "Salesforce, Inc.", Tech, "Application Software", N, 1108524, 260.0, 0.955, 0.32, 0.12, 1, 0.6, 2024, 20040623),
    e("ADBE", "Adobe Inc.", Tech, "Application Software", Q, 796343, 350.0, 0.42, 0.30, 0.10, 11, 0.0, 0, 0),
    e("IBM", "International Business Machines Corporation", Tech, "IT Consulting & Other Services", N, 51143, 300.0, 0.935, 0.22, 0.06, 12, 2.2, 2000, 0),
    e("QCOM", "QUALCOMM Incorporated", Tech, "Semiconductors", Q, 804328, 170.0, 1.08, 0.32, 0.08, 9, 2.1, 2003, 0),
    e("TXN", "Texas Instruments Incorporated", Tech, "Semiconductors", Q, 97476, 175.0, 0.908, 0.27, 0.07, 12, 3.1, 2000, 0),
    e("MU", "Micron Technology, Inc.", Tech, "Semiconductors", Q, 723125, 285.0, 1.12, 0.50, 0.15, 8, 0.2, 2021, 0),
    e("NOW", "ServiceNow, Inc.", Tech, "Systems Software", N, 1373715, 870.0, 0.207, 0.35, 0.20, 12, 0.0, 0, 20120629),
    e("INTU", "Intuit Inc.", Tech, "Application Software", Q, 896878, 660.0, 0.279, 0.30, 0.14, 7, 0.65, 2011, 0),
    e("AMAT", "Applied Materials, Inc.", Tech, "Semiconductor Materials & Equipment", Q, 6951, 255.0, 0.795, 0.38, 0.14, 10, 0.7, 2005, 0),
    e("LRCX", "Lam Research Corporation", Tech, "Semiconductor Materials & Equipment", Q, 707549, 170.0, 1.26, 0.42, 0.18, 6, 0.6, 2014, 0),
    e("KLAC", "KLA Corporation", Tech, "Semiconductor Materials & Equipment", Q, 319201, 1200.0, 0.132, 0.36, 0.16, 6, 0.65, 2005, 0),
    e("ADI", "Analog Devices, Inc.", Tech, "Semiconductors", Q, 6281, 270.0, 0.49, 0.28, 0.10, 10, 1.5, 2004, 0),
    e("PANW", "Palo Alto Networks, Inc.", Tech, "Systems Software", Q, 1327567, 185.0, 0.68, 0.36, 0.22, 7, 0.0, 0, 20120720),
    e("SNOW", "Snowflake Inc.", Tech, "Application Software", N, 1640147, 220.0, 0.34, 0.55, 0.10, 1, 0.0, 0, 20200916),
    e("PLTR", "Palantir Technologies Inc.", Tech, "Application Software", Q, 1321655, 180.0, 2.37, 0.65, 0.30, 12, 0.0, 0, 20200930),
    e("GOOGL", "Alphabet Inc. Class A", Comm, "Interactive Media & Services", Q, 1652044, 300.0, 12.1, 0.30, 0.18, 12, 0.3, 2024, 20040819),
    e("META", "Meta Platforms, Inc. Class A", Comm, "Interactive Media & Services", Q, 1326801, 650.0, 2.52, 0.38, 0.22, 12, 0.33, 2024, 20120518),
    e("NFLX", "Netflix, Inc.", Comm, "Movies & Entertainment", Q, 1065280, 95.0, 4.23, 0.40, 0.25, 12, 0.0, 0, 20020523),
    e("DIS", "The Walt Disney Company", Comm, "Movies & Entertainment", N, 1744489, 112.0, 1.8, 0.28, 0.06, 9, 0.9, 2000, 0),
    e("T", "AT&T Inc.", Comm, "Integrated Telecommunication Services", N, 732717, 25.0, 7.1, 0.20, 0.03, 12, 4.4, 2000, 0),
    e("VZ", "Verizon Communications Inc.", Comm, "Integrated Telecommunication Services", N, 732712, 41.0, 4.22, 0.18, 0.03, 12, 6.6, 2000, 0),
    e("CMCSA", "Comcast Corporation Class A", Comm, "Cable & Satellite", Q, 1166691, 30.0, 3.7, 0.25, 0.05, 12, 4.0, 2008, 0),
    e("CHTR", "Charter Communications, Inc. Class A", Comm, "Cable & Satellite", Q, 1091667, 210.0, 0.137, 0.35, 0.08, 12, 0.0, 0, 20100920),
    e("TMUS", "T-Mobile US, Inc.", Comm, "Wireless Telecommunication Services", Q, 1283699, 200.0, 1.12, 0.25, 0.14, 12, 1.7, 2023, 20070419),
    e("AMZN", "Amazon.com, Inc.", Disc, "Broadline Retail", Q, 1018724, 225.0, 10.7, 0.32, 0.22, 12, 0.0, 0, 0),
    e("TSLA", "Tesla, Inc.", Disc, "Automobile Manufacturers", Q, 1318605, 430.0, 3.32, 0.60, 0.30, 12, 0.0, 0, 20100629),
    e("HD", "The Home Depot, Inc.", Disc, "Home Improvement Retail", N, 354950, 350.0, 0.994, 0.24, 0.10, 1, 2.6, 2000, 0),
    e("MCD", "McDonald's Corporation", Disc, "Restaurants", N, 63908, 305.0, 0.714, 0.18, 0.09, 12, 2.4, 2000, 0),
    e("NKE", "NIKE, Inc. Class B", Disc, "Footwear", N, 320187, 63.0, 1.48, 0.32, 0.05, 5, 2.5, 2000, 0),
    e("SBUX", "Starbucks Corporation", Disc, "Restaurants", Q, 829224, 85.0, 1.14, 0.30, 0.08, 9, 2.9, 2010, 0),
    e("LOW", "Lowe's Companies, Inc.", Disc, "Home Improvement Retail", N, 60667, 245.0, 0.56, 0.27, 0.12, 1, 2.0, 2000, 0),
    e("BKNG", "Booking Holdings Inc.", Disc, "Hotels, Resorts & Cruise Lines", Q, 1075531, 5350.0, 0.0324, 0.30, 0.16, 12, 0.75, 2024, 0),
    e("MAR", "Marriott International, Inc. Class A", Disc, "Hotels, Resorts & Cruise Lines", Q, 1048286, 310.0, 0.272, 0.28, 0.11, 12, 0.85, 2000, 0),
    e("F", "Ford Motor Company", Disc, "Automobile Manufacturers", N, 37996, 13.0, 3.98, 0.35, 0.02, 12, 4.6, 2000, 0),
    e("GM", "General Motors Company", Disc, "Automobile Manufacturers", N, 1467858, 81.0, 0.95, 0.33, 0.06, 12, 0.8, 2014, 20101118),
    e("ABNB", "Airbnb, Inc. Class A", Disc, "Hotels, Resorts & Cruise Lines", Q, 1559720, 135.0, 0.61, 0.40, 0.08, 12, 0.0, 0, 20201210),
    e("WMT", "Walmart Inc.", Staples, "Consumer Staples Merchandise Retail", Q, 104169, 110.0, 7.97, 0.18, 0.11, 1, 0.85, 2000, 0),
    e("COST", "Costco Wholesale Corporation", Staples, "Consumer Staples Merchandise Retail", Q, 909832, 870.0, 0.443, 0.22, 0.15, 8, 0.6, 2004, 0),
    e("KO", "The Coca-Cola Company", Staples, "Soft Drinks & Non-alcoholic Beverages", N, 21344, 70.0, 4.3, 0.15, 0.06, 12, 2.9, 2000, 0),
    e("PEP", "PepsiCo, Inc.", Staples, "Soft Drinks & Non-alcoholic Beverages", Q, 77476, 145.0, 1.37, 0.16, 0.06, 12, 3.9, 2000, 0),
    e("PG", "The Procter & Gamble Company", Staples, "Household Products", N, 80424, 145.0, 2.34, 0.16, 0.06, 6, 2.9, 2000, 0),
    e("CL", "Colgate-Palmolive Company", Staples, "Household Products", N, 21665, 79.0, 0.807, 0.17, 0.05, 12, 2.6, 2000, 0),
    e("MO", "Altria Group, Inc.", Staples, "Tobacco", N, 764180, 58.0, 1.68, 0.20, 0.07, 12, 7.0, 2000, 0),
    e("PM", "Philip Morris International Inc.", Staples, "Tobacco", N, 1413329, 145.0, 1.56, 0.22, 0.09, 12, 3.7, 2008, 20080317),
    e("MDLZ", "Mondelez International, Inc. Class A", Staples, "Packaged Foods & Meats", Q, 1103982, 54.0, 1.29, 0.18, 0.05, 12, 3.5, 2001, 0),
    e("KHC", "The Kraft Heinz Company", Staples, "Packaged Foods & Meats", Q, 1637459, 24.0, 1.18, 0.24, -0.02, 12, 6.5, 2015, 20150706),
    e("EL", "The Estee Lauder Companies Inc. Class A", Staples, "Personal Care Products", N, 1001250, 105.0, 0.36, 0.40, 0.03, 6, 1.3, 2000, 0),
    e("TGT", "Target Corporation", Staples, "Consumer Staples Merchandise Retail", N, 27419, 98.0, 0.455, 0.30, 0.05, 1, 4.6, 2000, 0),
    e("JNJ", "Johnson & Johnson", Health, "Pharmaceuticals", N, 200406, 205.0, 2.41, 0.17, 0.07, 12, 2.5, 2000, 0),
    e("PFE", "Pfizer Inc.", Health, "Pharmaceuticals", N, 78003, 25.0, 5.68, 0.22, 0.0, 12, 6.8, 2000, 0),
    e("UNH", "UnitedHealth Group Incorporated", Health, "Managed Health Care", N, 731766, 330.0, 0.906, 0.30, 0.10, 12, 2.6, 2000, 0),
    e("ABBV", "AbbVie Inc.", Health, "Biotechnology", N, 1551152, 225.0, 1.77, 0.24, 0.12, 12, 3.0, 2013, 20130102),
    e("MRK", "Merck & Co., Inc.", Health, "Pharmaceuticals", N, 310158, 105.0, 2.5, 0.22, 0.06, 12, 3.4, 2000, 0),
    e("LLY", "Eli Lilly and Company", Health, "Pharmaceuticals", N, 59478, 1070.0, 0.947, 0.30, 0.20, 12, 0.6, 2000, 0),
    e("ABT", "Abbott Laboratories", Health, "Health Care Equipment", N, 1800, 125.0, 1.74, 0.20, 0.09, 12, 1.9, 2000, 0),
    e("TMO", "Thermo Fisher Scientific Inc.", Health, "Life Sciences Tools & Services", N, 97745, 580.0, 0.377, 0.24, 0.12, 12, 0.3, 2012, 0),
    e("CVS", "CVS Health Corporation", Health, "Health Care Services", N, 64803, 80.0, 1.27, 0.30, 0.04, 12, 3.4, 2000, 0),
    e("MDT", "Medtronic plc", Health, "Health Care Equipment", N, 1613103, 97.0, 1.28, 0.20, 0.04, 4, 2.9, 2000, 0),
    e("AMGN", "Amgen Inc.", Health, "Biotechnology", Q, 318154, 330.0, 0.538, 0.22, 0.08, 12, 2.9, 2011, 0),
    e("GILD", "Gilead Sciences, Inc.", Health, "Biotechnology", Q, 882095, 123.0, 1.24, 0.24, 0.08, 12, 2.6, 2015, 0),
    e("BMY", "Bristol-Myers Squibb Company", Health, "Pharmaceuticals", N, 14272, 54.0, 2.04, 0.24, 0.02, 12, 4.6, 2000, 0),
    e("ISRG", "Intuitive Surgical, Inc.", Health, "Health Care Equipment", Q, 1035267, 565.0, 0.358, 0.32, 0.20, 12, 0.0, 0, 0),
    e("VRTX", "Vertex Pharmaceuticals Incorporated", Health, "Biotechnology", Q, 875320, 450.0, 0.256, 0.30, 0.14, 12, 0.0, 0, 0),
    e("REGN", "Regeneron Pharmaceuticals, Inc.", Health, "Biotechnology", Q, 872589, 770.0, 0.105, 0.32, 0.13, 12, 0.45, 2025, 0),
    e("ZTS", "Zoetis Inc. Class A", Health, "Pharmaceuticals", N, 1555280, 125.0, 0.443, 0.25, 0.08, 12, 1.6, 2013, 20130201),
    e("HCA", "HCA Healthcare, Inc.", Health, "Health Care Facilities", N, 860730, 465.0, 0.234, 0.30, 0.14, 12, 0.6, 2018, 20110310),
    e("ELV", "Elevance Health, Inc.", Health, "Managed Health Care", N, 1156039, 350.0, 0.225, 0.28, 0.09, 12, 1.9, 2011, 0),
    e("JPM", "JPMorgan Chase & Co.", Fin, "Diversified Banks", N, 19617, 315.0, 2.75, 0.25, 0.12, 12, 1.9, 2000, 0),
    e("BAC", "Bank of America Corporation", Fin, "Diversified Banks", N, 70858, 53.0, 7.4, 0.30, 0.07, 12, 2.1, 2000, 0),
    e("WFC", "Wells Fargo & Company", Fin, "Diversified Banks", N, 72971, 93.0, 3.2, 0.28, 0.07, 12, 2.0, 2000, 0),
    e("C", "Citigroup Inc.", Fin, "Diversified Banks", N, 831001, 117.0, 1.8, 0.32, 0.03, 12, 2.3, 2000, 0),
    e("GS", "The Goldman Sachs Group, Inc.", Fin, "Investment Banking & Brokerage", N, 886982, 880.0, 0.305, 0.30, 0.10, 12, 1.8, 2000, 0),
    e("MS", "Morgan Stanley", Fin, "Investment Banking & Brokerage", N, 895421, 178.0, 1.59, 0.30, 0.10, 12, 2.6, 2000, 0),
    e("V", "Visa Inc. Class A", Fin, "Transaction & Payment Processing Services", N, 1403161, 350.0, 1.93, 0.22, 0.17, 9, 0.7, 2008, 20080319),
    e("MA", "Mastercard Incorporated Class A", Fin, "Transaction & Payment Processing Services", N, 1141391, 570.0, 0.9, 0.24, 0.18, 12, 0.55, 2006, 20060525),
    e("AXP", "American Express Company", Fin, "Consumer Finance", N, 4962, 370.0, 0.69, 0.28, 0.11, 12, 0.9, 2000, 0),
    e("SCHW", "The Charles Schwab Corporation", Fin, "Investment Banking & Brokerage", N, 316709, 100.0, 1.8, 0.30, 0.10, 12, 1.1, 2000, 0),
    e("SPGI", "S&P Global Inc.", Fin, "Financial Exchanges & Data", N, 64040, 520.0, 0.305, 0.22, 0.13, 12, 0.75, 2000, 0),
    e("ICE", "Intercontinental Exchange, Inc.", Fin, "Financial Exchanges & Data", N, 1571949, 160.0, 0.572, 0.22, 0.13, 12, 1.2, 2013, 20051116),
    e("CME", "CME Group Inc. Class A", Fin, "Financial Exchanges & Data", Q, 1156375, 275.0, 0.36, 0.20, 0.11, 12, 1.9, 2003, 20021206),
    e("MMC", "Marsh & McLennan Companies, Inc.", Fin, "Insurance Brokers", N, 62709, 185.0, 0.491, 0.20, 0.10, 12, 1.8, 2000, 0),
    e("USB", "U.S. Bancorp", Fin, "Diversified Banks", N, 36104, 53.0, 1.56, 0.27, 0.04, 12, 4.0, 2000, 0),
    e("PNC", "The PNC Financial Services Group, Inc.", Fin, "Diversified Banks", N, 713676, 208.0, 0.394, 0.27, 0.06, 12, 3.3, 2000, 0),
    e("COF", "Capital One Financial Corporation", Fin, "Consumer Finance", N, 927628, 240.0, 0.64, 0.32, 0.08, 12, 1.0, 2000, 0),
    e("PYPL", "PayPal Holdings, Inc.", Fin, "Transaction & Payment Processing Services", Q, 1633917, 59.0, 0.95, 0.38, 0.05, 12, 0.0, 0, 20150720),
    e("BA", "The Boeing Company", Ind, "Aerospace & Defense", N, 12927, 215.0, 0.76, 0.35, 0.06, 12, 0.0, 0, 0),
    e("CAT", "Caterpillar Inc.", Ind, "Construction Machinery & Heavy Transportation Equipment", N, 18230, 580.0, 0.468, 0.30, 0.13, 12, 1.0, 2000, 0),
    e("GE", "GE Aerospace", Ind, "Aerospace & Defense", N, 40545, 310.0, 1.06, 0.30, 0.10, 12, 0.5, 2000, 0),
    e("MMM", "3M Company", Ind, "Industrial Conglomerates", N, 66740, 160.0, 0.53, 0.25, 0.04, 12, 1.9, 2000, 0),
    e("HON", "Honeywell International Inc.", Ind, "Industrial Conglomerates", Q, 773840, 195.0, 0.635, 0.22, 0.07, 12, 2.3, 2000, 0),
    e("LMT", "Lockheed Martin Corporation", Ind, "Aerospace & Defense", N, 936468, 480.0, 0.232, 0.20, 0.09, 12, 2.8, 2000, 0),
    e("RTX", "RTX Corporation", Ind, "Aerospace & Defense", N, 101829, 185.0, 1.34, 0.22, 0.09, 12, 1.5, 2000, 0),
    e("NOC", "Northrop Grumman Corporation", Ind, "Aerospace & Defense", N, 1133421, 570.0, 0.143, 0.20, 0.10, 12, 1.6, 2000, 0),
    e("GD", "General Dynamics Corporation", Ind, "Aerospace & Defense", N, 40533, 340.0, 0.27, 0.20, 0.08, 12, 1.8, 2000, 0),
    e("UPS", "United Parcel Service, Inc. Class B", Ind, "Air Freight & Logistics", N, 1090727, 100.0, 0.848, 0.28, 0.03, 12, 6.5, 2000, 0),
    e("FDX", "FedEx Corporation", Ind, "Air Freight & Logistics", N, 1048911, 290.0, 0.236, 0.32, 0.07, 5, 2.0, 2002, 0),
    e("UNP", "Union Pacific Corporation", Ind, "Rail Transportation", N, 100885, 230.0, 0.593, 0.24, 0.11, 12, 2.4, 2000, 0),
    e("DE", "Deere & Company", Ind, "Agricultural & Farm Machinery", N, 315189, 470.0, 0.27, 0.27, 0.13, 10, 1.4, 2000, 0),
    e("ADP", "Automatic Data Processing, Inc.", Ind, "Human Resource & Employment Services", Q, 8670, 255.0, 0.405, 0.20, 0.12, 6, 2.4, 2000, 0),
    e("UBER", "Uber Technologies, Inc.", Ind, "Passenger Ground Transportation", N, 1543151, 82.0, 2.08, 0.40, 0.15, 12, 0.0, 0, 20190510),
    e("XOM", "Exxon Mobil Corporation", Energy, "Integrated Oil & Gas", N, 34088, 115.0, 4.25, 0.25, 0.06, 12, 3.5, 2000, 0),
    e("CVX", "Chevron Corporation", Energy, "Integrated Oil & Gas", N, 93410, 150.0, 2.0, 0.26, 0.06, 12, 4.5, 2000, 0),
    e("COP", "ConocoPhillips", Energy, "Oil & Gas Exploration & Production", N, 1163165, 95.0, 1.25, 0.32, 0.07, 12, 3.3, 2002, 0),
    e("SLB", "SLB N.V.", Energy, "Oil & Gas Equipment & Services", N, 87347, 39.0, 1.5, 0.35, 0.02, 12, 2.9, 2000, 0),
    e("EOG", "EOG Resources, Inc.", Energy, "Oil & Gas Exploration & Production", N, 821189, 105.0, 0.545, 0.32, 0.07, 12, 3.9, 2000, 0),
    e("NEE", "NextEra Energy, Inc.", Util, "Electric Utilities", N, 753308, 80.0, 2.06, 0.22, 0.09, 12, 2.8, 2000, 0),
    e("DUK", "Duke Energy Corporation", Util, "Electric Utilities", N, 1326160, 118.0, 0.777, 0.17, 0.05, 12, 3.6, 2006, 0),
    e("SO", "The Southern Company", Util, "Electric Utilities", N, 92122, 87.0, 1.1, 0.17, 0.06, 12, 3.4, 2000, 0),
    e("AMT", "American Tower Corporation", RealEstate, "Telecom Tower REITs", N, 1053507, 175.0, 0.468, 0.25, 0.07, 12, 3.9, 2012, 0),
    e("PLD", "Prologis, Inc.", RealEstate, "Industrial REITs", N, 1045609, 128.0, 0.928, 0.25, 0.08, 12, 3.2, 2000, 0),
    e("SPG", "Simon Property Group, Inc.", RealEstate, "Retail REITs", N, 1063761, 185.0, 0.326, 0.30, 0.06, 12, 4.6, 2000, 0),
    e("LIN", "Linde plc", Materials, "Industrial Gases", Q, 1707925, 425.0, 0.47, 0.20, 0.11, 12, 1.4, 2000, 20181031),
    e("DOW", "Dow Inc.", Materials, "Commodity Chemicals", N, 1751788, 24.0, 0.71, 0.35, -0.03, 12, 5.0, 2019, 20190402),
    e("FCX", "Freeport-McMoRan Inc.", Materials, "Copper", N, 831259, 50.0, 1.44, 0.42, 0.07, 12, 0.6, 2000, 0),
    e("NEM", "Newmont Corporation", Materials, "Gold", N, 1164727, 100.0, 1.1, 0.35, 0.06, 12, 1.0, 2000, 0),
];

/// Historical stock splits (public events). `ratio` new shares per old share.
pub(crate) const SPLITS: &[(&str, (i32, u32, u32), f64)] = &[
    ("AAPL", (2014, 6, 9), 7.0),
    ("AAPL", (2020, 8, 31), 4.0),
    ("NVDA", (2021, 7, 20), 4.0),
    ("NVDA", (2024, 6, 10), 10.0),
    ("TSLA", (2020, 8, 31), 5.0),
    ("TSLA", (2022, 8, 25), 3.0),
    ("AMZN", (2022, 6, 6), 20.0),
    ("GOOGL", (2022, 7, 18), 20.0),
    ("WMT", (2024, 2, 26), 3.0),
    ("AVGO", (2024, 7, 15), 10.0),
    ("NFLX", (2015, 7, 15), 7.0),
    ("NFLX", (2025, 11, 17), 10.0),
    ("LRCX", (2024, 10, 3), 10.0),
    ("PANW", (2022, 9, 14), 3.0),
    ("PANW", (2024, 12, 16), 2.0),
    ("ISRG", (2021, 10, 5), 3.0),
    ("V", (2015, 3, 19), 4.0),
    ("MA", (2014, 1, 22), 10.0),
    ("NKE", (2015, 12, 23), 2.0),
    ("SBUX", (2015, 4, 9), 2.0),
    ("UNP", (2014, 6, 9), 2.0),
];

// ---------------------------------------------------------------------------
// ETFs
// ---------------------------------------------------------------------------

struct Etf {
    t: &'static str,
    n: &'static str,
    cat: &'static str,
    mic: &'static str,
    px: f64,
    vol: f64,
    factor: Factor,
    rho: f64,
    adv_m: f64,
    dy: f64,
    per_year: u32,
    listed: u32,
    tracks: Option<(&'static str, f64)>,
}

#[rustfmt::skip]
const ETFS: &[Etf] = &[
    Etf { t: "SPY", n: "SPDR S&P 500 ETF Trust", cat: "US Large Cap Blend", mic: "ARCX", px: 685.0, vol: 0.16, factor: Factor::UsEq, rho: 0.99, adv_m: 70.0, dy: 1.1, per_year: 4, listed: 19930129, tracks: Some(("SPX", 0.1)) },
    Etf { t: "QQQ", n: "Invesco QQQ Trust, Series 1", cat: "US Large Cap Growth", mic: "XNAS", px: 620.0, vol: 0.21, factor: Factor::UsEq, rho: 0.95, adv_m: 45.0, dy: 0.5, per_year: 4, listed: 19990310, tracks: Some(("NDX", 0.0245)) },
    Etf { t: "IWM", n: "iShares Russell 2000 ETF", cat: "US Small Cap Blend", mic: "ARCX", px: 250.0, vol: 0.21, factor: Factor::UsEq, rho: 0.85, adv_m: 30.0, dy: 1.1, per_year: 4, listed: 20000522, tracks: Some(("RTY", 0.1)) },
    Etf { t: "DIA", n: "SPDR Dow Jones Industrial Average ETF Trust", cat: "US Large Cap Value", mic: "ARCX", px: 480.0, vol: 0.15, factor: Factor::UsEq, rho: 0.95, adv_m: 3.5, dy: 1.5, per_year: 12, listed: 19980120, tracks: Some(("INDU", 0.01)) },
    Etf { t: "TLT", n: "iShares 20+ Year Treasury Bond ETF", cat: "US Long-Term Treasury", mic: "XNAS", px: 87.0, vol: 0.14, factor: Factor::Rates, rho: 0.9, adv_m: 35.0, dy: 4.3, per_year: 12, listed: 20020722, tracks: None },
    Etf { t: "GLD", n: "SPDR Gold Shares", cat: "Commodities: Precious Metals", mic: "ARCX", px: 398.0, vol: 0.17, factor: Factor::Commodity, rho: 0.4, adv_m: 9.0, dy: 0.0, per_year: 0, listed: 20041118, tracks: Some(("GC1", 0.0915)) },
    Etf { t: "XLF", n: "Financial Select Sector SPDR Fund", cat: "US Sector: Financials", mic: "ARCX", px: 54.0, vol: 0.22, factor: Factor::UsEq, rho: 0.85, adv_m: 40.0, dy: 1.4, per_year: 4, listed: 19981222, tracks: None },
    Etf { t: "XLE", n: "Energy Select Sector SPDR Fund", cat: "US Sector: Energy", mic: "ARCX", px: 45.0, vol: 0.25, factor: Factor::UsEq, rho: 0.55, adv_m: 18.0, dy: 3.2, per_year: 4, listed: 19981222, tracks: None },
    Etf { t: "XLK", n: "Technology Select Sector SPDR Fund", cat: "US Sector: Technology", mic: "ARCX", px: 285.0, vol: 0.24, factor: Factor::UsEq, rho: 0.9, adv_m: 7.0, dy: 0.6, per_year: 4, listed: 19981222, tracks: None },
    Etf { t: "EEM", n: "iShares MSCI Emerging Markets ETF", cat: "Emerging Markets Equity", mic: "ARCX", px: 53.0, vol: 0.20, factor: Factor::Asia, rho: 0.8, adv_m: 30.0, dy: 2.2, per_year: 2, listed: 20030414, tracks: None },
    Etf { t: "EWJ", n: "iShares MSCI Japan ETF", cat: "Japan Equity", mic: "ARCX", px: 82.0, vol: 0.17, factor: Factor::Asia, rho: 0.8, adv_m: 6.0, dy: 1.8, per_year: 2, listed: 19960318, tracks: None },
    Etf { t: "EWG", n: "iShares MSCI Germany ETF", cat: "Germany Equity", mic: "ARCX", px: 42.0, vol: 0.20, factor: Factor::Europe, rho: 0.85, adv_m: 3.0, dy: 2.0, per_year: 2, listed: 19960318, tracks: None },
    Etf { t: "EWU", n: "iShares MSCI United Kingdom ETF", cat: "United Kingdom Equity", mic: "ARCX", px: 42.0, vol: 0.17, factor: Factor::Europe, rho: 0.85, adv_m: 2.0, dy: 3.0, per_year: 2, listed: 19960318, tracks: None },
];

// ---------------------------------------------------------------------------
// Indices
// ---------------------------------------------------------------------------

struct Idx {
    t: &'static str,
    n: &'static str,
    country: &'static str,
    region: &'static str,
    ccy: &'static str,
    level: f64,
    vol: f64,
    factor: Factor,
    rho: f64,
    session: Session,
    adv: f64,
}

const NY: Session = Session::US_EQUITY;

#[rustfmt::skip]
const INDICES: &[Idx] = &[
    Idx { t: "SPX", n: "S&P 500 Index", country: "US", region: "Americas", ccy: "USD", level: 6850.0, vol: 0.16, factor: Factor::UsEq, rho: 0.98, session: NY, adv: 2.6e9 },
    Idx { t: "INDU", n: "Dow Jones Industrial Average", country: "US", region: "Americas", ccy: "USD", level: 48000.0, vol: 0.15, factor: Factor::UsEq, rho: 0.95, session: NY, adv: 4.5e8 },
    Idx { t: "CCMP", n: "NASDAQ Composite Index", country: "US", region: "Americas", ccy: "USD", level: 23200.0, vol: 0.20, factor: Factor::UsEq, rho: 0.96, session: NY, adv: 7.0e9 },
    Idx { t: "NDX", n: "NASDAQ-100 Index", country: "US", region: "Americas", ccy: "USD", level: 25300.0, vol: 0.21, factor: Factor::UsEq, rho: 0.95, session: NY, adv: 9.0e8 },
    Idx { t: "RTY", n: "Russell 2000 Index", country: "US", region: "Americas", ccy: "USD", level: 2500.0, vol: 0.21, factor: Factor::UsEq, rho: 0.85, session: NY, adv: 1.5e9 },
    Idx { t: "VIX", n: "Cboe Volatility Index", country: "US", region: "Americas", ccy: "USD", level: 15.0, vol: 0.9, factor: Factor::UsEq, rho: 0.0, session: NY, adv: 0.0 },
    Idx { t: "SPTSX", n: "S&P/TSX Composite Index", country: "CA", region: "Americas", ccy: "CAD", level: 31500.0, vol: 0.13, factor: Factor::UsEq, rho: 0.75, session: NY, adv: 2.5e8 },
    Idx { t: "MEXBOL", n: "S&P/BMV IPC", country: "MX", region: "Americas", ccy: "MXN", level: 64000.0, vol: 0.17, factor: Factor::UsEq, rho: 0.6, session: Session::new(Tz::MexicoCity, (8, 30), (15, 0)), adv: 2.0e8 },
    Idx { t: "IBOV", n: "Ibovespa Brasil Sao Paulo Stock Exchange Index", country: "BR", region: "Americas", ccy: "BRL", level: 160000.0, vol: 0.22, factor: Factor::UsEq, rho: 0.5, session: Session::new(Tz::SaoPaulo, (10, 0), (17, 0)), adv: 8.0e9 },
    Idx { t: "UKX", n: "FTSE 100 Index", country: "GB", region: "EMEA", ccy: "GBP", level: 9900.0, vol: 0.14, factor: Factor::Europe, rho: 0.9, session: Session::new(Tz::London, (8, 0), (16, 30)), adv: 7.0e8 },
    Idx { t: "DAX", n: "Deutsche Boerse AG German Stock Index DAX", country: "DE", region: "EMEA", ccy: "EUR", level: 24400.0, vol: 0.18, factor: Factor::Europe, rho: 0.92, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 8.0e7 },
    Idx { t: "CAC", n: "CAC 40 Index", country: "FR", region: "EMEA", ccy: "EUR", level: 8150.0, vol: 0.17, factor: Factor::Europe, rho: 0.93, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 1.0e8 },
    Idx { t: "SX5E", n: "Euro Stoxx 50 Price EUR", country: "EU", region: "EMEA", ccy: "EUR", level: 5800.0, vol: 0.17, factor: Factor::Europe, rho: 0.97, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 5.0e8 },
    Idx { t: "IBEX", n: "IBEX 35 Index", country: "ES", region: "EMEA", ccy: "EUR", level: 17200.0, vol: 0.18, factor: Factor::Europe, rho: 0.85, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 2.0e8 },
    Idx { t: "FTSEMIB", n: "FTSE MIB Index", country: "IT", region: "EMEA", ccy: "EUR", level: 44500.0, vol: 0.20, factor: Factor::Europe, rho: 0.85, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 5.0e8 },
    Idx { t: "SMI", n: "Swiss Market Index", country: "CH", region: "EMEA", ccy: "CHF", level: 13200.0, vol: 0.14, factor: Factor::Europe, rho: 0.8, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 4.0e7 },
    Idx { t: "AEX", n: "AEX-Index", country: "NL", region: "EMEA", ccy: "EUR", level: 950.0, vol: 0.16, factor: Factor::Europe, rho: 0.88, session: Session::new(Tz::Cet, (9, 0), (17, 30)), adv: 8.0e7 },
    Idx { t: "NKY", n: "Nikkei 225", country: "JP", region: "APAC", ccy: "JPY", level: 50000.0, vol: 0.20, factor: Factor::Asia, rho: 0.85, session: Session::new(Tz::Tokyo, (9, 0), (15, 30)), adv: 1.5e9 },
    Idx { t: "HSI", n: "Hang Seng Index", country: "HK", region: "APAC", ccy: "HKD", level: 25800.0, vol: 0.22, factor: Factor::Asia, rho: 0.8, session: Session::new(Tz::HongKong, (9, 30), (16, 0)), adv: 2.0e10 },
    Idx { t: "SHCOMP", n: "Shanghai Stock Exchange Composite Index", country: "CN", region: "APAC", ccy: "CNY", level: 3950.0, vol: 0.18, factor: Factor::Asia, rho: 0.5, session: Session::new(Tz::Shanghai, (9, 30), (15, 0)), adv: 5.0e10 },
    Idx { t: "KOSPI", n: "Korea Stock Exchange KOSPI Index", country: "KR", region: "APAC", ccy: "KRW", level: 4200.0, vol: 0.20, factor: Factor::Asia, rho: 0.75, session: Session::new(Tz::Seoul, (9, 0), (15, 30)), adv: 5.0e8 },
    Idx { t: "AS51", n: "S&P/ASX 200 Index", country: "AU", region: "APAC", ccy: "AUD", level: 8700.0, vol: 0.14, factor: Factor::Asia, rho: 0.7, session: Session::new(Tz::Sydney, (10, 0), (16, 0)), adv: 7.0e8 },
    Idx { t: "SENSEX", n: "BSE Sensex 30 Index", country: "IN", region: "APAC", ccy: "INR", level: 85000.0, vol: 0.15, factor: Factor::Asia, rho: 0.55, session: Session::new(Tz::India, (9, 15), (15, 30)), adv: 1.0e7 },
    Idx { t: "TWSE", n: "Taiwan Stock Exchange Weighted Index", country: "TW", region: "APAC", ccy: "TWD", level: 28500.0, vol: 0.20, factor: Factor::Asia, rho: 0.7, session: Session::new(Tz::Taipei, (9, 0), (13, 30)), adv: 5.0e9 },
    Idx { t: "STI", n: "Straits Times Index STI", country: "SG", region: "APAC", ccy: "SGD", level: 4650.0, vol: 0.12, factor: Factor::Asia, rho: 0.7, session: Session::new(Tz::Singapore, (9, 0), (17, 0)), adv: 1.5e9 },
];

// ---------------------------------------------------------------------------
// Currencies and FX pairs
// ---------------------------------------------------------------------------

/// code, name, USD value of one unit at the anchor, annual vol vs USD,
/// loading on the dollar factor, half-spread contribution in bps.
pub(crate) struct Ccy {
    pub code: &'static str,
    pub name: &'static str,
    pub usd_value: f64,
    pub vol: f64,
    pub rho: f64,
    pub spread: f64,
}

#[rustfmt::skip]
pub(crate) const CCYS: &[Ccy] = &[
    Ccy { code: "USD", name: "US Dollar", usd_value: 1.0, vol: 0.0, rho: 0.0, spread: 0.0 },
    Ccy { code: "EUR", name: "Euro", usd_value: 1.17, vol: 0.075, rho: 0.8, spread: 0.4 },
    Ccy { code: "JPY", name: "Japanese Yen", usd_value: 1.0 / 157.0, vol: 0.10, rho: 0.5, spread: 0.5 },
    Ccy { code: "GBP", name: "British Pound", usd_value: 1.345, vol: 0.08, rho: 0.7, spread: 0.6 },
    Ccy { code: "CHF", name: "Swiss Franc", usd_value: 1.0 / 0.795, vol: 0.075, rho: 0.7, spread: 0.7 },
    Ccy { code: "CAD", name: "Canadian Dollar", usd_value: 1.0 / 1.375, vol: 0.06, rho: 0.5, spread: 0.6 },
    Ccy { code: "AUD", name: "Australian Dollar", usd_value: 0.668, vol: 0.10, rho: 0.6, spread: 0.6 },
    Ccy { code: "NZD", name: "New Zealand Dollar", usd_value: 0.577, vol: 0.10, rho: 0.6, spread: 0.9 },
    Ccy { code: "CNH", name: "Chinese Renminbi (Offshore)", usd_value: 1.0 / 6.99, vol: 0.04, rho: 0.4, spread: 1.5 },
    Ccy { code: "HKD", name: "Hong Kong Dollar", usd_value: 1.0 / 7.78, vol: 0.006, rho: 0.1, spread: 0.5 },
    Ccy { code: "SGD", name: "Singapore Dollar", usd_value: 1.0 / 1.285, vol: 0.05, rho: 0.6, spread: 1.5 },
    Ccy { code: "MXN", name: "Mexican Peso", usd_value: 1.0 / 18.0, vol: 0.13, rho: 0.4, spread: 2.5 },
    Ccy { code: "BRL", name: "Brazilian Real", usd_value: 1.0 / 5.45, vol: 0.15, rho: 0.4, spread: 5.0 },
    Ccy { code: "INR", name: "Indian Rupee", usd_value: 1.0 / 90.0, vol: 0.05, rho: 0.3, spread: 2.0 },
    Ccy { code: "KRW", name: "South Korean Won", usd_value: 1.0 / 1440.0, vol: 0.09, rho: 0.5, spread: 3.0 },
    Ccy { code: "SEK", name: "Swedish Krona", usd_value: 1.0 / 9.2, vol: 0.10, rho: 0.7, spread: 2.0 },
    Ccy { code: "NOK", name: "Norwegian Krone", usd_value: 1.0 / 10.1, vol: 0.11, rho: 0.6, spread: 2.5 },
];

pub(crate) fn ccy_index(code: &str) -> Option<usize> {
    CCYS.iter().position(|c| c.code == code)
}

/// Listed pairs (any pair of the currencies above also resolves on demand).
const FX_PAIRS: &[&str] = &[
    "EURUSD", "USDJPY", "GBPUSD", "USDCHF", "USDCAD", "AUDUSD", "NZDUSD", "USDCNH", "USDHKD", "USDSGD", "USDMXN",
    "USDBRL", "USDINR", "USDKRW", "USDSEK", "USDNOK", "EURJPY", "EURGBP", "EURCHF", "GBPJPY", "AUDJPY", "EURCAD",
    "EURAUD", "CHFJPY", "EURSEK", "EURNOK", "GBPCHF", "AUDNZD", "CADJPY", "NZDJPY",
];

// ---------------------------------------------------------------------------
// Crypto
// ---------------------------------------------------------------------------

struct Cx {
    t: &'static str,
    n: &'static str,
    px: f64,
    vol: f64,
    mu: f64,
    listed: u32,
    supply: f64,
    adv_usd: f64,
    dp: u8,
    lot: f64,
    spread: f64,
}

#[rustfmt::skip]
const CRYPTO: &[Cx] = &[
    Cx { t: "BTCUSD", n: "Bitcoin", px: 90000.0, vol: 0.55, mu: 0.42, listed: 20140101, supply: 19.97e6, adv_usd: 2.5e9, dp: 2, lot: 0.01, spread: 1.0 },
    Cx { t: "ETHUSD", n: "Ethereum", px: 3100.0, vol: 0.70, mu: 0.55, listed: 20160301, supply: 120.7e6, adv_usd: 1.5e9, dp: 2, lot: 0.1, spread: 1.5 },
    Cx { t: "SOLUSD", n: "Solana", px: 135.0, vol: 0.85, mu: 0.70, listed: 20200410, supply: 560.0e6, adv_usd: 8.0e8, dp: 2, lot: 1.0, spread: 3.0 },
    Cx { t: "XRPUSD", n: "XRP", px: 2.0, vol: 0.85, mu: 0.35, listed: 20150101, supply: 60.5e9, adv_usd: 6.0e8, dp: 4, lot: 100.0, spread: 3.0 },
    Cx { t: "ADAUSD", n: "Cardano", px: 0.38, vol: 0.85, mu: 0.30, listed: 20171001, supply: 36.5e9, adv_usd: 2.0e8, dp: 4, lot: 100.0, spread: 4.0 },
    Cx { t: "DOGEUSD", n: "Dogecoin", px: 0.135, vol: 1.0, mu: 0.45, listed: 20150101, supply: 151.0e9, adv_usd: 3.0e8, dp: 5, lot: 1000.0, spread: 4.0 },
    Cx { t: "AVAXUSD", n: "Avalanche", px: 13.5, vol: 0.90, mu: 0.20, listed: 20200922, supply: 430.0e6, adv_usd: 1.0e8, dp: 2, lot: 1.0, spread: 5.0 },
    Cx { t: "LINKUSD", n: "Chainlink", px: 13.5, vol: 0.85, mu: 0.40, listed: 20170920, supply: 680.0e6, adv_usd: 1.0e8, dp: 2, lot: 1.0, spread: 5.0 },
    Cx { t: "DOTUSD", n: "Polkadot", px: 2.1, vol: 0.85, mu: 0.05, listed: 20200819, supply: 1.6e9, adv_usd: 5.0e7, dp: 3, lot: 10.0, spread: 6.0 },
    Cx { t: "LTCUSD", n: "Litecoin", px: 80.0, vol: 0.75, mu: 0.20, listed: 20140101, supply: 76.0e6, adv_usd: 1.0e8, dp: 2, lot: 0.1, spread: 4.0 },
    Cx { t: "BCHUSD", n: "Bitcoin Cash", px: 560.0, vol: 0.80, mu: 0.10, listed: 20170801, supply: 19.95e6, adv_usd: 8.0e7, dp: 2, lot: 0.01, spread: 5.0 },
    Cx { t: "XLMUSD", n: "Stellar", px: 0.22, vol: 0.85, mu: 0.25, listed: 20150101, supply: 32.0e9, adv_usd: 5.0e7, dp: 5, lot: 1000.0, spread: 6.0 },
];

// ---------------------------------------------------------------------------
// Futures (generic front months)
// ---------------------------------------------------------------------------

struct Fut {
    t: &'static str,
    n: &'static str,
    group: &'static str,
    px: f64,
    vol: f64,
    mult: f64,
    tick: f64,
    dp: u8,
    mic: &'static str,
    exch: &'static str,
    open: (i32, i32),
    close: (i32, i32),
    adv: f64,
    factor: Factor,
    rho: f64,
    tracks: Option<(&'static str, f64)>,
}

#[rustfmt::skip]
const FUTURES: &[Fut] = &[
    Fut { t: "CL1", n: "WTI Crude Oil Future (Generic 1st)", group: "Energy", px: 58.0, vol: 0.35, mult: 1000.0, tick: 0.01, dp: 2, mic: "XNYM", exch: "NYMEX", open: (9, 0), close: (14, 30), adv: 400_000.0, factor: Factor::Commodity, rho: 0.5, tracks: None },
    Fut { t: "GC1", n: "Gold Future (Generic 1st)", group: "Precious Metals", px: 4350.0, vol: 0.17, mult: 100.0, tick: 0.1, dp: 1, mic: "XCEC", exch: "COMEX", open: (8, 20), close: (13, 30), adv: 200_000.0, factor: Factor::Commodity, rho: 0.4, tracks: None },
    Fut { t: "SI1", n: "Silver Future (Generic 1st)", group: "Precious Metals", px: 70.0, vol: 0.32, mult: 5000.0, tick: 0.005, dp: 3, mic: "XCEC", exch: "COMEX", open: (8, 25), close: (13, 25), adv: 70_000.0, factor: Factor::Commodity, rho: 0.4, tracks: None },
    Fut { t: "HG1", n: "Copper Future (Generic 1st)", group: "Industrial Metals", px: 5.6, vol: 0.24, mult: 25_000.0, tick: 0.0005, dp: 4, mic: "XCEC", exch: "COMEX", open: (8, 10), close: (13, 0), adv: 90_000.0, factor: Factor::Commodity, rho: 0.5, tracks: None },
    Fut { t: "NG1", n: "Natural Gas Future (Generic 1st)", group: "Energy", px: 4.0, vol: 0.60, mult: 10_000.0, tick: 0.001, dp: 3, mic: "XNYM", exch: "NYMEX", open: (9, 0), close: (14, 30), adv: 300_000.0, factor: Factor::Commodity, rho: 0.2, tracks: None },
    Fut { t: "ES1", n: "E-mini S&P 500 Future (Generic 1st)", group: "Equity Index", px: 6877.0, vol: 0.16, mult: 50.0, tick: 0.25, dp: 2, mic: "XCME", exch: "CME", open: (9, 30), close: (16, 15), adv: 1_500_000.0, factor: Factor::UsEq, rho: 0.98, tracks: Some(("SPX", 1.004)) },
    Fut { t: "NQ1", n: "E-mini Nasdaq-100 Future (Generic 1st)", group: "Equity Index", px: 25400.0, vol: 0.21, mult: 20.0, tick: 0.25, dp: 2, mic: "XCME", exch: "CME", open: (9, 30), close: (16, 15), adv: 600_000.0, factor: Factor::UsEq, rho: 0.95, tracks: Some(("NDX", 1.004)) },
    Fut { t: "YM1", n: "E-mini Dow Future (Generic 1st)", group: "Equity Index", px: 48150.0, vol: 0.15, mult: 5.0, tick: 1.0, dp: 0, mic: "XCBT", exch: "CBOT", open: (9, 30), close: (16, 15), adv: 150_000.0, factor: Factor::UsEq, rho: 0.95, tracks: Some(("INDU", 1.003)) },
    Fut { t: "ZN1", n: "10-Year US Treasury Note Future (Generic 1st)", group: "Interest Rate", px: 112.5, vol: 0.06, mult: 1000.0, tick: 0.015625, dp: 4, mic: "XCBT", exch: "CBOT", open: (8, 20), close: (15, 0), adv: 1_500_000.0, factor: Factor::Rates, rho: 0.9, tracks: None },
    Fut { t: "ZB1", n: "30-Year US Treasury Bond Future (Generic 1st)", group: "Interest Rate", px: 116.0, vol: 0.12, mult: 1000.0, tick: 0.03125, dp: 4, mic: "XCBT", exch: "CBOT", open: (8, 20), close: (15, 0), adv: 350_000.0, factor: Factor::Rates, rho: 0.9, tracks: None },
    Fut { t: "ZC1", n: "Corn Future (Generic 1st)", group: "Agriculture", px: 445.0, vol: 0.22, mult: 50.0, tick: 0.25, dp: 2, mic: "XCBT", exch: "CBOT", open: (9, 30), close: (14, 20), adv: 300_000.0, factor: Factor::Commodity, rho: 0.2, tracks: None },
    Fut { t: "ZS1", n: "Soybean Future (Generic 1st)", group: "Agriculture", px: 1060.0, vol: 0.18, mult: 50.0, tick: 0.25, dp: 2, mic: "XCBT", exch: "CBOT", open: (9, 30), close: (14, 20), adv: 200_000.0, factor: Factor::Commodity, rho: 0.2, tracks: None },
    Fut { t: "ZW1", n: "Chicago SRW Wheat Future (Generic 1st)", group: "Agriculture", px: 520.0, vol: 0.27, mult: 50.0, tick: 0.25, dp: 2, mic: "XCBT", exch: "CBOT", open: (9, 30), close: (14, 20), adv: 120_000.0, factor: Factor::Commodity, rho: 0.2, tracks: None },
];

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

fn date_from_code(code: u32, fallback: NaiveDate) -> NaiveDate {
    if code == 0 {
        return fallback;
    }
    NaiveDate::from_ymd_opt((code / 10_000) as i32, (code / 100) % 100, code % 100).unwrap_or(fallback)
}

/// Before every generated path; symbols listed earlier show full history.
fn old_listing() -> NaiveDate {
    ymd(1990, 1, 2)
}

fn base_instrument(key: SecurityKey, name: &str, asset: AssetClass, ccy: &str) -> Instrument {
    let mut i = Instrument::basic(key, name, asset, ccy);
    i.is_synthetic = true;
    i
}

#[allow(clippy::struct_field_names)]
pub(crate) struct Universe {
    pub syms: Vec<Sym>,
    by_key: HashMap<(String, MarketSector), usize>,
    pub instruments: Vec<Instrument>,
}

impl Universe {
    pub(crate) fn build(extra: usize) -> Universe {
        let mut syms: Vec<Sym> = Vec::with_capacity(EQUITIES.len() + 120 + extra);

        for row in EQUITIES {
            syms.push(equity_sym(row));
        }
        // Indices go before ETFs/futures so `Tracks` can reference them.
        for row in INDICES {
            syms.push(index_sym(row));
        }
        for row in FUTURES.iter().filter(|f| f.tracks.is_none()) {
            syms.push(future_sym(row, None));
        }
        let find = |syms: &[Sym], t: &str| syms.iter().position(|s| s.inst.key.symbol == t);
        for row in FUTURES.iter().filter(|f| f.tracks.is_some()) {
            let under = row.tracks.and_then(|(u, r)| find(&syms, u).map(|i| (i, r)));
            syms.push(future_sym(row, under));
        }
        for row in ETFS {
            let under = row.tracks.and_then(|(u, r)| find(&syms, u).map(|i| (i, r)));
            syms.push(etf_sym(row, under));
        }
        for pair in FX_PAIRS {
            if let Some(s) = fx_sym(pair) {
                syms.push(s);
            }
        }
        for row in CRYPTO {
            syms.push(crypto_sym(row));
        }
        for n in 1..=extra {
            syms.push(filler_sym(n));
        }

        let mut by_key = HashMap::with_capacity(syms.len());
        for (i, s) in syms.iter().enumerate() {
            by_key.insert((s.inst.key.symbol.clone(), s.inst.key.sector), i);
        }
        let instruments = syms.iter().map(|s| s.inst.clone()).collect();
        Universe { syms, by_key, instruments }
    }

    pub(crate) fn index_of(&self, key: &SecurityKey) -> Option<usize> {
        let i = *self.by_key.get(&(key.symbol.to_ascii_uppercase(), key.sector))?;
        let ours = &self.syms[i].inst.key;
        match (&key.exchange, &ours.exchange) {
            (None, _) => Some(i),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => Some(i),
            _ => None,
        }
    }

    pub(crate) fn by_ticker(&self, ticker: &str, sector: MarketSector) -> Option<usize> {
        self.by_key.get(&(ticker.to_ascii_uppercase(), sector)).copied()
    }

    pub(crate) fn by_cik(&self, cik: u64) -> Option<usize> {
        self.syms.iter().position(|s| s.p.kind == Kind::Equity && filing_cik(s) == cik)
    }
}

/// CIK used on synthetic filings: the real CIK when known, otherwise a
/// 10-digit value starting with 9 that no real registrant uses.
pub(crate) fn filing_cik(s: &Sym) -> u64 {
    s.inst.cik.unwrap_or(9_000_000_000 + s.hash % 999_999_999)
}

fn equity_sym(r: &E) -> Sym {
    let key = SecurityKey::equity(r.t);
    let mut inst = base_instrument(key.clone(), r.n, AssetClass::Equity, "USD");
    inst.exchange_mic = Some(if r.nas { "XNAS" } else { "XNYS" }.into());
    inst.exchange_name = Some(if r.nas { "NASDAQ" } else { "NYSE" }.into());
    inst.cik = (r.cik != 0).then_some(r.cik);
    inst.sector = Some(r.s.label().into());
    inst.industry = Some(r.ind.into());
    inst.country = Some("US".into());
    let hash = key_hash(&key);
    let mcap = r.px * r.sh * 1e9;
    let mut c = Cell::new(&[hash, tag("equity-params")]);
    let turnover = 0.0025 * (r.vol / 0.25).powf(1.5) * c.range(0.8, 1.25);
    let spread_bps = if mcap > 5e11 {
        0.5
    } else if mcap > 5e10 {
        2.0
    } else {
        5.0
    };
    let div = (r.dy > 0.0).then(|| DivSpec {
        yield_pct: r.dy,
        start_year: r.ds.max(1990),
        per_year: 4,
        growth: c.range(0.02, 0.10),
        first_month: 1 + (hash % 3) as u32,
        ex_day: 5 + ((hash >> 8) % 20) as u32,
    });
    let activity = (mcap / 3e11).sqrt().clamp(0.05, 4.0) * (r.vol / 0.3);
    Sym {
        p: Params {
            kind: Kind::Equity,
            model: Model::Gbm,
            ref_price: r.px,
            mu: r.mu,
            sigma: r.vol,
            rho: (0.75 - (r.vol - 0.2) * 0.6).clamp(0.35, 0.8),
            factor: Factor::UsEq,
            jumps_per_year: 2.5,
            jump_sd: 3.0,
            jump_mean: -0.2,
            kappa: 0.0,
            adv: (r.sh * 1e9 * turnover).round(),
            listed: date_from_code(r.listed, old_listing()),
            calendar: Calendar::Nyse,
            session: Session::US_EQUITY,
            periods_per_year: 252.0,
            spread_bps,
            lot: 100.0,
            vol_decimals: 0,
            shares: r.sh * 1e9,
            sector: Some(r.s),
            fye_month: r.fye,
            div,
            options: Some(OptSpec {
                style: ExerciseStyle::American,
                root: r.t.into(),
                weekly_root: r.t.into(),
                weeklies: true,
                q: r.dy / 100.0,
                activity,
                index_like: false,
            }),
            has_book: true,
            region: Some("Americas"),
            filler: false,
        },
        inst,
        hash,
    }
}

fn etf_sym(r: &Etf, under: Option<(usize, f64)>) -> Sym {
    let key = SecurityKey::equity(r.t);
    let mut inst = base_instrument(key.clone(), r.n, AssetClass::Etf, "USD");
    inst.exchange_mic = Some(r.mic.into());
    inst.exchange_name = Some(if r.mic == "XNAS" { "NASDAQ" } else { "NYSE Arca" }.into());
    inst.sector = Some("Exchange Traded Fund".into());
    inst.industry = Some(r.cat.into());
    inst.country = Some("US".into());
    let hash = key_hash(&key);
    let model = match under {
        Some((i, ratio)) => Model::Tracks { under: i, ratio },
        None => Model::Gbm,
    };
    let div = (r.dy > 0.0 && r.per_year > 0).then_some(DivSpec {
        yield_pct: r.dy,
        start_year: 1990,
        per_year: r.per_year,
        growth: 0.04,
        first_month: if r.per_year == 12 { 1 } else { 3 },
        ex_day: 18,
    });
    let index_like = matches!(r.t, "SPY" | "QQQ" | "IWM" | "DIA");
    Sym {
        p: Params {
            kind: Kind::Etf,
            model,
            ref_price: r.px,
            mu: 0.07,
            sigma: r.vol,
            rho: r.rho,
            factor: r.factor,
            jumps_per_year: 0.8,
            jump_sd: 2.5,
            jump_mean: -0.3,
            kappa: if r.factor == Factor::Rates { 0.2 } else { 0.0 },
            adv: r.adv_m * 1e6,
            listed: date_from_code(r.listed, old_listing()),
            calendar: Calendar::Nyse,
            session: Session::US_EQUITY,
            periods_per_year: 252.0,
            spread_bps: 0.3,
            lot: 100.0,
            vol_decimals: 0,
            shares: r.adv_m * 1e6 * 12.0,
            sector: None,
            fye_month: 12,
            div,
            options: Some(OptSpec {
                style: ExerciseStyle::American,
                root: r.t.into(),
                weekly_root: r.t.into(),
                weeklies: true,
                q: r.dy / 100.0,
                activity: if index_like { 6.0 } else { 0.8 },
                index_like,
            }),
            has_book: true,
            region: Some("Americas"),
            filler: false,
        },
        inst,
        hash,
    }
}

fn index_sym(r: &Idx) -> Sym {
    let key = SecurityKey::index(r.t);
    let mut inst = base_instrument(key.clone(), r.n, AssetClass::Index, r.ccy);
    inst.sector = Some("Index".into());
    inst.industry = Some(r.region.into());
    inst.country = Some(r.country.into());
    let hash = key_hash(&key);
    let us = r.session.tz == Tz::NewYork && r.country == "US";
    let options = match r.t {
        "SPX" => Some(OptSpec {
            style: ExerciseStyle::European,
            root: "SPX".into(),
            weekly_root: "SPXW".into(),
            weeklies: true,
            q: 0.012,
            activity: 10.0,
            index_like: true,
        }),
        "NDX" => Some(OptSpec {
            style: ExerciseStyle::European,
            root: "NDX".into(),
            weekly_root: "NDXP".into(),
            weeklies: true,
            q: 0.007,
            activity: 2.0,
            index_like: true,
        }),
        _ => None,
    };
    Sym {
        p: Params {
            kind: Kind::Index,
            model: if r.t == "VIX" { Model::VolIndex } else { Model::Gbm },
            ref_price: r.level,
            mu: 0.075,
            sigma: r.vol,
            rho: r.rho,
            factor: r.factor,
            jumps_per_year: 0.4,
            jump_sd: 2.0,
            jump_mean: -0.5,
            kappa: 0.0,
            adv: r.adv,
            listed: old_listing(),
            calendar: if us { Calendar::Nyse } else { Calendar::Weekdays },
            session: r.session,
            periods_per_year: if us { 252.0 } else { 260.0 },
            spread_bps: 0.0,
            lot: 1.0,
            vol_decimals: 0,
            shares: 0.0,
            sector: None,
            fye_month: 12,
            div: None,
            options,
            has_book: false,
            region: Some(r.region),
            filler: false,
        },
        inst,
        hash,
    }
}

fn future_sym(r: &Fut, under: Option<(usize, f64)>) -> Sym {
    let key = SecurityKey::new(r.t, None, MarketSector::Cmdty);
    let mut inst = base_instrument(key.clone(), r.n, AssetClass::Future, "USD");
    inst.exchange_mic = Some(r.mic.into());
    inst.exchange_name = Some(r.exch.into());
    inst.tick_size = r.tick;
    inst.price_decimals = r.dp;
    inst.multiplier = r.mult;
    inst.sector = Some("Commodity".into());
    inst.industry = Some(r.group.into());
    inst.country = Some("US".into());
    let hash = key_hash(&key);
    let model = match under {
        Some((i, ratio)) => Model::Tracks { under: i, ratio },
        None => Model::Gbm,
    };
    Sym {
        p: Params {
            kind: Kind::Future,
            model,
            ref_price: r.px,
            mu: 0.02,
            sigma: r.vol,
            rho: r.rho,
            factor: r.factor,
            jumps_per_year: 2.0,
            jump_sd: 2.5,
            jump_mean: 0.0,
            kappa: match r.group {
                "Interest Rate" => 0.3,
                "Energy" | "Agriculture" | "Industrial Metals" => 0.1,
                "Precious Metals" => 0.05,
                _ => 0.0,
            },
            adv: r.adv,
            listed: old_listing(),
            calendar: Calendar::Nyse,
            session: Session::new(Tz::NewYork, r.open, r.close),
            periods_per_year: 252.0,
            spread_bps: 0.0,
            lot: 1.0,
            vol_decimals: 0,
            shares: 0.0,
            sector: None,
            fye_month: 12,
            div: None,
            options: None,
            has_book: true,
            region: Some("Americas"),
            filler: false,
        },
        inst,
        hash,
    }
}

/// Builds an FX pair from two known currency codes, e.g. `GBPNZD`.
pub(crate) fn fx_sym(pair: &str) -> Option<Sym> {
    if pair.len() != 6 || !pair.is_ascii() {
        return None;
    }
    let up = pair.to_ascii_uppercase();
    let (b, q) = up.split_at(3);
    let (bi, qi) = (ccy_index(b)?, ccy_index(q)?);
    if bi == qi {
        return None;
    }
    let key = SecurityKey::currency(&up);
    let (cb, cq) = (&CCYS[bi], &CCYS[qi]);
    let mut inst = base_instrument(key.clone(), &format!("{} / {}", cb.name, cq.name), AssetClass::Fx, q);
    let dp: u8 = if matches!(q, "JPY" | "KRW") { 2 } else { 4 };
    inst.price_decimals = dp;
    inst.tick_size = 10f64.powi(-i32::from(dp));
    inst.tick_size = (inst.tick_size * 1e12).round() / 1e12;
    inst.sector = Some("Currency".into());
    let ref_price = cb.usd_value / cq.usd_value;
    let sigma = (cb.vol * cb.vol + cq.vol * cq.vol - 2.0 * cb.rho * cq.rho * cb.vol * cq.vol).max(1e-6).sqrt();
    let hash = key_hash(&key);
    Some(Sym {
        p: Params {
            kind: Kind::Fx,
            model: Model::FxPair { base: bi, quote: qi },
            ref_price,
            mu: 0.0,
            sigma,
            rho: 0.0,
            factor: Factor::Usd,
            jumps_per_year: 0.0,
            jump_sd: 0.0,
            jump_mean: 0.0,
            kappa: 0.0,
            adv: 0.0,
            listed: old_listing(),
            calendar: Calendar::Weekdays,
            session: Session::UTC_24H,
            periods_per_year: 260.0,
            spread_bps: (cb.spread + cq.spread).max(0.5),
            lot: 1_000_000.0,
            vol_decimals: 0,
            shares: 0.0,
            sector: None,
            fye_month: 12,
            div: None,
            options: None,
            has_book: true,
            region: None,
            filler: false,
        },
        inst,
        hash,
    })
}

fn crypto_sym(r: &Cx) -> Sym {
    let key = SecurityKey::currency(r.t);
    let mut inst = base_instrument(key.clone(), r.n, AssetClass::Crypto, "USD");
    inst.price_decimals = r.dp;
    inst.tick_size = (10f64.powi(-i32::from(r.dp)) * 1e12).round() / 1e12;
    inst.sector = Some("Crypto".into());
    let hash = key_hash(&key);
    Sym {
        p: Params {
            kind: Kind::Crypto,
            model: Model::Gbm,
            ref_price: r.px,
            mu: r.mu,
            sigma: r.vol,
            rho: 0.8,
            factor: Factor::Crypto,
            jumps_per_year: 4.0,
            jump_sd: 2.5,
            jump_mean: -0.2,
            kappa: 0.0,
            adv: (r.adv_usd / r.px).round(),
            listed: date_from_code(r.listed, old_listing()),
            calendar: Calendar::AllDays,
            session: Session::UTC_24H,
            periods_per_year: 365.0,
            spread_bps: r.spread,
            lot: r.lot,
            vol_decimals: 4,
            shares: r.supply,
            sector: None,
            fye_month: 12,
            div: None,
            options: None,
            has_book: true,
            region: None,
            filler: false,
        },
        inst,
        hash,
    }
}

fn filler_sym(n: usize) -> Sym {
    let t = format!("ZQ{n:04}");
    let key = SecurityKey::equity(&t);
    let hash = key_hash(&key);
    let mut c = Cell::new(&[hash, tag("filler")]);
    let sector = Sector::ALL[n % Sector::ALL.len()];
    let nas = n.is_multiple_of(2);
    let mut inst = base_instrument(key.clone(), &format!("Synthetic Filler Co. {n:04}"), AssetClass::Equity, "USD");
    inst.exchange_mic = Some(if nas { "XNAS" } else { "XNYS" }.into());
    inst.exchange_name = Some(if nas { "NASDAQ" } else { "NYSE" }.into());
    inst.sector = Some(sector.label().into());
    inst.industry = Some(sector.filler_industry().into());
    inst.country = Some("US".into());
    let px = (c.range(10.0, 300.0) * 100.0).round() / 100.0;
    let shares = c.range(0.05, 2.0) * 1e9;
    let vol = c.range(0.2, 0.6);
    let pays = c.chance(0.5);
    let div = pays.then(|| DivSpec {
        yield_pct: c.range(0.5, 4.0),
        start_year: 2000,
        per_year: 4,
        growth: c.range(0.0, 0.08),
        first_month: 1 + (hash % 3) as u32,
        ex_day: 5 + ((hash >> 8) % 20) as u32,
    });
    let q = div.map_or(0.0, |d| d.yield_pct / 100.0);
    Sym {
        p: Params {
            kind: Kind::Equity,
            model: Model::Gbm,
            ref_price: px,
            mu: 0.08,
            sigma: vol,
            rho: 0.5,
            factor: Factor::UsEq,
            jumps_per_year: 3.0,
            jump_sd: 3.0,
            jump_mean: -0.2,
            kappa: 0.0,
            adv: (shares * 0.004).round(),
            listed: old_listing(),
            calendar: Calendar::Nyse,
            session: Session::US_EQUITY,
            periods_per_year: 252.0,
            spread_bps: 10.0,
            lot: 100.0,
            vol_decimals: 0,
            shares,
            sector: Some(sector),
            fye_month: 12,
            div,
            options: Some(OptSpec {
                style: ExerciseStyle::American,
                root: t.clone(),
                weekly_root: t,
                weeklies: false,
                q,
                activity: 0.05,
                index_like: false,
            }),
            has_book: true,
            region: Some("Americas"),
            filler: true,
        },
        inst,
        hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_is_consistent() {
        let u = Universe::build(3);
        assert!(u.syms.len() > 200);
        let eq = u.syms.iter().filter(|s| s.p.kind == Kind::Equity && !s.p.filler).count();
        assert!(eq >= 100, "{eq} equities");
        for s in &u.syms {
            assert!(s.inst.is_synthetic);
            assert!(s.p.ref_price > 0.0, "{}", s.inst.key);
            assert_eq!(u.index_of(&s.inst.key).map(|i| &u.syms[i].inst.key), Some(&s.inst.key));
        }
        assert!(u.index_of(&SecurityKey::new("ZQ0003", Some("US"), MarketSector::Equity)).is_some());
        assert!(u.index_of(&SecurityKey::new("AAPL", None, MarketSector::Equity)).is_some());
        assert!(u.index_of(&SecurityKey::new("AAPL", Some("LN"), MarketSector::Equity)).is_none());
    }

    #[test]
    fn fx_pairs_build_on_demand() {
        let s = fx_sym("GBPNZD").unwrap();
        assert_eq!(s.inst.key.to_string(), "GBPNZD Curncy");
        assert!(fx_sym("USDUSD").is_none());
        assert!(fx_sym("XXXUSD").is_none());
        assert_eq!(fx_sym("USDJPY").unwrap().inst.price_decimals, 2);
    }
}
