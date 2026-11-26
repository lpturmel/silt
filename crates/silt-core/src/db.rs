//! Embedded SQLite Creep Ledger (WAL mode) storing volume telemetry,
//! directory snapshots, app rollups, cleanup candidates, and cleanup history.

use crate::attribution::{AppLocation, AppRollup, BucketKind, CleanupCandidate, SafetyTier};
use crate::scanner::{DirAggregate, ScanResult};
use crate::volume::VolumeMetrics;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVolume {
    pub uuid: String,
    pub name: String,
    pub last_mount_point: PathBuf,
    pub is_internal: bool,
    pub is_ignored: bool,
    pub ignore_reason: Option<String>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub latest_sample: Option<VolumeSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSample {
    pub id: i64,
    pub volume_uuid: String,
    pub sampled_at: i64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub physical_free_bytes: u64,
    pub available_important_bytes: u64,
    pub purgeable_bytes: u64,
    pub snapshot_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub id: i64,
    pub volume_uuid: String,
    pub taken_at: i64,
    pub trigger_kind: String,
    pub total_scanned_bytes: u64,
    pub skipped_paths: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupEvent {
    pub id: i64,
    pub volume_uuid: String,
    pub performed_at: i64,
    pub action_kind: String,
    pub summary: String,
    pub bytes_reclaimed: u64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create db dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite db at {}", path.display()))?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory SQLite")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;

            CREATE TABLE IF NOT EXISTS volumes (
                uuid TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                last_mount_point TEXT NOT NULL,
                is_internal INTEGER NOT NULL,
                is_ignored INTEGER NOT NULL DEFAULT 0,
                ignore_reason TEXT,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS volume_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                volume_uuid TEXT NOT NULL REFERENCES volumes(uuid) ON DELETE CASCADE,
                sampled_at INTEGER NOT NULL,
                total_bytes INTEGER NOT NULL,
                used_bytes INTEGER NOT NULL,
                physical_free_bytes INTEGER NOT NULL,
                available_important_bytes INTEGER NOT NULL,
                purgeable_bytes INTEGER NOT NULL,
                snapshot_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_volume_samples_vol_ts
                ON volume_samples(volume_uuid, sampled_at DESC);

            CREATE TABLE IF NOT EXISTS snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                volume_uuid TEXT NOT NULL REFERENCES volumes(uuid) ON DELETE CASCADE,
                taken_at INTEGER NOT NULL,
                trigger_kind TEXT NOT NULL,
                total_scanned_bytes INTEGER NOT NULL,
                skipped_paths INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_snapshots_vol_ts
                ON snapshots(volume_uuid, taken_at DESC);

            CREATE TABLE IF NOT EXISTS dir_entries (
                snapshot_id INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                parent_path TEXT,
                depth INTEGER NOT NULL,
                physical_bytes INTEGER NOT NULL,
                logical_bytes INTEGER NOT NULL,
                recent_7d_bytes INTEGER NOT NULL DEFAULT 0,
                recent_30d_bytes INTEGER NOT NULL DEFAULT 0,
                file_count INTEGER NOT NULL DEFAULT 0,
                app_id TEXT NOT NULL,
                category TEXT NOT NULL,
                PRIMARY KEY (snapshot_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_dir_entries_parent
                ON dir_entries(snapshot_id, parent_path);

            CREATE TABLE IF NOT EXISTS app_rollups (
                snapshot_id INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
                app_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                category TEXT NOT NULL,
                app_bundle_bytes INTEGER NOT NULL DEFAULT 0,
                data_bytes INTEGER NOT NULL DEFAULT 0,
                cache_bytes INTEGER NOT NULL DEFAULT 0,
                logs_bytes INTEGER NOT NULL DEFAULT 0,
                downloads_bytes INTEGER NOT NULL DEFAULT 0,
                dev_artifacts_bytes INTEGER NOT NULL DEFAULT 0,
                recent_7d_bytes INTEGER NOT NULL DEFAULT 0,
                recent_30d_bytes INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER NOT NULL DEFAULT 0,
                reclaimable_bytes INTEGER NOT NULL DEFAULT 0,
                is_orphaned INTEGER NOT NULL DEFAULT 0,
                locations_json TEXT NOT NULL DEFAULT '[]',
                PRIMARY KEY (snapshot_id, app_id)
            );

            CREATE TABLE IF NOT EXISTS cleanup_candidates (
                snapshot_id INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                label TEXT NOT NULL,
                app_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                category TEXT NOT NULL,
                safety_tier TEXT NOT NULL,
                bytes INTEGER NOT NULL,
                recent_7d_bytes INTEGER NOT NULL DEFAULT 0,
                reason TEXT NOT NULL,
                PRIMARY KEY (snapshot_id, path)
            );

            CREATE TABLE IF NOT EXISTS cleanup_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                volume_uuid TEXT NOT NULL REFERENCES volumes(uuid) ON DELETE CASCADE,
                performed_at INTEGER NOT NULL,
                action_kind TEXT NOT NULL,
                summary TEXT NOT NULL,
                bytes_reclaimed INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cleanup_events_vol_ts
                ON cleanup_events(volume_uuid, performed_at DESC);
            ",
        )?;
        Ok(())
    }

    pub fn upsert_volume(&self, vol: &VolumeMetrics) -> Result<()> {
        self.conn.execute(
            "
            INSERT INTO volumes (uuid, name, last_mount_point, is_internal, is_ignored, ignore_reason, first_seen_at, last_seen_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
            ON CONFLICT(uuid) DO UPDATE SET
                name = excluded.name,
                last_mount_point = excluded.last_mount_point,
                is_internal = excluded.is_internal,
                last_seen_at = excluded.last_seen_at
            ",
            params![
                vol.volume_uuid,
                vol.name,
                vol.mount_point.to_string_lossy().as_ref(),
                vol.is_internal as i32,
                vol.is_ignored as i32,
                vol.ignore_reason,
                vol.sampled_at,
            ],
        )?;
        Ok(())
    }

    pub fn record_volume_sample(&self, vol: &VolumeMetrics) -> Result<i64> {
        self.upsert_volume(vol)?;
        self.conn.execute(
            "
            INSERT INTO volume_samples (
                volume_uuid, sampled_at, total_bytes, used_bytes,
                physical_free_bytes, available_important_bytes, purgeable_bytes, snapshot_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ",
            params![
                vol.volume_uuid,
                vol.sampled_at,
                vol.total_bytes as i64,
                vol.used_bytes as i64,
                vol.physical_free_bytes as i64,
                vol.available_important_bytes as i64,
                vol.purgeable_bytes as i64,
                vol.snapshot_count as i64,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_volume_ignored(&self, uuid: &str, ignored: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE volumes SET is_ignored = ?2 WHERE uuid = ?1",
            params![uuid, ignored as i32],
        )?;
        Ok(())
    }

    pub fn list_volumes(&self) -> Result<Vec<StoredVolume>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT uuid, name, last_mount_point, is_internal, is_ignored, ignore_reason, first_seen_at, last_seen_at
            FROM volumes
            ORDER BY is_internal DESC, last_seen_at DESC
            ",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredVolume {
                uuid: row.get(0)?,
                name: row.get(1)?,
                last_mount_point: PathBuf::from(row.get::<_, String>(2)?),
                is_internal: row.get::<_, i32>(3)? != 0,
                is_ignored: row.get::<_, i32>(4)? != 0,
                ignore_reason: row.get(5)?,
                first_seen_at: row.get(6)?,
                last_seen_at: row.get(7)?,
                latest_sample: None,
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            let mut vol = r?;
            vol.latest_sample = self.latest_volume_sample(&vol.uuid)?;
            out.push(vol);
        }
        Ok(out)
    }

    pub fn latest_volume_sample(&self, volume_uuid: &str) -> Result<Option<VolumeSample>> {
        self.conn
            .query_row(
                "
                SELECT id, volume_uuid, sampled_at, total_bytes, used_bytes,
                       physical_free_bytes, available_important_bytes, purgeable_bytes, snapshot_count
                FROM volume_samples
                WHERE volume_uuid = ?1
                ORDER BY sampled_at DESC, id DESC
                LIMIT 1
                ",
                params![volume_uuid],
                row_to_sample,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn volume_samples(&self, volume_uuid: &str, since_ts: i64) -> Result<Vec<VolumeSample>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, volume_uuid, sampled_at, total_bytes, used_bytes,
                   physical_free_bytes, available_important_bytes, purgeable_bytes, snapshot_count
            FROM volume_samples
            WHERE volume_uuid = ?1 AND sampled_at >= ?2
            ORDER BY sampled_at ASC, id ASC
            ",
        )?;
        let rows = stmt.query_map(params![volume_uuid, since_ts], row_to_sample)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Persist a full or incremental `ScanResult` transactionally and return the new `snapshot_id`.
    pub fn save_scan_result(&mut self, scan: &ScanResult) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "
            INSERT INTO snapshots (volume_uuid, taken_at, trigger_kind, total_scanned_bytes, skipped_paths)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                scan.volume_uuid,
                scan.taken_at,
                scan.trigger_kind,
                scan.total_scanned_bytes as i64,
                scan.skipped_paths as i64,
            ],
        )?;
        let snapshot_id = tx.last_insert_rowid();

        {
            let mut dir_stmt = tx.prepare(
                "
                INSERT OR REPLACE INTO dir_entries (
                    snapshot_id, path, parent_path, depth, physical_bytes, logical_bytes,
                    recent_7d_bytes, recent_30d_bytes, file_count, app_id, category
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ",
            )?;
            for d in &scan.dirs {
                dir_stmt.execute(params![
                    snapshot_id,
                    d.path.to_string_lossy().as_ref(),
                    d.parent_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    d.depth as i64,
                    d.physical_bytes as i64,
                    d.logical_bytes as i64,
                    d.recent_7d_bytes as i64,
                    d.recent_30d_bytes as i64,
                    d.file_count as i64,
                    d.app_id,
                    d.category,
                ])?;
            }
        }

        {
            let mut app_stmt = tx.prepare(
                "
                INSERT OR REPLACE INTO app_rollups (
                    snapshot_id, app_id, display_name, category,
                    app_bundle_bytes, data_bytes, cache_bytes, logs_bytes,
                    downloads_bytes, dev_artifacts_bytes, recent_7d_bytes, recent_30d_bytes,
                    total_bytes, reclaimable_bytes, is_orphaned, locations_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ",
            )?;
            for a in &scan.app_rollups {
                let loc_json = serde_json::to_string(&a.locations).unwrap_or_else(|_| "[]".into());
                app_stmt.execute(params![
                    snapshot_id,
                    a.app_id,
                    a.display_name,
                    a.category,
                    a.app_bundle_bytes as i64,
                    a.data_bytes as i64,
                    a.cache_bytes as i64,
                    a.logs_bytes as i64,
                    a.downloads_bytes as i64,
                    a.dev_artifacts_bytes as i64,
                    a.recent_7d_bytes as i64,
                    a.recent_30d_bytes as i64,
                    a.total_bytes as i64,
                    a.reclaimable_bytes as i64,
                    a.is_orphaned as i32,
                    loc_json,
                ])?;
            }
        }

        {
            let mut cand_stmt = tx.prepare(
                "
                INSERT OR REPLACE INTO cleanup_candidates (
                    snapshot_id, path, label, app_id, display_name, category,
                    safety_tier, bytes, recent_7d_bytes, reason
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ",
            )?;
            for c in &scan.cleanup_candidates {
                cand_stmt.execute(params![
                    snapshot_id,
                    c.path.to_string_lossy().as_ref(),
                    c.label,
                    c.app_id,
                    c.display_name,
                    c.category,
                    c.safety_tier.as_str(),
                    c.bytes as i64,
                    c.recent_7d_bytes as i64,
                    c.reason,
                ])?;
            }
        }

        tx.commit()?;
        Ok(snapshot_id)
    }

    pub fn list_snapshots(&self, volume_uuid: &str, limit: usize) -> Result<Vec<SnapshotMeta>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, volume_uuid, taken_at, trigger_kind, total_scanned_bytes, skipped_paths
            FROM snapshots
            WHERE volume_uuid = ?1
            ORDER BY taken_at DESC, id DESC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![volume_uuid, limit as i64], row_to_snapshot)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn latest_snapshot(&self, volume_uuid: &str) -> Result<Option<SnapshotMeta>> {
        self.conn
            .query_row(
                "
                SELECT id, volume_uuid, taken_at, trigger_kind, total_scanned_bytes, skipped_paths
                FROM snapshots
                WHERE volume_uuid = ?1
                ORDER BY taken_at DESC, id DESC
                LIMIT 1
                ",
                params![volume_uuid],
                row_to_snapshot,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn snapshot_closest_to(&self, volume_uuid: &str, target_ts: i64, exclude_id: i64) -> Result<Option<SnapshotMeta>> {
        self.conn
            .query_row(
                "
                SELECT id, volume_uuid, taken_at, trigger_kind, total_scanned_bytes, skipped_paths
                FROM snapshots
                WHERE volume_uuid = ?1 AND id != ?2
                ORDER BY ABS(taken_at - ?3) ASC, taken_at ASC
                LIMIT 1
                ",
                params![volume_uuid, exclude_id, target_ts],
                row_to_snapshot,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn previous_snapshot(&self, volume_uuid: &str, current_id: i64) -> Result<Option<SnapshotMeta>> {
        self.conn
            .query_row(
                "
                SELECT id, volume_uuid, taken_at, trigger_kind, total_scanned_bytes, skipped_paths
                FROM snapshots
                WHERE volume_uuid = ?1 AND id < ?2
                ORDER BY taken_at DESC, id DESC
                LIMIT 1
                ",
                params![volume_uuid, current_id],
                row_to_snapshot,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_dir_entries(&self, snapshot_id: i64) -> Result<Vec<DirAggregate>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT path, parent_path, depth, physical_bytes, logical_bytes,
                   recent_7d_bytes, recent_30d_bytes, file_count, app_id, category
            FROM dir_entries
            WHERE snapshot_id = ?1
            ORDER BY physical_bytes DESC
            ",
        )?;
        let rows = stmt.query_map(params![snapshot_id], row_to_dir)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_dir_children(&self, snapshot_id: i64, parent_path: &Path) -> Result<Vec<DirAggregate>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT path, parent_path, depth, physical_bytes, logical_bytes,
                   recent_7d_bytes, recent_30d_bytes, file_count, app_id, category
            FROM dir_entries
            WHERE snapshot_id = ?1 AND parent_path = ?2
            ORDER BY physical_bytes DESC
            ",
        )?;
        let rows = stmt.query_map(
            params![snapshot_id, parent_path.to_string_lossy().as_ref()],
            row_to_dir,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_app_rollups(&self, snapshot_id: i64) -> Result<Vec<AppRollup>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT app_id, display_name, category,
                   app_bundle_bytes, data_bytes, cache_bytes, logs_bytes,
                   downloads_bytes, dev_artifacts_bytes, recent_7d_bytes, recent_30d_bytes,
                   total_bytes, reclaimable_bytes, is_orphaned, locations_json
            FROM app_rollups
            WHERE snapshot_id = ?1
            ORDER BY total_bytes DESC
            ",
        )?;
        let rows = stmt.query_map(params![snapshot_id], |row| {
            let loc_str: String = row.get(14)?;
            let locations: Vec<AppLocation> =
                serde_json::from_str(&loc_str).unwrap_or_default();
            Ok(AppRollup {
                app_id: row.get(0)?,
                display_name: row.get(1)?,
                category: row.get(2)?,
                app_bundle_bytes: row.get::<_, i64>(3)? as u64,
                data_bytes: row.get::<_, i64>(4)? as u64,
                cache_bytes: row.get::<_, i64>(5)? as u64,
                logs_bytes: row.get::<_, i64>(6)? as u64,
                downloads_bytes: row.get::<_, i64>(7)? as u64,
                dev_artifacts_bytes: row.get::<_, i64>(8)? as u64,
                recent_7d_bytes: row.get::<_, i64>(9)? as u64,
                recent_30d_bytes: row.get::<_, i64>(10)? as u64,
                total_bytes: row.get::<_, i64>(11)? as u64,
                reclaimable_bytes: row.get::<_, i64>(12)? as u64,
                is_orphaned: row.get::<_, i32>(13)? != 0,
                locations,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn app_size_history(&self, volume_uuid: &str, app_id: &str, limit: usize) -> Result<Vec<u64>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT r.total_bytes
            FROM app_rollups r
            JOIN snapshots s ON s.id = r.snapshot_id
            WHERE s.volume_uuid = ?1 AND r.app_id = ?2
            ORDER BY s.taken_at DESC
            LIMIT ?3
            ",
        )?;
        let mut pts: Vec<u64> = stmt
            .query_map(params![volume_uuid, app_id, limit as i64], |row| {
                Ok(row.get::<_, i64>(0)? as u64)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        pts.reverse();
        Ok(pts)
    }

    pub fn list_cleanup_candidates(&self, snapshot_id: i64) -> Result<Vec<CleanupCandidate>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT path, label, app_id, display_name, category,
                   safety_tier, bytes, recent_7d_bytes, reason
            FROM cleanup_candidates
            WHERE snapshot_id = ?1
            ORDER BY bytes DESC
            ",
        )?;
        let rows = stmt.query_map(params![snapshot_id], |row| {
            Ok(CleanupCandidate {
                path: PathBuf::from(row.get::<_, String>(0)?),
                label: row.get(1)?,
                app_id: row.get(2)?,
                display_name: row.get(3)?,
                category: row.get(4)?,
                safety_tier: SafetyTier::from_str_lossy(&row.get::<_, String>(5)?),
                bytes: row.get::<_, i64>(6)? as u64,
                recent_7d_bytes: row.get::<_, i64>(7)? as u64,
                reason: row.get(8)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn record_cleanup_event(
        &self,
        volume_uuid: &str,
        performed_at: i64,
        action_kind: &str,
        summary: &str,
        bytes_reclaimed: u64,
    ) -> Result<i64> {
        self.conn.execute(
            "
            INSERT INTO cleanup_events (volume_uuid, performed_at, action_kind, summary, bytes_reclaimed)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                volume_uuid,
                performed_at,
                action_kind,
                summary,
                bytes_reclaimed as i64,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_cleanup_events(&self, volume_uuid: &str, since_ts: i64) -> Result<Vec<CleanupEvent>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, volume_uuid, performed_at, action_kind, summary, bytes_reclaimed
            FROM cleanup_events
            WHERE volume_uuid = ?1 AND performed_at >= ?2
            ORDER BY performed_at ASC
            ",
        )?;
        let rows = stmt.query_map(params![volume_uuid, since_ts], |row| {
            Ok(CleanupEvent {
                id: row.get(0)?,
                volume_uuid: row.get(1)?,
                performed_at: row.get(2)?,
                action_kind: row.get(3)?,
                summary: row.get(4)?,
                bytes_reclaimed: row.get::<_, i64>(5)? as u64,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Prune old snapshots beyond `keep_days` (always keeping at least the 5 most recent per volume).
    pub fn prune_old_snapshots(&self, volume_uuid: &str, older_than_ts: i64) -> Result<usize> {
        let deleted = self.conn.execute(
            "
            DELETE FROM snapshots
            WHERE volume_uuid = ?1
              AND taken_at < ?2
              AND id NOT IN (
                  SELECT id FROM snapshots
                  WHERE volume_uuid = ?1
                  ORDER BY taken_at DESC
                  LIMIT 5
              )
            ",
            params![volume_uuid, older_than_ts],
        )?;
        Ok(deleted)
    }
}

fn row_to_sample(row: &rusqlite::Row<'_>) -> rusqlite::Result<VolumeSample> {
    Ok(VolumeSample {
        id: row.get(0)?,
        volume_uuid: row.get(1)?,
        sampled_at: row.get(2)?,
        total_bytes: row.get::<_, i64>(3)? as u64,
        used_bytes: row.get::<_, i64>(4)? as u64,
        physical_free_bytes: row.get::<_, i64>(5)? as u64,
        available_important_bytes: row.get::<_, i64>(6)? as u64,
        purgeable_bytes: row.get::<_, i64>(7)? as u64,
        snapshot_count: row.get::<_, i64>(8)? as u32,
    })
}

fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotMeta> {
    Ok(SnapshotMeta {
        id: row.get(0)?,
        volume_uuid: row.get(1)?,
        taken_at: row.get(2)?,
        trigger_kind: row.get(3)?,
        total_scanned_bytes: row.get::<_, i64>(4)? as u64,
        skipped_paths: row.get::<_, i64>(5)? as u64,
    })
}

fn row_to_dir(row: &rusqlite::Row<'_>) -> rusqlite::Result<DirAggregate> {
    Ok(DirAggregate {
        path: PathBuf::from(row.get::<_, String>(0)?),
        parent_path: row.get::<_, Option<String>>(1)?.map(PathBuf::from),
        depth: row.get::<_, i64>(2)? as u32,
        physical_bytes: row.get::<_, i64>(3)? as u64,
        logical_bytes: row.get::<_, i64>(4)? as u64,
        recent_7d_bytes: row.get::<_, i64>(5)? as u64,
        recent_30d_bytes: row.get::<_, i64>(6)? as u64,
        file_count: row.get::<_, i64>(7)? as u64,
        app_id: row.get(8)?,
        category: row.get(9)?,
    })
}

// Keep unused BucketKind import warning-free when compiling without extra traits
#[allow(dead_code)]
fn _bucket_kind_marker(_: BucketKind) {}
