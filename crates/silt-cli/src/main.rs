//! `silt-cli` — terminal companion for Silt storage intelligence & creep tracking.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use silt_core::attribution::AttributionEngine;
use silt_core::db::Database;
use silt_core::diff::{CreepBasis, TimeWindow, compute_creep_report};
use silt_core::format::{format_bytes, format_rate_per_day, format_signed_bytes};
use silt_core::scanner::{ScanConfig, scan_volume};
use silt_core::volume::{VolumeMetrics, discover_mounted_volumes};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "silt-cli",
    version,
    about = "Silt — macOS storage intelligence, creep tracking, and safe cleanup"
)]
struct Cli {
    /// Override SQLite database path (defaults to ~/Library/Application Support/Silt/silt.db)
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Output JSON instead of formatted tables
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show Remaining, macOS Available, Purgeable, and APFS Snapshots for all online & remembered drives
    Status,
    /// Run a filesystem snapshot and persist it to the Creep Ledger
    Scan {
        /// Mount point to scan (defaults to `/` for internal Macintosh HD)
        #[arg(long, default_value = "/")]
        volume: PathBuf,
        /// Also scan `/private/var` on internal volume
        #[arg(long)]
        include_private_var: bool,
    },
    /// Show Top Creepers (apps & directories growing over time)
    Creep {
        /// Mount point or Volume UUID (defaults to `/`)
        #[arg(long, default_value = "/")]
        volume: String,
        /// Time window for creep calculation
        #[arg(long, value_enum, default_value = "7d")]
        window: WindowArg,
        /// Max rows to print
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Show unified macOS App Attribution breakdown
    Apps {
        /// Mount point or Volume UUID (defaults to `/`)
        #[arg(long, default_value = "/")]
        volume: String,
        #[arg(long, value_enum, default_value = "size")]
        sort: AppSort,
        /// Show only orphaned leftovers from uninstalled apps
        #[arg(long)]
        orphans_only: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Preview safe cleanup candidates detected in the latest snapshot
    CleanupPreview {
        #[arg(long, default_value = "/")]
        volume: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Manage the opt-in background LaunchAgent collector (siltd)
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Install and start ~/Library/LaunchAgents/com.silt.daemon.plist
    Install {
        /// Optional explicit path to the `siltd` binary
        #[arg(long)]
        binary: Option<PathBuf>,
    },
    /// Stop and remove the LaunchAgent plist
    Uninstall,
    /// Show LaunchAgent installation status
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum WindowArg {
    #[value(name = "24h")]
    Hours24,
    #[value(name = "7d")]
    Days7,
    #[value(name = "30d")]
    Days30,
    #[value(name = "90d")]
    Days90,
    #[value(name = "last")]
    SinceLastConnected,
    #[value(name = "all")]
    All,
}

impl From<WindowArg> for TimeWindow {
    fn from(w: WindowArg) -> Self {
        match w {
            WindowArg::Hours24 => TimeWindow::Hours24,
            WindowArg::Days7 => TimeWindow::Days7,
            WindowArg::Days30 => TimeWindow::Days30,
            WindowArg::Days90 => TimeWindow::Days90,
            WindowArg::SinceLastConnected => TimeWindow::SinceLastConnected,
            WindowArg::All => TimeWindow::All,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum AppSort {
    Size,
    Growth,
    Reclaimable,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(silt_core::default_db_path);
    let mut db = Database::open(&db_path)?;

    match cli.command {
        Commands::Status => cmd_status(&db, cli.json),
        Commands::Scan {
            volume,
            include_private_var,
        } => cmd_scan(&mut db, &volume, include_private_var, cli.json),
        Commands::Creep {
            volume,
            window,
            limit,
        } => cmd_creep(&db, &volume, window.into(), limit, cli.json),
        Commands::Apps {
            volume,
            sort,
            orphans_only,
            limit,
        } => cmd_apps(&db, &volume, sort, orphans_only, limit, cli.json),
        Commands::CleanupPreview { volume, limit } => {
            cmd_cleanup_preview(&db, &volume, limit, cli.json)
        }
        Commands::Daemon { action } => cmd_daemon(action),
    }
}

fn cmd_status(db: &Database, json: bool) -> Result<()> {
    let mounted = discover_mounted_volumes()?;
    for v in &mounted {
        let _ = db.record_volume_sample(v);
    }
    let stored = db.list_volumes()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&mounted)?);
        return Ok(());
    }

    let fda = silt_core::has_full_disk_access();
    println!(
        "Silt Storage Status  |  Full Disk Access: {}",
        if fda { "Granted" } else { "Limited (optional)" }
    );
    println!("{}", "=".repeat(84));

    for v in &mounted {
        let badge = if v.is_ignored {
            format!(" [auto-ignored: {}]", v.ignore_reason.as_deref().unwrap_or("filtered"))
        } else if v.is_internal {
            " [Internal]".to_string()
        } else {
            " [External · Online]".to_string()
        };
        println!("\n{} ({}){}", v.name, v.mount_point.display(), badge);
        println!("  UUID:               {}", v.volume_uuid);
        println!("  Total Capacity:     {}", format_bytes(v.total_bytes));
        println!("  Active Used:        {}", format_bytes(v.used_bytes));
        println!(
            "  Physical Remaining: {}  (raw unallocated blocks)",
            format_bytes(v.physical_free_bytes)
        );
        println!(
            "  macOS Available:    {}  (Finder Available = Remaining + Purgeable)",
            format_bytes(v.available_important_bytes)
        );
        println!(
            "  Purgeable Headroom: {}  ({} APFS snapshots)",
            format_bytes(v.purgeable_bytes),
            v.snapshot_count
        );
    }

    // List remembered offline external drives
    let online_uuids: std::collections::HashSet<&str> =
        mounted.iter().map(|v| v.volume_uuid.as_str()).collect();
    let offline: Vec<_> = stored
        .iter()
        .filter(|s| !online_uuids.contains(s.uuid.as_str()) && !s.is_ignored)
        .collect();
    if !offline.is_empty() {
        println!("\nRemembered Offline Drives:");
        for s in offline {
            if let Some(sample) = &s.latest_sample {
                println!(
                    "  {} (last at {}) — {} available / {} total",
                    s.name,
                    s.last_mount_point.display(),
                    format_bytes(sample.available_important_bytes),
                    format_bytes(sample.total_bytes)
                );
            }
        }
    }

    Ok(())
}

fn cmd_scan(
    db: &mut Database,
    mount_point: &PathBuf,
    include_private_var: bool,
    json: bool,
) -> Result<()> {
    let mounted = discover_mounted_volumes()?;
    let vol: VolumeMetrics = mounted
        .into_iter()
        .find(|v| &v.mount_point == mount_point)
        .with_context(|| format!("mounted volume not found at {}", mount_point.display()))?;

    db.record_volume_sample(&vol)?;
    let attr = AttributionEngine::discover();
    let mut cfg = ScanConfig::default_for_volume(&vol, "cli");
    cfg.include_private_var = include_private_var;

    if !json {
        eprintln!(
            "Scanning {} ({})...",
            vol.name,
            cfg.roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let result = scan_volume(
        &cfg,
        &attr,
        if json {
            None
        } else {
            Some(&|p| {
                eprint!(
                    "\r  scanned {} files ({}) · skipped {} protected   ",
                    p.files_scanned,
                    format_bytes(p.bytes_scanned),
                    p.skipped_permissions
                );
            })
        },
    )?;

    if !json {
        eprintln!();
    }

    let snap_id = db.save_scan_result(&result)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "snapshot_id": snap_id,
                "volume_uuid": result.volume_uuid,
                "total_scanned_bytes": result.total_scanned_bytes,
                "total_files": result.total_files,
                "skipped_paths": result.skipped_paths,
                "dirs_recorded": result.dirs.len(),
                "apps_recorded": result.app_rollups.len(),
                "cleanup_candidates": result.cleanup_candidates.len(),
            }))?
        );
    } else {
        println!(
            "Saved snapshot #{snap_id} for {}: {} physical ({} files, {} apps, {} cleanup candidates, {} skipped)",
            vol.name,
            format_bytes(result.total_scanned_bytes),
            result.total_files,
            result.app_rollups.len(),
            result.cleanup_candidates.len(),
            result.skipped_paths,
        );
    }
    Ok(())
}

fn resolve_volume_uuid(db: &Database, query: &str) -> Result<String> {
    if let Ok(mounted) = discover_mounted_volumes()
        && let Some(v) = mounted
            .iter()
            .find(|v| v.mount_point.to_string_lossy() == query || v.volume_uuid == query || v.name.eq_ignore_ascii_case(query))
    {
        return Ok(v.volume_uuid.clone());
    }
    let stored = db.list_volumes()?;
    if let Some(s) = stored
        .iter()
        .find(|s| s.last_mount_point.to_string_lossy() == query || s.uuid == query || s.name.eq_ignore_ascii_case(query))
    {
        return Ok(s.uuid.clone());
    }
    anyhow::bail!("volume not found: {query} (run `silt-cli status` first)")
}

fn cmd_creep(
    db: &Database,
    volume: &str,
    window: TimeWindow,
    limit: usize,
    json: bool,
) -> Result<()> {
    let uuid = resolve_volume_uuid(db, volume)?;
    let report = compute_creep_report(db, &uuid, window)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("Creep Report ({}) — Window: {}", volume, window.label());
    match &report.basis {
        CreepBasis::ExactSnapshotDiff { from_ts, to_ts } => {
            println!("Basis: Exact Snapshot Diff ({from_ts} -> {to_ts})");
        }
        CreepBasis::EstimatedFromFileDates { window_days } => {
            println!("Basis: Day-1 Estimated Recent Activity ({window_days}d file dates — exact diffs start on Snapshot #2)");
        }
        CreepBasis::NoSnapshotsYet => {
            println!("No snapshots yet. Run `silt-cli scan` first.");
            return Ok(());
        }
    }

    println!(
        "Net Change: {} ({})",
        format_signed_bytes(report.net_delta_bytes),
        format_rate_per_day(report.avg_bytes_per_day)
    );
    if let Some(r) = &report.runway {
        print!("Runway Forecast: {}", format_rate_per_day(r.bytes_per_day));
        if let Some(d10) = r.days_until_ten_percent {
            print!(" · ~{d10:.0} days until <10% free");
        }
        println!();
    }

    println!("\nTop Creeping Apps / Categories:");
    println!(
        "{:<28} {:>12} {:>12} {:>12}  Category",
        "App", "Delta", "Total", "Reclaimable"
    );
    println!("{}", "-".repeat(82));
    for item in report.top_apps.iter().take(limit) {
        println!(
            "{:<28} {:>12} {:>12} {:>12}  {}",
            truncate(&item.label, 28),
            format_signed_bytes(item.delta_bytes),
            format_bytes(item.current_bytes),
            format_bytes(item.reclaimable_bytes),
            item.category
        );
    }

    Ok(())
}

fn cmd_apps(
    db: &Database,
    volume: &str,
    sort: AppSort,
    orphans_only: bool,
    limit: usize,
    json: bool,
) -> Result<()> {
    let uuid = resolve_volume_uuid(db, volume)?;
    let Some(snap) = db.latest_snapshot(&uuid)? else {
        anyhow::bail!("no snapshots for {volume}; run `silt-cli scan` first");
    };
    let mut apps = db.list_app_rollups(snap.id)?;
    if orphans_only {
        apps.retain(|a| a.is_orphaned);
    }
    match sort {
        AppSort::Size => apps.sort_by_key(|a| std::cmp::Reverse(a.total_bytes)),
        AppSort::Growth => apps.sort_by_key(|a| std::cmp::Reverse(a.recent_7d_bytes)),
        AppSort::Reclaimable => apps.sort_by_key(|a| std::cmp::Reverse(a.reclaimable_bytes)),
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&apps)?);
        return Ok(());
    }

    println!(
        "{:<26} {:>10} {:>10} {:>10} {:>10} {:>10}  Status",
        "App", "Total", "7d Active", "Caches", "Dev/Art", "Reclaim"
    );
    println!("{}", "-".repeat(96));
    for a in apps.iter().take(limit) {
        println!(
            "{:<26} {:>10} {:>10} {:>10} {:>10} {:>10}  {}",
            truncate(&a.display_name, 26),
            format_bytes(a.total_bytes),
            format_bytes(a.recent_7d_bytes),
            format_bytes(a.cache_bytes),
            format_bytes(a.dev_artifacts_bytes),
            format_bytes(a.reclaimable_bytes),
            if a.is_orphaned { "Orphaned" } else { &a.category }
        );
    }
    Ok(())
}

