//! macOS volume discovery and APFS-accurate capacity metrics.
//!
//! Uses CoreFoundation `CFURLCopyResourcePropertyForKey` (the C API underlying
//! `NSURL.resourceValues(forKeys:)`) to query exact Finder-compatible capacity
//! values alongside physical unallocated space and purgeable headroom.

use crate::apfs::{ApfsSnapshot, list_apfs_snapshots};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMetrics {
    pub volume_uuid: String,
    pub name: String,
    pub mount_point: PathBuf,
    pub is_internal: bool,
    pub is_removable: bool,
    pub is_online: bool,
    pub is_ignored: bool,
    pub ignore_reason: Option<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    /// True unallocated disk blocks (close to POSIX `df` available).
    pub physical_free_bytes: u64,
    /// Space macOS considers available for important usage (includes purgeable).
    pub available_important_bytes: u64,
    /// Space available for opportunistic / cache usage.
    pub available_opportunistic_bytes: u64,
    /// Reclaimable purgeable headroom (`available_important - physical_free`).
    pub purgeable_bytes: u64,
    pub snapshot_count: u32,
    pub snapshots: Vec<ApfsSnapshot>,
    pub sampled_at: i64,
}

impl VolumeMetrics {
    pub fn used_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    pub fn purgeable_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.purgeable_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    pub fn physical_free_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.physical_free_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }
}

/// Compute purgeable and used bytes consistently from total, physical free, and available important.
pub fn compute_capacity_breakdown(
    total_bytes: u64,
    physical_free_bytes: u64,
    available_important_bytes: u64,
) -> (u64, u64) {
    let effective_available = available_important_bytes.max(physical_free_bytes).min(total_bytes);
    let purgeable_bytes = effective_available.saturating_sub(physical_free_bytes);
    // "Active" non-purgeable used space = total - available_important
    let used_bytes = total_bytes.saturating_sub(effective_available);
    (used_bytes, purgeable_bytes)
}

#[cfg(target_os = "macos")]
mod macos_cf {
    use std::ffi::{CStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    type CFAllocatorRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CFURLRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CFBooleanRef = *const c_void;
    type CFErrorRef = *mut c_void;

    const K_CF_NUMBER_SINT64_TYPE: isize = 4;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFAllocatorDefault: CFAllocatorRef;
        static kCFURLVolumeUUIDStringKey: CFStringRef;
        static kCFURLVolumeTotalCapacityKey: CFStringRef;
        static kCFURLVolumeAvailableCapacityKey: CFStringRef;
        static kCFURLVolumeAvailableCapacityForImportantUsageKey: CFStringRef;
        static kCFURLVolumeAvailableCapacityForOpportunisticUsageKey: CFStringRef;
        static kCFURLVolumeIsInternalKey: CFStringRef;
        static kCFURLVolumeIsRemovableKey: CFStringRef;
        static kCFURLVolumeIsEjectableKey: CFStringRef;
        static kCFURLVolumeIsReadOnlyKey: CFStringRef;
        static kCFURLVolumeIsLocalKey: CFStringRef;
        static kCFURLVolumeLocalizedNameKey: CFStringRef;

        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFAllocatorRef,
            buffer: *const u8,
            buf_len: isize,
            is_directory: u8,
        ) -> CFURLRef;

        fn CFURLCopyResourcePropertyForKey(
            url: CFURLRef,
            key: CFStringRef,
            property_value_type_ref_ptr: *mut CFTypeRef,
            error: *mut CFErrorRef,
        ) -> u8;

        fn CFNumberGetValue(
            number: CFNumberRef,
            the_type: isize,
            value_ptr: *mut c_void,
        ) -> u8;

        fn CFBooleanGetValue(boolean: CFBooleanRef) -> u8;

        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;

