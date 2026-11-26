//! Interactive multi-series Metal-accelerated storage timeline chart
//! showing Active Used, Purgeable Headroom, Physical Remaining, and Cleanup Milestone pins.

use crate::theme::Theme;
use chrono::{Local, TimeZone};
use gpui::{
    Bounds, IntoElement, ParentElement, PathBuilder, Styled, canvas, div, fill, point, px, size,
};
use silt_core::db::{CleanupEvent, VolumeSample};
use silt_core::format::format_bytes;

pub fn render_timeline_chart(
    samples: Vec<VolumeSample>,
    cleanup_events: Vec<CleanupEvent>,
    total_capacity: u64,
    theme: &Theme,
) -> impl IntoElement {
    let used_color = theme.used_color;
    let purge_color = theme.purgeable_color;
    let free_color = theme.free_color;
    let grid_color = theme.border_subtle;
    let pin_color = theme.good_green;

    let max_bytes = total_capacity.max(
        samples
            .iter()
            .map(|s| s.total_bytes.max(s.used_bytes + s.purgeable_bytes))
            .max()
            .unwrap_or(1),
    );

    let left_label = samples
        .first()
        .and_then(|s| Local.timestamp_opt(s.sampled_at, 0).single())
        .map(|dt| dt.format("%b %d %H:%M").to_string())
        .unwrap_or_else(|| "Now".to_string());
    let right_label = samples
        .last()
        .and_then(|s| Local.timestamp_opt(s.sampled_at, 0).single())
        .map(|dt| dt.format("%b %d %H:%M").to_string())
        .unwrap_or_else(|| "Now".to_string());

    let sample_count = samples.len();

    div()
        .flex()
        .flex_col()
        .w_full()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .h(px(190.0))
                .gap_2()
                // Y-axis capacity ticks
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .w(px(62.0))
                        .h_full()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(format_bytes(max_bytes))
                        .child(format_bytes(max_bytes / 2))
                        .child("0 B"),
                )
                // GPU-painted chart canvas
                .child(
                    canvas(
                        |bounds, _, _| bounds,
                        move |bounds, _, window, _| {
                            let w = f32::from(bounds.size.width).max(1.0);
                            let h = f32::from(bounds.size.height).max(1.0);
                            let pad_y = 6.0_f32;
                            let plot_h = (h - pad_y * 2.0).max(1.0);

                            // Horizontal gridlines (0%, 25%, 50%, 75%, 100%)
                            for i in 0..=4 {
                                let y = bounds.origin.y + px(pad_y + plot_h * (i as f32 / 4.0));
                                window.paint_quad(fill(
                                    Bounds {
                                        origin: point(bounds.origin.x, y),
                                        size: size(px(w), px(1.0)),
                                    },
                                    grid_color,
                                ));
                            }

                            if samples.is_empty() || max_bytes == 0 {
                                return;
                            }

                            let t_min = samples.first().map(|s| s.sampled_at).unwrap_or(0);
                            let t_max = samples.last().map(|s| s.sampled_at).unwrap_or(t_min);
                            let t_span = (t_max - t_min).max(1) as f64;

                            // Build synthetic 2-point horizontal band when only 1 sample exists (Day 1)
                            let plot_points: Vec<(f32, &VolumeSample)> = if samples.len() == 1 {
                                vec![(0.0, &samples[0]), (w, &samples[0])]
                            } else {
                                samples
                                    .iter()
                                    .map(|s| {
                                        let x_frac = ((s.sampled_at - t_min) as f64 / t_span) as f32;
                                        (x_frac * w, s)
                                    })
                                    .collect()
                            };

                            let y_for = |val: u64| -> gpui::Pixels {
                                let frac = (val as f64 / max_bytes as f64).clamp(0.0, 1.0) as f32;
                                bounds.origin.y + px(pad_y + plot_h * (1.0 - frac))
                            };
                            let bottom_y = bounds.origin.y + px(pad_y + plot_h);

                            // 1. Stacked Purgeable + Used upper band
                            let mut purge_fill = PathBuilder::fill();
                            let mut purge_line = PathBuilder::stroke(px(1.5));
                            for (idx, (x_off, s)) in plot_points.iter().enumerate() {
                                let x = bounds.origin.x + px(*x_off);
                                let y = y_for(s.used_bytes.saturating_add(s.purgeable_bytes));
                                if idx == 0 {
                                    purge_fill.move_to(point(x, bottom_y));
                                    purge_fill.line_to(point(x, y));
                                    purge_line.move_to(point(x, y));
                                } else {
                                    purge_fill.line_to(point(x, y));
                                    purge_line.line_to(point(x, y));
                                }
                            }
                            if let Some((last_x, _)) = plot_points.last() {
                                purge_fill.line_to(point(bounds.origin.x + px(*last_x), bottom_y));
                                purge_fill.close();
                            }
                            if let Ok(p) = purge_fill.build() {
                                window.paint_path(p, purge_color.opacity(0.18));
                            }
                            if let Ok(p) = purge_line.build() {
                                window.paint_path(p, purge_color.opacity(0.85));
                            }

                            // 2. Active Used area + crisp line
                            let mut used_fill = PathBuilder::fill();
                            let mut used_line = PathBuilder::stroke(px(2.25));
                            for (idx, (x_off, s)) in plot_points.iter().enumerate() {
                                let x = bounds.origin.x + px(*x_off);
                                let y = y_for(s.used_bytes);
                                if idx == 0 {
                                    used_fill.move_to(point(x, bottom_y));
                                    used_fill.line_to(point(x, y));
                                    used_line.move_to(point(x, y));
                                } else {
                                    used_fill.line_to(point(x, y));
                                    used_line.line_to(point(x, y));
                                }
                            }
                            if let Some((last_x, _)) = plot_points.last() {
                                used_fill.line_to(point(bounds.origin.x + px(*last_x), bottom_y));
                                used_fill.close();
                            }
                            if let Ok(p) = used_fill.build() {
                                window.paint_path(p, used_color.opacity(0.26));
                            }
                            if let Ok(p) = used_line.build() {
                                window.paint_path(p, used_color);
                            }

                            // 3. Physical Remaining line (green)
                            let mut free_line = PathBuilder::stroke(px(1.75));
                            for (idx, (x_off, s)) in plot_points.iter().enumerate() {
                                let x = bounds.origin.x + px(*x_off);
                                let y = y_for(s.physical_free_bytes);
                                if idx == 0 {
                                    free_line.move_to(point(x, y));
                                } else {
                                    free_line.line_to(point(x, y));
                                }
                            }
                            if let Ok(p) = free_line.build() {
                                window.paint_path(p, free_color);
                            }

                            // 4. Vertical Cleanup Event Milestone Pins
                            if samples.len() >= 2 {
                                for ev in &cleanup_events {
                                    if ev.performed_at >= t_min && ev.performed_at <= t_max {
                                        let x_frac =
                                            ((ev.performed_at - t_min) as f64 / t_span) as f32;
                                        let x = bounds.origin.x + px(x_frac * w);
                                        window.paint_quad(fill(
                                            Bounds {
                                                origin: point(x, bounds.origin.y + px(pad_y)),
                                                size: size(px(2.0), px(plot_h)),
                                            },
                                            pin_color.opacity(0.7),
                                        ));
                                        window.paint_quad(fill(
                                            Bounds {
                                                origin: point(
                                                    x - px(3.0),
                                                    bounds.origin.y + px(pad_y - 2.0),
                                                ),
                                                size: size(px(8.0), px(8.0)),
                                            },
                                            pin_color,
                                        ));
                                    }
                                }
                            }
                        },
                    )
                    .flex_1()
                    .h_full(),
                ),
        )
        // X-axis footer labels + legend
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .pl(px(70.0))
                .text_xs()
                .text_color(theme.text_muted)
                .child(left_label)
                .child(if sample_count <= 1 {
                    "Baseline recorded — trend line fills in automatically over time".to_string()
                } else {
                    format!("{sample_count} samples")
                })
                .child(right_label),
        )
}
