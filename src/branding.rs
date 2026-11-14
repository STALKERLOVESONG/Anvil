//! Logo, window icon and the "ANVIL" title in OCR A Extended.
//!
//! The font is a licensed Microsoft/Monotype font, so it isn't shipped: it's
//! loaded from Windows' own font folder at startup, and the title falls back
//! to the normal UI font on PCs that don't have it.

use crate::app::AnvilApp;
use eframe::egui::{self, Color32, FontData, FontFamily, RichText, TextureHandle, TextureOptions};
use std::sync::Arc;

const LOGO_PNG: &[u8] = include_bytes!("../assets/anvil_logo.png");
pub const TITLE_FONT: &str = "ocr-a-extended";

fn font_paths() -> Vec<std::path::PathBuf> {
    let mut v = vec![std::path::PathBuf::from(r"C:\Windows\Fonts\OCRAEXT.TTF")];
    if let Ok(w) = std::env::var("WINDIR") {
        v.push(std::path::Path::new(&w).join("Fonts").join("OCRAEXT.TTF"));
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        v.push(std::path::Path::new(&l).join(r"Microsoft\Windows\Fonts\OCRAEXT.TTF"));
    }
    v
}

pub fn logo_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

/// Window / taskbar icon.
pub fn window_icon() -> Option<egui::IconData> {
    let (width, height, rgba) = logo_rgba()?;
    Some(egui::IconData { rgba, width, height })
}

pub(crate) struct Brand {
    pub logo: Option<TextureHandle>,
    /// True when OCR A Extended was found and registered.
    pub title_font: bool,
    pub show_about: bool,
}

impl Brand {
    pub fn new(ctx: &egui::Context) -> Brand {
        let logo = logo_rgba().map(|(w, h, px)| {
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            ctx.load_texture("anvil_logo", img, TextureOptions::LINEAR)
        });
        let mut title_font = false;
        if let Some(data) = font_paths().into_iter().find_map(|p| std::fs::read(p).ok()) {
            if ttf_parser::Face::parse(&data, 0).is_ok() {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert(TITLE_FONT.into(), Arc::new(FontData::from_owned(data)));
                fonts.families.insert(FontFamily::Name(TITLE_FONT.into()), vec![TITLE_FONT.into()]);
                ctx.set_fonts(fonts);
                title_font = true;
            }
        }
        Brand { logo, title_font, show_about: false }
    }

    /// The "ANVIL" wordmark: OCR A Extended, italic, antialiased.
    pub fn title(&self, size: f32) -> RichText {
        let t = RichText::new("ANVIL").size(size).italics().color(Color32::from_rgb(214, 226, 255));
        if self.title_font {
            t.family(FontFamily::Name(TITLE_FONT.into()))
        } else {
            t.strong()
        }
    }
}

impl AnvilApp {
    /// Logo + title at the left of the menu bar (click for About).
    pub(crate) fn brand_header(&mut self, ui: &mut egui::Ui) {
        let mut clicked = false;
        if let Some(t) = &self.brand.logo {
            clicked |= ui.add(egui::Image::new((t.id(), egui::vec2(20.0, 20.0))).sense(egui::Sense::click())).on_hover_text("About Anvil").clicked();
        }
        clicked |= ui.add(egui::Label::new(self.brand.title(17.0)).sense(egui::Sense::click())).clicked();
        if clicked {
            self.brand.show_about = !self.brand.show_about;
        }
        ui.separator();
    }

    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.brand.show_about;
        egui::Window::new("About Anvil").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(t) = &self.brand.logo {
                    ui.add(egui::Image::new((t.id(), egui::vec2(128.0, 128.0))));
                }
                ui.label(self.brand.title(34.0));
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(4.0);
                ui.label(RichText::new("A Hammer-style level editor written in Rust.\nExports to Unity, Roblox Studio and Blender-friendly files.").weak());
                if !self.brand.title_font {
                    ui.label(RichText::new("(OCR A Extended isn't installed, so the title uses the normal font.)").small().weak());
                }
            });
            self.update_about_ui(ui);
        });
        self.brand.show_about = open;
    }
}
