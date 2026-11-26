//! Human-readable byte, duration, and rate formatting helpers.

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
}
