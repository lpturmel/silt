//! Left Sidebar: Internal, Connected External, and Remembered Offline Drives
//! plus Settings & Full Disk Access status indicator.

use crate::state::{ActiveTab, DriveEntry, SiltState};
use crate::theme::Theme;
use crate::widgets::capacity_bar::render_capacity_bar;
use chrono::{Local, TimeZone};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use silt_core::format::{format_bytes, format_signed_bytes};

pub fn render_sidebar(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let internal: Vec<&DriveEntry> = state
        .drives
        .iter()
        .filter(|d| d.is_internal && !d.is_ignored)
        .collect();
    let online_ext: Vec<&DriveEntry> = state
        .drives
        .iter()
        .filter(|d| !d.is_internal && d.is_online && !d.is_ignored)
        .collect();
    let offline_ext: Vec<&DriveEntry> = state
        .drives
        .iter()
        .filter(|d| !d.is_internal && !d.is_online && !d.is_ignored)
        .collect();

    div()
        .flex()
        .flex_col()
        .w(px(260.0))
        .h_full()
        .bg(theme.bg_sidebar)
        .border_r_1()
        .border_color(theme.border)
        .p_3()
        .gap_3()
        // Brand Header
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .py_1()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.text_primary)
                                .child("Silt"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("macOS Storage Creep"),
                        ),
                )
                .child(
                    div()
                        .id("sidebar-refresh-btn")
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(theme.bg_surface)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_surface_hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_from_disk();
                            cx.notify();
                        }))
                        .child("Refresh"),
                ),
        )
        // Scrollable Drives List
        .child(
            div()
                .id("sidebar-drives-scroll")
                .flex_1()
                .flex()
                .flex_col()
                .gap_3()
                .overflow_y_scroll()
                .child(section_label("INTERNAL DRIVE", theme))
                .children(internal.into_iter().map(|d| render_drive_card(state, d, theme, cx)))
                .when(!online_ext.is_empty(), |el| {
                    el.child(section_label("EXTERNAL DRIVES", theme)).children(
                        online_ext
                            .into_iter()
                            .map(|d| render_drive_card(state, d, theme, cx)),
                    )
                })
                .when(!offline_ext.is_empty(), |el| {
                    el.child(section_label("OFFLINE (REMEMBERED)", theme))
                        .children(
                            offline_ext
                                .into_iter()
                                .map(|d| render_drive_card(state, d, theme, cx)),
                        )
                }),
        )
        // Bottom Status & Settings
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .when(!state.has_fda, |el| {
                    el.child(
                        div()
                            .id("sidebar-fda-pill")
                            .px_2()
                            .py_1p5()
                            .rounded(px(8.0))
                            .bg(theme.warning_amber.opacity(0.14))
                            .border_1()
                            .border_color(theme.warning_amber.opacity(0.4))
                            .text_xs()
                            .text_color(theme.warning_amber)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_tab = ActiveTab::Settings;
                                cx.notify();
                            }))
                            .child("Limited Access · Grant Full Disk Access"),
                    )
                })
                .child(
                    div()
                        .id("sidebar-settings-btn")
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px_2p5()
                        .py_2()
                        .rounded(px(8.0))
                        .bg(if state.active_tab == ActiveTab::Settings {
                            theme.accent_soft
                        } else {
                            theme.bg_surface
                        })
                        .border_1()
                        .border_color(if state.active_tab == ActiveTab::Settings {
                            theme.accent
                        } else {
                            theme.border_subtle
                        })
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_surface_hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_tab = ActiveTab::Settings;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_primary)
                                .child("Settings & Collector"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if state.daemon_installed {
                                    theme.good_green
                                } else {
                                    theme.text_muted
                                })
                                .child(if state.daemon_installed { "Auto" } else { "Manual" }),
                        ),
                ),
        )
}

fn section_label(label: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .px_2()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(label)
}

fn render_drive_card(
    state: &SiltState,
    drive: &DriveEntry,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> gpui::Stateful<gpui::Div> {
    let is_selected = state.selected_volume_uuid.as_deref() == Some(drive.uuid.as_str());
    let uuid = drive.uuid.clone();
    let card_id = SharedString::from(format!("drive-{}", drive.uuid));

    let badge_text = if !drive.is_online {
        let seen = Local
            .timestamp_opt(drive.last_seen_at, 0)
            .single()
            .map(|dt| dt.format("%b %d").to_string())
            .unwrap_or_else(|| "Offline".to_string());
        (format!("Seen {seen}"), theme.text_muted)
    } else if let Some(delta) = drive.recent_7d_delta {
        let color = if delta > 500_000_000 {
            theme.creep_hot
        } else if delta > 0 {
            theme.creep_warm
        } else {
            theme.good_green
        };
        (format!("{} /7d", format_signed_bytes(delta)), color)
    } else {
        ("New".to_string(), theme.accent)
    };

    div()
        .id(card_id)
        .flex()
        .flex_col()
        .gap_1p5()
        .p_2p5()
        .rounded(px(10.0))
        .bg(if is_selected {
            theme.bg_elevated
        } else {
            theme.bg_surface
        })
        .border_1()
        .border_color(if is_selected {
            theme.accent
        } else {
            theme.border_subtle
        })
        .opacity(if drive.is_online { 1.0 } else { 0.68 })
        .cursor_pointer()
        .hover(|s| s.bg(theme.bg_surface_hover))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_volume(uuid.clone());
            if this.active_tab == ActiveTab::Settings {
                this.active_tab = ActiveTab::Overview;
            }
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(drive.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(badge_text.1)
                        .child(badge_text.0),
                ),
        )
        .child(render_capacity_bar(
            drive.used_frac(),
            drive.purgeable_frac(),
            drive.free_frac(),
            6.0,
            theme,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(format!("{} avail", format_bytes(drive.available_important_bytes)))
                .child(format!("{} free", format_bytes(drive.physical_free_bytes))),
        )
}

trait WhenExt: Sized {
    fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }
}

impl<T> WhenExt for T {}
