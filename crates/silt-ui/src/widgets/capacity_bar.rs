//! Segmented macOS capacity bar showing Active Used | Purgeable | Physical Remaining.

use crate::theme::Theme;
use gpui::{Div, IntoElement, ParentElement, Styled, div, px, relative};

pub fn render_capacity_bar(
    used_frac: f64,
    purgeable_frac: f64,
    free_frac: f64,
    height_px: f32,
    theme: &Theme,
) -> Div {
    let used_pct = (used_frac.clamp(0.0, 1.0) * 100.0) as f32;
    let purge_pct = (purgeable_frac.clamp(0.0, 1.0) * 100.0) as f32;
    let free_pct = (free_frac.clamp(0.0, 1.0) * 100.0) as f32;

    div()
        .flex()
        .flex_row()
        .w_full()
        .h(px(height_px))
        .rounded(px(height_px / 2.0))
        .overflow_hidden()
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border_subtle)
        .children([
            if used_pct > 0.2 {
                Some(
                    div()
                        .h_full()
                        .w(relative(used_pct / 100.0))
                        .bg(theme.used_color),
                )
            } else {
                None
            },
            if purge_pct > 0.2 {
                Some(
                    div()
                        .h_full()
                        .w(relative(purge_pct / 100.0))
                        .bg(theme.purgeable_color),
                )
            } else {
                None
            },
            if free_pct > 0.2 {
                Some(
                    div()
                        .h_full()
                        .w(relative(free_pct / 100.0))
                        .bg(theme.free_color.opacity(0.35)),
                )
            } else {
                None
            },
        ]
        .into_iter()
        .flatten()
        .map(IntoElement::into_any_element))
}
