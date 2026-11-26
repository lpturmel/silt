//! macOS APFS local snapshot inspection and confirmed pruning.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApfsSnapshot {
    pub name: String,
    pub uuid: Option<String>,
    pub xid: Option<u64>,
    pub date_stamp: Option<String>,
    pub purgeable: bool,
    pub limits_shrink: bool,
    pub mount_point: String,
}

/// Extract a Time Machine date stamp (`YYYY-MM-DD-HHMMSS`) from a snapshot name
/// like `com.apple.TimeMachine.2026-11-25-143022.local`.
pub fn extract_tm_date_stamp(name: &str) -> Option<String> {
    let stripped = name.strip_prefix("com.apple.TimeMachine.")?;
    let date_part = stripped
        .strip_suffix(".local")
        .or_else(|| stripped.strip_suffix(".backup"))
        .unwrap_or(stripped);
    if date_part.len() >= 17 && date_part.chars().filter(|&c| c == '-').count() >= 3 {
        Some(date_part.to_string())
    } else {
        None
    }
}

/// Enumerate APFS local snapshots on a mount point using `diskutil apfs listSnapshots -plist`,
/// falling back to `tmutil listlocalsnapshots`.
pub fn list_apfs_snapshots(mount_point: &Path) -> Vec<ApfsSnapshot> {
    let mount_str = mount_point.to_string_lossy().to_string();

    if let Ok(output) = Command::new("diskutil")
        .args(["apfs", "listSnapshots", &mount_str, "-plist"])
        .output()
        && output.status.success()
        && let Ok(parsed) = parse_diskutil_snapshots_plist(&output.stdout, &mount_str)
    {
        return parsed;
    }

    // Fallback: tmutil listlocalsnapshots
    if let Ok(output) = Command::new("tmutil")
        .args(["listlocalsnapshots", &mount_str])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_tmutil_snapshots_text(&stdout, &mount_str);
    }

    Vec::new()
}

pub fn parse_diskutil_snapshots_plist(bytes: &[u8], mount_point: &str) -> Result<Vec<ApfsSnapshot>> {
    let val: plist::Value = plist::from_bytes(bytes).context("parse diskutil plist")?;
    let dict = val
        .as_dictionary()
        .context("expected dictionary root in diskutil output")?;
    let Some(snaps) = dict.get("Snapshots").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut result = Vec::with_capacity(snaps.len());
    for item in snaps {
        let Some(d) = item.as_dictionary() else {
            continue;
        };
        let name = d
            .get("SnapshotName")
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let uuid = d
            .get("SnapshotUUID")
            .and_then(|v| v.as_string())
            .map(ToOwned::to_owned);
        let xid = d.get("SnapshotXID").and_then(|v| v.as_unsigned_integer());
        let purgeable = d
            .get("Purgeable")
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);
        let limits_shrink = d
            .get("LimitingContainerShrink")
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);
        let date_stamp = extract_tm_date_stamp(&name);

        result.push(ApfsSnapshot {
            name,
            uuid,
            xid,
            date_stamp,
            purgeable,
            limits_shrink,
            mount_point: mount_point.to_string(),
        });
    }
    Ok(result)
}

pub fn parse_tmutil_snapshots_text(text: &str, mount_point: &str) -> Vec<ApfsSnapshot> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("com.apple."))
        .map(|name| ApfsSnapshot {
            name: name.to_string(),
            uuid: None,
            xid: None,
            date_stamp: extract_tm_date_stamp(name),
            purgeable: true,
            limits_shrink: false,
            mount_point: mount_point.to_string(),
        })
        .collect()
}

/// Delete a local Time Machine APFS snapshot by date stamp (e.g., `2026-11-25-143022`).
/// Only call this after explicit user confirmation.
pub fn delete_local_snapshot(date_stamp: &str) -> Result<String> {
    if date_stamp.is_empty()
        || !date_stamp
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-')
    {
        anyhow::bail!("invalid Time Machine snapshot date stamp: {date_stamp}");
    }

    let output = Command::new("tmutil")
        .args(["deletelocalsnapshots", date_stamp])
        .output()
        .context("failed to execute tmutil deletelocalsnapshots")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        anyhow::bail!(
            "tmutil deletelocalsnapshots failed ({}): {}",
            output.status,
            if stderr.is_empty() { stdout } else { stderr }
        );
    }

    Ok(if stdout.is_empty() {
        format!("Deleted local snapshot {date_stamp}")
    } else {
        stdout
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tm_date_stamp() {
        assert_eq!(
            extract_tm_date_stamp("com.apple.TimeMachine.2026-11-25-143022.local"),
            Some("2026-11-25-143022".to_string())
        );
        assert_eq!(
            extract_tm_date_stamp("com.apple.os.update-ABC123"),
            None
        );
    }

    #[test]
    fn parses_tmutil_text_output() {
        let sample = "\
Snapshots for disk /:
com.apple.TimeMachine.2026-11-24-090000.local
com.apple.TimeMachine.2026-11-25-180000.local
";
        let snaps = parse_tmutil_snapshots_text(sample, "/");
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].date_stamp.as_deref(), Some("2026-11-24-090000"));
        assert!(snaps[0].purgeable);
    }
}
