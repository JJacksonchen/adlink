//! Exact integer monetary representation in Fen (1 CNY = 100 Fen) to eliminate
//! floating-point drift during cross-platform financial reconciliation.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::{Add, AddAssign, Sub};

use serde::{Deserialize, Serialize};

/// Integer monetary value stored in Chinese Fen (0.01 CNY).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fen(pub i64);

impl Fen {
    pub const ZERO: Self = Self(0);

    /// Converts an integer fen amount directly.
    pub const fn from_fen(fen: i64) -> Self {
        Self(fen)
    }

    /// Converts Yuan (floating-point as returned by Ocean Engine / Kuaishou APIs) into rounded Fen.
    pub fn from_yuan_f64(yuan: f64) -> Self {
        if !yuan.is_finite() {
            return Self(0);
        }
        Self((yuan * 100.0).round() as i64)
    }

    /// Returns the raw integer value in Fen.
    pub const fn as_fen(self) -> i64 {
        self.0
    }

    /// Converts to Yuan as `f64` for reporting display and formula evaluation.
    pub fn to_yuan_f64(self) -> f64 {
        self.0 as f64 / 100.0
    }

    /// Checked addition returning `None` on overflow.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0).map(Self)
    }

    /// Saturating addition safe for aggregations over very large datasets.
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Scales a fen amount by a rational ratio (e.g. attribution split weight).
    pub fn scale(self, weight: f64) -> Self {
        if !weight.is_finite() {
            return Self::ZERO;
        }
        Self((self.0 as f64 * weight).round() as i64)
    }
}

impl Add for Fen {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for Fen {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl Sub for Fen {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl fmt::Display for Fen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{}{}.{:02}", sign, abs / 100, abs % 100)
    }
}

/// Exchange rate table mapping ISO-4217 currency codes (e.g. `USD`, `EUR`, `GBP`, `JPY`)
/// to AdLink's canonical accounting base currency (`CNY` -> Fen).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FxRateTable {
    pub rates_to_cny: BTreeMap<String, f64>,
}

impl Default for FxRateTable {
    fn default() -> Self {
        let pairs = [
            ("CNY", 1.0),
            ("RMB", 1.0),
            ("USD", 7.25),
            ("EUR", 7.85),
            ("GBP", 9.15),
            ("JPY", 0.048),
            ("HKD", 0.93),
            ("SGD", 5.40),
            ("AUD", 4.75),
            ("CAD", 5.25),
        ];
        Self {
            rates_to_cny: pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }
}

impl FxRateTable {
    /// Builds a rate table starting from [`Self::default`] merged with custom overrides.
    pub fn with_overrides(overrides: BTreeMap<String, f64>) -> Self {
        let mut table = Self::default();
        for (k, v) in overrides {
            if v.is_finite() && v > 0.0 {
                table.rates_to_cny.insert(k.trim().to_ascii_uppercase(), v);
            }
        }
        table
    }

    /// Returns the FX multiplier converting 1 major unit of `currency` into CNY (defaults to `1.0`).
    pub fn rate_for(&self, currency: &str) -> f64 {
        let key = currency.trim().to_ascii_uppercase();
        if key.is_empty() {
            return 1.0;
        }
        self.rates_to_cny.get(&key).copied().unwrap_or(1.0)
    }

    /// Converts an amount in `currency` major units (e.g. `$12.50` USD) to base `Fen` (integer 0.01 CNY).
    pub fn major_to_base_fen(&self, amount_major: f64, currency: &str) -> Fen {
        if !amount_major.is_finite() {
            return Fen::ZERO;
        }
        Fen::from_yuan_f64(amount_major * self.rate_for(currency))
    }

    /// Converts micros (`1_000_000 micros = 1 major currency unit`, as in Google Ads `cost_micros`) to base `Fen`.
    pub fn micros_to_base_fen(&self, micros: i64, currency: &str) -> Fen {
        let major = micros as f64 / 1_000_000.0;
        self.major_to_base_fen(major, currency)
    }
}
