//! Safe cleanup staging cart, protected-path guardrails, macOS Trash execution,
//! and APFS local snapshot pruning.

use crate::apfs::delete_local_snapshot;
use crate::attribution::SafetyTier;
use crate::db::Database;
use anyhow::{Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeleteMode {
    /// Default reversible cleanup: moves files/folders to macOS Trash (supports Put Back).
    Trash,
    /// Permanent deletion (explicit user confirmation only).
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CleanupTarget {
    Path {
        path: PathBuf,
        label: String,
        bytes: u64,
        safety_tier: SafetyTier,
        app_id: String,
    },
    ApfsSnapshot {
        mount_point: String,
        date_stamp: String,
        name: String,
        estimated_bytes: u64,
    },
}

impl CleanupTarget {
    pub fn id(&self) -> String {
        match self {
            Self::Path { path, .. } => format!("path:{}", path.to_string_lossy()),
            Self::ApfsSnapshot { date_stamp, .. } => format!("snapshot:{date_stamp}"),
        }
    }

    pub fn bytes(&self) -> u64 {
        match self {
            Self::Path { bytes, .. } => *bytes,
            Self::ApfsSnapshot {
                estimated_bytes, ..
            } => *estimated_bytes,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Path { label, .. } => label,
            Self::ApfsSnapshot { name, .. } => name,
        }
    }
}

/// Hard-block system-critical and sensitive credential paths from ever being staged or deleted.
pub fn is_protected_path(path: &Path) -> bool {
    let normalized = path.to_string_lossy();
    let exact_or_parent = [
        "/",
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/private/etc",
        "/private/var/db",
        "/Library/Apple",
        "/Applications",
        "/Users",
    ];
    for p in exact_or_parent {
        if path == Path::new(p) {
            return true;
        }
    }

    let protected_prefixes = [
        "/System/",
        "/usr/",
        "/bin/",
        "/sbin/",
        "/private/etc/",
        "/Library/Apple/",
    ];
    for prefix in protected_prefixes {
        if normalized.starts_with(prefix) {
            return true;
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        if path == home_path
            || path == home_path.join("Library")
            || path == home_path.join("Documents")
            || path == home_path.join("Desktop")
            || path == home_path.join("Pictures")
            || path.starts_with(home_path.join(".ssh"))
            || path.starts_with(home_path.join(".gnupg"))
            || path.starts_with(home_path.join(".aws"))
            || path.starts_with(home_path.join(".config/gh"))
        {
            return true;
        }
    }

    false
}

/// Deduplicated Cleanup Staging Cart.
///
/// Staging a parent directory automatically removes any already-staged child paths
/// so reclaimable byte estimates are never double-counted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StagingCart {
    items: Vec<CleanupTarget>,
}

impl StagingCart {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[CleanupTarget] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn contains_id(&self, id: &str) -> bool {
        self.items.iter().any(|item| item.id() == id)
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        self.items.iter().any(|item| match item {
            CleanupTarget::Path { path: p, .. } => p == path || path.starts_with(p),
            CleanupTarget::ApfsSnapshot { .. } => false,
        })
    }

    pub fn stage(&mut self, target: CleanupTarget) -> Result<()> {
        match &target {
            CleanupTarget::Path {
                path, safety_tier, ..
            } => {
                if *safety_tier == SafetyTier::SystemProtected || is_protected_path(path) {
                    bail!("refusing to stage protected path: {}", path.display());
                }
                // Already covered by an existing ancestor (or exact duplicate)
                if self.contains_path(path) {
                    return Ok(());
                }
                // Remove any staged children subsumed by this parent path
                self.items.retain(|existing| match existing {
                    CleanupTarget::Path { path: child, .. } => !child.starts_with(path),
                    CleanupTarget::ApfsSnapshot { .. } => true,
                });
                self.items.push(target);
            }
            CleanupTarget::ApfsSnapshot { .. } => {
                let id = target.id();
                if !self.contains_id(&id) {
                    self.items.push(target);
                }
            }
        }
        Ok(())
    }

    pub fn unstage(&mut self, id: &str) {
        self.items.retain(|item| item.id() != id);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn total_reclaimable_bytes(&self) -> u64 {
        self.items.iter().map(CleanupTarget::bytes).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupExecutionReport {
    pub succeeded: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub bytes_reclaimed: u64,
}

/// Execute all items in `cart`, defaulting file deletions to macOS Trash (`DeleteMode::Trash`)
/// and recording a milestone in `cleanup_events` when `db` is provided.
pub fn execute_cart(
    cart: &StagingCart,
    mode: DeleteMode,
    db: Option<&Database>,
    volume_uuid: &str,
) -> CleanupExecutionReport {
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut bytes_reclaimed: u64 = 0;

    for target in cart.items() {
        match target {
            CleanupTarget::Path {
                path,
                label,
                bytes,
                safety_tier,
                ..
            } => {
                if *safety_tier == SafetyTier::SystemProtected || is_protected_path(path) {
                    failed.push((
                        label.clone(),
                        format!("Blocked protected path {}", path.display()),
                    ));
                    continue;
                }
                if !path.exists() {
                    succeeded.push(format!("{label} (already removed)"));
                    continue;
                }
                let res = match mode {
                    DeleteMode::Trash => trash::delete(path).map_err(anyhow::Error::from),
                    DeleteMode::Permanent => {
                        if path.is_dir() {
                            std::fs::remove_dir_all(path).map_err(anyhow::Error::from)
                        } else {
                            std::fs::remove_file(path).map_err(anyhow::Error::from)
                        }
                    }
                };
                match res {
                    Ok(()) => {
                        bytes_reclaimed = bytes_reclaimed.saturating_add(*bytes);
                        succeeded.push(label.clone());
                    }
                    Err(err) => failed.push((label.clone(), err.to_string())),
                }
            }
            CleanupTarget::ApfsSnapshot {
                date_stamp,
                name,
                estimated_bytes,
                ..
            } => match delete_local_snapshot(date_stamp) {
                Ok(_) => {
                    bytes_reclaimed = bytes_reclaimed.saturating_add(*estimated_bytes);
                    succeeded.push(name.clone());
                }
                Err(err) => failed.push((name.clone(), err.to_string())),
            },
        }
    }

    if !succeeded.is_empty()
        && let Some(db) = db
    {
        let action_kind = match mode {
            DeleteMode::Trash => "trash",
            DeleteMode::Permanent => "permanent",
        };
        let summary = if succeeded.len() == 1 {
            format!("Cleaned {}", succeeded[0])
        } else {
            format!("Cleaned {} items ({}, ...)", succeeded.len(), succeeded[0])
        };
        let _ = db.record_cleanup_event(
            volume_uuid,
            Utc::now().timestamp(),
            action_kind,
            &summary,
            bytes_reclaimed,
        );
    }

    CleanupExecutionReport {
        succeeded,
        failed,
        bytes_reclaimed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn staging_cart_deduplicates_nested_paths_and_blocks_protected() {
        let mut cart = StagingCart::new();

        // Protected paths rejected
        assert!(
            cart.stage(CleanupTarget::Path {
                path: PathBuf::from("/System/Library"),
                label: "System".into(),
                bytes: 100,
                safety_tier: SafetyTier::SafeCache,
                app_id: "sys".into(),
            })
            .is_err()
        );

        let dir = tempdir().unwrap();
        let parent = dir.path().join("cache_root");
        let child = parent.join("sub");

        cart.stage(CleanupTarget::Path {
            path: child.clone(),
            label: "sub".into(),
            bytes: 400,
            safety_tier: SafetyTier::SafeCache,
            app_id: "app".into(),
        })
        .unwrap();
        assert_eq!(cart.total_reclaimable_bytes(), 400);

        // Staging parent subsumes child cleanly
        cart.stage(CleanupTarget::Path {
            path: parent.clone(),
            label: "cache_root".into(),
            bytes: 1_000,
            safety_tier: SafetyTier::SafeCache,
            app_id: "app".into(),
        })
        .unwrap();
        assert_eq!(cart.len(), 1);
        assert_eq!(cart.total_reclaimable_bytes(), 1_000);

        // Staging child again is a no-op because parent is already staged
        cart.stage(CleanupTarget::Path {
            path: child,
            label: "sub".into(),
            bytes: 400,
            safety_tier: SafetyTier::SafeCache,
            app_id: "app".into(),
        })
        .unwrap();
        assert_eq!(cart.len(), 1);
    }

    #[test]
    fn permanent_cleanup_removes_temp_files_and_logs_event() {
        let dir = tempdir().unwrap();
        let target_file = dir.path().join("junk.tmp");
        std::fs::write(&target_file, vec![0u8; 4096]).unwrap();

        let db = Database::open_in_memory().unwrap();
        let vol = crate::volume::VolumeMetrics {
            volume_uuid: "v1".into(),
            name: "Test".into(),
            mount_point: dir.path().to_path_buf(),
            is_internal: true,
            is_removable: false,
            is_online: true,
            is_ignored: false,
            ignore_reason: None,
            total_bytes: 1_000_000,
            used_bytes: 500_000,
            physical_free_bytes: 500_000,
            available_important_bytes: 500_000,
            available_opportunistic_bytes: 500_000,
            purgeable_bytes: 0,
            snapshot_count: 0,
            snapshots: Vec::new(),
            sampled_at: Utc::now().timestamp(),
        };
        db.upsert_volume(&vol).unwrap();

        let mut cart = StagingCart::new();
        cart.stage(CleanupTarget::Path {
            path: target_file.clone(),
            label: "junk.tmp".into(),
            bytes: 4096,
            safety_tier: SafetyTier::SafeCache,
            app_id: "test".into(),
        })
        .unwrap();

        let report = execute_cart(&cart, DeleteMode::Permanent, Some(&db), "v1");
        assert_eq!(report.succeeded.len(), 1);
        assert!(report.failed.is_empty());
        assert!(!target_file.exists());

        let events = db.list_cleanup_events("v1", 0).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].bytes_reclaimed, 4096);
    }
}
