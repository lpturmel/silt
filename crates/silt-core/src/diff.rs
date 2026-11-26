//! Time-series creep diff engine, Day-1 recent-activity fallback, and runway forecasting.

use crate::db::{CleanupEvent, Database, VolumeSample};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeWindow {
    Hours24,
    Days7,
    Days30,
    Days90,
    SinceLastConnected,
    All,
}

impl TimeWindow {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hours24 => "24h",
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Days90 => "90d",
            Self::SinceLastConnected => "Since Last Connected",
            Self::All => "All Time",
        }
    }

    pub fn duration_secs(self) -> Option<i64> {
        match self {
            Self::Hours24 => Some(86_400),
            Self::Days7 => Some(7 * 86_400),
            Self::Days30 => Some(30 * 86_400),
            Self::Days90 => Some(90 * 86_400),
            Self::SinceLastConnected | Self::All => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CreepBasis {
    ExactSnapshotDiff { from_ts: i64, to_ts: i64 },
    EstimatedFromFileDates { window_days: u32 },
    NoSnapshotsYet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreepItem {
    pub key: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub category: String,
    pub current_bytes: u64,
    pub previous_bytes: u64,
    pub delta_bytes: i64,
    pub growth_pct: f64,
    pub bytes_per_day: f64,
    pub sparkline: Vec<u64>,
    pub reclaimable_bytes: u64,
    pub is_orphaned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunwayForecast {
    /// Positive = drive is filling up (losing free space), negative = drive is freeing space.
    pub bytes_per_day: f64,
    pub days_until_ten_percent: Option<f64>,
    pub days_until_full: Option<f64>,
    pub sample_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreepReport {
    pub volume_uuid: String,
    pub window: TimeWindow,
    pub basis: CreepBasis,
    pub net_delta_bytes: i64,
    pub avg_bytes_per_day: f64,
    pub runway: Option<RunwayForecast>,
    pub top_apps: Vec<CreepItem>,
    pub top_dirs: Vec<CreepItem>,
    pub dir_deltas: HashMap<PathBuf, i64>,
    pub cleanup_events: Vec<CleanupEvent>,
}

/// Compute a full `CreepReport` for a volume over the requested `TimeWindow`.
///
/// - If `>= 2` snapshots exist, computes exact `T2 - T1` deltas per app and directory.
/// - If only `1` snapshot exists (Day 1 cold start), seamlessly falls back to `recent_7d_bytes` /
///   `recent_30d_bytes` captured from file `mtime`/`birthtime`, tagged with `EstimatedFromFileDates`.
pub fn compute_creep_report(
    db: &Database,
    volume_uuid: &str,
    window: TimeWindow,
) -> Result<CreepReport> {
    let now = Utc::now().timestamp();
    let since_ts = window
        .duration_secs()
        .map(|d| now - d)
        .unwrap_or(0);

    let samples = db.volume_samples(volume_uuid, since_ts)?;
    let cleanup_events = db.list_cleanup_events(volume_uuid, since_ts)?;
    let runway = compute_runway_forecast(&samples);

    let Some(latest) = db.latest_snapshot(volume_uuid)? else {
        return Ok(CreepReport {
            volume_uuid: volume_uuid.to_string(),
            window,
            basis: CreepBasis::NoSnapshotsYet,
            net_delta_bytes: 0,
            avg_bytes_per_day: 0.0,
            runway,
            top_apps: Vec::new(),
            top_dirs: Vec::new(),
            dir_deltas: HashMap::new(),
            cleanup_events,
        });
    };

    // Choose baseline snapshot
    let baseline = match window {
        TimeWindow::SinceLastConnected => db.previous_snapshot(volume_uuid, latest.id)?,
        TimeWindow::All => {
            let all = db.list_snapshots(volume_uuid, 1000)?;
            all.into_iter().last().filter(|s| s.id != latest.id)
        }
        _ => {
            let target = latest.taken_at - window.duration_secs().unwrap_or(7 * 86_400);
            db.snapshot_closest_to(volume_uuid, target, latest.id)?
        }
    };

    let latest_apps = db.list_app_rollups(latest.id)?;
    let latest_dirs = db.list_dir_entries(latest.id)?;

    if let Some(base) = baseline {
        let elapsed_secs = (latest.taken_at - base.taken_at).max(60) as f64;
        let elapsed_days = (elapsed_secs / 86_400.0).max(1.0 / 24.0);

        let base_apps: HashMap<String, u64> = db
            .list_app_rollups(base.id)?
            .into_iter()
            .map(|a| (a.app_id, a.total_bytes))
            .collect();
        let base_dirs: HashMap<PathBuf, u64> = db
            .list_dir_entries(base.id)?
            .into_iter()
            .map(|d| (d.path, d.physical_bytes))
            .collect();

        let mut top_apps = Vec::with_capacity(latest_apps.len());
        for app in &latest_apps {
            let prev = base_apps.get(&app.app_id).copied().unwrap_or(0);
            let delta = app.total_bytes as i64 - prev as i64;
            let pct = if prev > 0 {
                (delta as f64 / prev as f64) * 100.0
            } else if delta > 0 {
                100.0
            } else {
                0.0
            };
            let sparkline = db.app_size_history(volume_uuid, &app.app_id, 14)?;
            top_apps.push(CreepItem {
                key: app.app_id.clone(),
                label: app.display_name.clone(),
                path: app.locations.first().map(|l| l.path.clone()),
                category: app.category.clone(),
                current_bytes: app.total_bytes,
                previous_bytes: prev,
                delta_bytes: delta,
                growth_pct: pct,
                bytes_per_day: delta as f64 / elapsed_days,
                sparkline,
                reclaimable_bytes: app.reclaimable_bytes,
                is_orphaned: app.is_orphaned,
            });
        }
        top_apps.sort_by_key(|a| std::cmp::Reverse(a.delta_bytes));

        let mut dir_deltas = HashMap::with_capacity(latest_dirs.len());
        let mut top_dirs = Vec::with_capacity(latest_dirs.len());
        for d in &latest_dirs {
            let prev = base_dirs.get(&d.path).copied().unwrap_or(0);
            let delta = d.physical_bytes as i64 - prev as i64;
            dir_deltas.insert(d.path.clone(), delta);
            let pct = if prev > 0 {
                (delta as f64 / prev as f64) * 100.0
            } else if delta > 0 {
                100.0
            } else {
                0.0
            };
            top_dirs.push(CreepItem {
                key: d.path.to_string_lossy().into_owned(),
                label: d
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| d.path.to_string_lossy().into_owned()),
                path: Some(d.path.clone()),
                category: d.category.clone(),
                current_bytes: d.physical_bytes,
                previous_bytes: prev,
                delta_bytes: delta,
                growth_pct: pct,
                bytes_per_day: delta as f64 / elapsed_days,
                sparkline: Vec::new(),
                reclaimable_bytes: 0,
                is_orphaned: false,
            });
        }
        top_dirs.sort_by_key(|a| std::cmp::Reverse(a.delta_bytes));

        let net_delta_bytes = latest.total_scanned_bytes as i64 - base.total_scanned_bytes as i64;
        let avg_bytes_per_day = net_delta_bytes as f64 / elapsed_days;

        return Ok(CreepReport {
            volume_uuid: volume_uuid.to_string(),
            window,
            basis: CreepBasis::ExactSnapshotDiff {
                from_ts: base.taken_at,
                to_ts: latest.taken_at,
            },
            net_delta_bytes,
            avg_bytes_per_day,
            runway,
            top_apps,
            top_dirs,
            dir_deltas,
            cleanup_events,
        });
    }

    // Day-1 Cold-Start Fallback: estimate recent growth from file mtime/birthtime
    let use_30d = matches!(window, TimeWindow::Days30 | TimeWindow::Days90 | TimeWindow::All);
    let window_days: u32 = if use_30d { 30 } else { 7 };

    let mut top_apps: Vec<CreepItem> = latest_apps
        .iter()
        .map(|app| {
            let recent = if use_30d {
                app.recent_30d_bytes
            } else {
                app.recent_7d_bytes
            };
            let prev = app.total_bytes.saturating_sub(recent);
            let pct = if prev > 0 {
                (recent as f64 / prev as f64) * 100.0
            } else if recent > 0 {
                100.0
            } else {
                0.0
            };
            CreepItem {
                key: app.app_id.clone(),
                label: app.display_name.clone(),
                path: app.locations.first().map(|l| l.path.clone()),
                category: app.category.clone(),
                current_bytes: app.total_bytes,
                previous_bytes: prev,
                delta_bytes: recent as i64,
                growth_pct: pct,
                bytes_per_day: recent as f64 / window_days as f64,
                sparkline: vec![prev, app.total_bytes],
                reclaimable_bytes: app.reclaimable_bytes,
                is_orphaned: app.is_orphaned,
            }
        })
        .collect();
    top_apps.sort_by_key(|a| std::cmp::Reverse(a.delta_bytes));

    let mut dir_deltas = HashMap::with_capacity(latest_dirs.len());
    let mut top_dirs: Vec<CreepItem> = latest_dirs
        .iter()
        .map(|d| {
            let recent = if use_30d {
                d.recent_30d_bytes
            } else {
                d.recent_7d_bytes
            };
            dir_deltas.insert(d.path.clone(), recent as i64);
            let prev = d.physical_bytes.saturating_sub(recent);
            CreepItem {
                key: d.path.to_string_lossy().into_owned(),
                label: d
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| d.path.to_string_lossy().into_owned()),
                path: Some(d.path.clone()),
                category: d.category.clone(),
                current_bytes: d.physical_bytes,
                previous_bytes: prev,
                delta_bytes: recent as i64,
                growth_pct: if prev > 0 {
                    (recent as f64 / prev as f64) * 100.0
                } else {
                    100.0
                },
                bytes_per_day: recent as f64 / window_days as f64,
                sparkline: Vec::new(),
                reclaimable_bytes: 0,
                is_orphaned: false,
            }
        })
        .collect();
    top_dirs.sort_by_key(|a| std::cmp::Reverse(a.delta_bytes));

    let net_delta_bytes: i64 = top_apps.iter().map(|a| a.delta_bytes).sum();
    let avg_bytes_per_day = net_delta_bytes as f64 / window_days as f64;

    Ok(CreepReport {
        volume_uuid: volume_uuid.to_string(),
        window,
        basis: CreepBasis::EstimatedFromFileDates { window_days },
        net_delta_bytes,
        avg_bytes_per_day,
        runway,
        top_apps,
        top_dirs,
        dir_deltas,
        cleanup_events,
    })
}

/// Ordinary least-squares linear regression over `VolumeSample`s to estimate
/// daily space-creep rate and days remaining until 10% free / completely full.
pub fn compute_runway_forecast(samples: &[VolumeSample]) -> Option<RunwayForecast> {
    if samples.len() < 2 {
        return None;
    }
    let first_ts = samples.first()?.sampled_at as f64;
    let last = samples.last()?;
    let span_secs = (last.sampled_at as f64) - first_ts;
    if span_secs < 60.0 {
        return None;
    }

    let n = samples.len() as f64;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xy = 0.0;

    for s in samples {
        let x_days = ((s.sampled_at as f64) - first_ts) / 86_400.0;
        let y_used = s.used_bytes as f64;
        sum_x += x_days;
        sum_y += y_used;
        sum_xx += x_days * x_days;
        sum_xy += x_days * y_used;
    }

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < 1e-9 {
        return None;
    }
    let slope_bytes_per_day = (n * sum_xy - sum_x * sum_y) / denom;

    let (days_until_ten_percent, days_until_full) = if slope_bytes_per_day > 100_000.0 {
        let ten_pct_reserve = (last.total_bytes as f64) * 0.10;
        let headroom_10 = (last.available_important_bytes as f64 - ten_pct_reserve).max(0.0);
        let headroom_full = last.available_important_bytes as f64;
        (
            Some(headroom_10 / slope_bytes_per_day),
            Some(headroom_full / slope_bytes_per_day),
        )
    } else {
        (None, None)
    };

    Some(RunwayForecast {
        bytes_per_day: slope_bytes_per_day,
        days_until_ten_percent,
        days_until_full,
        sample_count: samples.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::AppRollup;
    use crate::scanner::{DirAggregate, ScanResult};
    use crate::volume::VolumeMetrics;

    fn make_vol(uuid: &str, sampled_at: i64, used: u64) -> VolumeMetrics {
        let total = 500_000_000_000;
        let avail = total - used;
        VolumeMetrics {
            volume_uuid: uuid.to_string(),
            name: "Macintosh HD".to_string(),
            mount_point: PathBuf::from("/"),
            is_internal: true,
            is_removable: false,
            is_online: true,
            is_ignored: false,
            ignore_reason: None,
            total_bytes: total,
            used_bytes: used,
            physical_free_bytes: avail - 10_000_000_000,
            available_important_bytes: avail,
            available_opportunistic_bytes: avail,
            purgeable_bytes: 10_000_000_000,
            snapshot_count: 1,
            snapshots: Vec::new(),
            sampled_at,
        }
    }

    #[test]
    fn day1_fallback_transitions_to_exact_diff_and_runway_forecast() {
        let mut db = Database::open_in_memory().unwrap();
        let now = Utc::now().timestamp();
        let t1 = now - 5 * 86_400;

        // Seed 5 days of samples growing by +2 GB/day
        for day in 0..=5 {
            let v = make_vol(
                "vol-1",
                t1 + day * 86_400,
                390_000_000_000 + (day as u64) * 2_000_000_000,
            );
            db.record_volume_sample(&v).unwrap();
        }

        // Snapshot 1 only -> Day-1 EstimatedFromFileDates
        let scan1 = ScanResult {
            volume_uuid: "vol-1".to_string(),
            taken_at: t1,
            trigger_kind: "manual".to_string(),
            total_scanned_bytes: 100_000_000,
            total_logical_bytes: 100_000_000,
            total_files: 10,
            skipped_paths: 0,
            dirs: vec![DirAggregate {
                path: PathBuf::from("/Users/test/Caches"),
                parent_path: Some(PathBuf::from("/Users/test")),
                depth: 1,
                physical_bytes: 100_000_000,
                logical_bytes: 100_000_000,
                recent_7d_bytes: 30_000_000,
                recent_30d_bytes: 60_000_000,
                file_count: 10,
                app_id: "com.test.app".to_string(),
                category: "Applications".to_string(),
            }],
            app_rollups: vec![AppRollup {
                app_id: "com.test.app".to_string(),
                display_name: "TestApp".to_string(),
                category: "Applications".to_string(),
                app_bundle_bytes: 0,
                data_bytes: 0,
                cache_bytes: 100_000_000,
                logs_bytes: 0,
                downloads_bytes: 0,
                dev_artifacts_bytes: 0,
                recent_7d_bytes: 30_000_000,
                recent_30d_bytes: 60_000_000,
                total_bytes: 100_000_000,
                reclaimable_bytes: 100_000_000,
                is_orphaned: false,
                locations: Vec::new(),
            }],
            cleanup_candidates: Vec::new(),
        };
        db.save_scan_result(&scan1).unwrap();

        let rep1 = compute_creep_report(&db, "vol-1", TimeWindow::Days7).unwrap();
        assert_eq!(rep1.basis, CreepBasis::EstimatedFromFileDates { window_days: 7 });
        assert_eq!(rep1.top_apps[0].delta_bytes, 30_000_000);

        // Runway forecast should detect ~+2 GB/day slope
        let runway = rep1.runway.unwrap();
        assert!((runway.bytes_per_day - 2_000_000_000.0).abs() < 1_000_000.0);
        // At 400 GB used out of 500 GB (100 GB available), 10% reserve = 50 GB -> 50 GB / 2 GB/day = 25 days
        assert!((runway.days_until_ten_percent.unwrap() - 25.0).abs() < 0.5);

        // Snapshot 2 -> switches automatically to ExactSnapshotDiff
        let mut scan2 = scan1.clone();
        scan2.taken_at = now;
        scan2.total_scanned_bytes = 250_000_000;
        scan2.dirs[0].physical_bytes = 250_000_000;
        scan2.app_rollups[0].total_bytes = 250_000_000;
        db.save_scan_result(&scan2).unwrap();

        let rep2 = compute_creep_report(&db, "vol-1", TimeWindow::Days7).unwrap();
        assert!(matches!(rep2.basis, CreepBasis::ExactSnapshotDiff { .. }));
        assert_eq!(rep2.top_apps[0].delta_bytes, 150_000_000);
    }
}
