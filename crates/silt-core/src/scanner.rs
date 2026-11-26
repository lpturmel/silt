//! High-speed APFS-correct parallel filesystem scanner.
//!
//! Uses `st_blocks * 512` (physical allocated blocks) with macOS `SF_DATALESS`
//! detection so iCloud Drive stubs and sparse VM images never inflate totals,
//! deduplicates hardlinks by `(st_dev, st_ino)`, and captures Day-1 `mtime`/`birthtime`
//! recent-activity buckets for immediate cold-start creep insights.

use crate::attribution::{
    AppLocation, AppRollup, AttributionEngine, BucketKind, CleanupCandidate, SafetyTier,
};
use crate::volume::VolumeMetrics;
use anyhow::Result;
use chrono::Utc;
use jwalk::WalkDir;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

/// macOS `SF_DATALESS` flag (`<sys/stat.h>`): file content is evicted to iCloud / cloud storage.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub volume_uuid: String,
    pub mount_point: PathBuf,
    pub roots: Vec<PathBuf>,
    pub trigger_kind: String,
    pub max_persisted_depth: u32,
    pub min_persisted_bytes: u64,
    pub include_private_var: bool,
}

impl ScanConfig {
    /// Default scan scope: for internal macOS drive, scans `$HOME`, `/Applications`,
    /// `/Library` (and optionally `/private/var`); for external drives, scans the mount point.
    pub fn default_for_volume(vol: &VolumeMetrics, trigger_kind: &str) -> Self {
        let roots = if vol.is_internal && vol.mount_point == Path::new("/") {
            let mut r = Vec::new();
            if let Ok(home) = std::env::var("HOME") {
                r.push(PathBuf::from(home));
            }
            for p in ["/Applications", "/Library"] {
                let pb = PathBuf::from(p);
                if pb.exists() {
                    r.push(pb);
                }
            }
            r
        } else {
            vec![vol.mount_point.clone()]
        };

        Self {
            volume_uuid: vol.volume_uuid.clone(),
            mount_point: vol.mount_point.clone(),
            roots,
            trigger_kind: trigger_kind.to_string(),
            max_persisted_depth: 5,
            min_persisted_bytes: 5 * 1_000_000, // 5 MB threshold for deep directories
            include_private_var: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub skipped_permissions: u64,
    pub current_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirAggregate {
    pub path: PathBuf,
    pub parent_path: Option<PathBuf>,
    pub depth: u32,
    pub physical_bytes: u64,
    pub logical_bytes: u64,
    pub recent_7d_bytes: u64,
    pub recent_30d_bytes: u64,
    pub file_count: u64,
    pub app_id: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub volume_uuid: String,
    pub taken_at: i64,
    pub trigger_kind: String,
    pub total_scanned_bytes: u64,
    pub total_logical_bytes: u64,
    pub total_files: u64,
    pub skipped_paths: u64,
    pub dirs: Vec<DirAggregate>,
    pub app_rollups: Vec<AppRollup>,
    pub cleanup_candidates: Vec<CleanupCandidate>,
}

#[derive(Default)]
struct DirAcc {
    physical_bytes: u64,
    logical_bytes: u64,
    recent_7d_bytes: u64,
    recent_30d_bytes: u64,
    file_count: u64,
}

#[derive(Default)]
struct AppAcc {
    display_name: String,
    category: String,
    is_orphaned: bool,
    app_bundle_bytes: u64,
    data_bytes: u64,
    cache_bytes: u64,
    logs_bytes: u64,
    downloads_bytes: u64,
    dev_artifacts_bytes: u64,
    recent_7d_bytes: u64,
    recent_30d_bytes: u64,
    total_bytes: u64,
    reclaimable_bytes: u64,
    locations: HashMap<PathBuf, (BucketKind, SafetyTier, u64, u64)>,
}

struct CandidateAcc {
    label: String,
    app_id: String,
    display_name: String,
    category: String,
    safety_tier: SafetyTier,
    bytes: u64,
    recent_7d_bytes: u64,
    reason: String,
}

/// Compute true physical disk allocation for a regular file on macOS/Unix.
pub fn file_physical_bytes(meta: &std::fs::Metadata) -> u64 {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as MacMetaExt;
        if (meta.st_flags() & SF_DATALESS) != 0 {
            return 0;
        }
    }
    let blocks_bytes = meta.blocks().saturating_mul(512);
    // On some synthetic/tmpfs filesystems blocks may be 0 for tiny inline files;
    // on APFS `st_blocks * 512` accurately reflects sparse & compressed allocation.
    if blocks_bytes == 0 && meta.len() > 0 && meta.len() <= 4096 {
        meta.len()
    } else {
        blocks_bytes
    }
}

fn file_activity_timestamp(meta: &std::fs::Metadata) -> i64 {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let btime = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    mtime.max(btime)
}

/// Scan the configured volume roots in parallel and produce directory aggregates,
/// unified app rollups, and safe cleanup candidates.
pub fn scan_volume(
    config: &ScanConfig,
    attribution: &AttributionEngine,
    progress_cb: Option<&dyn Fn(ScanProgress)>,
) -> Result<ScanResult> {
    let now = Utc::now().timestamp();
    let cutoff_7d = now - 7 * 86_400;
    let cutoff_30d = now - 30 * 86_400;

    let mut roots = config.roots.clone();
    if config.include_private_var && config.mount_point == Path::new("/") {
        let pv = PathBuf::from("/private/var");
        if pv.exists() && !roots.contains(&pv) {
            roots.push(pv);
        }
    }

    let mut dir_map: HashMap<PathBuf, DirAcc> = HashMap::with_capacity(16_384);
    let mut app_map: HashMap<String, AppAcc> = HashMap::with_capacity(512);
    let mut candidate_map: HashMap<PathBuf, CandidateAcc> = HashMap::with_capacity(256);
    let mut seen_hardlinks: HashSet<(u64, u64)> = HashSet::new();

    let files_counter = AtomicU64::new(0);
    let bytes_counter = AtomicU64::new(0);
    let skipped_counter = AtomicU64::new(0);
    let mut total_logical_bytes: u64 = 0;

    for root in &roots {
        let Ok(root_meta) = std::fs::symlink_metadata(root) else {
            skipped_counter.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let root_dev = root_meta.dev();

        let walker = WalkDir::new(root)
            .skip_hidden(false)
            .follow_links(false)
            .process_read_dir(move |_depth, _path, _state, children| {
                // Prune virtual, firmlink, and cross-device mount directories BEFORE descending
                // so mounted APFS sub-volumes (e.g. ~/dev2 on a separate disk) are never walked.
                for child in children.iter_mut().flatten() {
                    if !child.file_type.is_dir() {
                        continue;
                    }
                    let p = child.path();
                    if should_prune_dir(&p) {
                        child.read_children_path = None;
                        continue;
                    }
                    if let Ok(m) = std::fs::symlink_metadata(&p)
                        && m.dev() != root_dev
                    {
                        child.read_children_path = None;
                    }
                }
            });

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(_) => {
                    skipped_counter.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };

            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    skipped_counter.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };

            // Enforce single-volume boundary (`st_dev`)
            if meta.dev() != root_dev {
                continue;
            }

            if meta.is_dir() {
                dir_map.entry(path).or_default();
                continue;
            }

            if !meta.is_file() {
                continue;
            }

            // Hardlink deduplication
            if meta.nlink() > 1 && !seen_hardlinks.insert((meta.dev(), meta.ino())) {
                continue;
            }

            let phys = file_physical_bytes(&meta);
            let logical = meta.len();
            let ts = file_activity_timestamp(&meta);
            let r7 = if ts >= cutoff_7d { phys } else { 0 };
            let r30 = if ts >= cutoff_30d { phys } else { 0 };

            let f_count = files_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let b_count = bytes_counter.fetch_add(phys, Ordering::Relaxed) + phys;
            total_logical_bytes = total_logical_bytes.saturating_add(logical);

            if let Some(cb) = progress_cb
                && f_count.is_multiple_of(4_096)
            {
                cb(ScanProgress {
                    files_scanned: f_count,
                    bytes_scanned: b_count,
                    skipped_permissions: skipped_counter.load(Ordering::Relaxed),
                    current_root: root.clone(),
                });
            }

            // Bubble physical/logical/recent metrics up through ancestor directories up to `root`
            let mut cur = path.parent();
            while let Some(dir_path) = cur {
                if !dir_path.starts_with(root) {
                    break;
                }
                let acc = dir_map.entry(dir_path.to_path_buf()).or_default();
                acc.physical_bytes = acc.physical_bytes.saturating_add(phys);
                acc.logical_bytes = acc.logical_bytes.saturating_add(logical);
                acc.recent_7d_bytes = acc.recent_7d_bytes.saturating_add(r7);
                acc.recent_30d_bytes = acc.recent_30d_bytes.saturating_add(r30);
                acc.file_count = acc.file_count.saturating_add(1);
                if dir_path == root {
                    break;
                }
                cur = dir_path.parent();
            }

            // Attribute file to its owning App / Category / Cleanup candidate
            if phys > 0 {
                let attr = attribution.classify_path_on_volume(
                    &path,
                    false,
                    Some(&config.mount_point),
                );
                let app_entry = app_map.entry(attr.app_id.clone()).or_insert_with(|| AppAcc {
                    display_name: attr.display_name.clone(),
                    category: attr.category.clone(),
                    is_orphaned: attr.is_orphaned,
                    ..Default::default()
                });
                match attr.bucket {
                    BucketKind::AppBundle => {
                        app_entry.app_bundle_bytes = app_entry.app_bundle_bytes.saturating_add(phys)
                    }
                    BucketKind::ApplicationData => {
                        app_entry.data_bytes = app_entry.data_bytes.saturating_add(phys)
                    }
                    BucketKind::Cache => {
                        app_entry.cache_bytes = app_entry.cache_bytes.saturating_add(phys)
                    }
                    BucketKind::Logs => {
                        app_entry.logs_bytes = app_entry.logs_bytes.saturating_add(phys)
                    }
                    BucketKind::Downloads => {
                        app_entry.downloads_bytes = app_entry.downloads_bytes.saturating_add(phys)
                    }
                    BucketKind::DevArtifact | BucketKind::SystemBackup => {
                        app_entry.dev_artifacts_bytes =
                            app_entry.dev_artifacts_bytes.saturating_add(phys)
                    }
                    BucketKind::Other => {
                        app_entry.data_bytes = app_entry.data_bytes.saturating_add(phys)
                    }
                }
                app_entry.recent_7d_bytes = app_entry.recent_7d_bytes.saturating_add(r7);
                app_entry.recent_30d_bytes = app_entry.recent_30d_bytes.saturating_add(r30);
                app_entry.total_bytes = app_entry.total_bytes.saturating_add(phys);
                if attr.safety_tier.is_auto_reclaimable() {
                    app_entry.reclaimable_bytes = app_entry.reclaimable_bytes.saturating_add(phys);
                }

                // Track location bucket (use cleanup_root or parent dir capped to depth 4)
                let loc_path = attr
                    .cleanup_root
                    .clone()
                    .unwrap_or_else(|| compact_location_path(&path, root));
                let loc = app_entry
                    .locations
                    .entry(loc_path)
                    .or_insert((attr.bucket, attr.safety_tier, 0, 0));
                loc.2 = loc.2.saturating_add(phys);
                loc.3 = loc.3.saturating_add(r7);

                // Register cleanup candidate if reclaimable
                if let Some(croot) = attr.cleanup_root
                    && attr.safety_tier.is_auto_reclaimable()
                {
                    let cand = candidate_map.entry(croot.clone()).or_insert_with(|| CandidateAcc {
                        label: croot
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| attr.display_name.clone()),
                        app_id: attr.app_id.clone(),
                        display_name: attr.display_name.clone(),
                        category: attr.category.clone(),
                        safety_tier: attr.safety_tier,
                        bytes: 0,
                        recent_7d_bytes: 0,
                        reason: attr
                            .reason
                            .clone()
                            .unwrap_or_else(|| attr.safety_tier.label().to_string()),
                    });
                    cand.bytes = cand.bytes.saturating_add(phys);
                    cand.recent_7d_bytes = cand.recent_7d_bytes.saturating_add(r7);
                }
            }
        }
    }

