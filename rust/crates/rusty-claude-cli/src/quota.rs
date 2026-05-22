use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Daily output token limit for Azure AI Foundry deployments.
/// When exhausted, `resolve_provider()` falls back to OpenRouter free.
pub const DAILY_AZURE_TOKEN_LIMIT: u32 = 44_000;

#[derive(Serialize, Deserialize, Debug)]
pub struct QuotaState {
    pub date: String,
    pub azure_output_tokens_used: u32,
    pub daily_limit: u32,
}

impl Default for QuotaState {
    fn default() -> Self {
        Self {
            date: today_utc(),
            azure_output_tokens_used: 0,
            daily_limit: DAILY_AZURE_TOKEN_LIMIT,
        }
    }
}

impl QuotaState {
    /// Load quota state from disk. Returns default (fresh day) if file is
    /// missing, corrupt, or belongs to a previous day.
    pub fn load() -> Self {
        let path = quota_path();
        let mut state = match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<QuotaState>(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        // Auto-reset on new day
        let today = today_utc();
        if state.date != today {
            state.date = today;
            state.azure_output_tokens_used = 0;
            state.daily_limit = DAILY_AZURE_TOKEN_LIMIT;
            state.save();
        }
        state
    }

    /// Persist quota state to disk. Silently ignores write failures (non-fatal).
    pub fn save(&self) {
        let path = quota_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, json);
        }
    }

    /// Remaining Azure tokens for today.
    pub fn remaining(&self) -> u32 {
        self.daily_limit
            .saturating_sub(self.azure_output_tokens_used)
    }

    /// Whether the daily Azure quota has been exhausted.
    pub fn is_azure_exhausted(&self) -> bool {
        self.azure_output_tokens_used >= self.daily_limit
    }

    /// Record output tokens consumed by an Azure request.
    /// Returns `true` if quota is still available after recording.
    pub fn record_azure_usage(&mut self, output_tokens: u32) -> bool {
        self.azure_output_tokens_used = self.azure_output_tokens_used.saturating_add(output_tokens);
        self.save();
        !self.is_azure_exhausted()
    }

    /// Format quota as a compact string for the welcome banner.
    /// e.g. "12K / 44K" or "44K / 44K (exhausted)"
    pub fn display_compact(&self) -> String {
        let used_k = self.azure_output_tokens_used / 1000;
        let limit_k = self.daily_limit / 1000;
        if self.is_azure_exhausted() {
            format!("{}K / {}K (exhausted)", used_k, limit_k)
        } else {
            format!("{}K / {}K", used_k, limit_k)
        }
    }
}

fn quota_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".neuroncli").join("quota.json")
}

/// Current UTC date as YYYY-MM-DD without pulling in chrono.
fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Unix epoch was 1970-01-01 (Thursday). Calculate date from days.
    let days = secs / 86400;
    epoch_days_to_date(days)
}

/// Convert days since 1970-01-01 to YYYY-MM-DD.
fn epoch_days_to_date(days: u64) -> String {
    // Civil calendar algorithm (Howard Hinnant)
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_to_date_known_values() {
        assert_eq!(epoch_days_to_date(0), "1970-01-01");
        assert_eq!(epoch_days_to_date(18_262), "2020-01-01");
        assert_eq!(epoch_days_to_date(19_723), "2024-01-01");
    }

    #[test]
    fn default_quota_has_correct_limit() {
        let q = QuotaState::default();
        assert_eq!(q.daily_limit, DAILY_AZURE_TOKEN_LIMIT);
        assert_eq!(q.azure_output_tokens_used, 0);
        assert!(!q.is_azure_exhausted());
    }

    #[test]
    fn record_usage_saturates_quota() {
        let mut q = QuotaState::default();
        q.daily_limit = 100;
        assert!(q.record_azure_usage(50)); // still available
        assert_eq!(q.remaining(), 50);
        assert!(!q.record_azure_usage(60)); // exhausted
        assert!(q.is_azure_exhausted());
    }

    #[test]
    fn display_compact_shows_exhausted() {
        let mut q = QuotaState::default();
        q.azure_output_tokens_used = 44_000;
        assert!(q.display_compact().contains("exhausted"));
    }
}
