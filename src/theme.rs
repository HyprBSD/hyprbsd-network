//! HyprBSD's look: colours, fonts and egui's style.

use crate::paths::FONTS;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme,
};
use std::path::Path;
use std::sync::Arc;

pub const BACKGROUND: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
pub const SURFACE: Color32 = Color32::from_rgb(0x2b, 0x2f, 0x38);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x33, 0x38, 0x43);
pub const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe6, 0xeb);
pub const DIM: Color32 = Color32::from_rgb(0x8b, 0x91, 0x9c);
pub const FAINT: Color32 = Color32::from_rgb(0x4b, 0x50, 0x5c);
pub const ORANGE: Color32 = Color32::from_rgb(0xef, 0x6f, 0x3c);
pub const TEAL: Color32 = Color32::from_rgb(0x3c, 0xc0, 0xb0);
pub const RED: Color32 = Color32::from_rgb(0xe0, 0x55, 0x61);

pub const RADIUS: u8 = 6;

fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts(Path::new(FONTS)));
    ctx.set_theme(Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles.extend([
            (TextStyle::Small, body(13.0)),
            (TextStyle::Body, body(15.0)),
            (TextStyle::Button, body(15.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.scroll = egui::style::ScrollStyle::solid();

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.weak_text_color = Some(DIM);
        v.extreme_bg_color = BACKGROUND;
        v.text_edit_bg_color = Some(BACKGROUND);
        v.warn_fg_color = ORANGE;
        v.error_fg_color = RED;
        v.panel_fill = BACKGROUND;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, FAINT);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.selection.bg_fill = ORANGE.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.text_cursor.stroke = Stroke::new(2.0, ORANGE);
        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.noninteractive, BACKGROUND),
            (&mut w.inactive, SURFACE),
            (&mut w.hovered, SURFACE_HOVER),
            (&mut w.active, FAINT),
            (&mut w.open, SURFACE_HOVER),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(RADIUS);
            state.expansion = 0.0;
        }
        w.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
        w.inactive.bg_stroke = Stroke::new(1.0, FAINT);
    });
}

/// egui's fonts, with Noto Sans first when it's installed.
fn fonts(dir: &Path) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut add = |name: &str, file: &str, family: FontFamily| match std::fs::read(dir.join(file)) {
        Ok(bytes) => {
            fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.to_owned());
        }
        Err(e) => eprintln!("hyprbsd-network: {}: {e}", dir.join(file).display()),
    };
    add("noto-sans", "NotoSans-Regular.ttf", FontFamily::Proportional);
    add("noto-sans-semibold", "NotoSans-SemiBold.ttf", semibold_family());
    fonts.families.entry(semibold_family()).or_default().extend(fallbacks);
    fonts
}
