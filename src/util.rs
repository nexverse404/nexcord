//! Small, allocation-conscious formatting helpers used by the UI and search layers.

use std::time::Duration;

/// `Duration` -> `m:ss` / `h:mm:ss`.
pub fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// yt-dlp reports duration as a float number of seconds.
pub fn fmt_secs(secs: Option<f64>) -> String {
    match secs {
        Some(s) if s.is_finite() && s >= 0.0 => {
            fmt_duration(Duration::from_secs(s.max(0.0) as u64))
        }
        _ => "live".to_owned(),
    }
}

/// Truncate by characters (not bytes) so non-ASCII titles never panic.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if s.chars().count() > max {
        out.push('…');
        out
    } else {
        s.to_owned()
    }
}

/// `custom_id` and select-menu values are limited to 100 **bytes**. Cap at a
/// UTF-8 boundary so a long id can never produce an invalid component payload.
pub fn clamp_id(s: &str) -> String {
    const LIMIT: usize = 100;
    if s.len() <= LIMIT {
        return s.to_owned();
    }
    let mut end = LIMIT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// Escape Discord markdown so track titles cannot break an embed's layout.
pub fn clean_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' | '*' | '_' | '`' | '|' | '>' | '~' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// A single-line progress bar with a position knob.
///
/// `▰`/`▱` (the old bar) render as a row of empty boxes in Discord's font, which
/// looks broken at 0:00. `━`, `─` and `●` are box-drawing characters that join
/// into one continuous line — the same shape professional players use.
pub fn progress_bar(position: Duration, total: Option<Duration>, width: usize) -> String {
    let width = width.max(2);
    let total = total.unwrap_or_default();
    let ratio = if total.as_secs_f64() > 0.0 {
        (position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (ratio * (width - 1) as f64).round() as usize;
    let mut bar = String::with_capacity(width * 3);
    bar.push_str(&"━".repeat(filled));
    bar.push('●');
    bar.push_str(&"─".repeat(width - 1 - filled));
    bar
}

/// Parse `1:23`, `01:02:03` or plain seconds into a `Duration`.
pub fn parse_time(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<f64>() {
        if secs.is_finite() && secs >= 0.0 {
            return Some(Duration::from_secs_f64(secs));
        }
        return None;
    }
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0u64;
    for part in parts {
        let n = part.trim().parse::<u64>().ok()?;
        total = total.checked_mul(60)?.checked_add(n)?;
    }
    Some(Duration::from_secs(total))
}

/// Lowercase + keep only alphanumerics; used for search scoring and dedupe keys.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_owned()
}

pub fn tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .filter(|t| t.len() > 1)
        .map(str::to_owned)
        .collect()
}

pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("www.")
        || s.starts_with("youtube:")
        || s.starts_with("sc:")
}

/// In-place Fisher–Yates with a tiny xorshift PRNG.
/// Avoids pulling in a whole RNG crate for one button.
pub fn shuffle<T>(items: &mut [T]) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        ^ 0x9e37_79b9_7f4a_7c15;
    for i in (1..items.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        assert_eq!(fmt_duration(Duration::from_secs(83)), "1:23");
        assert_eq!(fmt_duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(fmt_secs(None), "live");
    }

    #[test]
    fn parses_seek() {
        assert_eq!(parse_time("1:23"), Some(Duration::from_secs(83)));
        assert_eq!(parse_time("1:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_time("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_time("nope"), None);
    }

    #[test]
    fn truncates_unicode() {
        assert_eq!(truncate("日本語の曲名です", 4), "日本語…");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn progress_bar_has_one_knob_and_constant_width() {
        let total = Some(Duration::from_secs(100));
        let empty = progress_bar(Duration::ZERO, total, 16);
        let half = progress_bar(Duration::from_secs(50), total, 16);
        let full = progress_bar(Duration::from_secs(100), total, 16);
        assert_eq!(empty.chars().filter(|c| *c == '●').count(), 1);
        assert_eq!(half.chars().filter(|c| *c == '●').count(), 1);
        assert_eq!(full.chars().filter(|c| *c == '●').count(), 1);
        assert_eq!(empty.chars().count(), 16);
        assert_eq!(half.chars().count(), 16);
        assert_eq!(full.chars().count(), 16);
        assert!(half.contains('━'));
        assert!(empty.contains('─'));
        assert!(!empty.contains('▱'));
    }

    #[test]
    fn clamps_component_ids_without_splitting_utf8() {
        assert_eq!(clamp_id("nx:top:s1"), "nx:top:s1");
        let clamped = clamp_id(&"é".repeat(80));
        assert!(clamped.len() <= 100);
        assert!(clamped.chars().all(|c| c == 'é'));
    }
}