    // Build persisted DirAggregate list (keep all roots + shallow dirs up to max_persisted_depth + dirs >= min_persisted_bytes)
    let mut dirs = Vec::with_capacity(dir_map.len().min(16_384));
    for (dir_path, acc) in dir_map {
        let depth = relative_depth(&dir_path, &roots);
        let keep = depth <= config.max_persisted_depth || acc.physical_bytes >= config.min_persisted_bytes;
        if !keep {
            continue;
        }
        let attr = attribution.classify_path_on_volume(
            &dir_path,
            true,
            Some(&config.mount_point),
        );
        let parent_path = if roots.iter().any(|r| r == &dir_path) {
            Some(config.mount_point.clone())
        } else {
            dir_path.parent().map(Path::to_path_buf)
        };
        dirs.push(DirAggregate {
            path: dir_path,
            parent_path,
            depth,
            physical_bytes: acc.physical_bytes,
            logical_bytes: acc.logical_bytes,
            recent_7d_bytes: acc.recent_7d_bytes,
            recent_30d_bytes: acc.recent_30d_bytes,
            file_count: acc.file_count,
            app_id: attr.app_id,
            category: attr.category,
        });
    }
    dirs.sort_by_key(|a| std::cmp::Reverse(a.physical_bytes));

    // Build sorted AppRollups
    let mut app_rollups: Vec<AppRollup> = app_map
        .into_iter()
        .map(|(app_id, acc)| {
            let mut locations: Vec<AppLocation> = acc
                .locations
                .into_iter()
                .map(|(path, (bucket, safety_tier, bytes, recent_7d_bytes))| AppLocation {
                    path,
                    bucket,
                    safety_tier,
                    bytes,
                    recent_7d_bytes,
                })
                .collect();
            locations.sort_by_key(|a| std::cmp::Reverse(a.bytes));
            locations.truncate(20);

            AppRollup {
                app_id,
                display_name: acc.display_name,
                category: acc.category,
                app_bundle_bytes: acc.app_bundle_bytes,
                data_bytes: acc.data_bytes,
                cache_bytes: acc.cache_bytes,
                logs_bytes: acc.logs_bytes,
                downloads_bytes: acc.downloads_bytes,
                dev_artifacts_bytes: acc.dev_artifacts_bytes,
                recent_7d_bytes: acc.recent_7d_bytes,
                recent_30d_bytes: acc.recent_30d_bytes,
                total_bytes: acc.total_bytes,
                reclaimable_bytes: acc.reclaimable_bytes,
                is_orphaned: acc.is_orphaned,
                locations,
            }
        })
        .collect();
    app_rollups.sort_by_key(|a| std::cmp::Reverse(a.total_bytes));

