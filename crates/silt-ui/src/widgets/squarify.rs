//! Bruls-Huizing-van Wijk Squarified Treemap layout algorithm.
//!
//! Produces low-aspect-ratio proportional rectangles (`0.0..=1.0` normalized coordinates)
//! so the GPUI Space Map renders cleanly at any window size without overlap.

#[derive(Debug, Clone)]
pub struct TreemapInput<T> {
    pub value: u64,
    pub payload: T,
}

#[derive(Debug, Clone)]
pub struct TreemapRect<T> {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub payload: T,
}

/// Lay out `items` into proportional squarified rectangles filling `[0..width, 0..height]`.
pub fn squarify<T: Clone>(
    items: &[TreemapInput<T>],
    width: f32,
    height: f32,
) -> Vec<TreemapRect<T>> {
    if items.is_empty() || width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }

    let mut positive: Vec<&TreemapInput<T>> = items.iter().filter(|i| i.value > 0).collect();
    if positive.is_empty() {
        return Vec::new();
    }
    positive.sort_by_key(|i| std::cmp::Reverse(i.value));

    let total: f64 = positive.iter().map(|i| i.value as f64).sum();
    if total <= 0.0 {
        return Vec::new();
    }

    let total_area = (width as f64) * (height as f64);
    let areas: Vec<(f64, &TreemapInput<T>)> = positive
        .into_iter()
        .map(|i| ((i.value as f64 / total) * total_area, i))
        .collect();

    let mut out = Vec::with_capacity(areas.len());
    let mut rx = 0.0_f64;
    let mut ry = 0.0_f64;
    let mut rw = width as f64;
    let mut rh = height as f64;

    let mut idx = 0;
    while idx < areas.len() {
        let short_side = rw.min(rh).max(1e-6);
        let mut row: Vec<(f64, &TreemapInput<T>)> = vec![areas[idx]];
        idx += 1;

        while idx < areas.len() {
            let current_worst = worst_aspect(&row, short_side);
            let mut candidate = row.clone();
            candidate.push(areas[idx]);
            let next_worst = worst_aspect(&candidate, short_side);
            if next_worst <= current_worst {
                row.push(areas[idx]);
                idx += 1;
            } else {
                break;
            }
        }

        let row_sum: f64 = row.iter().map(|(a, _)| *a).sum();
        if rw >= rh {
            // Vertical strip on the left
            let strip_w = (row_sum / rh.max(1e-6)).min(rw);
            let mut cy = ry;
            for (area, item) in &row {
                let tile_h = (*area / strip_w.max(1e-6)).min((ry + rh - cy).max(0.0));
                out.push(TreemapRect {
                    x: rx as f32,
                    y: cy as f32,
                    w: strip_w as f32,
                    h: tile_h as f32,
                    payload: item.payload.clone(),
                });
                cy += tile_h;
            }
            rx += strip_w;
            rw = (rw - strip_w).max(0.0);
        } else {
            // Horizontal strip along the top
            let strip_h = (row_sum / rw.max(1e-6)).min(rh);
            let mut cx = rx;
            for (area, item) in &row {
                let tile_w = (*area / strip_h.max(1e-6)).min((rx + rw - cx).max(0.0));
                out.push(TreemapRect {
                    x: cx as f32,
                    y: ry as f32,
                    w: tile_w as f32,
                    h: strip_h as f32,
                    payload: item.payload.clone(),
                });
                cx += tile_w;
            }
            ry += strip_h;
            rh = (rh - strip_h).max(0.0);
        }
    }

    out
}

fn worst_aspect<T>(row: &[(f64, &TreemapInput<T>)], short_side: f64) -> f64 {
    let sum: f64 = row.iter().map(|(a, _)| *a).sum();
    if sum <= 0.0 {
        return f64::INFINITY;
    }
    let min_a = row
        .iter()
        .map(|(a, _)| *a)
        .fold(f64::INFINITY, f64::min)
        .max(1e-9);
    let max_a = row.iter().map(|(a, _)| *a).fold(0.0_f64, f64::max);
    let s2 = short_side * short_side;
    let sum2 = sum * sum;
    ((s2 * max_a) / sum2).max(sum2 / (s2 * min_a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_fills_bounds_proportionally() {
        let inputs = vec![
            TreemapInput { value: 600, payload: "A" },
            TreemapInput { value: 300, payload: "B" },
            TreemapInput { value: 100, payload: "C" },
        ];
        let rects = squarify(&inputs, 1000.0, 500.0);
        assert_eq!(rects.len(), 3);
        let total_area: f32 = rects.iter().map(|r| r.w * r.h).sum();
        assert!((total_area - 500_000.0).abs() < 10.0);
        for r in &rects {
            assert!(r.x >= 0.0 && r.y >= 0.0);
            assert!(r.x + r.w <= 1000.5 && r.y + r.h <= 500.5);
        }
    }
}