        fn CFRelease(cf: CFTypeRef);
    }

    pub struct CfVolumeProps {
        pub uuid: Option<String>,
        pub name: Option<String>,
        pub total_bytes: Option<u64>,
        pub physical_free_bytes: Option<u64>,
        pub available_important_bytes: Option<u64>,
        pub available_opportunistic_bytes: Option<u64>,
        pub is_internal: Option<bool>,
        pub is_removable: Option<bool>,
        pub is_ejectable: Option<bool>,
        pub is_read_only: Option<bool>,
        pub is_local: Option<bool>,
    }

    unsafe fn copy_prop(url: CFURLRef, key: CFStringRef) -> Option<CFTypeRef> {
        let mut value: CFTypeRef = ptr::null();
        let mut err: CFErrorRef = ptr::null_mut();
        let ok = unsafe { CFURLCopyResourcePropertyForKey(url, key, &mut value, &mut err) };
        if !err.is_null() {
            unsafe { CFRelease(err as CFTypeRef) };
        }
        if ok != 0 && !value.is_null() {
            Some(value)
        } else {
            None
        }
    }

    unsafe fn read_u64(url: CFURLRef, key: CFStringRef) -> Option<u64> {
        let cf = unsafe { copy_prop(url, key)? };
        let mut out: i64 = 0;
        let ok = unsafe {
            CFNumberGetValue(
                cf as CFNumberRef,
                K_CF_NUMBER_SINT64_TYPE,
                &mut out as *mut i64 as *mut c_void,
            )
        };
        unsafe { CFRelease(cf) };
        if ok != 0 && out >= 0 {
            Some(out as u64)
        } else {
            None
        }
    }

    unsafe fn read_bool(url: CFURLRef, key: CFStringRef) -> Option<bool> {
        let cf = unsafe { copy_prop(url, key)? };
        let val = unsafe { CFBooleanGetValue(cf as CFBooleanRef) != 0 };
        unsafe { CFRelease(cf) };
        Some(val)
    }

    unsafe fn read_string(url: CFURLRef, key: CFStringRef) -> Option<String> {
        let cf = unsafe { copy_prop(url, key)? };
        let mut buf = [0u8; 1024];
        let ok = unsafe {
            CFStringGetCString(
                cf as CFStringRef,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        unsafe { CFRelease(cf) };
        if ok != 0 {
            let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
            Some(cstr.to_string_lossy().into_owned())
        } else {
            None
        }
    }

    pub fn query_cf_volume_props(path: &Path) -> Option<CfVolumeProps> {
        let bytes = path.as_os_str().as_bytes();
        unsafe {
            let url = CFURLCreateFromFileSystemRepresentation(
                kCFAllocatorDefault,
                bytes.as_ptr(),
                bytes.len() as isize,
                1,
            );
            if url.is_null() {
                return None;
            }
            let props = CfVolumeProps {
                uuid: read_string(url, kCFURLVolumeUUIDStringKey),
                name: read_string(url, kCFURLVolumeLocalizedNameKey),
                total_bytes: read_u64(url, kCFURLVolumeTotalCapacityKey),
                physical_free_bytes: read_u64(url, kCFURLVolumeAvailableCapacityKey),
                available_important_bytes: read_u64(
                    url,
                    kCFURLVolumeAvailableCapacityForImportantUsageKey,
                ),
                available_opportunistic_bytes: read_u64(
                    url,
                    kCFURLVolumeAvailableCapacityForOpportunisticUsageKey,
                ),
                is_internal: read_bool(url, kCFURLVolumeIsInternalKey),
                is_removable: read_bool(url, kCFURLVolumeIsRemovableKey),
                is_ejectable: read_bool(url, kCFURLVolumeIsEjectableKey),
                is_read_only: read_bool(url, kCFURLVolumeIsReadOnlyKey),
                is_local: read_bool(url, kCFURLVolumeIsLocalKey),
            };
            CFRelease(url);
            Some(props)
        }
    }
}

/// Query POSIX `statfs` fallback for total and physical free bytes.
fn statfs_capacity(path: &Path) -> Result<(u64, u64)> {
    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .context("invalid mount path containing NUL")?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("statfs failed");
    }
    let block_size = stat.f_bsize as u64;
    let total = stat.f_blocks.saturating_mul(block_size);
    let free = stat.f_bavail.saturating_mul(block_size);
    Ok((total, free))
}