    // Build sorted CleanupCandidates (ignore tiny < 1 MB items unless running unit tests with tiny min_persisted_bytes)
    let min_cand_bytes = config.min_persisted_bytes.min(1_000_000);
    let mut cleanup_candidates: Vec<CleanupCandidate> = candidate_map
        .into_iter()
        .filter(|(_, c)| c.bytes >= min_cand_bytes)
        .map(|(path, c)| CleanupCandidate {
            path,
            label: c.label,
            app_id: c.app_id,
            display_name: c.display_name,
            category: c.category,
            safety_tier: c.safety_tier,
            bytes: c.bytes,
            recent_7d_bytes: c.recent_7d_bytes,
            reason: c.reason,
        })
        .collect();
    cleanup_candidates.sort_by_key(|a| std::cmp::Reverse(a.bytes));

    let total_scanned_bytes = bytes_counter.load(Ordering::Relaxed);
    let total_files = files_counter.load(Ordering::Relaxed);
    let skipped_paths = skipped_counter.load(Ordering::Relaxed);

    Ok(ScanResult {
        volume_uuid: config.volume_uuid.clone(),
        taken_at: now,
        trigger_kind: config.trigger_kind.clone(),
        total_scanned_bytes,
        total_logical_bytes,
        total_files,
        skipped_paths,
        dirs,
        app_rollups,
        cleanup_candidates,
    })
}

