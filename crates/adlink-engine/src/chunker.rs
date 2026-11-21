//! Platform-aware inclusive date-range chunking for long historical backfills.

use chrono::{Duration, NaiveDate};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChunkerError {
    #[error("start_date ({start}) cannot be after end_date ({end})")]
    InvalidRange { start: NaiveDate, end: NaiveDate },
    #[error("max_days_inclusive must be at least 1")]
    ZeroChunkDays,
}

/// Splits long date ranges into compliant inclusive sub-windows matching each platform's API limits
/// (e.g. 1 day for hourly, 7 days for Alipay/Kuaishou, 30 days for XHS/Ocean/Bilibili, 90 days for Tencent).
pub struct DateRangeChunker;

impl DateRangeChunker {
    /// Splits `[start, end]` inclusively into consecutive `(chunk_start, chunk_end)` spans
    /// each containing at most `max_days_inclusive` calendar days.
    pub fn split(
        start: NaiveDate,
        end: NaiveDate,
        max_days_inclusive: u32,
    ) -> Result<Vec<(NaiveDate, NaiveDate)>, ChunkerError> {
        if start > end {
            return Err(ChunkerError::InvalidRange { start, end });
        }
        if max_days_inclusive == 0 {
            return Err(ChunkerError::ZeroChunkDays);
        }

        let step = i64::from(max_days_inclusive - 1);
        let mut chunks = Vec::new();
        let mut cursor = start;
        while cursor <= end {
            let candidate_end = cursor + Duration::days(step);
            let chunk_end = if candidate_end > end {
                end
            } else {
                candidate_end
            };
            chunks.push((cursor, chunk_end));
            match chunk_end.succ_opt() {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(chunks)
    }
}
