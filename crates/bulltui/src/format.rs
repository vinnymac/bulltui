//! Display formatting helpers (durations, timestamps, bytes, JSON).

use chrono::{Local, TimeZone};
use serde_json::Value;

/// Format an epoch-millis timestamp as a local datetime, or `—` if absent.
pub fn datetime(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => match Local.timestamp_millis_opt(ms).single() {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => "—".to_string(),
        },
        None => "—".to_string(),
    }
}

/// Format an epoch-millis timestamp as a local `HH:MM:SS` (for the live feed).
pub fn time_only(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M:%S").to_string(),
        None => "—".to_string(),
    }
}

/// Format an epoch-millis timestamp relative to now (e.g. "5m ago", "in 2h").
pub fn relative(ms: Option<i64>, now_ms: i64) -> String {
    let ms = match ms {
        Some(v) => v,
        None => return "—".to_string(),
    };
    let diff = now_ms - ms;
    let (suffix, abs) = if diff >= 0 {
        ("ago", diff)
    } else {
        ("from now", -diff)
    };
    format!("{} {}", human_duration(abs), suffix)
}

/// A forward countdown to `target_ms` from `now_ms`: "in 5m 3s", "due", or
/// "overdue 2m" once the target has passed.
pub fn countdown(target_ms: Option<i64>, now_ms: i64) -> String {
    match target_ms {
        Some(t) => {
            let d = t - now_ms;
            if d.abs() < 1000 {
                "due".to_string()
            } else if d > 0 {
                format!("in {}", human_duration(d))
            } else {
                format!("overdue {}", human_duration(-d))
            }
        }
        None => "—".to_string(),
    }
}

/// Humanize a duration in milliseconds (e.g. "1d 2h", "350ms").
pub fn human_duration(ms: i64) -> String {
    if ms < 0 {
        return format!("-{}", human_duration(-ms));
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1000;
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Duration between two optional timestamps, if both present.
pub fn duration_between(start: Option<i64>, end: Option<i64>) -> String {
    match (start, end) {
        (Some(s), Some(e)) => human_duration(e - s),
        _ => "—".to_string(),
    }
}

/// Humanize a byte count.
pub fn bytes(n: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 0 {
        return "—".to_string();
    }
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Truncate a string to `max` display columns, adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else if max == 1 {
        "…".to_string()
    } else {
        let kept: String = chars[..max - 1].iter().collect();
        format!("{kept}…")
    }
}

/// Collapse a string into a single line (for table cells).
pub fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Pretty-print a JSON value over multiple lines.
pub fn pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Pretty-print a raw JSON string (falling back to the raw text).
pub fn pretty_json_str(s: &str) -> String {
    match serde_json::from_str::<Value>(s) {
        Ok(v) => pretty_json(&v),
        Err(_) => s.to_string(),
    }
}

/// Render a progress value as a short string.
pub fn progress(v: &Value) -> String {
    match v {
        Value::Number(n) => format!("{n}%"),
        Value::Null => "—".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        other => one_line(&other.to_string()),
    }
}

/// Parse a delay into milliseconds. Accepts a bare integer (ms) or a single
/// suffixed value: `500ms`, `30s`, `5m`, `2h`, `1d`. Returns `None` if invalid.
pub fn parse_delay(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return if n < 0 { None } else { Some(n) };
    }
    let split = s.find(|c: char| c.is_alphabetic())?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.trim().parse().ok()?;
    if n < 0.0 {
        return None;
    }
    let mult = match unit.trim() {
        "ms" => 1.0,
        "s" => 1_000.0,
        "m" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        _ => return None,
    };
    Some((n * mult) as i64)
}

/// Current time in epoch millis.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_durations() {
        assert_eq!(human_duration(500), "500ms");
        assert_eq!(human_duration(5_000), "5s");
        assert_eq!(human_duration(65_000), "1m 5s");
        assert_eq!(human_duration(3_700_000), "1h 1m");
        assert_eq!(human_duration(90_000_000), "1d 1h");
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1048576), "1.0 MB");
    }

    #[test]
    fn truncates() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("hi", 0), "");
    }

    #[test]
    fn parses_delays() {
        assert_eq!(parse_delay("0"), Some(0));
        assert_eq!(parse_delay("1500"), Some(1500));
        assert_eq!(parse_delay("500ms"), Some(500));
        assert_eq!(parse_delay("30s"), Some(30_000));
        assert_eq!(parse_delay("5m"), Some(300_000));
        assert_eq!(parse_delay("2h"), Some(7_200_000));
        assert_eq!(parse_delay("1d"), Some(86_400_000));
        assert_eq!(parse_delay(""), None);
        assert_eq!(parse_delay("soon"), None);
        assert_eq!(parse_delay("-5"), None);
    }
}