/// Fast incremental rescan: only walks `dirty_subtrees`, merges updated subtrees
/// into `previous_dirs`, and recalculates ancestor totals cleanly.
pub fn rescan_dirty_subtrees(
    previous_dirs: &[DirAggregate],
    dirty_subtrees: &[PathBuf],
    config: &ScanConfig,
    attribution: &AttributionEngine,
) -> Result<ScanResult> {
    if dirty_subtrees.is_empty() {
        return scan_volume(config, attribution, None);
    }

    // Deduplicate nested dirty roots
    let mut roots = dirty_subtrees.to_vec();
    roots.sort_by_key(|p| p.components().count());
    let mut deduped: Vec<PathBuf> = Vec::new();
    for p in roots {
        if p.exists() && !deduped.iter().any(|existing| p.starts_with(existing)) {
            deduped.push(p);
        }
    }
    if deduped.is_empty() {
        return scan_volume(config, attribution, None);
    }

    let sub_cfg = ScanConfig {
        roots: deduped.clone(),
        ..config.clone()
    };
    let partial = scan_volume(&sub_cfg, attribution, None)?;

    // Build delta per dirty root compared to previous_dirs
    let prev_lookup: HashMap<&Path, &DirAggregate> = previous_dirs
        .iter()
        .map(|d| (d.path.as_path(), d))
        .collect();
    let new_lookup: HashMap<&Path, &DirAggregate> = partial
        .dirs
        .iter()
        .map(|d| (d.path.as_path(), d))
        .collect();

    let mut merged: HashMap<PathBuf, DirAggregate> = previous_dirs
        .iter()
        .filter(|d| !deduped.iter().any(|r| d.path.starts_with(r)))
        .cloned()
        .map(|d| (d.path.clone(), d))
        .collect();

    for d in &partial.dirs {
        merged.insert(d.path.clone(), d.clone());
    }

    // Propagate size deltas to ancestors above each dirty root
    for dirty_root in &deduped {
        let old_phys = prev_lookup
            .get(dirty_root.as_path())
            .map(|d| d.physical_bytes as i64)
            .unwrap_or(0);
        let new_phys = new_lookup
            .get(dirty_root.as_path())
            .map(|d| d.physical_bytes as i64)
            .unwrap_or(0);
        let delta_phys = new_phys - old_phys;

        let old_log = prev_lookup
            .get(dirty_root.as_path())
            .map(|d| d.logical_bytes as i64)
            .unwrap_or(0);
        let new_log = new_lookup
            .get(dirty_root.as_path())
            .map(|d| d.logical_bytes as i64)
            .unwrap_or(0);
        let delta_log = new_log - old_log;

        let old_files = prev_lookup
            .get(dirty_root.as_path())
            .map(|d| d.file_count as i64)
            .unwrap_or(0);
        let new_files = new_lookup
            .get(dirty_root.as_path())
            .map(|d| d.file_count as i64)
            .unwrap_or(0);
        let delta_files = new_files - old_files;

        let mut cur = dirty_root.parent();
        while let Some(anc) = cur {
            if let Some(entry) = merged.get_mut(anc) {
                entry.physical_bytes = (entry.physical_bytes as i64 + delta_phys).max(0) as u64;
                entry.logical_bytes = (entry.logical_bytes as i64 + delta_log).max(0) as u64;
                entry.file_count = (entry.file_count as i64 + delta_files).max(0) as u64;
            }
            cur = anc.parent();
        }
    }

    let mut dirs: Vec<DirAggregate> = merged.into_values().collect();
    dirs.sort_by_key(|a| std::cmp::Reverse(a.physical_bytes));

    let total_scanned_bytes = config
        .roots
        .iter()
        .filter_map(|r| dirs.iter().find(|d| &d.path == r))
        .map(|d| d.physical_bytes)
        .sum();
    let total_logical_bytes = config
        .roots
        .iter()
        .filter_map(|r| dirs.iter().find(|d| &d.path == r))
        .map(|d| d.logical_bytes)
        .sum();
    let total_files = config
        .roots
        .iter()
        .filter_map(|r| dirs.iter().find(|d| &d.path == r))
        .map(|d| d.file_count)
        .sum();

    Ok(ScanResult {
        volume_uuid: config.volume_uuid.clone(),
        taken_at: Utc::now().timestamp(),
        trigger_kind: config.trigger_kind.clone(),
        total_scanned_bytes,
        total_logical_bytes,
        total_files,
        skipped_paths: partial.skipped_paths,
        dirs,
        app_rollups: partial.app_rollups,
        cleanup_candidates: partial.cleanup_candidates,
    })
}

