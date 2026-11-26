//! FSEvents dirty-directory tracker for low-overhead incremental scans.

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct DirtyWatcher {
    _watcher: RecommendedWatcher,
    dirty_dirs: Arc<Mutex<HashSet<PathBuf>>>,
    watched_roots: HashSet<PathBuf>,
}

impl DirtyWatcher {
    pub fn start(roots: &[PathBuf]) -> anyhow::Result<Self> {
        let dirty_dirs = Arc::new(Mutex::new(HashSet::new()));
        let dirty_ref = Arc::clone(&dirty_dirs);

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let mut guard = match dirty_ref.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for path in event.paths {
                    let target_dir = if path.is_dir() {
                        path
                    } else {
                        path.parent().map(Path::to_path_buf).unwrap_or(path)
                    };
                    // Coalesce deep paths to depth <= 5 to keep dirty set compact
                    let coalesced = coalesce_dir_depth(&target_dir, 5);
                    if guard.len() < 4_096 {
                        guard.insert(coalesced);
                    }
                }
            }
        })?;

        let mut watched_roots = HashSet::new();
        for root in roots {
            if root.exists() && watcher.watch(root, RecursiveMode::Recursive).is_ok() {
                watched_roots.insert(root.clone());
            }
        }

        Ok(Self {
            _watcher: watcher,
            dirty_dirs,
            watched_roots,
        })
    }

    pub fn watch_root(&mut self, root: &Path) {
        if root.exists()
            && !self.watched_roots.contains(root)
            && self._watcher.watch(root, RecursiveMode::Recursive).is_ok()
        {
            self.watched_roots.insert(root.to_path_buf());
        }
    }

    pub fn unwatch_root(&mut self, root: &Path) {
        if self.watched_roots.remove(root) {
            let _ = self._watcher.unwatch(root);
        }
    }

    /// Drain all accumulated dirty directories belonging under `root_prefix`.
    pub fn drain_dirty_under(&self, root_prefix: &Path) -> Vec<PathBuf> {
        let Ok(mut guard) = self.dirty_dirs.lock() else {
            return Vec::new();
        };
        let matching: Vec<PathBuf> = guard
            .iter()
            .filter(|p| p.starts_with(root_prefix))
            .cloned()
            .collect();
        for m in &matching {
            guard.remove(m);
        }
        matching
    }
}

/// Truncate a path to at most `max_components` components so noisy build directories
/// coalesce cleanly into a handful of subtree roots.
pub fn coalesce_dir_depth(path: &Path, max_components: usize) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components().take(max_components) {
        out.push(comp);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_deep_paths_cleanly() {
        let deep = Path::new("/Users/alice/dev/project/target/debug/deps/sub");
        let coalesced = coalesce_dir_depth(deep, 5);
        assert_eq!(coalesced, PathBuf::from("/Users/alice/dev/project"));
    }
}
