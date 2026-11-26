//! `siltd` — ultra-light opt-in macOS background collector for Silt.
//!
//! Runs via user LaunchAgent (`Nice = 10`, `LowPriorityIO = true`):
//! - Samples volume telemetry (Remaining / Available / Purgeable / APFS snapshots) every 30 min
//! - Tracks dirty directories via macOS FSEvents for fast incremental rescans
//! - Detects external drive plug-ins and captures automatic "Since Last Connected" snapshots
//! - Sends lightweight notifications on `~/Library/Application Support/Silt/silt.sock`

mod mount_monitor;
mod watcher;

use crate::mount_monitor::MountMonitor;
use crate::watcher::DirtyWatcher;
use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use silt_core::attribution::AttributionEngine;
use silt_core::db::Database;
use silt_core::scanner::{ScanConfig, rescan_dirty_subtrees, scan_volume};
use silt_core::volume::VolumeMetrics;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "siltd", version, about = "Silt background storage telemetry collector")]
struct Args {
    /// Override SQLite database path
    #[arg(long)]
    db: Option<PathBuf>,

    /// Run a single telemetry sample (and optional scan) then exit
    #[arg(long)]
    once: bool,

    /// Also run a full/incremental scan when `--once` is passed
    #[arg(long)]
    scan: bool,

    /// Seconds between lightweight volume capacity samples (default: 1800 = 30 min)
    #[arg(long, default_value_t = 1800)]
    sample_interval_secs: u64,

    /// Seconds between incremental directory scans per volume (default: 86400 = 24h)
    #[arg(long, default_value_t = 86_400)]
    scan_interval_secs: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let db_path = args.db.unwrap_or_else(silt_core::default_db_path);
    let socket_path = silt_core::default_socket_path();

    let mut monitor = MountMonitor::new();
    let initial = monitor.poll()?;

    // Record initial volume samples immediately
    {
        let db = Database::open(&db_path)?;
        for v in &initial.online {
            let _ = db.record_volume_sample(v);
        }
    }

    if args.once {
        if args.scan {
            let mut db = Database::open(&db_path)?;
            let attr = AttributionEngine::discover();
            for v in initial.online.iter().filter(|v| !v.is_ignored) {
                run_volume_snapshot(&mut db, v, &attr, &[], "daemon-once")?;
            }
            notify_ui(&socket_path, "snapshot");
        }
        return Ok(());
    }

    // Start FSEvents dirty-directory watcher on initial non-ignored roots
    let mut watch_roots = Vec::new();
    for v in initial.online.iter().filter(|v| !v.is_ignored) {
        let cfg = ScanConfig::default_for_volume(v, "daemon");
        watch_roots.extend(cfg.roots);
    }
    let mut dirty_watcher = DirtyWatcher::start(&watch_roots).ok();

    let mut last_sample = Instant::now();
    let mut last_scan_per_vol: HashMap<String, Instant> = HashMap::new();
    let sample_interval = Duration::from_secs(args.sample_interval_secs.max(60));
    let scan_interval = Duration::from_secs(args.scan_interval_secs.max(300));

    // Record initial baseline timestamps from DB so daemon doesn't immediately re-scan
    // if the user already scanned recently
    if let Ok(db) = Database::open(&db_path) {
        let now_ts = Utc::now().timestamp();
        for v in initial.online.iter().filter(|v| !v.is_ignored) {
            if let Ok(Some(snap)) = db.latest_snapshot(&v.volume_uuid) {
                let age = (now_ts - snap.taken_at).max(0) as u64;
                if age < args.scan_interval_secs {
                    last_scan_per_vol.insert(
                        v.volume_uuid.clone(),
                        Instant::now()
                            .checked_sub(Duration::from_secs(age))
                            .unwrap_or_else(Instant::now),
                    );
                }
            }
        }
    }

    loop {
        std::thread::sleep(Duration::from_secs(30));

        let Ok(diff) = monitor.poll() else {
            continue;
        };

        // Handle unmounted drives cleanly
        if let Some(w) = dirty_watcher.as_mut() {
            for (_uuid, path) in &diff.unmounted {
                w.unwatch_root(path);
            }
        }

        // Handle newly connected external drives: immediately sample + snapshot
        // so "Since Last Connected" creep diffs are ready right away!
        if !diff.newly_mounted.is_empty()
            && let Ok(mut db) = Database::open(&db_path)
        {
            let attr = AttributionEngine::discover();
            for v in &diff.newly_mounted {
                let _ = db.record_volume_sample(v);
                if let Some(w) = dirty_watcher.as_mut() {
                    w.watch_root(&v.mount_point);
                }
                if !v.is_internal {
                    let _ = run_volume_snapshot(&mut db, v, &attr, &[], "mount-auto");
                    last_scan_per_vol.insert(v.volume_uuid.clone(), Instant::now());
                    notify_ui(&socket_path, "mount-snapshot");
                }
            }
        }

        // Periodic lightweight volume telemetry sample
        if last_sample.elapsed() >= sample_interval {
            if let Ok(db) = Database::open(&db_path) {
                for v in &diff.online {
                    let _ = db.record_volume_sample(v);
                }
            }
            notify_ui(&socket_path, "sample");
            last_sample = Instant::now();
        }

        // Daily low-priority incremental scan per online tracked drive
        let due_vols: Vec<&VolumeMetrics> = diff
            .online
            .iter()
            .filter(|v| !v.is_ignored)
            .filter(|v| {
                last_scan_per_vol
                    .get(&v.volume_uuid)
                    .is_none_or(|t| t.elapsed() >= scan_interval)
            })
            .collect();

        if !due_vols.is_empty()
            && let Ok(mut db) = Database::open(&db_path)
        {
            let attr = AttributionEngine::discover();
            for v in due_vols {
                let dirty = dirty_watcher
                    .as_ref()
                    .map(|w| w.drain_dirty_under(&v.mount_point))
                    .unwrap_or_default();
                if run_volume_snapshot(&mut db, v, &attr, &dirty, "daemon-scheduled").is_ok() {
                    let cutoff = Utc::now().timestamp() - 365 * 86_400;
                    let _ = db.prune_old_snapshots(&v.volume_uuid, cutoff);
                    last_scan_per_vol.insert(v.volume_uuid.clone(), Instant::now());
                    notify_ui(&socket_path, "snapshot");
                }
            }
        }
    }
}

fn run_volume_snapshot(
    db: &mut Database,
    vol: &VolumeMetrics,
    attr: &AttributionEngine,
    dirty_paths: &[PathBuf],
    trigger: &str,
) -> Result<()> {
    let cfg = ScanConfig::default_for_volume(vol, trigger);
    let result = if !dirty_paths.is_empty()
        && let Ok(Some(prev_snap)) = db.latest_snapshot(&vol.volume_uuid)
        && let Ok(prev_dirs) = db.list_dir_entries(prev_snap.id)
        && !prev_dirs.is_empty()
    {
        rescan_dirty_subtrees(&prev_dirs, dirty_paths, &cfg, attr)?
    } else {
        scan_volume(&cfg, attr, None)?
    };
    db.save_scan_result(&result)?;
    Ok(())
}

fn notify_ui(socket_path: &Path, event: &str) {
    if let Ok(mut stream) = UnixStream::connect(socket_path) {
        let _ = writeln!(stream, "{event}");
    }
}
