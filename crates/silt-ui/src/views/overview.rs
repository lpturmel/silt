//! Overview & Creep Timeline View:
//! - KPI strip (Physical Remaining, macOS Available, Purgeable / Snapshots, Creep Rate & Runway)
//! - Segmented macOS storage breakdown bar
//! - Multi-series Timeline Chart with Cleanup Milestone pins
//! - Top Creepers leaderboard with inline sparklines & quick stage/inspect actions

use crate::state::{ActiveTab, SiltState};
use crate::theme::Theme;
use crate::widgets::capacity_bar::render_capacity_bar;
use crate::widgets::sparkline::render_sparkline;
use crate::widgets::timeline_chart::render_timeline_chart;
use gpui::{
    Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use silt_core::diff::{CreepBasis, TimeWindow};
use silt_core::format::{format_bytes, format_rate_per_day, format_signed_bytes};

pub fn render_overview(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let Some(drive) = state.selected_drive() else {
        return div()
            .id("overview-empty")
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(theme.text_muted)
            .child("No drive selected.");
    };

    let (creep_value, creep_sub, creep_color) = match &state.creep_report {
        Some(rep) if !matches!(rep.basis, CreepBasis::NoSnapshotsYet) => {
            let rate_str = format_rate_per_day(rep.avg_bytes_per_day);
            let sub = if let Some(runway) = &rep.runway {
                if let Some(d10) = runway.days_until_ten_percent {
                    format!("{rate_str} · ~{d10:.0}d to <10% free")
                } else {
                    format!("{rate_str} · stable")
                }
            } else {
                match rep.basis {
                    CreepBasis::EstimatedFromFileDates { window_days } => {
                        format!("{rate_str} · Day-1 {window_days}d estimate")
                    }
                    _ => rate_str,
                }
            };
            let color = if rep.net_delta_bytes > 1_000_000_000 {
                theme.creep_hot
            } else if rep.net_delta_bytes > 0 {
                theme.creep_warm
            } else {
                theme.good_green
            };
            (format_signed_bytes(rep.net_delta_bytes), sub, color)
        }
        _ => (
            "—".to_string(),
            "Run first scan to track creep".to_string(),
            theme.text_muted,
        ),
    };

    let windows = if drive.is_internal {
        vec![
            TimeWindow::Hours24,
            TimeWindow::Days7,
            TimeWindow::Days30,
            TimeWindow::Days90,
            TimeWindow::All,
        ]
    } else {
        vec![
            TimeWindow::SinceLastConnected,
            TimeWindow::Days7,
            TimeWindow::Days30,
            TimeWindow::All,
        ]
    };

    let basis_badge = match state.creep_report.as_ref().map(|r| &r.basis) {
        Some(CreepBasis::ExactSnapshotDiff { .. }) => ("Exact Snapshot Diff", theme.good_green),
        Some(CreepBasis::EstimatedFromFileDates { .. }) => {
            ("Day-1 Estimate (file dates)", theme.warning_amber)
        }
        _ => ("Awaiting First Scan", theme.text_muted),
    };

    let top_apps = state
        .creep_report
        .as_ref()
        .map(|r| r.top_apps.iter().take(10).cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    div()
        .id("overview-scroll")
        .flex_1()
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .overflow_y_scroll()
        // KPI Row
        .child(
            div()
                .flex()
                .flex_row()
                .gap_3()
                .w_full()
                .child(kpi_card(
                    "PHYSICAL REMAINING",
                    format_bytes(drive.physical_free_bytes),
                    format!("of {} total (raw blocks)", format_bytes(drive.total_bytes)),
                    theme.free_color,
                    theme,
                ))
                .child(kpi_card(
                    "MACOS AVAILABLE",
                    format_bytes(drive.available_important_bytes),
                    "Finder-compatible available space".to_string(),
                    theme.accent,
                    theme,
                ))
                .child(kpi_card(
                    "PURGEABLE & SNAPSHOTS",
                    format_bytes(drive.purgeable_bytes),
                    format!("{} local APFS snapshot(s)", drive.snapshot_count),
                    theme.purgeable_color,
                    theme,
                ))
                .child(kpi_card(
                    "CREEP VELOCITY",
                    creep_value,
                    creep_sub,
                    creep_color,
                    theme,
                )),
        )
        // Unified Storage Breakdown Bar Card
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2p5()
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
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(format!(
                                    "{} — Storage Breakdown ({})",
                                    drive.name,
                                    drive.mount_point.display()
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_4()
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .child(legend_dot(
                                    format!("Used {}", format_bytes(drive.used_bytes)),
                                    theme.used_color,
                                ))
                                .child(legend_dot(
                                    format!("Purgeable {}", format_bytes(drive.purgeable_bytes)),
                                    theme.purgeable_color,
                                ))
                                .child(legend_dot(
                                    format!(
                                        "Remaining {}",
                                        format_bytes(drive.physical_free_bytes)
                                    ),
                                    theme.free_color,
                                )),
                        ),
                )
                .child(render_capacity_bar(
                    drive.used_frac(),
                    drive.purgeable_frac(),
                    drive.free_frac(),
                    14.0,
                    theme,
                )),
        )
        // Timeline Graph Card
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
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
                                        .child("Storage Creep Over Time"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child("Active used (indigo) · Purgeable (amber) · Remaining (green) · Cleanup pins"),
                                ),
                        )
                        // Window selector pills
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_1()
                                .children(windows.into_iter().map(|w| {
                                    let active = state.selected_window == w;
                                    let id = SharedString::from(format!("win-{}", w.label()));
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
                                            theme.bg_elevated
                                        })
                                        .border_1()
                                        .border_color(if active {
                                            theme.accent
                                        } else {
                                            theme.border_subtle
                                        })
                                        .text_color(if active {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_window(w);
                                            cx.notify();
                                        }))
                                        .child(w.label())
                                })),
                        ),
                )
                .child(render_timeline_chart(
                    state.samples.clone(),
                    state.cleanup_events.clone(),
                    drive.total_bytes,
                    theme,
                )),
        )
        // Top Creepers Card
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
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
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child("Top Creepers"),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(99.0))
                                        .bg(basis_badge.1.opacity(0.15))
                                        .text_xs()
                                        .text_color(basis_badge.1)
                                        .child(basis_badge.0),
                                ),
                        )
                        .child(
                            div()
                                .id("overview-view-all-apps")
                                .text_xs()
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active_tab = ActiveTab::Apps;
                                    cx.notify();
                                }))
                                .child("Inspect All Apps →"),
                        ),
                )
                .child(if top_apps.is_empty() {
                    div()
                        .py_6()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child("Click \"Scan Now\" in the top bar to capture your first snapshot and see which apps are creeping.")
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .children(top_apps.into_iter().enumerate().map(|(idx, item)| {
                            let delta_color = if item.delta_bytes > 500_000_000 {
                                theme.creep_hot
                            } else if item.delta_bytes > 0 {
                                theme.creep_warm
                            } else {
                                theme.good_green
                            };
                            let app_key = item.key.clone();
                            let row_id = SharedString::from(format!("creeper-row-{idx}"));

                            div()
                                .id(row_id)
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .rounded(px(8.0))
                                .bg(theme.bg_elevated)
                                .hover(|s| s.bg(theme.bg_surface_hover))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.expanded_apps.insert(app_key.clone());
                                    this.active_tab = ActiveTab::Apps;
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(theme.text_primary)
                                                .child(item.label.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.text_muted)
                                                .child(if item.is_orphaned {
                                                    "Orphaned App Leftover".to_string()
                                                } else {
                                                    item.category.clone()
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_2()
                                        .child(render_sparkline(
                                            item.sparkline.clone(),
                                            delta_color,
                                            88.0,
                                            24.0,
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_5()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .items_end()
                                                .w(px(100.0))
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(delta_color)
                                                        .child(format_signed_bytes(item.delta_bytes)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(format_rate_per_day(item.bytes_per_day)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .items_end()
                                                .w(px(90.0))
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(theme.text_primary)
                                                        .child(format_bytes(item.current_bytes)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(if item.reclaimable_bytes > 0 {
                                                            format!(
                                                                "{} cleanable",
                                                                format_bytes(item.reclaimable_bytes)
                                                            )
                                                        } else {
                                                            "total".to_string()
                                                        }),
                                                ),
                                        ),
                                )
                        }))
                        .into_any_element()
                }),
        )
}

fn kpi_card(
    title: &'static str,
    value: String,
    subtitle: String,
    accent: Hsla,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .p_3p5()
        .rounded(px(12.0))
        .bg(theme.bg_surface)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(title),
        )
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(accent)
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(subtitle),
        )
}

fn legend_dot(label: String, color: Hsla) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(div().w(px(8.0)).h(px(8.0)).rounded(px(4.0)).bg(color))
        .child(label)
}
