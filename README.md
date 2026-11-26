# Silt

**Silt** is a native macOS storage intelligence and cleanup application built in **Rust + GPUI**.

Instead of only showing a static snapshot of what is big today, Silt tracks how storage silently settles across your internal APFS drive and external drives over time:

- **Remaining vs. Available vs. Purgeable**: Resolves the macOS discrepancy between physical free space (`statfs`), Finder's Available space (`NSURLVolumeAvailableCapacityForImportantUsageKey`), and APFS purgeable snapshots.
- **Creep Timeline & Velocity**: Records volume telemetry and directory snapshots in an embedded SQLite database (`~/Library/Application Support/Silt/silt.db`) with Day-1 `mtime`/`birthtime` recent-activity seeding and exact snapshot-to-snapshot diffs (`+Δ`, growth rate, runway forecast).
- **Unprivileged macOS App Attribution**: Unifies `.app` bundles, `~/Library/{Application Support, Caches, Containers, Group Containers, Logs}`, `com.apple.quarantine` download attribution, developer tool ecosystems (Cargo, Xcode, Docker/OrbStack, Homebrew, npm/pnpm, AI models), and orphaned leftovers from uninstalled apps.
- **External Drives (Online & Offline)**: Tracks drives by Volume UUID, remembers unplugged drives, auto-ignores transient `.dmg` mounts and Time Machine volumes, and computes *"Since Last Connected"* diffs on plug-in.
- **Interactive Visualizations**: Native Metal-accelerated GPUI dashboard with time-series charts, app breakdowns, and a zoomable Squarified Treemap with a **Creep Heatmap** mode.
- **Safe Cleanup Workbench**: Curated macOS + developer cleanup cards with a deduplicated Staging Cart defaulting to **macOS Trash** (reversible via Put Back) and hard-protected system paths.

## Workspace Crates

- `crates/silt-core`: Volume & APFS inspector, parallel physical-block scanner, attribution engine, SQLite Creep Ledger & diff engine, safe cleanup engine.
- `crates/silt-cli`: Fast terminal companion (`silt-cli status | scan | creep | apps | daemon`).
- `crates/silt-ui`: Native Metal-accelerated GPUI desktop app (`silt`).
- `crates/silt-daemon`: Opt-in background `LaunchAgent` collector (`siltd`) with `FSEvents` dirty-directory tracking and external mount detection.

## Quick Start

```bash
# Run CLI checks
cargo run -p silt-cli -- status
cargo run -p silt-cli -- scan
cargo run -p silt-cli -- creep --days 7

# Launch native GPUI app
cargo run -p silt-ui

# Build macOS Silt.app bundle
./scripts/bundle-macos.sh
```
