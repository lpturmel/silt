//! Adaptive macOS Dark / Light theme palette and Creep Heatmap color scale.

use gpui::{Hsla, WindowAppearance, rgb, rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePreference {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
pub struct TilePalette {
    pub bg: Hsla,
    pub title: Hsla,
    pub subtitle: Hsla,
    pub delta: Hsla,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub is_dark: bool,
    pub bg_window: Hsla,
    pub bg_sidebar: Hsla,
    pub bg_surface: Hsla,
    pub bg_surface_hover: Hsla,
    pub bg_elevated: Hsla,
    pub border: Hsla,
    pub border_subtle: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_on_accent: Hsla,
    pub text_on_good: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub used_color: Hsla,
    pub purgeable_color: Hsla,
    pub free_color: Hsla,
    pub creep_warm: Hsla,
    pub creep_hot: Hsla,
    pub good_green: Hsla,
    pub warning_amber: Hsla,
    pub danger_red: Hsla,
}

impl Theme {
    pub fn resolve(pref: ThemePreference, appearance: WindowAppearance) -> Self {
        let is_dark = match pref {
            ThemePreference::Dark => true,
            ThemePreference::Light => false,
            ThemePreference::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        if is_dark { Self::dark() } else { Self::light() }
    }

    pub fn dark() -> Self {
        Self {
            is_dark: true,
            bg_window: rgb(0x111318).into(),
            bg_sidebar: rgb(0x161920).into(),
            bg_surface: rgb(0x1b1f28).into(),
            bg_surface_hover: rgb(0x232834).into(),
            bg_elevated: rgb(0x252b38).into(),
            border: rgb(0x2c3342).into(),
            border_subtle: rgb(0x212632).into(),
            text_primary: rgb(0xf1f5f9).into(),
            text_secondary: rgb(0x94a3b8).into(),
            text_muted: rgb(0x64748b).into(),
            text_on_accent: rgb(0x0b1220).into(),
            text_on_good: rgb(0x052e16).into(),
            accent: rgb(0x38bdf8).into(),
            accent_soft: rgba(0x38bdf826).into(),
            used_color: rgb(0x6366f1).into(),
            purgeable_color: rgb(0xf59e0b).into(),
            free_color: rgb(0x10b981).into(),
            creep_warm: rgb(0xf59e0b).into(),
            creep_hot: rgb(0xf43f5e).into(),
            good_green: rgb(0x22c55e).into(),
            warning_amber: rgb(0xf59e0b).into(),
            danger_red: rgb(0xef4444).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            is_dark: false,
            bg_window: rgb(0xf4f6f9).into(),
            bg_sidebar: rgb(0xeaeef4).into(),
            bg_surface: rgb(0xffffff).into(),
            bg_surface_hover: rgb(0xf1f5f9).into(),
            bg_elevated: rgb(0xf8fafc).into(),
            border: rgb(0xdbe2ec).into(),
            border_subtle: rgb(0xe7ecf3).into(),
            text_primary: rgb(0x0f172a).into(),
            text_secondary: rgb(0x475569).into(),
            text_muted: rgb(0x64748b).into(),
            text_on_accent: rgb(0xffffff).into(),
            text_on_good: rgb(0xffffff).into(),
            accent: rgb(0x0284c7).into(),
            accent_soft: rgba(0x0284c71f).into(),
            used_color: rgb(0x4f46e5).into(),
            purgeable_color: rgb(0xd97706).into(),
            free_color: rgb(0x059669).into(),
            creep_warm: rgb(0xd97706).into(),
            creep_hot: rgb(0xe11d48).into(),
            good_green: rgb(0x16a34a).into(),
            warning_amber: rgb(0xd97706).into(),
            danger_red: rgb(0xdc2626).into(),
        }
    }

    /// Map a category string to a harmonious, readable tile fill color for the Treemap.
    pub fn category_color(&self, category: &str) -> Hsla {
        match category {
            "Applications" => {
                if self.is_dark { rgb(0x3b4d8f).into() } else { rgb(0x6366f1).into() }
            }
            "Developer" => {
                if self.is_dark { rgb(0x1e5c6b).into() } else { rgb(0x0891b2).into() }
            }
            "Containers & VMs" => {
                if self.is_dark { rgb(0x5b3d8a).into() } else { rgb(0x7c3aed).into() }
            }
            "AI Models" => {
                if self.is_dark { rgb(0x7c3264).into() } else { rgb(0xc026d3).into() }
            }
            "Downloads" => {
                if self.is_dark { rgb(0x7c5321).into() } else { rgb(0xd97706).into() }
            }
            "Media" => {
                if self.is_dark { rgb(0x1e6049).into() } else { rgb(0x059669).into() }
            }
            "Orphaned Apps" => {
                if self.is_dark { rgb(0x7f2d38).into() } else { rgb(0xe11d48).into() }
            }
            "Backups" => {
                if self.is_dark { rgb(0x5a4c28).into() } else { rgb(0xca8a04).into() }
            }
            "macOS System" => {
                if self.is_dark { rgb(0x475569).into() } else { rgb(0x64748b).into() }
            }
            "User Files" => {
                if self.is_dark { rgb(0x0f6b6e).into() } else { rgb(0x0d9488).into() }
            }
            "Documents" => {
                if self.is_dark { rgb(0x1d4ed8).into() } else { rgb(0x2563eb).into() }
            }
            _ => {
                if self.is_dark { rgb(0x334155).into() } else { rgb(0x475569).into() }
            }
        }
    }

    /// Return high-contrast tile fill + label palette for Category treemap mode.
    pub fn category_tile_palette(&self, category: &str) -> TilePalette {
        let bg = self.category_color(category);
        TilePalette {
            bg,
            title: rgb(0xf8fafc).into(),
            subtitle: rgba(0xf8fafcd9).into(),
            delta: rgba(0xfef08af0).into(),
        }
    }

    /// Return high-contrast tile fill + label palette for Creep Heatmap mode in both Dark & Light themes.
    pub fn creep_heatmap_tile_palette(&self, delta_bytes: i64, max_positive_delta: i64) -> TilePalette {
        let bg = self.creep_heatmap_color(delta_bytes, max_positive_delta);
        if !self.is_dark && delta_bytes >= 0 {
            let ratio = if delta_bytes == 0 || max_positive_delta <= 0 {
                0.0
            } else {
                ((delta_bytes as f64 + 1.0).ln() / (max_positive_delta as f64 + 1.0).ln())
                    .clamp(0.0, 1.0)
            };
            // Light slate / amber tiles need dark ink for strong WCAG contrast
            if ratio < 0.65 {
                return TilePalette {
                    bg,
                    title: rgb(0x0f172a).into(),
                    subtitle: rgb(0x1e293b).into(),
                    delta: rgb(0x7c2d12).into(),
                };
            }
        }
        TilePalette {
            bg,
            title: rgb(0xf8fafc).into(),
            subtitle: rgba(0xf8fafcd9).into(),
            delta: rgba(0xfef08af0).into(),
        }
    }

    /// Novel Creep Heatmap scale: maps growth intensity `0.0..=1.0` (and negative shrinking)
    /// to cool slate (static) -> amber (creeping) -> crimson (surging), or teal-green if shrinking.
    pub fn creep_heatmap_color(&self, delta_bytes: i64, max_positive_delta: i64) -> Hsla {
        if delta_bytes < 0 {
            return if self.is_dark {
                rgb(0x14533e).into()
            } else {
                rgb(0x10b981).into()
            };
        }
        if delta_bytes == 0 || max_positive_delta <= 0 {
            return if self.is_dark {
                rgb(0x242c3a).into()
            } else {
                rgb(0xcbd5e1).into()
            };
        }
        // Log-scaled intensity so moderate creep (e.g., 500 MB) is still clearly visible
        // alongside a huge 20 GB spike
        let ratio = ((delta_bytes as f64 + 1.0).ln() / (max_positive_delta as f64 + 1.0).ln())
            .clamp(0.0, 1.0);
        if ratio < 0.35 {
            if self.is_dark { rgb(0x2f3b52).into() } else { rgb(0x94a3b8).into() }
        } else if ratio < 0.65 {
            if self.is_dark { rgb(0x854d0e).into() } else { rgb(0xf59e0b).into() }
        } else if ratio < 0.85 {
            if self.is_dark { rgb(0x9a3412).into() } else { rgb(0xea580c).into() }
        } else if self.is_dark {
            rgb(0x9f1239).into()
        } else {
            rgb(0xe11d48).into()
        }
    }
}
