# AGENTS.md — Silt Repository Instructions

## Mandatory Feature Review with Screenshots

When building or modifying any feature in **Silt** (especially UI, layout, charting, treemap, cleanup workflows, or user-visible behavior):

1. **Always start a dedicated reviewer agent/subagent** once the implementation builds and passes unit tests.
2. **Require screenshot validation**: The reviewer must launch the built app (`Silt.app` or `cargo run -p silt-ui`), capture real screenshots of every affected view/state (e.g., Overview & Creep Timeline, Apps & Attribution, Space Map / Creep Heatmap, Cleanup Workbench, Settings, Dark/Light themes, and modals/banners), and inspect those screenshots directly before approving the change.
3. **Verify both visual quality and functional accuracy**: Confirm there are no clipped labels, overlapping controls, unreadable contrast, broken layouts, or missing states—and verify numbers match `silt-cli` / macOS system values.
4. **Fix findings before merging**: Do not merge a feature PR until the reviewer's screenshot-backed review passes cleanly.

---

## Workspace & Build Quick Reference

- `crates/silt-core`: Core macOS volume/APFS metrics, parallel scanner, attribution engine, SQLite Creep Ledger, diff engine, safe cleanup engine.
- `crates/silt-ui`: Native Metal-accelerated GPUI desktop app (`silt`, built with `gpui` `runtime_shaders`).
- `crates/silt-daemon`: Opt-in background LaunchAgent collector (`siltd`) with `FSEvents` dirty-directory tracking and mount monitoring.
- `crates/silt-cli`: Terminal companion (`silt-cli`).
- `scripts/bundle-macos.sh`: Builds and ad-hoc codesigns `target/bundle/Silt.app`.

### Standard Checks

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
./scripts/bundle-macos.sh
```
