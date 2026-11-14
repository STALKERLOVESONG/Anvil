//! UI themes: dark (default), light, Hammer grey, midnight blue and high
//! contrast, each with a pickable accent color.

use eframe::egui::{self, Color32, Stroke, Visuals};

pub const THEME_NAMES: [&str; 5] = ["Dark", "Light", "Hammer grey", "Midnight blue", "High contrast"];

fn c(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// Panel / window / widget fills for a theme.
fn shade(v: &mut Visuals, panel: [u8; 3], window: [u8; 3], extreme: [u8; 3], widget: [u8; 3], hover: [u8; 3], active: [u8; 3]) {
    v.panel_fill = c(panel);
    v.window_fill = c(window);
    v.extreme_bg_color = c(extreme);
    v.faint_bg_color = c(window).gamma_multiply(1.04);
    v.widgets.noninteractive.bg_fill = c(window);
    v.widgets.noninteractive.weak_bg_fill = c(window);
    for (wv, col) in [(&mut v.widgets.inactive, widget), (&mut v.widgets.hovered, hover), (&mut v.widgets.active, active), (&mut v.widgets.open, hover)] {
        wv.bg_fill = c(col);
        wv.weak_bg_fill = c(col);
    }
}

pub fn visuals(theme: u8, accent: [u8; 3]) -> Visuals {
    let mut v = match theme {
        1 => Visuals::light(),
        _ => Visuals::dark(),
    };
    match theme {
        2 => shade(&mut v, [58, 58, 58], [66, 66, 66], [38, 38, 38], [84, 84, 84], [100, 100, 100], [118, 118, 118]),
        3 => shade(&mut v, [20, 26, 40], [26, 33, 52], [12, 16, 26], [38, 48, 72], [50, 62, 92], [62, 76, 110]),
        4 => {
            shade(&mut v, [0, 0, 0], [8, 8, 8], [0, 0, 0], [28, 28, 28], [50, 50, 50], [70, 70, 70]);
            v.override_text_color = Some(Color32::WHITE);
            for wv in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
                wv.bg_stroke = Stroke::new(1.0f32, Color32::from_gray(200));
            }
            v.window_stroke = Stroke::new(1.5f32, Color32::WHITE);
        }
        _ => {}
    }
    let a = c(accent);
    v.selection.bg_fill = a.gamma_multiply(if theme == 1 { 0.55 } else { 0.75 });
    v.selection.stroke = Stroke::new(1.0f32, if theme == 1 { Color32::BLACK } else { Color32::WHITE });
    v.hyperlink_color = a;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0f32, a);
    v.widgets.active.bg_stroke = Stroke::new(1.5f32, a);
    v
}

/// Applies the theme when it changed since last time.
pub fn apply(ctx: &egui::Context, theme: u8, accent: [u8; 3], last: &mut Option<(u8, [u8; 3])>) {
    if *last == Some((theme, accent)) {
        return;
    }
    ctx.set_visuals(visuals(theme, accent));
    *last = Some((theme, accent));
}
