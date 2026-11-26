//! `silt-core` — macOS storage intelligence, time-series creep tracking,
//! unprivileged app attribution, and safe cleanup engine.

pub mod apfs;
pub mod attribution;
pub mod cleanup;
pub mod db;
pub mod diff;
pub mod format;
pub mod scanner;
pub mod volume;

use std::path::PathBuf;

/// Returns `~/Library/Application Support/Silt`.
pub fn default_data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Silt")
    } else {
        PathBuf::from("/tmp/silt")
    }
}

/// Default SQLite database path: `~/Library/Application Support/Silt/silt.db`.
pub fn default_db_path() -> PathBuf {
    default_data_dir().join("silt.db")
}

/// Default Unix domain socket path for `siltd` -> `silt-ui` notifications.
pub fn default_socket_path() -> PathBuf {
    default_data_dir().join("silt.sock")
}

/// Optional user-extensible attribution rulebook at `~/.config/silt/rules.toml`.
pub fn default_rules_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("silt")
            .join("rules.toml")
    } else {
        PathBuf::from("/tmp/silt-rules.toml")
    }
}

/// Non-intrusive check for macOS Full Disk Access (TCC).
/// Returns `true` if protected directories such as `~/Library/Safari` or
/// `~/Library/Mail` are readable.
pub fn has_full_disk_access() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let probes = [
        PathBuf::from(&home).join("Library/Mail"),
        PathBuf::from(&home).join("Library/Safari"),
        PathBuf::from(&home).join("Library/Messages"),
    ];
    for probe in probes {
        if probe.exists() {
            return std::fs::read_dir(&probe).is_ok();
        }
    }
    // If none of those folders exist on this Mac, fall back to TCC DB probe
    std::fs::File::open("/Library/Application Support/com.apple.TCC/TCC.db").is_ok()
}
