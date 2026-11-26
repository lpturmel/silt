//! Silt — Native Metal-accelerated GPUI desktop application for macOS storage intelligence.

mod state;
mod theme;
mod views;
mod widgets;

use crate::state::{ActiveTab, SiltState};
use crate::theme::Theme;
use gpui::{
    App, AppContext, Application, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, px, size,
};
use silt_core::cleanup::{CleanupTarget, DeleteMode};
use silt_core::format::format_bytes;

impl Render for SiltState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::resolve(self.theme_pref, window.appearance());

        let tab_content = match self.active_tab {
            ActiveTab::Overview => views::overview::render_overview(self, &theme, cx).into_any_element(),
            ActiveTab::Apps => views::apps::render_apps(self, &theme, cx).into_any_element(),
            ActiveTab::SpaceMap => views::treemap::render_treemap(self, &theme, cx).into_any_element(),
            ActiveTab::Cleanup => views::cleanup::render_cleanup(self, &theme, cx).into_any_element(),
            ActiveTab::Settings => views::settings::render_settings(self, &theme, cx).into_any_element(),
        };

        let has_snapshot_in_cart = self
            .cart
            .items()
            .iter()
            .any(|t| matches!(t, CleanupTarget::ApfsSnapshot { .. }));

        div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme.bg_window)
            .text_color(theme.text_primary)
            // Left Sidebar
            .child(views::sidebar::render_sidebar(self, &theme, cx))
            // Main Workspace Column
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .h_full()
                    .overflow_hidden()
                    // Top Navigation & Scan Bar
                    .child(render_top_bar(self, &theme, cx))
                    // Optional Toast Notification Banner
                    .children(self.toast_message.clone().map(|(msg, is_err)| {
                        let color = if is_err {
                            theme.danger_red
                        } else {
                            theme.good_green
                        };
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_5()
                            .py_2()
                            .bg(color.opacity(0.14))
                            .border_b_1()
                            .border_color(color.opacity(0.35))
                            .child(div().text_xs().text_color(color).child(msg))
                            .child(
                                div()
                                    .id("dismiss-toast")
                                    .text_xs()
                                    .text_color(color)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toast_message = None;
                                        cx.notify();
                                    }))
                                    .child("Dismiss"),
                            )
                    }))
                    // Active Tab View
                    .child(tab_content)
                    // Persistent Bottom Staging Cart Tray (visible whenever items are staged)
                    .children(if !self.cart.is_empty() {
                        Some(render_staging_cart_bar(
                            self,
                            has_snapshot_in_cart,
                            &theme,
                            cx,
                        ))
                    } else {
                        None
                    }),
            )
            // Confirmation Modal for APFS Snapshot Pruning / Permanent Deletion
            .children(self.confirm_cleanup_modal.map(|mode| {
                render_confirm_modal(self, mode, has_snapshot_in_cart, &theme, cx)
            }))
    }
}

fn render_top_bar(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let tabs = [
        ("Overview", ActiveTab::Overview),
        ("Apps", ActiveTab::Apps),
        ("Space Map", ActiveTab::SpaceMap),
        ("Cleanup", ActiveTab::Cleanup),
    ];

    let drive_label = state
        .selected_drive()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Silt".to_string());

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_5()
        .py_3()
        .bg(theme.bg_surface)
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(drive_label),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .children(tabs.into_iter().map(|(label, tab)| {
                            let active = state.active_tab == tab;
                            div()
                                .id(SharedString::from(format!("tab-{label}")))
                                .px_3()
                                .py_1p5()
                                .rounded(px(7.0))
                                .text_xs()
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .bg(if active {
                                    theme.accent_soft
                                } else {
                                    gpui::rgba(0x00000000).into()
                                })
                                .text_color(if active {
                                    theme.accent
                                } else {
                                    theme.text_secondary
                                })
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.bg_surface_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active_tab = tab;
                                    cx.notify();
                                }))
                                .child(label)
                        })),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2p5()
                .children(state.scan_status_text.clone().map(|txt| {
                    div()
                        .text_xs()
                        .text_color(theme.accent)
                        .child(txt)
                }))
                .child(
                    div()
                        .id("top-scan-now-btn")
                        .px_3p5()
                        .py_1p5()
                        .rounded(px(7.0))
                        .bg(if state.is_scanning {
                            theme.bg_elevated
                        } else {
                            theme.accent
                        })
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if state.is_scanning {
                            theme.text_muted
                        } else {
                            gpui::rgb(0x0b1220).into()
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_scan(cx);
                        }))
                        .child(if state.is_scanning {
                            "Scanning…"
                        } else {
                            "Scan Now"
                        }),
                ),
        )
}

