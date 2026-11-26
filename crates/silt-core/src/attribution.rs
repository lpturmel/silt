//! Unprivileged macOS app, container, download quarantine, and developer
//! artifact attribution engine.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SafetyTier {
    SafeCache,
    RebuildableArtifact,
    OrphanedData,
    StaleInstaller,
    ReviewRequired,
    SystemProtected,
}

impl SafetyTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SafeCache => "safe_cache",
            Self::RebuildableArtifact => "rebuildable_artifact",
            Self::OrphanedData => "orphaned_data",
            Self::StaleInstaller => "stale_installer",
            Self::ReviewRequired => "review_required",
            Self::SystemProtected => "system_protected",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "safe_cache" => Self::SafeCache,
            "rebuildable_artifact" => Self::RebuildableArtifact,
            "orphaned_data" => Self::OrphanedData,
            "stale_installer" => Self::StaleInstaller,
            "system_protected" => Self::SystemProtected,
            _ => Self::ReviewRequired,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SafeCache => "Safe Cache",
            Self::RebuildableArtifact => "Rebuildable Artifact",
            Self::OrphanedData => "Orphaned App Data",
            Self::StaleInstaller => "Stale Installer",
            Self::ReviewRequired => "Review Required",
            Self::SystemProtected => "System Protected",
        }
    }

    pub fn is_auto_reclaimable(self) -> bool {
        matches!(
            self,
            Self::SafeCache
                | Self::RebuildableArtifact
                | Self::OrphanedData
                | Self::StaleInstaller
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BucketKind {
    AppBundle,
    ApplicationData,
    Cache,
    Logs,
    Downloads,
    DevArtifact,
    SystemBackup,
    Other,
}

impl BucketKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppBundle => "app_bundle",
            Self::ApplicationData => "application_data",
            Self::Cache => "cache",
            Self::Logs => "logs",
            Self::Downloads => "downloads",
            Self::DevArtifact => "dev_artifact",
            Self::SystemBackup => "system_backup",
            Self::Other => "other",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "app_bundle" => Self::AppBundle,
            "application_data" => Self::ApplicationData,
            "cache" => Self::Cache,
            "logs" => Self::Logs,
            "downloads" => Self::Downloads,
            "dev_artifact" => Self::DevArtifact,
            "system_backup" => Self::SystemBackup,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AppBundle => "App Bundle",
            Self::ApplicationData => "App Data",
            Self::Cache => "Cache",
            Self::Logs => "Logs",
            Self::Downloads => "Downloads",
            Self::DevArtifact => "Dev Artifacts",
            Self::SystemBackup => "Device Backups",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppIdentity {
    pub app_id: String,
    pub display_name: String,
    pub bundle_id: Option<String>,
    pub app_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
    pub is_installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathAttribution {
    pub app_id: String,
    pub display_name: String,
    pub category: String,
    pub bucket: BucketKind,
    pub safety_tier: SafetyTier,
    pub is_orphaned: bool,
    pub cleanup_root: Option<PathBuf>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLocation {
    pub path: PathBuf,
    pub bucket: BucketKind,
    pub safety_tier: SafetyTier,
    pub bytes: u64,
    pub recent_7d_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRollup {
    pub app_id: String,
    pub display_name: String,
    pub category: String,
    pub app_bundle_bytes: u64,
    pub data_bytes: u64,
    pub cache_bytes: u64,
    pub logs_bytes: u64,
    pub downloads_bytes: u64,
    pub dev_artifacts_bytes: u64,
    pub recent_7d_bytes: u64,
    pub recent_30d_bytes: u64,
    pub total_bytes: u64,
    pub reclaimable_bytes: u64,
    pub is_orphaned: bool,
    pub locations: Vec<AppLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupCandidate {
    pub path: PathBuf,
    pub label: String,
    pub app_id: String,
    pub display_name: String,
    pub category: String,
    pub safety_tier: SafetyTier,
    pub bytes: u64,
    pub recent_7d_bytes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRule {
    pub path_contains: String,
    pub app_id: String,
    pub display_name: String,
    pub category: String,
    pub safety_tier: SafetyTier,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CustomRuleFile {
    #[serde(default)]
    rules: Vec<CustomRule>,
}

pub struct AttributionEngine {
    home_dir: PathBuf,
    by_bundle_id: HashMap<String, AppIdentity>,
    by_name_lower: HashMap<String, AppIdentity>,
    custom_rules: Vec<CustomRule>,
}

impl AttributionEngine {
    /// Discover installed macOS apps from `/Applications`, `~/Applications`, and
    /// `/System/Applications` plus optional user rules from `~/.config/silt/rules.toml`.
    pub fn discover() -> Self {
        let home_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        let mut apps = Vec::new();
        for dir in [
            PathBuf::from("/Applications"),
            home_dir.join("Applications"),
            PathBuf::from("/System/Applications"),
        ] {
            if dir.exists() {
                index_app_bundles(&dir, 0, &mut apps);
            }
        }
        let custom_rules = load_custom_rules(&crate::default_rules_path());
        Self::from_apps(home_dir, apps, custom_rules)
    }

    pub fn from_apps(
        home_dir: PathBuf,
        apps: Vec<AppIdentity>,
        custom_rules: Vec<CustomRule>,
    ) -> Self {
        let mut by_bundle_id = HashMap::new();
        let mut by_name_lower = HashMap::new();
        for app in apps {
            if let Some(bid) = &app.bundle_id {
                by_bundle_id.insert(bid.to_lowercase(), app.clone());
            }
            by_name_lower.insert(app.display_name.to_lowercase(), app);
        }
        Self {
            home_dir,
            by_bundle_id,
            by_name_lower,
            custom_rules,
        }
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    /// Classify a file or directory path into an attributed App / Category / SafetyTier.
    pub fn classify_path(&self, path: &Path, is_dir: bool) -> PathAttribution {
        let path_str = path.to_string_lossy();

        // 0. User custom rules first
        for rule in &self.custom_rules {
            if path_str.contains(&rule.path_contains) {
                return PathAttribution {
                    app_id: rule.app_id.clone(),
                    display_name: rule.display_name.clone(),
                    category: rule.category.clone(),
                    bucket: BucketKind::DevArtifact,
                    safety_tier: rule.safety_tier,
                    is_orphaned: false,
                    cleanup_root: Some(path.to_path_buf()),
                    reason: Some(format!("Matched custom rule ({})", rule.path_contains)),
                };
            }
        }

        // 1. Installed `.app` bundle
        if let Some(app_bundle_root) = find_enclosing_app_bundle(path) {
            let bundle_name = app_bundle_root
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Application".to_string());
            if let Some(app) = self.by_name_lower.get(&bundle_name.to_lowercase()) {
                return PathAttribution {
                    app_id: app.app_id.clone(),
                    display_name: app.display_name.clone(),
                    category: "Applications".to_string(),
                    bucket: BucketKind::AppBundle,
                    safety_tier: SafetyTier::ReviewRequired,
                    is_orphaned: false,
                    cleanup_root: None,
                    reason: None,
                };
            }
            return PathAttribution {
                app_id: format!("app:{}", slug(&bundle_name)),
                display_name: bundle_name,
                category: "Applications".to_string(),
                bucket: BucketKind::AppBundle,
                safety_tier: SafetyTier::ReviewRequired,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }

        // 2. Developer & Power-User Rules (high precision before generic ~/Library matching)
        if let Some(dev_attr) = self.classify_developer_path(path, is_dir) {
            return dev_attr;
        }

        // 3. macOS ~/Library & /Library structured app containers
        if let Some(lib_attr) = self.classify_library_path(path) {
            return lib_attr;
        }

        // 4. Downloads directory & files with `com.apple.quarantine` xattr + stale installer detection
        let downloads_dir = self.home_dir.join("Downloads");
        if path == downloads_dir {
            return PathAttribution {
                app_id: "folder:downloads".to_string(),
                display_name: "Downloads".to_string(),
                category: "Downloads".to_string(),
                bucket: BucketKind::Downloads,
                safety_tier: SafetyTier::ReviewRequired,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }
        if path.starts_with(&downloads_dir) {
            return self.classify_download_path(path, is_dir);
        }

        // Exact top-level macOS roots so root treemap tiles get accurate categories
        if path == Path::new("/Applications") || path == self.home_dir.join("Applications") {
            return PathAttribution {
                app_id: "root:applications".to_string(),
                display_name: "Applications".to_string(),
                category: "Applications".to_string(),
                bucket: BucketKind::AppBundle,
                safety_tier: SafetyTier::SystemProtected,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }
        if path == Path::new("/Library") || path == self.home_dir.join("Library") {
            return PathAttribution {
                app_id: "root:library".to_string(),
                display_name: "Library".to_string(),
                category: "macOS System".to_string(),
                bucket: BucketKind::ApplicationData,
                safety_tier: SafetyTier::SystemProtected,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }
        if path == self.home_dir {
            let user_label = self
                .home_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Home".to_string());
            return PathAttribution {
                app_id: format!("home:{}", slug(&user_label)),
                display_name: user_label,
                category: "User Files".to_string(),
                bucket: BucketKind::Other,
                safety_tier: SafetyTier::SystemProtected,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }

        // 5. Fallback coarse user folders
        if let Ok(rel) = path.strip_prefix(&self.home_dir)
            && let Some(first) = rel.components().next()
        {
            let top = first.as_os_str().to_string_lossy().to_string();
            let category = match top.as_str() {
                "Movies" | "Pictures" | "Music" => "Media".to_string(),
                "Documents" | "Desktop" => "Documents".to_string(),
                _ => "User Files".to_string(),
            };
            return PathAttribution {
                app_id: format!("folder:{}", slug(&top)),
                display_name: format!("~/{top}"),
                category,
                bucket: BucketKind::Other,
                safety_tier: SafetyTier::ReviewRequired,
                is_orphaned: false,
                cleanup_root: None,
                reason: None,
            };
        }

        // External volume or other root
        let label = path
            .components()
            .nth(1)
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| "System".to_string());
        PathAttribution {
            app_id: format!("root:{}", slug(&label)),
            display_name: label,
            category: "Other".to_string(),
            bucket: BucketKind::Other,
            safety_tier: SafetyTier::ReviewRequired,
            is_orphaned: false,
            cleanup_root: None,
            reason: None,
        }
    }

    fn classify_developer_path(&self, path: &Path, _is_dir: bool) -> Option<PathAttribution> {
        let home = &self.home_dir;

        // Xcode DerivedData, Simulators, DeviceSupport, Archives
        let xcode_rules: &[(&str, &str, SafetyTier, &str)] = &[
            (
                "Library/Developer/Xcode/DerivedData",
                "Xcode DerivedData",
                SafetyTier::RebuildableArtifact,
                "Xcode build intermediates — safely rebuilt on next build",
            ),
            (
                "Library/Developer/Xcode/iOS DeviceSupport",
                "Xcode DeviceSupport",
                SafetyTier::RebuildableArtifact,
                "Old iOS debugging symbols",
            ),
            (
                "Library/Developer/Xcode/Archives",
                "Xcode Archives",
                SafetyTier::ReviewRequired,
                "Archived Xcode app builds",
            ),
            (
                "Library/Developer/CoreSimulator",
                "Xcode Simulators",
                SafetyTier::RebuildableArtifact,
                "iOS/macOS Simulator runtimes & device images",
            ),
        ];
        for (rel, label, tier, reason) in xcode_rules {
            let root = home.join(rel);
            if path.starts_with(&root) {
                return Some(PathAttribution {
                    app_id: "com.apple.dt.Xcode".to_string(),
                    display_name: "Xcode".to_string(),
                    category: "Developer".to_string(),
                    bucket: BucketKind::DevArtifact,
                    safety_tier: *tier,
                    is_orphaned: false,
                    cleanup_root: Some(root),
                    reason: Some(format!("{label}: {reason}")),
                });
            }
        }

        // Cargo registry & git cache
        for (rel, reason) in [
            (".cargo/registry", "Cargo crate source & cache — re-downloaded on demand"),
            (".cargo/git", "Cargo git dependency checkouts"),
            (".rustup/toolchains", "Installed Rust toolchains"),
        ] {
            let root = home.join(rel);
            if path.starts_with(&root) {
                let tier = if rel.starts_with(".rustup") {
                    SafetyTier::ReviewRequired
                } else {
                    SafetyTier::SafeCache
                };
                return Some(PathAttribution {
                    app_id: "dev:rust".to_string(),
                    display_name: "Rust / Cargo".to_string(),
                    category: "Developer".to_string(),
                    bucket: BucketKind::DevArtifact,
                    safety_tier: tier,
                    is_orphaned: false,
                    cleanup_root: (tier != SafetyTier::ReviewRequired).then_some(root),
                    reason: Some(reason.to_string()),
                });
            }
        }

        // Any Rust project `target/` directory validated by sibling `Cargo.toml`
        if let Some(target_root) = find_rust_target_root(path) {
            return Some(PathAttribution {
                app_id: "dev:rust".to_string(),
                display_name: "Rust / Cargo".to_string(),
                category: "Developer".to_string(),
                bucket: BucketKind::DevArtifact,
                safety_tier: SafetyTier::RebuildableArtifact,
                is_orphaned: false,
                cleanup_root: Some(target_root),
                reason: Some("Cargo build target directory — safely rebuilt by cargo".to_string()),
            });
        }

        // Node / JS caches & node_modules
        let simple_dev_rules: &[(&str, &str, &str, &str, SafetyTier, &str)] = &[
            (
                ".npm",
                "dev:npm",
                "npm",
                "Developer",
                SafetyTier::SafeCache,
                "npm download cache",
            ),
            (
                "Library/Caches/pnpm",
                "dev:pnpm",
                "pnpm",
                "Developer",
                SafetyTier::SafeCache,
                "pnpm package cache",
            ),
            (
                "Library/Caches/Yarn",
                "dev:yarn",
                "Yarn",
                "Developer",
                SafetyTier::SafeCache,
                "Yarn package cache",
            ),
            (
                "Library/Caches/Homebrew",
                "dev:homebrew",
                "Homebrew",
                "Developer",
                SafetyTier::SafeCache,
                "Homebrew downloaded bottle archives",
            ),
            (
                ".cache/pip",
                "dev:python",
                "Python (pip)",
                "Developer",
                SafetyTier::SafeCache,
                "pip wheel cache",
            ),
            (
                ".cache/uv",
                "dev:uv",
                "Python (uv)",
                "Developer",
                SafetyTier::SafeCache,
                "uv package cache",
            ),
            (
                ".gradle/caches",
                "dev:gradle",
                "Gradle",
                "Developer",
                SafetyTier::SafeCache,
                "Gradle build & dependency cache",
            ),
            (
                "Library/Containers/com.docker.docker",
                "com.docker.docker",
                "Docker Desktop",
                "Containers & VMs",
                SafetyTier::ReviewRequired,
                "Docker Desktop VM disk & images",
            ),
            (
                "Library/Group Containers/group.com.docker",
                "com.docker.docker",
                "Docker Desktop",
                "Containers & VMs",
                SafetyTier::ReviewRequired,
                "Docker Desktop shared group data",
            ),
            (
                ".orbstack",
                "dev.kdrag0n.MacVirt",
                "OrbStack",
                "Containers & VMs",
                SafetyTier::ReviewRequired,
                "OrbStack containers & Linux VM data",
            ),
            (
                ".colima",
                "dev:colima",
                "Colima",
                "Containers & VMs",
                SafetyTier::ReviewRequired,
                "Colima VM disk",
            ),
            (
                ".ollama/models",
                "com.electron.ollama",
                "Ollama",
                "AI Models",
                SafetyTier::ReviewRequired,
                "Local LLM model weights",
            ),
            (
                ".cache/huggingface",
                "dev:huggingface",
                "Hugging Face",
                "AI Models",
                SafetyTier::RebuildableArtifact,
                "Cached Hugging Face model weights",
            ),
            (
                ".lmstudio",
                "dev:lmstudio",
                "LM Studio",
                "AI Models",
                SafetyTier::ReviewRequired,
                "LM Studio downloaded models",
            ),
            (
                "Library/Application Support/MobileSync/Backup",
                "com.apple.mobilesync",
                "iOS Device Backups",
                "Backups",
                SafetyTier::ReviewRequired,
                "Local iPhone/iPad backups",
            ),
        ];

        for (rel, app_id, display_name, category, tier, reason) in simple_dev_rules {
            let root = home.join(rel);
            if path.starts_with(&root) {
                return Some(PathAttribution {
                    app_id: (*app_id).to_string(),
                    display_name: (*display_name).to_string(),
                    category: (*category).to_string(),
                    bucket: if *category == "Backups" {
                        BucketKind::SystemBackup
                    } else {
                        BucketKind::DevArtifact
                    },
                    safety_tier: *tier,
                    is_orphaned: false,
                    cleanup_root: tier.is_auto_reclaimable().then_some(root),
                    reason: Some((*reason).to_string()),
                });
            }
        }

        // Node project `node_modules`
        if let Some(nm_root) = find_named_ancestor_with_sibling(path, "node_modules", "package.json")
        {
            return Some(PathAttribution {
                app_id: "dev:node_modules".to_string(),
                display_name: "Node.js (node_modules)".to_string(),
                category: "Developer".to_string(),
                bucket: BucketKind::DevArtifact,
                safety_tier: SafetyTier::RebuildableArtifact,
                is_orphaned: false,
                cleanup_root: Some(nm_root),
                reason: Some("Project node_modules — reinstallable via package manager".to_string()),
            });
        }

        None
    }

    fn classify_library_path(&self, path: &Path) -> Option<PathAttribution> {
        let user_lib = self.home_dir.join("Library");
        let sys_lib = Path::new("/Library");

        let (lib_root, rel) = if let Ok(r) = path.strip_prefix(&user_lib) {
            (user_lib.as_path(), r)
        } else if let Ok(r) = path.strip_prefix(sys_lib) {
            (sys_lib, r)
        } else {
            return None;
        };

        let mut comps = rel.components();
        let section = comps.next()?.as_os_str().to_string_lossy();
        let key_comp = comps.next()?.as_os_str().to_string_lossy();
        let section_root = lib_root.join(section.as_ref()).join(key_comp.as_ref());

        let (bucket, default_tier) = match section.as_ref() {
            "Caches" | "HTTPStorages" | "WebKit" | "Saved Application State" => {
                (BucketKind::Cache, SafetyTier::SafeCache)
            }
            "Logs" => (BucketKind::Logs, SafetyTier::SafeCache),
            "Application Support" | "Containers" | "Group Containers" | "Preferences" => {
                (BucketKind::ApplicationData, SafetyTier::ReviewRequired)
            }
            _ => return None,
        };

        // Match against installed apps (strip TeamID prefix for Group Containers like `UBF8T346G9.com.microsoft.teams`)
        let clean_key = strip_group_prefix(&key_comp);
        if let Some(app) = self.match_installed_app(clean_key) {
            return Some(PathAttribution {
                app_id: app.app_id.clone(),
                display_name: app.display_name.clone(),
                category: "Applications".to_string(),
                bucket,
                safety_tier: default_tier,
                is_orphaned: false,
                cleanup_root: default_tier.is_auto_reclaimable().then_some(section_root),
                reason: default_tier
                    .is_auto_reclaimable()
                    .then(|| format!("{} {}", app.display_name, bucket.label())),
            });
        }

        // Apple system components are never marked orphaned
        if clean_key.starts_with("com.apple.") || clean_key.starts_with("Apple") {
            return Some(PathAttribution {
                app_id: format!("apple:{}", slug(clean_key)),
                display_name: prettify_bundle_id(clean_key),
                category: "macOS System".to_string(),
                bucket,
                safety_tier: if bucket == BucketKind::Cache || bucket == BucketKind::Logs {
                    SafetyTier::SafeCache
                } else {
                    SafetyTier::SystemProtected
                },
                is_orphaned: false,
                cleanup_root: (bucket == BucketKind::Cache || bucket == BucketKind::Logs)
                    .then_some(section_root),
                reason: Some(format!("macOS {}", bucket.label())),
            });
        }

        // Orphaned app detection: reverse-DNS bundle ID in Containers/Caches/Application Support
        // that doesn't match any installed app.
        let looks_like_bundle = looks_like_bundle_id(clean_key);
        let is_orphaned = looks_like_bundle
            && matches!(
                section.as_ref(),
                "Containers" | "Caches" | "Saved Application State" | "HTTPStorages"
            );
        let safety_tier = if is_orphaned {
            SafetyTier::OrphanedData
        } else {
            default_tier
        };

        let display_name = prettify_bundle_id(clean_key);
        let cleanup_root = if safety_tier.is_auto_reclaimable() {
            Some(section_root)
        } else {
            None
        };
        let reason = if is_orphaned {
            Some(format!("Leftover data for uninstalled app ({clean_key})"))
        } else if safety_tier == SafetyTier::SafeCache {
            Some(format!("{display_name} {}", bucket.label()))
        } else {
            None
        };

        Some(PathAttribution {
            app_id: clean_key.to_lowercase(),
            display_name,
            category: if is_orphaned {
                "Orphaned Apps".to_string()
            } else {
                "Applications".to_string()
            },
            bucket,
            safety_tier,
            is_orphaned,
            cleanup_root,
            reason,
        })
    }

    fn classify_download_path(&self, path: &Path, is_dir: bool) -> PathAttribution {
        // Attempt unprivileged `com.apple.quarantine` xattr attribution
        let agent = read_quarantine_agent(path);
        let (app_id, display_name) = if let Some(agent_name) = agent {
            if let Some(app) = self.match_installed_app(&agent_name) {
                (app.app_id.clone(), app.display_name.clone())
            } else {
                (format!("download-agent:{}", slug(&agent_name)), agent_name)
            }
        } else {
            ("folder:downloads".to_string(), "Downloads".to_string())
        };

        // Detect stale installer packages (.dmg, .pkg, .iso, .xip) older than 14 days
        let is_installer = !is_dir
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "dmg" | "pkg" | "iso" | "xip"))
                .unwrap_or(false);

        let is_stale_installer = is_installer
            && std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
                .map(|age| age > Duration::from_secs(14 * 86_400))
                .unwrap_or(false);

        let safety_tier = if is_stale_installer {
            SafetyTier::StaleInstaller
        } else {
            SafetyTier::ReviewRequired
        };

        PathAttribution {
            app_id,
            display_name,
            category: "Downloads".to_string(),
            bucket: BucketKind::Downloads,
            safety_tier,
            is_orphaned: false,
            cleanup_root: is_stale_installer.then(|| path.to_path_buf()),
            reason: is_stale_installer
                .then(|| "Installer archive older than 14 days in Downloads".to_string()),
        }
    }

    fn match_installed_app(&self, key: &str) -> Option<&AppIdentity> {
        let lower = key.to_lowercase();
        if let Some(app) = self.by_bundle_id.get(&lower) {
            return Some(app);
        }
        if let Some(app) = self.by_name_lower.get(&lower) {
            return Some(app);
        }
        // Prefix match for helper/renderer bundle IDs (e.g., `com.google.Chrome.helper`)
        for (bid, app) in &self.by_bundle_id {
            if lower.starts_with(bid)
                && lower
                    .as_bytes()
                    .get(bid.len())
                    .is_some_and(|&b| b == b'.' || b == b'-' || b == b' ')
            {
                return Some(app);
            }
        }
        None
    }
}

/// Read macOS `com.apple.quarantine` extended attribute (`flags;timestamp;AgentName;UUID`)
/// and return the downloading application name (e.g. `"Google Chrome"`, `"Safari"`, `"Slack"`).
pub fn read_quarantine_agent(path: &Path) -> Option<String> {
    let raw = xattr::get(path, "com.apple.quarantine").ok()??;
    let text = String::from_utf8_lossy(&raw);
    parse_quarantine_agent(&text)
}

pub fn parse_quarantine_agent(raw: &str) -> Option<String> {
    let mut parts = raw.trim().split(';');
    let _flags = parts.next()?;
    let _timestamp = parts.next()?;
    let agent = parts.next()?.trim();
    if agent.is_empty() {
        None
    } else {
        Some(agent.to_string())
    }
}

fn load_custom_rules(path: &Path) -> Vec<CustomRule> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<CustomRuleFile>(&s).ok())
        .map(|f| f.rules)
        .unwrap_or_default()
}

fn index_app_bundles(dir: &Path, depth: u8, out: &mut Vec<AppIdentity>) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            if let Some(app) = read_app_bundle_identity(&path) {
                out.push(app);
            }
        } else {
            index_app_bundles(&path, depth + 1, out);
        }
    }
}

pub fn read_app_bundle_identity(app_path: &Path) -> Option<AppIdentity> {
    let info_plist = app_path.join("Contents/Info.plist");
    let fallback_name = app_path.file_stem()?.to_string_lossy().into_owned();

    let Ok(val) = plist::Value::from_file(&info_plist) else {
        return Some(AppIdentity {
            app_id: format!("app:{}", slug(&fallback_name)),
            display_name: fallback_name,
            bundle_id: None,
            app_path: Some(app_path.to_path_buf()),
            icon_path: None,
            is_installed: true,
        });
    };
    let dict = val.as_dictionary()?;
    let bundle_id = dict
        .get("CFBundleIdentifier")
        .and_then(|v| v.as_string())
        .map(ToOwned::to_owned);
    let display_name = dict
        .get("CFBundleDisplayName")
        .or_else(|| dict.get("CFBundleName"))
        .and_then(|v| v.as_string())
        .map(ToOwned::to_owned)
        .unwrap_or(fallback_name);
    let icon_path = dict
        .get("CFBundleIconFile")
        .and_then(|v| v.as_string())
        .map(|icon| {
            let mut p = app_path.join("Contents/Resources").join(icon);
            if p.extension().is_none() {
                p.set_extension("icns");
            }
            p
        });
    let app_id = bundle_id
        .clone()
        .unwrap_or_else(|| format!("app:{}", slug(&display_name)));

    Some(AppIdentity {
        app_id,
        display_name,
        bundle_id,
        app_path: Some(app_path.to_path_buf()),
        icon_path,
        is_installed: true,
    })
}

fn find_enclosing_app_bundle(path: &Path) -> Option<PathBuf> {
    for anc in path.ancestors() {
        if anc.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(anc.to_path_buf());
        }
    }
    None
}

fn find_rust_target_root(path: &Path) -> Option<PathBuf> {
    find_named_ancestor_with_sibling(path, "target", "Cargo.toml")
}

fn find_named_ancestor_with_sibling(
    path: &Path,
    dir_name: &str,
    sibling_marker: &str,
) -> Option<PathBuf> {
    for anc in path.ancestors() {
        if anc.file_name().and_then(|n| n.to_str()) == Some(dir_name)
            && let Some(parent) = anc.parent()
            && parent.join(sibling_marker).exists()
        {
            return Some(anc.to_path_buf());
        }
    }
    None
}

fn strip_group_prefix(key: &str) -> &str {
    // Group Containers look like `UBF8T346G9.com.microsoft.teams` or `group.com.foo.bar`
    if let Some(rest) = key.strip_prefix("group.") {
        return rest;
    }
    if let Some((prefix, rest)) = key.split_once('.')
        && prefix.len() == 10
        && prefix.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return rest;
    }
    key
}

fn looks_like_bundle_id(s: &str) -> bool {
    let lower = s.to_lowercase();
    let prefixes = ["com.", "org.", "io.", "net.", "app.", "dev.", "co.", "ai.", "me."];
    prefixes.iter().any(|p| lower.starts_with(p)) && s.matches('.').count() >= 2
}

fn prettify_bundle_id(s: &str) -> String {
    let stripped = s
        .strip_suffix(".savedState")
        .unwrap_or(s);
    if looks_like_bundle_id(stripped)
        && let Some(last) = stripped.rsplit('.').next()
        && !last.is_empty()
    {
        let mut chars = last.chars();
        if let Some(first) = chars.next() {
            return format!("{}{}", first.to_uppercase(), chars.as_str());
        }
    }
    stripped.to_string()
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_quarantine_xattr_agent() {
        assert_eq!(
            parse_quarantine_agent("0081;65a1b2c3;Google Chrome;1234-5678"),
            Some("Google Chrome".to_string())
        );
        assert_eq!(
            parse_quarantine_agent("0083;65a1b2c3;Safari;"),
            Some("Safari".to_string())
        );
        assert_eq!(parse_quarantine_agent("invalid"), None);
    }

    #[test]
    fn attributes_installed_and_orphaned_containers_and_dev_targets() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_path_buf();

        let installed = vec![AppIdentity {
            app_id: "com.google.Chrome".to_string(),
            display_name: "Google Chrome".to_string(),
            bundle_id: Some("com.google.Chrome".to_string()),
            app_path: Some(PathBuf::from("/Applications/Google Chrome.app")),
            icon_path: None,
            is_installed: true,
        }];
        let engine = AttributionEngine::from_apps(home.clone(), installed, Vec::new());

        // Installed app cache
        let chrome_cache = home.join("Library/Caches/com.google.Chrome/Default/Cache");
        let a = engine.classify_path(&chrome_cache, false);
        assert_eq!(a.display_name, "Google Chrome");
        assert_eq!(a.bucket, BucketKind::Cache);
        assert_eq!(a.safety_tier, SafetyTier::SafeCache);
        assert!(!a.is_orphaned);

        // Orphaned uninstalled app container
        let orphan = home.join("Library/Containers/com.oldvendor.OldApp/Data");
        let b = engine.classify_path(&orphan, true);
        assert!(b.is_orphaned);
        assert_eq!(b.safety_tier, SafetyTier::OrphanedData);

        // Rust Cargo target directory
        let proj = home.join("code/myproj");
        std::fs::create_dir_all(proj.join("target/debug")).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let c = engine.classify_path(&proj.join("target/debug/myproj"), false);
        assert_eq!(c.display_name, "Rust / Cargo");
        assert_eq!(c.safety_tier, SafetyTier::RebuildableArtifact);
        assert_eq!(c.cleanup_root, Some(proj.join("target")));
    }
}
