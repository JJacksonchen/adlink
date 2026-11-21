//! Exact integer monetary representation in Fen (1 CNY = 100 Fen) to eliminate
//! floating-point drift during cross-platform financial reconciliation.

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
