//! Inline GPUI Metal-rendered vector sparkline for per-app growth trends.

use gpui::{Hsla, IntoElement, PathBuilder, Styled, canvas, point, px};

pub fn render_sparkline(points_data: Vec<u64>, stroke_color: Hsla, width: f32, height: f32) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            if points_data.len() < 2 {
                return;
            }
            let min_v = *points_data.iter().min().unwrap_or(&0) as f64;
            let max_v = *points_data.iter().max().unwrap_or(&1) as f64;
            let span = (max_v - min_v).max(1.0);

            let pad = 2.0_f32;
            let w = (f32::from(bounds.size.width) - pad * 2.0).max(1.0);
            let h = (f32::from(bounds.size.height) - pad * 2.0).max(1.0);
            let step_x = w / (points_data.len() - 1) as f32;

            let mut stroke = PathBuilder::stroke(px(1.75));
            let mut fill = PathBuilder::fill();

            let mut first_pt = point(bounds.origin.x + px(pad), bounds.origin.y + px(pad + h));
            let mut last_x = bounds.origin.x + px(pad);

            for (i, &val) in points_data.iter().enumerate() {
                let norm = ((val as f64 - min_v) / span) as f32;
                let x = bounds.origin.x + px(pad + i as f32 * step_x);
                let y = bounds.origin.y + px(pad + h * (1.0 - norm));
                let pt = point(x, y);
                if i == 0 {
                    stroke.move_to(pt);
                    fill.move_to(point(x, bounds.origin.y + px(pad + h)));
                    fill.line_to(pt);
                    first_pt = pt;
                } else {
                    stroke.line_to(pt);
                    fill.line_to(pt);
                }
                last_x = x;
            }

            fill.line_to(point(last_x, bounds.origin.y + px(pad + h)));
            fill.line_to(point(first_pt.x, bounds.origin.y + px(pad + h)));
            fill.close();

            if let Ok(path) = fill.build() {
                window.paint_path(path, stroke_color.opacity(0.16));
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, stroke_color);
            }
        },
    )
    .w(px(width))
    .h(px(height))
}
