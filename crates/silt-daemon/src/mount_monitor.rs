//! External volume mount/unmount detector so `siltd` automatically snapshots
//! tracked external drives on plug-in for "Since Last Connected" creep diffs.

use silt_core::volume::{VolumeMetrics, discover_mounted_volumes};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug)]
pub struct MountDiff {
    pub newly_mounted: Vec<VolumeMetrics>,
    pub unmounted: Vec<(String, PathBuf)>,
    pub online: Vec<VolumeMetrics>,
}

pub struct MountMonitor {
    known_online: HashMap<String, PathBuf>,
}

impl MountMonitor {
    pub fn new() -> Self {
        Self {
            known_online: HashMap::new(),
        }
    }

    /// Poll currently mounted volumes and report newly connected & disconnected non-ignored drives.
    pub fn poll(&mut self) -> anyhow::Result<MountDiff> {
        let all = discover_mounted_volumes()?;
        let mut newly_mounted = Vec::new();
        let mut current_uuids = HashSet::new();

        for vol in &all {
            if vol.is_ignored {
                continue;
            }
            current_uuids.insert(vol.volume_uuid.clone());
            if !self.known_online.contains_key(&vol.volume_uuid) {
                self.known_online
                    .insert(vol.volume_uuid.clone(), vol.mount_point.clone());
                newly_mounted.push(vol.clone());
            }
        }

        let mut unmounted = Vec::new();
        self.known_online.retain(|uuid, path| {
            if current_uuids.contains(uuid) {
                true
            } else {
                unmounted.push((uuid.clone(), path.clone()));
                false
            }
        });

        Ok(MountDiff {
            newly_mounted,
            unmounted,
            online: all,
        })
    }
}
