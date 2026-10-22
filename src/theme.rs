//! Colours, widget style, pixel font and the little neon padlock window icon.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(2, 5, 3);
pub const PANEL: Color32 = Color32::from_rgb(6, 11, 8);
pub const PANEL_2: Color32 = Color32::from_rgb(9, 16, 11);
pub const PANEL_3: Color32 = Color32::from_rgb(13, 24, 16);
pub const BORDER: Color32 = Color32::from_rgb(28, 58, 40);
pub const NEON: Color32 = Color32::from_rgb(57, 255, 20);
pub const NEON_DIM: Color32 = Color32::from_rgb(27, 112, 30);
pub const NEON_BG: Color32 = Color32::from_rgb(9, 42, 16);
pub const CYAN: Color32 = Color32::from_rgb(0, 229, 255);
pub const AMBER: Color32 = Color32::from_rgb(255, 212, 0);
pub const AMBER_BG: Color32 = Color32::from_rgb(35, 28, 0);
pub const RED: Color32 = Color32::from_rgb(255, 59, 59);
pub const RED_BG: Color32 = Color32::from_rgb(38, 0, 0);
pub const TEXT: Color32 = Color32::from_rgb(201, 255, 212);
pub const MUTED: Color32 = Color32::from_rgb(111, 143, 120);
pub const DIM: Color32 = Color32::from_rgb(63, 90, 72);

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);

    // Press Start 2P (SIL OFL 1.1) for the keygen/cracktro look.  It has no
    // CJK glyphs, so the default families remain as fallbacks for Chinese.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "press_start".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/PressStart2P-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "ark_pixel".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/ark-pixel-10px-monospaced-zh_hans.ttf"
        ))),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            // Ark Pixel has sane 10px metrics and full CJK; Press Start 2P is
            // reserved for the display family because its bitmap glyphs are
            // taller than its reported line height at small sizes.
            list.insert(0, "ark_pixel".to_owned());
        }
    }
    fonts.families.insert(
        FontFamily::Name("press".into()),
        vec!["press_start".to_owned(), "ark_pixel".to_owned()],
    );
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, NEON_DIM);
    v.window_corner_radius = CornerRadius::same(0);
    v.menu_corner_radius = CornerRadius::same(0);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.extreme_bg_color = Color32::from_rgb(1, 3, 2);
    v.faint_bg_color = PANEL_2;
    v.code_bg_color = Color32::from_rgb(2, 7, 4);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = CYAN;
    v.warn_fg_color = AMBER;
    v.error_fg_color = RED;

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(0);

    v.widgets.inactive.bg_fill = PANEL_2;
    v.widgets.inactive.weak_bg_fill = PANEL_2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = CornerRadius::same(0);

    v.widgets.hovered.bg_fill = NEON_BG;
    v.widgets.hovered.weak_bg_fill = NEON_BG;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, NEON);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, NEON);
    v.widgets.hovered.corner_radius = CornerRadius::same(0);

    v.widgets.active.bg_fill = NEON;
    v.widgets.active.weak_bg_fill = NEON;
    v.widgets.active.bg_stroke = Stroke::new(1.0, NEON);
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    v.widgets.active.corner_radius = CornerRadius::same(0);

    v.selection.bg_fill = NEON_BG;
    v.selection.stroke = Stroke::new(1.0, NEON);

    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        style.spacing.item_spacing = egui::vec2(7.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 5.0);
        style.spacing.interact_size = egui::vec2(34.0, 24.0);
        style.spacing.slider_width = 96.0;
        style.spacing.scroll.bar_width = 5.0;

        // Everything is monospace/pixel; CJK falls through to the default
        // font stack because Press Start 2P has no Chinese glyphs.
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::new(20.0, FontFamily::Monospace));
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(11.5, FontFamily::Monospace));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(11.0, FontFamily::Monospace));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::new(9.5, FontFamily::Monospace));
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(10.5, FontFamily::Monospace),
        );
    });
}

/// 64x64 RGBA padlock, drawn procedurally so the binary stays asset-free.
pub fn make_icon() -> egui::IconData {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let neon = [57u8, 255, 20];
    let dark = [3u8, 6, 4];
    for y in 0..N {
        for x in 0..N {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let mut c = [0u8, 0, 0, 0];
            // square dark tile
            let in_tile = (4.0..60.0).contains(&fx) && (4.0..60.0).contains(&fy);
            if in_tile {
                c = [dark[0], dark[1], dark[2], 255];
            }
            // shackle: ring segment above the body
            let d = ((fx - 32.0).powi(2) + (fy - 27.0).powi(2)).sqrt();
            if (10.5..=14.5).contains(&d) && fy < 31.0 && fy > 12.0 {
                c = [neon[0], neon[1], neon[2], 255];
            }
            // body
            if (18.0..=46.0).contains(&fx) && (30.0..=53.0).contains(&fy) {
                c = [neon[0], neon[1], neon[2], 255];
            }
            // keyhole
            let kd = ((fx - 32.0).powi(2) + (fy - 39.0).powi(2)).sqrt();
            if kd <= 4.0 || ((29.5..=34.5).contains(&fx) && (39.0..=48.0).contains(&fy)) {
                c = [dark[0], dark[1], dark[2], 255];
            }
            // hard outline
            if in_tile && (fx < 5.0 || fy < 5.0 || fx > 59.0 || fy > 59.0) {
                c = [28, 58, 40, 255];
            }
            let i = ((y * N + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&c);
        }
    }
    egui::IconData {
        rgba,
        width: N,
        height: N,
    }
}