fn should_prune_dir(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some(
            "/System/Volumes/Data"
                | "/System/Volumes/VM"
                | "/System/Volumes/Preboot"
                | "/dev"
                | "/net"
                | "/home"
                | "/Volumes"
        )
    )
}

fn relative_depth(path: &Path, roots: &[PathBuf]) -> u32 {
    for r in roots {
        if let Ok(rel) = path.strip_prefix(r) {
            return rel.components().count() as u32;
        }
    }
    path.components().count() as u32
}

fn compact_location_path(file_path: &Path, root: &Path) -> PathBuf {
    let parent = file_path.parent().unwrap_or(file_path);
    if let Ok(rel) = parent.strip_prefix(root) {
        let mut out = root.to_path_buf();
        for comp in rel.components().take(3) {
            out.push(comp);
        }
        return out;
    }
    parent.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scanner_handles_sparse_files_hardlinks_and_incremental_rescan() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let sub_a = root.join("alpha");
        let sub_b = root.join("beta");
        std::fs::create_dir_all(&sub_a).unwrap();
        std::fs::create_dir_all(&sub_b).unwrap();

        // Write a real 64 KiB file and a hardlink to it
        let real_file = sub_a.join("data.bin");
        std::fs::write(&real_file, vec![42u8; 65_536]).unwrap();
        std::fs::hard_link(&real_file, sub_a.join("data_hardlink.bin")).unwrap();

        // Create a 10 MiB sparse file (logical 10 MiB, physical 0 blocks on APFS)
        let sparse_path = sub_b.join("sparse.img");
        {
            let f = std::fs::File::create(&sparse_path).unwrap();
            f.set_len(10 * 1024 * 1024).unwrap();
        }

        let attr = AttributionEngine::from_apps(root.clone(), Vec::new(), Vec::new());
        let cfg = ScanConfig {
            volume_uuid: "test-vol".to_string(),
            mount_point: root.clone(),
            roots: vec![root.clone()],
            trigger_kind: "test".to_string(),
            max_persisted_depth: 5,
            min_persisted_bytes: 0,
            include_private_var: false,
        };

        let res1 = scan_volume(&cfg, &attr, None).unwrap();
        // Hardlink must not be double-counted (2 files total: data.bin + sparse.img)
        assert_eq!(res1.total_files, 2);
        // Sparse file logical size > 10 MiB, physical size << 1 MiB
        assert!(res1.total_logical_bytes > 10 * 1024 * 1024);
        assert!(res1.total_scanned_bytes < 1024 * 1024);
        // Newly created files should appear in recent_7d_bytes
        let root_agg = res1.dirs.iter().find(|d| d.path == root).unwrap();
        assert_eq!(root_agg.recent_7d_bytes, res1.total_scanned_bytes);

        // Now add a new file in sub_b and run incremental dirty-subtree rescan
        std::fs::write(sub_b.join("added.bin"), vec![7u8; 131_072]).unwrap();
        let res_inc =
            rescan_dirty_subtrees(&res1.dirs, std::slice::from_ref(&sub_b), &cfg, &attr).unwrap();
        let res_full = scan_volume(&cfg, &attr, None).unwrap();

        assert_eq!(res_inc.total_scanned_bytes, res_full.total_scanned_bytes);
        assert_eq!(res_inc.total_files, res_full.total_files);
    }
}
