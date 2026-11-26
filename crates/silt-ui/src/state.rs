//! Reactive application state and background worker orchestration for `silt-ui`.

use crate::theme::ThemePreference;
use gpui::{Context, WeakEntity};
use silt_core::apfs::ApfsSnapshot;
use silt_core::attribution::{AppRollup, AttributionEngine, CleanupCandidate};
use silt_core::cleanup::{CleanupTarget, DeleteMode, StagingCart, execute_cart};
use silt_core::db::{CleanupEvent, Database, SnapshotMeta, VolumeSample};
use silt_core::diff::{CreepReport, TimeWindow, compute_creep_report};
use silt_core::format::format_bytes;
use silt_core::scanner::{DirAggregate, ScanConfig, scan_volume};
use silt_core::volume::{VolumeMetrics, discover_mounted_volumes};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Overview,
    Apps,
    SpaceMap,
    Cleanup,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSortMode {
    FastestGrowing,
    LargestTotal,
    MostReclaimable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreemapColorMode {
    Category,
    CreepHeatmap,
}

#[derive(Debug, Clone)]
pub struct DriveEntry {
    pub uuid: String,
    pub name: String,
    pub mount_point: PathBuf,
    pub is_internal: bool,
    pub is_online: bool,
    pub is_ignored: bool,
    pub ignore_reason: Option<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub physical_free_bytes: u64,
    pub available_important_bytes: u64,
    pub purgeable_bytes: u64,
    pub snapshot_count: u32,
    pub snapshots: Vec<ApfsSnapshot>,
    pub last_seen_at: i64,
    pub recent_7d_delta: Option<i64>,
}

impl DriveEntry {
    pub fn used_frac(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    pub fn purgeable_frac(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.purgeable_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    pub fn free_frac(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.physical_free_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }
}

pub struct SiltState {
    pub db_path: PathBuf,
    pub theme_pref: ThemePreference,
    pub active_tab: ActiveTab,
    pub drives: Vec<DriveEntry>,
    pub selected_volume_uuid: Option<String>,
    pub selected_window: TimeWindow,

    // Active volume loaded data
    pub samples: Vec<VolumeSample>,
    pub latest_snapshot: Option<SnapshotMeta>,
    pub creep_report: Option<CreepReport>,
    pub app_rollups: Vec<AppRollup>,
    pub dir_entries: Vec<DirAggregate>,
    pub cleanup_candidates: Vec<CleanupCandidate>,
    pub cleanup_events: Vec<CleanupEvent>,

    // Apps tab state
    pub app_sort: AppSortMode,
    pub orphans_only: bool,
    pub expanded_apps: HashSet<String>,

    // Treemap tab state
    pub treemap_color_mode: TreemapColorMode,
    pub treemap_root: Option<PathBuf>,
    pub treemap_selected_path: Option<PathBuf>,

    // Cleanup Workbench & Staging Cart
    pub cart: StagingCart,
    pub confirm_cleanup_modal: Option<DeleteMode>,

    // Runtime & settings
    pub is_scanning: bool,
    pub scan_status_text: Option<String>,
    pub toast_message: Option<(String, bool)>, // (message, is_error)
    pub has_fda: bool,
    pub include_private_var: bool,
    pub daemon_installed: bool,
}

impl SiltState {
    pub fn new(db_path: PathBuf) -> Self {
        let mut state = Self {
            db_path,
            theme_pref: ThemePreference::System,
            active_tab: ActiveTab::Overview,
            drives: Vec::new(),
            selected_volume_uuid: None,
            selected_window: TimeWindow::Days7,
            samples: Vec::new(),
            latest_snapshot: None,
            creep_report: None,
            app_rollups: Vec::new(),
            dir_entries: Vec::new(),
            cleanup_candidates: Vec::new(),
            cleanup_events: Vec::new(),
            app_sort: AppSortMode::FastestGrowing,
            orphans_only: false,
            expanded_apps: HashSet::new(),
            treemap_color_mode: TreemapColorMode::CreepHeatmap,
            treemap_root: None,
            treemap_selected_path: None,
            cart: StagingCart::new(),
            confirm_cleanup_modal: None,
            is_scanning: false,
            scan_status_text: None,
            toast_message: None,
            has_fda: silt_core::has_full_disk_access(),
            include_private_var: false,
            daemon_installed: launch_agent_plist_path().exists(),
        };
        state.refresh_from_disk();
        state
    }

    pub fn selected_drive(&self) -> Option<&DriveEntry> {
        let uuid = self.selected_volume_uuid.as_deref()?;
        self.drives.iter().find(|d| d.uuid == uuid)
    }

    pub fn refresh_from_disk(&mut self) {
        self.has_fda = silt_core::has_full_disk_access();
        self.daemon_installed = launch_agent_plist_path().exists();

        let Ok(db) = Database::open(&self.db_path) else {
            return;
        };

        let mounted = discover_mounted_volumes().unwrap_or_default();
        let mut online_map: HashMap<String, VolumeMetrics> = HashMap::new();
        for v in mounted {
            let _ = db.record_volume_sample(&v);
            online_map.insert(v.volume_uuid.clone(), v);
        }

        let stored = db.list_volumes().unwrap_or_default();
        let mut drives = Vec::with_capacity(stored.len());

        for sv in stored {
            let recent_7d_delta = compute_creep_report(&db, &sv.uuid, TimeWindow::Days7)
                .ok()
                .filter(|r| !matches!(r.basis, silt_core::diff::CreepBasis::NoSnapshotsYet))
                .map(|r| r.net_delta_bytes);

            if let Some(live) = online_map.remove(&sv.uuid) {
                drives.push(DriveEntry {
                    uuid: live.volume_uuid,
                    name: live.name,
                    mount_point: live.mount_point,
                    is_internal: live.is_internal,
                    is_online: true,
                    is_ignored: sv.is_ignored,
                    ignore_reason: sv.ignore_reason.or(live.ignore_reason),
                    total_bytes: live.total_bytes,
                    used_bytes: live.used_bytes,
                    physical_free_bytes: live.physical_free_bytes,
                    available_important_bytes: live.available_important_bytes,
                    purgeable_bytes: live.purgeable_bytes,
                    snapshot_count: live.snapshot_count,
                    snapshots: live.snapshots,
                    last_seen_at: live.sampled_at,
                    recent_7d_delta,
                });
            } else if let Some(sample) = sv.latest_sample {
                drives.push(DriveEntry {
                    uuid: sv.uuid,
                    name: sv.name,
                    mount_point: sv.last_mount_point,
                    is_internal: sv.is_internal,
                    is_online: false,
                    is_ignored: sv.is_ignored,
                    ignore_reason: sv.ignore_reason,
                    total_bytes: sample.total_bytes,
                    used_bytes: sample.used_bytes,
                    physical_free_bytes: sample.physical_free_bytes,
                    available_important_bytes: sample.available_important_bytes,
                    purgeable_bytes: sample.purgeable_bytes,
                    snapshot_count: sample.snapshot_count,
                    snapshots: Vec::new(),
                    last_seen_at: sv.last_seen_at,
                    recent_7d_delta,
                });
            }
        }

        // Order: Internal first, then Online Externals, then Offline Externals
        drives.sort_by(|a, b| {
            b.is_internal
                .cmp(&a.is_internal)
                .then_with(|| b.is_online.cmp(&a.is_online))
                .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
        });

        self.drives = drives;

        if self.selected_volume_uuid.is_none()
            || !self
                .drives
                .iter()
                .any(|d| Some(&d.uuid) == self.selected_volume_uuid.as_ref())
        {
            self.selected_volume_uuid = self
                .drives
                .iter()
                .find(|d| !d.is_ignored)
                .or_else(|| self.drives.first())
                .map(|d| d.uuid.clone());
        }

        self.reload_selected_volume_data();
    }

    pub fn reload_selected_volume_data(&mut self) {
        let Some(uuid) = self.selected_volume_uuid.clone() else {
            return;
        };
        let Ok(db) = Database::open(&self.db_path) else {
            return;
        };

        let now = chrono::Utc::now().timestamp();
        let since = self
            .selected_window
            .duration_secs()
            .map(|d| now - d)
            .unwrap_or(0);

        self.samples = db.volume_samples(&uuid, since).unwrap_or_default();
        self.cleanup_events = db.list_cleanup_events(&uuid, since).unwrap_or_default();
        self.latest_snapshot = db.latest_snapshot(&uuid).ok().flatten();
        self.creep_report = compute_creep_report(&db, &uuid, self.selected_window).ok();

        if let Some(snap) = &self.latest_snapshot {
            self.app_rollups = db.list_app_rollups(snap.id).unwrap_or_default();
            self.dir_entries = db.list_dir_entries(snap.id).unwrap_or_default();
            self.cleanup_candidates = db.list_cleanup_candidates(snap.id).unwrap_or_default();
        } else {
            self.app_rollups.clear();
            self.dir_entries.clear();
            self.cleanup_candidates.clear();
        }

        // Reset treemap root to the selected drive's mount point
        if let Some(drive) = self.selected_drive() {
            let root = drive.mount_point.clone();
            if self
                .treemap_root
                .as_ref()
                .is_none_or(|r| !r.starts_with(&root))
            {
                self.treemap_root = Some(root);
            }
        }
    }

    pub fn select_volume(&mut self, uuid: String) {
        self.selected_volume_uuid = Some(uuid);
        self.treemap_root = None;
        self.treemap_selected_path = None;
        self.reload_selected_volume_data();
    }

    pub fn select_window(&mut self, window: TimeWindow) {
        self.selected_window = window;
        self.reload_selected_volume_data();
    }

    pub fn toggle_volume_ignored(&mut self, uuid: &str, ignored: bool) {
        if let Ok(db) = Database::open(&self.db_path) {
            let _ = db.set_volume_ignored(uuid, ignored);
        }
        self.refresh_from_disk();
    }

    /// Spawn a background filesystem scan for the currently selected volume.
    pub fn start_scan(&mut self, cx: &mut Context<Self>) {
        if self.is_scanning {
            return;
        }
        let Some(drive) = self.selected_drive().cloned() else {
            return;
        };
        if !drive.is_online {
            self.toast_message = Some((
                format!("{} is offline — reconnect it to scan.", drive.name),
                true,
            ));
            cx.notify();
            return;
        }

        self.is_scanning = true;
        self.scan_status_text = Some(format!("Scanning {}…", drive.name));
        self.toast_message = None;
        cx.notify();

        let db_path = self.db_path.clone();
        let include_private_var = self.include_private_var;
        let mount_point = drive.mount_point.clone();
        let drive_name = drive.name.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let outcome: anyhow::Result<(u64, u64, usize)> = cx
                .background_executor()
                .spawn(async move {
                    let mounted = discover_mounted_volumes()?;
                    let vol = mounted
                        .into_iter()
                        .find(|v| v.mount_point == mount_point)
                        .ok_or_else(|| {
                            anyhow::anyhow!("volume {} is no longer mounted", mount_point.display())
                        })?;

                    let mut db = Database::open(&db_path)?;
                    db.record_volume_sample(&vol)?;

                    let attr = AttributionEngine::discover();
                    let mut cfg = ScanConfig::default_for_volume(&vol, "ui");
                    cfg.include_private_var = include_private_var;
                    let result = scan_volume(&cfg, &attr, None)?;
                    let bytes = result.total_scanned_bytes;
                    let files = result.total_files;
                    let cands = result.cleanup_candidates.len();
                    db.save_scan_result(&result)?;
                    Ok((bytes, files, cands))
                })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.is_scanning = false;
                state.scan_status_text = None;
                match outcome {
                    Ok((bytes, files, cands)) => {
                        state.refresh_from_disk();
                        state.toast_message = Some((
                            format!(
                                "Scanned {drive_name}: {} across {files} files ({cands} cleanup targets)",
                                format_bytes(bytes)
                            ),
                            false,
                        ));
                    }
                    Err(err) => {
                        state.toast_message = Some((format!("Scan failed: {err}"), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Execute the Cleanup Staging Cart on a background thread and refresh metrics.
    pub fn run_staged_cleanup(&mut self, mode: DeleteMode, cx: &mut Context<Self>) {
        if self.cart.is_empty() {
            return;
        }
        let Some(uuid) = self.selected_volume_uuid.clone() else {
            return;
        };
        let cart_snapshot = self.cart.clone();
        let db_path = self.db_path.clone();
        self.confirm_cleanup_modal = None;
        self.scan_status_text = Some("Running safe cleanup…".to_string());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let report = cx
                .background_executor()
                .spawn(async move {
                    let db = Database::open(&db_path).ok();
                    execute_cart(&cart_snapshot, mode, db.as_ref(), &uuid)
                })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.scan_status_text = None;
                state.cart.clear();
                state.refresh_from_disk();
                if report.failed.is_empty() {
                    let verb = match mode {
                        DeleteMode::Trash => "Moved to Trash",
                        DeleteMode::Permanent => "Removed",
                    };
                    state.toast_message = Some((
                        format!(
                            "{verb}: {} item(s) · ~{} reclaimed",
                            report.succeeded.len(),
                            format_bytes(report.bytes_reclaimed)
                        ),
                        false,
                    ));
                } else {
                    state.toast_message = Some((
                        format!(
                            "Cleaned {} item(s), {} failed ({})",
                            report.succeeded.len(),
                            report.failed.len(),
                            report.failed[0].1
                        ),
                        true,
                    ));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn stage_target(&mut self, target: CleanupTarget) {
        if let Err(err) = self.cart.stage(target) {
            self.toast_message = Some((err.to_string(), true));
        }
    }

    pub fn stage_all_safe_caches(&mut self) {
        let targets: Vec<CleanupTarget> = self
            .cleanup_candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.safety_tier,
                    silt_core::attribution::SafetyTier::SafeCache
                )
            })
            .map(|c| CleanupTarget::Path {
                path: c.path.clone(),
                label: format!("{} · {}", c.display_name, c.label),
                bytes: c.bytes,
                safety_tier: c.safety_tier,
                app_id: c.app_id.clone(),
            })
            .collect();
        for t in targets {
            let _ = self.cart.stage(t);
        }
    }

    pub fn toggle_daemon_launch_agent(&mut self) {
        let plist_path = launch_agent_plist_path();
        if plist_path.exists() {
            let _ = Command::new("launchctl")
                .args(["unload", "-w", &plist_path.to_string_lossy()])
                .output();
            let _ = std::fs::remove_file(&plist_path);
            self.daemon_installed = false;
            self.toast_message = Some((
                "Background collector (siltd) disabled and LaunchAgent removed.".to_string(),
                false,
            ));
        } else {
            let daemon_bin = resolve_siltd_binary_path();
            if let Some(parent) = plist_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let log_dir = silt_core::default_data_dir();
            let _ = std::fs::create_dir_all(&log_dir);
            let plist_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.silt.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>LowPriorityIO</key>
    <true/>
    <key>Nice</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>{}/siltd.log</string>
    <key>StandardErrorPath</key>
    <string>{}/siltd.err.log</string>
</dict>
</plist>
"#,
                daemon_bin.display(),
                log_dir.display(),
                log_dir.display()
            );
            match std::fs::write(&plist_path, plist_xml) {
                Ok(()) => {
                    let _ = Command::new("launchctl")
                        .args(["load", "-w", &plist_path.to_string_lossy()])
                        .output();
                    self.daemon_installed = true;
                    self.toast_message = Some((
                        "Background collector (siltd) enabled via macOS LaunchAgent.".to_string(),
                        false,
                    ));
                }
                Err(err) => {
                    self.toast_message =
                        Some((format!("Failed to install LaunchAgent: {err}"), true));
                }
            }
        }
    }
}

pub fn reveal_in_finder(path: &Path) {
    let _ = Command::new("open").args(["-R", &path.to_string_lossy()]).spawn();
}

pub fn open_full_disk_access_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn();
}

pub fn launch_agent_plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join("com.silt.daemon.plist")
}

fn resolve_siltd_binary_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("siltd");
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from("/Applications/Silt.app/Contents/MacOS/siltd")
}
