//! Human-readable byte, duration, path, and rate formatting helpers.

use std::path::Path;

/// Format an unsigned byte count using macOS Finder-compatible decimal SI units
/// (KB, MB, GB, TB) so numbers match macOS Finder & System Settings.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    let mut val = bytes as f64;
    let mut unit_idx = 0;
    while val >= 1_000.0 && unit_idx < UNITS.len() - 1 {
        val /= 1_000.0;
        unit_idx += 1;
    }
    if val >= 100.0 {
        format!("{val:.0} {}", UNITS[unit_idx])
    } else if val >= 10.0 {
        format!("{val:.1} {}", UNITS[unit_idx])
    } else {
        format!("{val:.2} {}", UNITS[unit_idx])
    }
}

/// Format a signed byte delta with explicit `+` / `-` sign.
pub fn format_signed_bytes(delta: i64) -> String {
    if delta == 0 {
        return "0 B".to_string();
    }
    let sign = if delta > 0 { "+" } else { "-" };
    format!("{sign}{}", format_bytes(delta.unsigned_abs()))
}

/// Format a growth rate in bytes per day (e.g., `+1.2 GB/day`).
pub fn format_rate_per_day(bytes_per_day: f64) -> String {
    if bytes_per_day.abs() < 1.0 {
        return "0 B/day".to_string();
    }
    let sign = if bytes_per_day > 0.0 { "+" } else { "-" };
    format!("{sign}{}/day", format_bytes(bytes_per_day.abs() as u64))
}

/// Shorten a filesystem path for UI display: replaces `$HOME` with `~` and
/// middle-truncates with `…` if the string exceeds `max_chars`.
pub fn compact_display_path(path: &Path, max_chars: usize) -> String {
    let raw = path.to_string_lossy();
    let display = if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = raw.strip_prefix(&home)
    {
        if rest.is_empty() {
            "~".to_string()
        } else if rest.starts_with('/') {
            format!("~{rest}")
        } else {
            raw.into_owned()
        }
    } else {
        raw.into_owned()
    };

    let chars: Vec<char> = display.chars().collect();
    if chars.len() <= max_chars || max_chars < 10 {
        return display;
    }
    let head = (max_chars - 1) / 3;
    let tail = (max_chars - 1) - head;
    let prefix: String = chars[..head].iter().collect();
    let suffix: String = chars[chars.len() - tail..].iter().collect();
    format!("{prefix}…{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decimal_si_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.50 KB");
        assert_eq!(format_bytes(42_500_000), "42.5 MB");
        assert_eq!(format_bytes(512_000_000_000), "512 GB");
    }

    #[test]
    fn formats_signed_deltas() {
        assert_eq!(format_signed_bytes(0), "0 B");
        assert_eq!(format_signed_bytes(2_500_000), "+2.50 MB");
        assert_eq!(format_signed_bytes(-15_000_000_000), "-15.0 GB");
    }

    #[test]
    fn compacts_long_paths_middle_ellipsis() {
        let p = Path::new("/Library/Group Containers/UBF8T346G9.com.microsoft.teams/Data/VeryLongNestedCacheFolder");
        let compact = compact_display_path(p, 40);
        assert!(compact.chars().count() <= 40);
        assert!(compact.contains('…'));
        assert!(compact.ends_with("NestedCacheFolder"));
    }
}
