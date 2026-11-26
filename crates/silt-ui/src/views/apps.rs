//! Apps & Attribution View: unified macOS per-app footprints combining
//! `.app` + `Application Support` + `Caches` + `Containers` + `Logs` +
//! `com.apple.quarantine` Downloads + Developer artifacts, with Orphan filter.

use crate::state::{AppSortMode, SiltState, reveal_in_finder};
use crate::theme::Theme;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use silt_core::attribution::AppRollup;
use silt_core::cleanup::CleanupTarget;
use silt_core::format::{compact_display_path, format_bytes, format_signed_bytes};
use std::collections::HashMap;

pub fn render_apps(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let delta_lookup: HashMap<&str, i64> = state
        .creep_report
        .as_ref()
        .map(|r| {
            r.top_apps
                .iter()
                .map(|i| (i.key.as_str(), i.delta_bytes))
                .collect()
        })
        .unwrap_or_default();

    let mut apps: Vec<(&AppRollup, i64)> = state
        .app_rollups
        .iter()
        .filter(|a| !state.orphans_only || a.is_orphaned)
        .map(|a| {
            let delta = delta_lookup
                .get(a.app_id.as_str())
                .copied()
                .unwrap_or(a.recent_7d_bytes as i64);
            (a, delta)
        })
        .collect();

    match state.app_sort {
        AppSortMode::FastestGrowing => {
            apps.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
        }
        AppSortMode::LargestTotal => {
            apps.sort_by_key(|(a, _)| std::cmp::Reverse(a.total_bytes));
        }
        AppSortMode::MostReclaimable => {
            apps.sort_by_key(|(a, _)| std::cmp::Reverse(a.reclaimable_bytes));
        }
    }

    let orphan_count = state.app_rollups.iter().filter(|a| a.is_orphaned).count();

    div()
        .flex_1()
        .flex()
        .flex_col()
        .h_full()
        .p_5()
        .gap_3()
        // Header Controls
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
                                .child("Apps & Attribution"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("Unifies .app bundles, ~/Library Containers & Caches, quarantine-attributed Downloads, and Dev artifacts"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(sort_pill(
                            "Fastest Growing",
                            AppSortMode::FastestGrowing,
                            state.app_sort,
                            theme,
                            cx,
                        ))
                        .child(sort_pill(
                            "Largest Total",
                            AppSortMode::LargestTotal,
                            state.app_sort,
                            theme,
                            cx,
                        ))
                        .child(sort_pill(
                            "Most Reclaimable",
                            AppSortMode::MostReclaimable,
                            state.app_sort,
                            theme,
                            cx,
                        ))
                        .child(
                            div()
                                .id("apps-orphan-filter")
                                .px_2p5()
                                .py_1()
                                .rounded(px(6.0))
                                .text_xs()
                                .cursor_pointer()
                                .bg(if state.orphans_only {
                                    theme.danger_red.opacity(0.18)
                                } else {
                                    theme.bg_surface
                                })
                                .border_1()
                                .border_color(if state.orphans_only {
                                    theme.danger_red
                                } else {
                                    theme.border
                                })
                                .text_color(if state.orphans_only {
                                    theme.danger_red
                                } else {
                                    theme.text_secondary
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.orphans_only = !this.orphans_only;
                                    cx.notify();
                                }))
                                .child(format!("Orphaned ({orphan_count})")),
                        ),
                ),
        )
        // Table Body
        .child(
            div()
                .id("apps-table-scroll")
                .flex_1()
                .flex()
                .flex_col()
                .gap_2()
                .overflow_y_scroll()
                .children(if apps.is_empty() {
                    vec![
                        div()
                            .p_6()
                            .rounded(px(12.0))
                            .bg(theme.bg_surface)
                            .border_1()
                            .border_color(theme.border)
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("No attributed apps yet — run a scan to build your unified app breakdown.")
                            .into_any_element(),
                    ]
                } else {
                    apps.into_iter()
                        .take(80)
                        .enumerate()
                        .map(|(idx, (app, delta))| {
                            render_app_row(state, app, delta, idx, theme, cx).into_any_element()
                        })
                        .collect()
                }),
        )
}

fn sort_pill(
    label: &'static str,
    mode: AppSortMode,
    current: AppSortMode,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> impl IntoElement {
    let active = mode == current;
    let id = SharedString::from(format!("sort-{label}"));
    div()
        .id(id)
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
            this.app_sort = mode;
            cx.notify();
        }))
        .child(label)
}

