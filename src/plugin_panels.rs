//! Windows for "panel" plugins: Anvil draws the widgets a plugin describes
//! (labels, inputs, buttons, images, links) and sends clicks / typing back.
//! Images are fetched in the background, only from the plugin's own hosts.

use crate::app::AnvilApp;
use crate::plugin_web::{url_allowed, Widget};
use crate::plugins::Outcome;
use eframe::egui::{self, RichText, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc::{self, Receiver};

pub(crate) enum WebImage {
    Loading(Receiver<Option<egui::ColorImage>>),
    Ready(TextureHandle),
    Failed,
}

#[derive(Default)]
pub(crate) struct PanelState {
    pub images: HashMap<String, WebImage>,
}

fn fetch(url: String, hosts: Vec<String>) -> Receiver<Option<egui::ColorImage>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let img = (|| {
            if !url_allowed(&url, &hosts) {
                return None;
            }
            let resp = ureq::get(&url).set("User-Agent", "Anvil-plugin").timeout(std::time::Duration::from_secs(20)).call().ok()?;
            if !url_allowed(resp.get_url(), &hosts) {
                return None;
            }
            let mut b = Vec::new();
            resp.into_reader().take(6 * 1024 * 1024).read_to_end(&mut b).ok()?;
            let mut r = image::ImageReader::new(std::io::Cursor::new(b)).with_guessed_format().ok()?;
            r.limits(crate::texture::image_limits());
            let img = r.decode().ok()?.thumbnail(256, 256).to_rgba8();
            Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
        })();
        let _ = tx.send(img);
    });
    rx
}

impl AnvilApp {
    fn panel_image(&mut self, ctx: &egui::Context, url: &str, hosts: &[String]) -> Option<egui::TextureId> {
        let st = self.panel_state.images.entry(url.to_string()).or_insert_with(|| WebImage::Loading(fetch(url.to_string(), hosts.to_vec())));
        match st {
            WebImage::Ready(t) => Some(t.id()),
            WebImage::Failed => None,
            WebImage::Loading(rx) => match rx.try_recv() {
                Ok(Some(img)) => {
                    let t = ctx.load_texture(format!("plugin_img:{url}"), img, TextureOptions::LINEAR);
                    let id = t.id();
                    *st = WebImage::Ready(t);
                    Some(id)
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    *st = WebImage::Failed;
                    None
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    None
                }
            },
        }
    }

    fn draw_widgets(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, ws: &[Widget], hosts: &[String], events: &mut Vec<(String, String)>) {
        for w in ws {
            match w {
                Widget::Label { text, strong, small } => {
                    let mut t = RichText::new(text);
                    if *strong {
                        t = t.strong();
                    }
                    if *small {
                        t = t.small();
                    }
                    ui.add(egui::Label::new(t).wrap());
                }
                Widget::Input { id, value, hint, password } => {
                    let mut v = value.clone();
                    if ui.add(egui::TextEdit::singleline(&mut v).hint_text(hint.as_str()).password(*password).desired_width(220.0)).changed() {
                        events.push((id.clone(), v));
                    }
                }
                Widget::Button { id, text } => {
                    if ui.button(text).clicked() {
                        events.push((id.clone(), String::new()));
                    }
                }
                Widget::Image { url, w, h, id } => match self.panel_image(ctx, url, hosts) {
                    Some(t) => {
                        let r = ui.add(egui::Image::new((t, egui::vec2(*w, *h))).sense(egui::Sense::click()));
                        if let (true, Some(id)) = (r.clicked(), id) {
                            events.push((id.clone(), String::new()));
                        }
                    }
                    None => {
                        ui.add_sized([*w, *h], egui::Spinner::new());
                    }
                },
                Widget::Link { text, url } => {
                    if url.starts_with("https://") {
                        ui.hyperlink_to(text, url);
                    } else {
                        ui.label(text);
                    }
                }
                Widget::Progress(p) => {
                    ui.add(egui::ProgressBar::new(*p).desired_width(200.0));
                }
                Widget::Separator => {
                    ui.separator();
                }
                Widget::Row(items) => {
                    let items = items.clone();
                    ui.horizontal_wrapped(|ui| self.draw_widgets(ui, ctx, &items, hosts, events));
                }
            }
        }
    }

    fn apply_plugin_imports(&mut self, out: &Outcome) {
        for (path, name) in &out.imports {
            if !self.settings.model_library.iter().any(|e| e.path == *path) {
                self.settings.model_library.push(crate::settings::ModelEntry { path: path.clone(), name: name.clone(), ..Default::default() });
            }
            self.models.forget(path);
            self.forget_thumb(path);
            self.active_model = Some(path.clone());
            self.tool = crate::app::Tool::Prop;
            self.status = format!("Added {} to the model library. Click in a view to place it.", if name.is_empty() { path } else { name });
        }
        if !out.imports.is_empty() {
            let _ = self.settings.save();
        }
    }

    /// Draws every open panel plugin window.
    pub(crate) fn plugin_panels(&mut self, ctx: &egui::Context) {
        let open = self.open_panels.clone();
        for file in open {
            let Some(idx) = self.plugins.plugins.iter().position(|p| p.file == file && p.kind == crate::plugins::PluginKind::Panel) else {
                self.open_panels.retain(|f| *f != file);
                continue;
            };
            let (name, hosts) = (self.plugins.plugins[idx].name.clone(), self.plugins.plugins[idx].hosts.clone());
            let (widgets, out) = self.plugins.panel(idx, &self.map, &self.models, self.settings.grid);
            self.apply_plugin_imports(&out);
            if let Some(s) = out.status {
                self.status = s;
            }
            let mut events = Vec::new();
            let mut keep = true;
            egui::Window::new(name).id(egui::Id::new(("plugin_panel", &file))).open(&mut keep).default_width(560.0).default_height(560.0).vscroll(true).show(ctx, |ui| {
                self.draw_widgets(ui, ctx, &widgets, &hosts, &mut events);
            });
            for (id, value) in events {
                let out = self.plugins.panel_event(idx, &self.map, &self.models, self.settings.grid, &id, &value);
                self.apply_plugin_imports(&out);
                if let Some(s) = out.status {
                    self.status = s;
                }
            }
            if !keep {
                self.open_panels.retain(|f| *f != file);
            }
            // panels poll downloads etc.
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
    }
}
