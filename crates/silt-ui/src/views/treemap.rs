//! Space Map View: Interactive Zoomable Squarified Treemap with
//! Category Color Mode and Novel Creep Heatmap Mode.

use crate::state::{SiltState, TreemapColorMode, reveal_in_finder};
use crate::theme::Theme;
use crate::widgets::squarify::{TreemapInput, squarify};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, relative,
};
use silt_core::attribution::SafetyTier;
use silt_core::cleanup::{CleanupTarget, is_protected_path};
use silt_core::format::{compact_display_path, format_bytes, format_signed_bytes};
use silt_core::scanner::DirAggregate;
use std::path::PathBuf;

pub fn render_treemap(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let Some(drive) = state.selected_drive() else {
        return div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(theme.text_muted)
            .child("Select a drive to view its Space Map.");
    };

    let current_root = state
        .treemap_root
        .clone()
        .unwrap_or_else(|| drive.mount_point.clone());

    // Direct children of `current_root` from our latest snapshot
    let mut children: Vec<&DirAggregate> = state
        .dir_entries
        .iter()
        .filter(|d| d.parent_path.as_deref() == Some(current_root.as_path()))
        .collect();
    children.sort_by_key(|d| std::cmp::Reverse(d.physical_bytes));
    children.truncate(48);

    let dir_deltas = state
        .creep_report
        .as_ref()
        .map(|r| &r.dir_deltas);

    let max_pos_delta: i64 = children
        .iter()
        .map(|d| {
            dir_deltas
                .and_then(|m| m.get(&d.path).copied())
                .unwrap_or(d.recent_7d_bytes as i64)
        })
        .filter(|&v| v > 0)
        .max()
        .unwrap_or(1);

    // Lay out squarified rectangles in normalized [0..1000, 0..1000] space so we can
    // position GPUI tiles using `relative(x / 1000.0)` and adapt smoothly to any resize!
    let inputs: Vec<TreemapInput<DirAggregate>> = children
        .iter()
        .map(|d| TreemapInput {
            value: d.physical_bytes,
            payload: (*d).clone(),
        })
        .collect();
    let rects = squarify(&inputs, 1000.0, 1000.0);

    // Breadcrumb trail from drive mount_point to current_root
    let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
    let mut walk = current_root.clone();
    loop {
        let label = if walk == drive.mount_point {
            drive.name.clone()
        } else {
            walk.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| walk.display().to_string())
        };
        crumbs.push((label, walk.clone()));
        if walk == drive.mount_point {
            break;
        }
        match walk.parent() {
            Some(p) if p.starts_with(&drive.mount_point) => walk = p.to_path_buf(),
            _ => break,
        }
    }
    crumbs.reverse();

    let selected_info: Option<DirAggregate> = state
        .treemap_selected_path
        .as_ref()
        .and_then(|p| state.dir_entries.iter().find(|d| &d.path == p).cloned());

    div()
        .flex_1()
        .flex()
        .flex_col()
        .h_full()
        .p_5()
        .gap_3()
        // Header & Color Mode Switcher
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
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child("Visual Space Map"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("Click any rectangle to zoom in · Toggle Creep Heatmap to spot growing folders immediately"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(mode_pill(
                            "Creep Heatmap",
                            TreemapColorMode::CreepHeatmap,
                            state.treemap_color_mode,
                            theme,
                            cx,
                        ))
                        .child(mode_pill(
                            "By Category",
                            TreemapColorMode::Category,
                            state.treemap_color_mode,
                            theme,
                            cx,
                        )),
                ),
        )
        // Breadcrumbs + Heatmap Legend
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .px_3()
                .py_2()
                .rounded(px(8.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .children(crumbs.into_iter().enumerate().map(|(i, (label, path))| {
                            let id = SharedString::from(format!("crumb-{i}"));
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1p5()
                                .children(if i > 0 {
                                    Some(div().text_xs().text_color(theme.text_muted).child("/"))
                                } else {
                                    None
                                })
                                .child(
                                    div()
                                        .id(id)
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(4.0))
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_primary)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.bg_surface_hover))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.treemap_root = Some(path.clone());
                                            this.treemap_selected_path = Some(path.clone());
                                            cx.notify();
                                        }))
                                        .child(label),
                                )
                        })),
                )
                .child(if state.treemap_color_mode == TreemapColorMode::CreepHeatmap {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("Static")
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(10.0))
                                .rounded(px(2.0))
                                .bg(theme.creep_heatmap_color(0, 100)),
                        )
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(10.0))
                                .rounded(px(2.0))
                                .bg(theme.creep_heatmap_color(40, 100)),
                        )
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(10.0))
                                .rounded(px(2.0))
                                .bg(theme.creep_heatmap_color(100, 100)),
                        )
                        .child("Rapidly Creeping")
                        .into_any_element()
                } else {
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(format!("{} folders shown", rects.len()))
                        .into_any_element()
                }),
        )
        // Squarified Treemap Canvas Area
        .child(
            div()
                .relative()
                .flex_1()
                .w_full()
                .min_h(px(320.0))
                .rounded(px(12.0))
                .overflow_hidden()
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .children(if rects.is_empty() {
                    vec![
                        div()
                            .flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("No child subdirectories recorded at this level (or run a scan first).")
                            .into_any_element(),
                    ]
                } else {
                    rects
                        .into_iter()
                        .enumerate()
                        .map(|(idx, rect)| {
                            let d = rect.payload;
                            let delta = dir_deltas
                                .and_then(|m| m.get(&d.path).copied())
                                .unwrap_or(d.recent_7d_bytes as i64);
                            let palette = match state.treemap_color_mode {
                                TreemapColorMode::Category => theme.category_tile_palette(&d.category),
                                TreemapColorMode::CreepHeatmap => {
                                    theme.creep_heatmap_tile_palette(delta, max_pos_delta)
                                }
                            };
                            let has_children = state
                                .dir_entries
                                .iter()
                                .any(|c| c.parent_path.as_deref() == Some(d.path.as_path()));
                            let name = d
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| d.path.display().to_string());
                            let show_label = rect.w >= 75.0 && rect.h >= 55.0;
                            let show_sub = rect.w >= 105.0 && rect.h >= 95.0;
                            let click_path = d.path.clone();

                            div()
                                .id(SharedString::from(format!("tm-tile-{idx}")))
                                .absolute()
                                .left(relative(rect.x / 1000.0))
                                .top(relative(rect.y / 1000.0))
                                .w(relative(rect.w / 1000.0))
                                .h(relative(rect.h / 1000.0))
                                .p(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .justify_between()
                                        .size_full()
                                        .p_2()
                                        .rounded(px(6.0))
                                        .bg(palette.bg)
                                        .border_1()
                                        .border_color(theme.bg_window.opacity(0.45))
                                        .overflow_hidden()
                                        .cursor_pointer()
                                        .hover(|s| s.border_color(theme.text_primary))
                                        .children(if show_label {
                                            Some(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_0p5()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(palette.title)
                                                            .child(name),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(palette.subtitle)
                                                            .child(format_bytes(d.physical_bytes)),
                                                    ),
                                            )
                                        } else {
                                            None
                                        })
                                        .children(if show_sub && delta != 0 {
                                            Some(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(palette.delta)
                                                    .child(format_signed_bytes(delta)),
                                            )
                                        } else {
                                            None
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.treemap_selected_path = Some(click_path.clone());
                                    if has_children {
                                        this.treemap_root = Some(click_path.clone());
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect()
                }),
        )
        // Selected Tile Inspector Strip
        .children(selected_info.map(|sel| {
            let reveal_p = sel.path.clone();
            let stage_p = sel.path.clone();
            let stage_label = sel
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| sel.path.display().to_string());
            let stage_bytes = sel.physical_bytes;
            let stage_app_id = sel.app_id.clone();
            let protected = is_protected_path(&sel.path);
            let is_staged = state.cart.contains_path(&sel.path);
            let delta = dir_deltas
                .and_then(|m| m.get(&sel.path).copied())
                .unwrap_or(sel.recent_7d_bytes as i64);

            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_2p5()
                .rounded(px(10.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(format!(
                                    "{} · {} ({} delta · {} files)",
                                    compact_display_path(&sel.path, 58),
                                    format_bytes(sel.physical_bytes),
                                    format_signed_bytes(delta),
                                    sel.file_count
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(format!("Category: {} · App ID: {}", sel.category, sel.app_id)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_shrink_0()
                        .gap_2()
                        .child(
                            div()
                                .id("tm-reveal-btn")
                                .px_2p5()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(theme.bg_elevated)
                                .border_1()
                                .border_color(theme.border)
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .cursor_pointer()
                                .on_click(move |_, _, _| reveal_in_finder(&reveal_p))
                                .child("Reveal in Finder"),
                        )
                        .children(if !protected {
                            Some(
                                div()
                                    .id("tm-stage-btn")
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(6.0))
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
                                        let target = CleanupTarget::Path {
                                            path: stage_p.clone(),
                                            label: stage_label.clone(),
                                            bytes: stage_bytes,
                                            safety_tier: SafetyTier::ReviewRequired,
                                            app_id: stage_app_id.clone(),
                                        };
                                        let id = target.id();
                                        if this.cart.contains_id(&id) {
                                            this.cart.unstage(&id);
                                        } else {
                                            this.stage_target(target);
                                        }
                                        cx.notify();
                                    }))
                                    .child(if is_staged {
                                        "Staged ✓"
                                    } else {
                                        "+ Stage for Cleanup"
                                    }),
                            )
                        } else {
                            None
                        }),
                )
        }))
}

fn mode_pill(
    label: &'static str,
    mode: TreemapColorMode,
    current: TreemapColorMode,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> impl IntoElement {
    let active = mode == current;
    div()
        .id(SharedString::from(format!("tm-mode-{label}")))
        .px_2p5()
        .py_1()
        .rounded(px(6.0))
        .text_xs()
        .cursor_pointer()
        .bg(if active {
            theme.accent_soft
        } else {
            theme.bg_surface
        })
        .border_1()
        .border_color(if active { theme.accent } else { theme.border })
        .text_color(if active {
            theme.accent
        } else {
            theme.text_secondary
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.treemap_color_mode = mode;
            cx.notify();
        }))
        .child(label)
}
