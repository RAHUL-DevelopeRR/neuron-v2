//! Minimal UTC time helpers for session expiry and quota reset times, so the
//! CLI does not need a date/time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Parses an RFC 3339 / ISO 8601 timestamp (`2026-10-17T00:00:00Z`,
/// `2026-10-17T05:30:00.5+05:30`, `2026-10-17`) or a bare number of Unix
/// seconds (milliseconds are accepted when the value is clearly too large).
pub fn parse_timestamp(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Ok(number) = text.parse::<u64>() {
        // Values past year 5138 in seconds are almost certainly milliseconds.
        return Some(if number > 100_000_000_000 {
            number / 1000
        } else {
            number
        });
    }
    let (date, time) = match text.find(['T', 't', ' ']) {
        Some(split) => (&text[..split], &text[split + 1..]),
        None => (text, ""),
    };
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (clock, offset_seconds) = split_offset(time)?;
    let mut clock_parts = clock.split(':');
    let hour = parse_clock_field(clock_parts.next(), 23)?;
    let minute = parse_clock_field(clock_parts.next(), 59)?;
    let second = match clock_parts.next() {
        Some(seconds) => {
            let whole = seconds.split('.').next().unwrap_or_default();
            parse_clock_field(Some(whole), 60)?
        }
        None => 0,
    };

    let days = days_from_civil(year, month, day);
    let local = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(local - offset_seconds).ok()
}

fn parse_clock_field(field: Option<&str>, max: i64) -> Option<i64> {
    match field {
        None | Some("") => Some(0),
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|value| (0..=max).contains(value)),
    }
}

/// Splits `HH:MM:SS(.frac)` from a trailing `Z` or `±HH:MM` offset and
/// returns the offset in seconds east of UTC.
fn split_offset(time: &str) -> Option<(&str, i64)> {
    if time.is_empty() {
        return Some(("", 0));
    }
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    if let Some(sign_at) = time.rfind(['+', '-']) {
        let (clock, offset) = time.split_at(sign_at);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let digits = offset[1..].replace(':', "");
        if digits.len() != 4 {
            return None;
        }
        let hours = digits[..2].parse::<i64>().ok()?;
        let minutes = digits[2..].parse::<i64>().ok()?;
        return Some((clock, sign * (hours * 3_600 + minutes * 60)));
    }
    Some((time, 0))
}

/// Formats Unix seconds as `2026-10-17 00:00 UTC`.
pub fn format_utc(unix: u64) -> String {
    let unix = i64::try_from(unix).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(unix.div_euclid(86_400));
    let seconds_of_day = unix.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60
    )
}

/// Human description of how far `target` is from `now`
/// (`in 6d 2h`, `in 45m`, `5m ago`).
pub fn describe_relative(target: u64, now: u64) -> String {
    let (delta, future) = if target >= now {
        (target - now, true)
    } else {
        (now - target, false)
    };
    let days = delta / 86_400;
    let hours = (delta % 86_400) / 3_600;
    let minutes = (delta % 3_600) / 60;
    let span = if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    };
    if future {
        format!("in {span}")
    } else {
        format!("{span} ago")
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{describe_relative, format_utc, parse_timestamp};

    #[test]
    fn parses_rfc3339_offsets_fractions_and_unix_numbers() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2026-10-17T00:00:00Z"), Some(1_792_195_200));
        assert_eq!(
            parse_timestamp("2026-10-17T05:30:00.250+05:30"),
            Some(1_792_195_200)
        );
        assert_eq!(parse_timestamp("2026-10-17"), Some(1_792_195_200));
        assert_eq!(parse_timestamp("1792195200"), Some(1_792_195_200));
        assert_eq!(parse_timestamp("1792195200000"), Some(1_792_195_200));
        assert_eq!(parse_timestamp("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(parse_timestamp("not a date"), None);
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn formats_utc_and_relative_spans() {
        assert_eq!(format_utc(1_792_195_200), "2026-10-17 00:00 UTC");
        assert_eq!(format_utc(1_709_208_000), "2024-02-29 12:00 UTC");
        assert_eq!(describe_relative(1_000 + 90_000, 1_000), "in 1d 1h");
        assert_eq!(describe_relative(1_000 + 3_660, 1_000), "in 1h 1m");
        assert_eq!(describe_relative(1_000, 1_000 + 300), "5m ago");
        assert_eq!(describe_relative(1_000, 1_000), "in 1m");
    }
}