fn cmd_cleanup_preview(db: &Database, volume: &str, limit: usize, json: bool) -> Result<()> {
    let uuid = resolve_volume_uuid(db, volume)?;
    let Some(snap) = db.latest_snapshot(&uuid)? else {
        anyhow::bail!("no snapshots for {volume}; run `silt-cli scan` first");
    };
    let cands = db.list_cleanup_candidates(snap.id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&cands)?);
        return Ok(());
    }

    let total: u64 = cands.iter().map(|c| c.bytes).sum();
    println!(
        "Safe Cleanup Candidates ({} total, {} reclaimable):",
        cands.len(),
        format_bytes(total)
    );
    println!(
        "{:<22} {:>10} {:<20} Path",
        "App", "Size", "Tier"
    );
    println!("{}", "-".repeat(94));
    for c in cands.iter().take(limit) {
        println!(
            "{:<22} {:>10} {:<20} {}",
            truncate(&c.display_name, 22),
            format_bytes(c.bytes),
            c.safety_tier.label(),
            c.path.display()
        );
    }
    Ok(())
}

fn cmd_daemon(action: DaemonAction) -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let plist_path = PathBuf::from(&home)
        .join("Library/LaunchAgents")
        .join("com.silt.daemon.plist");

    match action {
        DaemonAction::Status => {
            if plist_path.exists() {
                println!("siltd LaunchAgent: Installed ({})", plist_path.display());
            } else {
                println!("siltd LaunchAgent: Not installed (run `silt-cli daemon install` or enable in Silt Settings)");
            }
            Ok(())
        }
        DaemonAction::Install { binary } => {
            let bin_path = binary.unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(|d| d.join("siltd")))
                    .filter(|p| p.exists())
                    .unwrap_or_else(|| PathBuf::from("/Applications/Silt.app/Contents/MacOS/siltd"))
            });
            if let Some(parent) = plist_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let log_dir = silt_core::default_data_dir();
            std::fs::create_dir_all(&log_dir)?;
            let xml = format!(
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
                bin_path.display(),
                log_dir.display(),
                log_dir.display()
            );
            std::fs::write(&plist_path, xml)?;
            let _ = std::process::Command::new("launchctl")
                .args(["load", "-w", &plist_path.to_string_lossy()])
                .output();
            println!(
                "Installed and loaded siltd LaunchAgent at {} (binary: {})",
                plist_path.display(),
                bin_path.display()
            );
            Ok(())
        }
        DaemonAction::Uninstall => {
            if plist_path.exists() {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", "-w", &plist_path.to_string_lossy()])
                    .output();
                std::fs::remove_file(&plist_path)?;
                println!("Unloaded and removed {}", plist_path.display());
            } else {
                println!("siltd LaunchAgent is not installed.");
            }
            Ok(())
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