fn render_staging_cart_bar(
    state: &SiltState,
    has_snapshot_in_cart: bool,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_5()
        .py_3()
        .bg(theme.bg_elevated)
        .border_t_1()
        .border_color(theme.accent.opacity(0.6))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded(px(99.0))
                        .bg(theme.accent_soft)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.accent)
                        .child(format!("{} staged", state.cart.len())),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(format!(
                            "Will reclaim ~{}",
                            format_bytes(state.cart.total_reclaimable_bytes())
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(if has_snapshot_in_cart {
                            "(includes APFS snapshot prune — requires confirmation)"
                        } else {
                            "(moves files safely to macOS Trash)"
                        }),
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
                        .id("cart-clear-btn")
                        .px_3()
                        .py_1p5()
                        .rounded(px(7.0))
                        .bg(theme.bg_surface)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cart.clear();
                            cx.notify();
                        }))
                        .child("Clear"),
                )
                .child(
                    div()
                        .id("cart-trash-btn")
                        .px_3p5()
                        .py_1p5()
                        .rounded(px(7.0))
                        .bg(theme.good_green)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui::rgb(0x052e16))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if has_snapshot_in_cart {
                                this.confirm_cleanup_modal = Some(DeleteMode::Trash);
                                cx.notify();
                            } else {
                                this.run_staged_cleanup(DeleteMode::Trash, cx);
                            }
                        }))
                        .child(if has_snapshot_in_cart {
                            "Review & Clean…"
                        } else {
                            "Move to Trash"
                        }),
                ),
        )
}

fn render_confirm_modal(
    state: &SiltState,
    mode: DeleteMode,
    has_snapshot_in_cart: bool,
    theme: &Theme,
    cx: &mut Context<SiltState>,
) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000099))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .w(px(460.0))
                .p_5()
                .rounded(px(14.0))
                .bg(theme.bg_surface)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child("Confirm Cleanup Action"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(if has_snapshot_in_cart {
                            format!(
                                "You have {} item(s) staged (~{}), including local APFS snapshot(s). Regular files will move to macOS Trash, while APFS snapshot deletions via tmutil cannot be undone.",
                                state.cart.len(),
                                format_bytes(state.cart.total_reclaimable_bytes())
                            )
                        } else {
                            format!(
                                "Proceed with cleaning {} item(s) (~{})?",
                                state.cart.len(),
                                format_bytes(state.cart.total_reclaimable_bytes())
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .pt_2()
                        .child(
                            div()
                                .id("modal-cancel-btn")
                                .px_3()
                                .py_1p5()
                                .rounded(px(7.0))
                                .bg(theme.bg_elevated)
                                .border_1()
                                .border_color(theme.border)
                                .text_xs()
                                .text_color(theme.text_secondary)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_cleanup_modal = None;
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("modal-confirm-btn")
                                .px_3p5()
                                .py_1p5()
                                .rounded(px(7.0))
                                .bg(theme.danger_red)
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_staged_cleanup(mode, cx);
                                }))
                                .child("Confirm & Clean"),
                        ),
                ),
        )
}

fn main() {
    let db_path = silt_core::default_db_path();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1240.0), px(820.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Silt — macOS Storage Intelligence".into()),
                appears_transparent: false,
                ..Default::default()
            }),
            ..Default::default()
        };

        cx.open_window(options, |_, cx| {
            let view = cx.new(|_| SiltState::new(db_path.clone()));
            // Automatically trigger an initial background scan if no snapshot exists yet (Day 1)
            view.update(cx, |state, cx| {
                if state.latest_snapshot.is_none()
                    && state.selected_drive().is_some_and(|d| d.is_online)
                {
                    state.start_scan(cx);
                }
            });
            view
        })
        .expect("open Silt GPUI window");
    });
}
