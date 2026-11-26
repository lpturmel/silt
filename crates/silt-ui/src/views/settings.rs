//! Settings & Collector View:
//! - Full Disk Access (TCC) status & System Settings shortcut
//! - Opt-in Background LaunchAgent (`siltd`) toggle
//! - Theme preference (System / Dark / Light)
//! - Scan scope (`/private/var` toggle) & auto-ignored external drives management

use crate::state::{SiltState, launch_agent_plist_path, open_full_disk_access_settings};
use crate::theme::{Theme, ThemePreference};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

pub fn render_settings(state: &SiltState, theme: &Theme, cx: &mut Context<SiltState>) -> impl IntoElement {
    let ignored_drives: Vec<_> = state.drives.iter().filter(|d| d.is_ignored).collect();

    div()
        .id("settings-scroll")
        .flex_1()
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .overflow_y_scroll()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child("Settings & Collector"),
        )
        // Background LaunchAgent Collector Card
        .child(
            setting_card(
                "Background Space Collector (siltd)",
                "Opt-in macOS LaunchAgent (Nice = 10, LowPriorityIO) that records hourly volume telemetry, tracks FSEvents dirty directories, and snapshots external drives automatically on plug-in.",
                theme,
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if state.daemon_installed {
                                        theme.good_green
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(if state.daemon_installed {
                                        "Active — LaunchAgent installed"
                                    } else {
                                        "Inactive — Manual snapshots when app opens"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(launch_agent_plist_path().display().to_string()),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-daemon-toggle")
                            .px_3()
                            .py_1p5()
                            .rounded(px(8.0))
                            .bg(if state.daemon_installed {
                                theme.danger_red.opacity(0.16)
                            } else {
                                theme.accent_soft
                            })
                            .border_1()
                            .border_color(if state.daemon_installed {
                                theme.danger_red
                            } else {
                                theme.accent
                            })
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if state.daemon_installed {
                                theme.danger_red
                            } else {
                                theme.accent
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_daemon_launch_agent();
                                cx.notify();
                            }))
                            .child(if state.daemon_installed {
                                "Disable Background Collector"
                            } else {
                                "Enable Background Collector"
                            }),
                    ),
            ),
        )
        // Full Disk Access Card
        .child(
            setting_card(
                "macOS Full Disk Access (Optional)",
                "Granting Full Disk Access to Silt.app lets the scanner inspect protected macOS locations such as ~/Library/Mail, ~/Library/Messages, and Safari caches without permission skips.",
                theme,
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if state.has_fda {
                                theme.good_green
                            } else {
                                theme.warning_amber
                            })
                            .child(if state.has_fda {
                                "Full Disk Access Granted ✓"
                            } else {
                                "Limited Access (TCC-protected folders skipped gracefully)"
                            }),
                    )
                    .child(
                        div()
                            .id("settings-open-fda")
                            .px_3()
                            .py_1p5()
                            .rounded(px(8.0))
                            .bg(theme.bg_elevated)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .cursor_pointer()
                            .on_click(|_, _, _| open_full_disk_access_settings())
                            .child("Open System Settings →"),
                    ),
            ),
        )
        // Appearance & Scan Scope Card
        .child(
            setting_card(
                "Appearance & Internal Scan Scope",
                "Choose adaptive macOS Dark/Light appearance and whether internal scans also include /private/var.",
                theme,
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child("Theme"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_1p5()
                                    .children([
                                        ("System", ThemePreference::System),
                                        ("Dark", ThemePreference::Dark),
                                        ("Light", ThemePreference::Light),
                                    ]
                                    .into_iter()
                                    .map(|(lbl, pref)| {
                                        let active = state.theme_pref == pref;
                                        div()
                                            .id(SharedString::from(format!("theme-{lbl}")))
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
                                                this.theme_pref = pref;
                                                cx.notify();
                                            }))
                                            .child(lbl)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child("Include /private/var in internal Macintosh HD scans"),
                            )
                            .child(
                                div()
                                    .id("settings-private-var-toggle")
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(6.0))
                                    .bg(if state.include_private_var {
                                        theme.accent_soft
                                    } else {
                                        theme.bg_elevated
                                    })
                                    .border_1()
                                    .border_color(if state.include_private_var {
                                        theme.accent
                                    } else {
                                        theme.border
                                    })
                                    .text_xs()
                                    .text_color(if state.include_private_var {
                                        theme.accent
                                    } else {
                                        theme.text_secondary
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.include_private_var = !this.include_private_var;
                                        cx.notify();
                                    }))
                                    .child(if state.include_private_var {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    }),
                            ),
                    ),
            ),
        )
        // Ignored Volumes & Database Card
        .child(
            setting_card(
                "Auto-Ignored Volumes & Local Ledger",
                "Transient .dmg images, network shares, and Time Machine backup drives are filtered automatically so your sidebar stays focused.",
                theme,
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(format!("SQLite Creep Ledger: {}", state.db_path.display())),
                    )
                    .children(ignored_drives.into_iter().enumerate().map(|(i, d)| {
                        let uuid = d.uuid.clone();
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(6.0))
                            .bg(theme.bg_elevated)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_secondary)
                                    .child(format!(
                                        "{} ({})",
                                        d.name,
                                        d.ignore_reason.as_deref().unwrap_or("Ignored")
                                    )),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("unignore-{i}")))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(5.0))
                                    .bg(theme.accent_soft)
                                    .text_xs()
                                    .text_color(theme.accent)
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_volume_ignored(&uuid, false);
                                        cx.notify();
                                    }))
                                    .child("Track Anyway"),
                            )
                    })),
            ),
        )
}

fn setting_card(title: &'static str, desc: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .p_4()
        .rounded(px(12.0))
        .bg(theme.bg_surface)
        .border_1()
        .border_color(theme.border)
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
        )
}