/// Detect whether an external mount should be auto-ignored by default
/// (transient `.dmg` disk images, Time Machine backup volumes, or network shares).
pub fn detect_auto_ignore_reason(
    mount_point: &Path,
    is_internal: bool,
    is_read_only: bool,
    is_ejectable: bool,
    is_local: bool,
) -> Option<String> {
    if is_internal {
        return None;
    }

    if !is_local {
        return Some("Network volume".to_string());
    }

    // Time Machine backup drive heuristics (fast local marker checks)
    if mount_point.join("Backups.backupdb").exists()
        || mount_point.join(".com.apple.timemachine.donotpresent").exists()
        || mount_point.join("backup_manifest.plist").exists()
    {
        return Some("Time Machine backup volume".to_string());
    }

    // Read-only ejectable volumes are almost always mounted `.dmg` installer images
    if is_read_only && is_ejectable {
        return Some("Read-only disk image (DMG)".to_string());
    }

    // Inspect diskutil info -plist for virtual Disk Image protocol
    if let Ok(output) = Command::new("diskutil")
        .args(["info", "-plist", &mount_point.to_string_lossy()])
        .output()
        && output.status.success()
        && let Ok(val) = plist::from_bytes::<plist::Value>(&output.stdout)
        && let Some(dict) = val.as_dictionary()
    {
        let bus = dict
            .get("BusProtocol")
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let virt = dict
            .get("VirtualOrPhysical")
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if bus.eq_ignore_ascii_case("Disk Image") || virt.eq_ignore_ascii_case("Virtual") {
            return Some("Transient disk image (DMG)".to_string());
        }
    }

    None
}

/// Inspect a single mounted volume path and return full macOS capacity metrics.
pub fn inspect_volume(mount_point: &Path) -> Result<VolumeMetrics> {
    let (stat_total, stat_free) = statfs_capacity(mount_point)?;
    let now = Utc::now().timestamp();

    #[cfg(target_os = "macos")]
    let cf = macos_cf::query_cf_volume_props(mount_point);
    #[cfg(not(target_os = "macos"))]
    let cf: Option<()> = None;

    #[cfg(target_os = "macos")]
    let (
        uuid,
        name,
        total_bytes,
        physical_free_bytes,
        available_important_bytes,
        available_opportunistic_bytes,
        is_internal,
        is_removable,
        is_ejectable,
        is_read_only,
        is_local,
    ) = if let Some(p) = cf {
        let total = p.total_bytes.unwrap_or(stat_total);
        // Use statfs f_bavail / CFURLVolumeAvailableCapacity for physical remaining
        let phys_free = p.physical_free_bytes.unwrap_or(stat_free);
        let avail_imp = p.available_important_bytes.unwrap_or(phys_free);
        let avail_opp = p.available_opportunistic_bytes.unwrap_or(phys_free);
        let internal = p
            .is_internal
            .unwrap_or_else(|| mount_point == Path::new("/"));
        let removable = p.is_removable.unwrap_or(!internal);
        let ejectable = p.is_ejectable.unwrap_or(!internal);
        let read_only = p.is_read_only.unwrap_or(false);
        let local = p.is_local.unwrap_or(true);
        let uuid = p
            .uuid
            .unwrap_or_else(|| format!("mount:{}", mount_point.to_string_lossy()));
        let default_name = if mount_point == Path::new("/") {
            "Macintosh HD".to_string()
        } else {
            mount_point
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| mount_point.to_string_lossy().into_owned())
        };
        let name = p.name.unwrap_or(default_name);
        (
            uuid,
            name,
            total,
            phys_free,
            avail_imp,
            avail_opp,
            internal,
            removable,
            ejectable,
            read_only,
            local,
        )
    } else {
        let is_internal = mount_point == Path::new("/");
        (
            format!("mount:{}", mount_point.to_string_lossy()),
            mount_point.to_string_lossy().into_owned(),
            stat_total,
            stat_free,
            stat_free,
            stat_free,
            is_internal,
            !is_internal,
            !is_internal,
            false,
            true,
        )
    };

    #[cfg(not(target_os = "macos"))]
    let (
        uuid,
        name,
        total_bytes,
        physical_free_bytes,
        available_important_bytes,
        available_opportunistic_bytes,
        is_internal,
        is_removable,
        is_ejectable,
        is_read_only,
        is_local,
    ) = {
        let _ = cf;
        let is_internal = mount_point == Path::new("/");
        (
            format!("mount:{}", mount_point.to_string_lossy()),
            mount_point.to_string_lossy().into_owned(),
            stat_total,
            stat_free,
            stat_free,
            stat_free,
            is_internal,
            !is_internal,
            !is_internal,
            false,
            true,
        )
    };

    let effective_available_important = available_important_bytes
        .max(physical_free_bytes)
        .min(total_bytes);
    let effective_available_opportunistic = available_opportunistic_bytes
        .max(physical_free_bytes)
        .min(total_bytes);
    let (used_bytes, purgeable_bytes) =
        compute_capacity_breakdown(total_bytes, physical_free_bytes, effective_available_important);

    let ignore_reason = detect_auto_ignore_reason(
        mount_point,
        is_internal,
        is_read_only,
        is_ejectable,
        is_local,
    );
    let is_ignored = ignore_reason.is_some();

    let snapshots = if !is_ignored && is_local {
        list_apfs_snapshots(mount_point)
    } else {
        Vec::new()
    };

    Ok(VolumeMetrics {
        volume_uuid: uuid,
        name,
        mount_point: mount_point.to_path_buf(),
        is_internal,
        is_removable,
        is_online: true,
        is_ignored,
        ignore_reason,
        total_bytes,
        used_bytes,
        physical_free_bytes,
        available_important_bytes: effective_available_important,
        available_opportunistic_bytes: effective_available_opportunistic,
        purgeable_bytes,
        snapshot_count: snapshots.len() as u32,
        snapshots,
        sampled_at: now,
    })
}