fn render_app_row(
    state: &SiltState,
    app: &AppRollup,
    delta: i64,
    idx: usize,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> impl IntoElement {
    let expanded = state.expanded_apps.contains(&app.app_id);
    let app_id_toggle = app.app_id.clone();
    let delta_color = if delta > 500_000_000 {
        theme.creep_hot
    } else if delta > 0 {
        theme.creep_warm
    } else {
        theme.text_muted
    };

    let mut container = div()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(theme.bg_surface)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .id(SharedString::from(format!("app-row-{idx}")))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_3p5()
                .py_2p5()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_surface_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded_apps.remove(&app_id_toggle) {
                        this.expanded_apps.insert(app_id_toggle.clone());
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(if expanded { "▼" } else { "▶" }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme.text_primary)
                                                .child(app.display_name.clone()),
                                        )
                                        .children(if app.is_orphaned {
                                            Some(
                                                div()
                                                    .px_1p5()
                                                    .py_0p5()
                                                    .rounded(px(4.0))
                                                    .bg(theme.danger_red.opacity(0.18))
                                                    .text_xs()
                                                    .text_color(theme.danger_red)
                                                    .child("Orphaned"),
                                            )
                                        } else {
                                            None
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(format!(
                                            "{} · {} location(s)",
                                            app.category,
                                            app.locations.len()
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_shrink_0()
                        .items_center()
                        .gap_5()
                        .child(metric_col(
                            "Delta",
                            format_signed_bytes(delta),
                            delta_color,
                            theme,
                        ))
                        .child(metric_col(
                            "Caches",
                            format_bytes(app.cache_bytes + app.logs_bytes),
                            theme.text_secondary,
                            theme,
                        ))
                        .child(metric_col(
                            "Reclaimable",
                            format_bytes(app.reclaimable_bytes),
                            if app.reclaimable_bytes > 0 {
                                theme.good_green
                            } else {
                                theme.text_muted
                            },
                            theme,
                        ))
                        .child(metric_col(
                            "Total",
                            format_bytes(app.total_bytes),
                            theme.text_primary,
                            theme,
                        )),
                ),
        );

    if expanded {
        container = container.child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .px_4()
                .pb_3()
                .pt_1()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(app.locations.iter().enumerate().map(|(lidx, loc)| {
                    let reveal_path = loc.path.clone();
                    let stage_path = loc.path.clone();
                    let stage_label = format!("{} ({})", app.display_name, loc.bucket.label());
                    let stage_bytes = loc.bytes;
                    let stage_tier = loc.safety_tier;
                    let stage_app_id = app.app_id.clone();
                    let is_staged = state.cart.contains_path(&loc.path);

                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .px_2p5()
                        .py_1p5()
                        .rounded(px(6.0))
                        .bg(theme.bg_elevated)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_primary)
                                        .child(format!(
                                            "{} · {}",
                                            loc.bucket.label(),
                                            loc.safety_tier.label()
                                        )),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(compact_display_path(&loc.path, 72)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_shrink_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(format_bytes(loc.bytes)),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("reveal-{idx}-{lidx}")))
                                        .px_2()
                                        .py_1()
                                        .rounded(px(5.0))
                                        .bg(theme.bg_surface)
                                        .border_1()
                                        .border_color(theme.border)
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .cursor_pointer()
                                        .on_click(move |_, _, _| {
                                            reveal_in_finder(&reveal_path);
                                        })
                                        .child("Reveal"),
                                )
                                .children(
                                    if loc.safety_tier != silt_core::attribution::SafetyTier::SystemProtected {
                                        Some(
                                            div()
                                                .id(SharedString::from(format!("stage-{idx}-{lidx}")))
                                                .px_2()
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
                                                    let target = CleanupTarget::Path {
                                                        path: stage_path.clone(),
                                                        label: stage_label.clone(),
                                                        bytes: stage_bytes,
                                                        safety_tier: stage_tier,
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
                                                .child(if is_staged { "Staged ✓" } else { "+ Stage" }),
                                        )
                                    } else {
                                        None
                                    },
                                ),
                        )
                })),
        );
    }

    container
}

fn metric_col(label: &'static str, value: String, color: gpui::Hsla, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_end()
        .w(px(86.0))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(label),
        )
}
