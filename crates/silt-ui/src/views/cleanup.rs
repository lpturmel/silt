//! Cleanup Workbench View: Curated macOS + Developer cleanup categories
//! (Safe App Caches, Rebuildable Dev Artifacts, Orphaned App Leftovers,
//! Stale Downloads Installers, APFS Local Snapshots) with Staging Cart integration.

use crate::state::{SiltState, reveal_in_finder};
use crate::theme::Theme;
use gpui::{
    Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use silt_core::attribution::{CleanupCandidate, SafetyTier};
use silt_core::cleanup::CleanupTarget;
use silt_core::format::format_bytes;

pub fn render_cleanup(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let total_reclaimable: u64 = state.cleanup_candidates.iter().map(|c| c.bytes).sum();

    let groups: Vec<(&'static str, &'static str, SafetyTier, Hsla)> = vec![
        (
            "Safe App Caches & Logs",
            "Rebuilt automatically by apps when needed — lowest risk",
            SafetyTier::SafeCache,
            theme.good_green,
        ),
        (
            "Rebuildable Developer Artifacts",
            "Cargo target/, Xcode DerivedData & Simulators, node_modules, HF caches",
            SafetyTier::RebuildableArtifact,
            theme.accent,
        ),
        (
            "Orphaned App Leftovers",
            "Containers & caches belonging to apps no longer installed",
            SafetyTier::OrphanedData,
            theme.warning_amber,
        ),
        (
            "Stale Downloads Installers",
            ".dmg, .pkg, .iso, and .xip installers older than 14 days",
            SafetyTier::StaleInstaller,
            theme.purgeable_color,
        ),
    ];

    let snapshots = state
        .selected_drive()
        .map(|d| d.snapshots.clone())
        .unwrap_or_default();

    div()
        .id("cleanup-scroll")
        .flex_1()
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .overflow_y_scroll()
        // Header Banner
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .p_4()
                .rounded(px(12.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child("Cleanup Workbench"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(format!(
                                    "{} safe cleanup target(s) found · {} potentially reclaimable · Defaults to macOS Trash (reversible)",
                                    state.cleanup_candidates.len(),
                                    format_bytes(total_reclaimable)
                                )),
                        ),
                )
                .child(
                    div()
                        .id("stage-all-safe-btn")
                        .px_3()
                        .py_1p5()
                        .rounded(px(8.0))
                        .bg(theme.good_green.opacity(0.18))
                        .border_1()
                        .border_color(theme.good_green)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.good_green)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stage_all_safe_caches();
                            cx.notify();
                        }))
                        .child("+ Stage All Safe Caches"),
                ),
        )
        // Safety Tier Cards
        .children(groups.into_iter().map(|(title, desc, tier, accent)| {
            let items: Vec<&CleanupCandidate> = state
                .cleanup_candidates
                .iter()
                .filter(|c| c.safety_tier == tier)
                .take(25)
                .collect();
            let group_bytes: u64 = items.iter().map(|c| c.bytes).sum();

            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .rounded(px(12.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(desc),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(accent)
                                .child(format_bytes(group_bytes)),
                        ),
                )
                .child(if items.is_empty() {
                    div()
                        .py_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("Nothing flagged in this category.")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .children(items.into_iter().enumerate().map(|(idx, cand)| {
                            let is_staged = state.cart.contains_path(&cand.path);
                            let reveal_p = cand.path.clone();
                            let target = CleanupTarget::Path {
                                path: cand.path.clone(),
                                label: format!("{} · {}", cand.display_name, cand.label),
                                bytes: cand.bytes,
                                safety_tier: cand.safety_tier,
                                app_id: cand.app_id.clone(),
                            };
                            let target_id = target.id();

                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .rounded(px(8.0))
                                .bg(theme.bg_elevated)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme.text_primary)
                                                .child(format!(
                                                    "{} — {}",
                                                    cand.display_name, cand.reason
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(cand.path.display().to_string()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.text_primary)
                                                .child(format_bytes(cand.bytes)),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "cln-rev-{}-{idx}",
                                                    tier.as_str()
                                                )))
                                                .px_2()
                                                .py_1()
                                                .rounded(px(5.0))
                                                .bg(theme.bg_surface)
                                                .border_1()
                                                .border_color(theme.border)
                                                .text_xs()
                                                .text_color(theme.text_secondary)
                                                .cursor_pointer()
                                                .on_click(move |_, _, _| reveal_in_finder(&reveal_p))
                                                .child("Reveal"),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "cln-stg-{}-{idx}",
                                                    tier.as_str()
                                                )))
                                                .px_2p5()
                                                .py_1()
                                                .rounded(px(5.0))
                                                .bg(if is_staged {
                                                    theme.good_green.opacity(0.2)
                                                } else {
                                                    theme.accent_soft
                                                })
                                                .text_xs()
                                                .text_color(if is_staged {
                                                    theme.good_green
                                                } else {
                                                    theme.accent
                                                })
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if this.cart.contains_id(&target_id) {
                                                        this.cart.unstage(&target_id);
                                                    } else {
                                                        this.stage_target(target.clone());
                                                    }
                                                    cx.notify();
                                                }))
                                                .child(if is_staged { "Staged ✓" } else { "+ Stage" }),
                                        ),
                                )
                        }))
                        .into_any_element()
                })
        }))
        // APFS Local Snapshots Card
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .rounded(px(12.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child("APFS Local Snapshots"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child("Time Machine & macOS local snapshots holding purgeable blocks (pruning is permanent)"),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.purgeable_color)
                                .child(format!("{} snapshot(s)", snapshots.len())),
                        ),
                )
                .child(if snapshots.is_empty() {
                    div()
                        .py_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("No local APFS snapshots detected on this drive.")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .children(snapshots.into_iter().enumerate().map(|(idx, snap)| {
                            let can_prune = snap.date_stamp.is_some();
                            let stage_btn = snap.date_stamp.clone().map(|ds| {
                                let target = CleanupTarget::ApfsSnapshot {
                                    mount_point: snap.mount_point.clone(),
                                    date_stamp: ds,
                                    name: snap.name.clone(),
                                    estimated_bytes: 0,
                                };
                                let tid = target.id();
                                let is_staged = state.cart.contains_id(&tid);
                                div()
                                    .id(SharedString::from(format!("snap-stg-{idx}")))
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(5.0))
                                    .bg(if is_staged {
                                        theme.warning_amber.opacity(0.22)
                                    } else {
                                        theme.bg_surface
                                    })
                                    .border_1()
                                    .border_color(theme.warning_amber.opacity(0.5))
                                    .text_xs()
                                    .text_color(theme.warning_amber)
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.cart.contains_id(&tid) {
                                            this.cart.unstage(&tid);
                                        } else {
                                            this.stage_target(target.clone());
                                        }
                                        cx.notify();
                                    }))
                                    .child(if is_staged { "Staged ✓" } else { "+ Stage Prune" })
                            });

                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .rounded(px(8.0))
                                .bg(theme.bg_elevated)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(theme.text_primary)
                                                .child(snap.name),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(if can_prune {
                                                    "Time Machine local snapshot (tmutil prunable)"
                                                } else if snap.purgeable {
                                                    "Purgeable macOS snapshot"
                                                } else {
                                                    "System snapshot (read-only)"
                                                }),
                                        ),
                                )
                                .children(stage_btn)
                        }))
                        .into_any_element()
                }),
        )
}