/// Discover the primary internal volume (`/`) and all mounted external volumes under `/Volumes`.
/// Deduplicates symlink aliases to `/` (e.g. `/Volumes/Macintosh HD -> /`).
pub fn discover_mounted_volumes() -> Result<Vec<VolumeMetrics>> {
    let mut volumes = Vec::new();
    let mut seen_uuids = std::collections::HashSet::new();

    // Primary internal volume: query `/System/Volumes/Data` if present on APFS for accurate
    // data-volume UUID/snapshots while presenting mount_point as `/`.
    let primary_probe = if Path::new("/System/Volumes/Data").exists() {
        Path::new("/System/Volumes/Data")
    } else {
        Path::new("/")
    };

    if let Ok(mut root_vol) = inspect_volume(primary_probe) {
        root_vol.mount_point = PathBuf::from("/");
        root_vol.is_internal = true;
        // Prefer the localized name of `/` (e.g., "Macintosh HD" instead of "Data")
        #[cfg(target_os = "macos")]
        if let Some(root_cf) = macos_cf::query_cf_volume_props(Path::new("/"))
            && let Some(root_name) = root_cf.name
        {
            root_vol.name = root_name;
        }
        seen_uuids.insert(root_vol.volume_uuid.clone());
        volumes.push(root_vol);
    }

    // External volumes mounted under /Volumes
    if let Ok(entries) = std::fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let path = entry.path();
            // Skip symlinks pointing back to `/` (such as `/Volumes/Macintosh HD`)
            if let Ok(meta) = std::fs::symlink_metadata(&path)
                && meta.file_type().is_symlink()
                && let Ok(target) = std::fs::read_link(&path)
                && target == Path::new("/")
            {
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            if let Ok(vol) = inspect_volume(&path)
                && seen_uuids.insert(vol.volume_uuid.clone())
            {
                volumes.push(vol);
            }
        }
    }

    Ok(volumes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn capacity_breakdown_computes_purgeable_and_used() {
        // 500 GB total, 20 GB physical free, 65 GB available important => 45 GB purgeable, 435 GB active used
        let (used, purgeable) = compute_capacity_breakdown(500, 20, 65);
        assert_eq!(purgeable, 45);
        assert_eq!(used, 435);
    }

    #[test]
    fn capacity_breakdown_clamps_when_available_below_physical() {
        let (used, purgeable) = compute_capacity_breakdown(500, 80, 70);
        assert_eq!(purgeable, 0);
        assert_eq!(used, 420);
    }

    #[test]
    fn auto_ignores_time_machine_and_readonly_dmg() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Backups.backupdb")).unwrap();
        assert_eq!(
            detect_auto_ignore_reason(dir.path(), false, false, true, true).as_deref(),
            Some("Time Machine backup volume")
        );

        let dir2 = tempdir().unwrap();
        assert_eq!(
            detect_auto_ignore_reason(dir2.path(), false, true, true, true).as_deref(),
            Some("Read-only disk image (DMG)")
        );
        assert_eq!(
            detect_auto_ignore_reason(dir2.path(), false, false, true, false).as_deref(),
            Some("Network volume")
        );
        assert_eq!(
            detect_auto_ignore_reason(dir2.path(), true, true, false, true),
            None
        );
    }
}
