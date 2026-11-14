//! "Content" window: mount Half-Life / Garry's Mod / Source game content,
//! browse its props and materials and import them into the map.

use crate::app::{AnvilApp, Tool};
use crate::map::TextureDef;
use crate::mount::{self, Mounts};
use crate::settings::ModelEntry;
use eframe::egui::{self, RichText};
use std::sync::mpsc::channel;
use std::sync::Arc;

impl AnvilApp {
    pub(crate) fn open_content(&mut self) {
        self.show_content = true;
        if self.settings.mounts.is_empty() {
            let found = mount::detect_steam();
            if !found.is_empty() {
                self.settings.mounts = found;
                let _ = self.settings.save();
            }
        }
        if self.mounts.is_none() && self.mounts_rx.is_none() {
            self.start_mount_scan();
        }
    }

    pub(crate) fn start_mount_scan(&mut self) {
        let paths = self.settings.mounts.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Mounts::build(&paths));
        });
        self.mounts_rx = Some(rx);
        self.content_sel = None;
        self.content_preview = None;
    }

    pub(crate) fn poll_mounts(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.mounts_rx else { return };
        match rx.try_recv() {
            Ok(m) => {
                self.status = format!("Mounted {} models and {} materials", m.models.len(), m.materials.len());
                self.mounts = Some(Arc::new(m));
                self.mounts_rx = None;
                self.content_key.2 = usize::MAX; // refresh the list
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            Err(_) => self.mounts_rx = None,
        }
    }

    fn import_model(&mut self, rel: &str) {
        let Some(m) = self.mounts.clone() else { return };
        match mount::import_model(&m, rel, self.settings.source_scale) {
            Ok(obj) => {
                let path = obj.to_string_lossy().to_string();
                self.models.forget(&path);
                self.forget_thumb(&path);
                self.scene_cache.clear();
                if !self.settings.model_library.iter().any(|e| e.path == path) {
                    self.settings.model_library.push(ModelEntry { path: path.clone(), ..Default::default() });
                    let _ = self.settings.save();
                }
                self.active_model = Some(path);
                self.tool = Tool::Prop;
                self.status = format!("Imported {rel}. Click in a viewport to place it.");
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    fn import_material(&mut self, rel: &str, apply: bool, decal: bool) {
        let Some(m) = self.mounts.clone() else { return };
        match mount::import_material(&m, rel) {
            Ok(f) => {
                self.push_undo();
                let id = match self.map.textures.iter().find(|t| t.image == f.image) {
                    Some(t) => t.id,
                    None => {
                        let mut t = TextureDef::new(f.name.clone(), f.image.clone());
                        t.normal_map = f.normal.clone();
                        self.map.add_texture(t)
                    }
                };
                self.active_texture = Some(id);
                if apply {
                    for sid in self.selection.clone() {
                        if let Some(o) = self.map.get_mut(sid) {
                            if o.texture.is_none() {
                                o.color = [255, 255, 255];
                            }
                            o.texture = Some(id);
                        }
                    }
                }
                if decal {
                    self.tool = Tool::Decal;
                }
                self.status = format!("Imported material {rel}{}", if f.translucent { " (transparent: good as a decal)" } else { "" });
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    pub(crate) fn content_window(&mut self, ctx: &egui::Context) {
        if !self.show_content {
            return;
        }
        // keep the filtered list in sync with the search box / tab
        let total = self.mounts.as_ref().map(|m| m.models.len() + m.materials.len()).unwrap_or(0);
        let key = (self.content_search.to_ascii_lowercase(), self.content_tab_models, total);
        if key != self.content_key {
            let words: Vec<String> = key.0.split_whitespace().map(|s| s.to_string()).collect();
            self.content_filtered = match &self.mounts {
                Some(m) => {
                    let src = if key.1 { &m.models } else { &m.materials };
                    src.iter().filter(|p| words.iter().all(|w| p.contains(w.as_str()))).cloned().collect()
                }
                None => Vec::new(),
            };
            self.content_key = key;
        }

        let mut open = true;
        let mut rescan = false;
        let mut detect = false;
        let mut add_folder = false;
        let mut add_file = false;
        let mut remove: Option<usize> = None;
        let mut import_model: Option<String> = None;
        let mut import_mat: Option<(String, bool, bool)> = None;
        let mut select: Option<String> = None;
        let scanning = self.mounts_rx.is_some();
        egui::Window::new("Half-Life / Source content").open(&mut open).default_width(560.0).default_height(640.0).show(ctx, |ui| {
            egui::CollapsingHeader::new(format!("Mounted ({})", self.settings.mounts.len())).default_open(self.settings.mounts.is_empty()).show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Mount a Steam game folder (Garry's Mod, Half-Life 2, CS:S…), a *_dir.vpk, a GMod .gma addon, \
                         or any folder with models/ and materials/ inside. Imported props become normal OBJ files in content/.",
                    )
                    .small()
                    .weak(),
                );
                for (i, m) in self.settings.mounts.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("x").on_hover_text("Unmount").clicked() {
                            remove = Some(i);
                        }
                        ui.label(RichText::new(m).monospace().small());
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Auto-detect Steam games").clicked() {
                        detect = true;
                    }
                    if ui.button("Add folder…").clicked() {
                        add_folder = true;
                    }
                    if ui.button("Add .vpk / .gma…").clicked() {
                        add_file = true;
                    }
                    if ui.button("Rescan").clicked() {
                        rescan = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Import scale (units per Source unit)");
                    if ui.add(egui::DragValue::new(&mut self.settings.source_scale).speed(0.001).range(0.001..=10.0)).changed() {
                        let _ = self.settings.save();
                    }
                })
                .response
                .on_hover_text("0.07 makes a 72-unit Half-Life character about 5 studs tall");
                if let Some(m) = &self.mounts {
                    egui::CollapsingHeader::new(format!("Sources found ({})", m.summary.len())).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt("mount_sum").max_height(120.0).show(ui, |ui| {
                            for s in &m.summary {
                                ui.label(RichText::new(s).monospace().small());
                            }
                        });
                    });
                }
            });
            ui.separator();
            if scanning {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning mounted content…");
                });
            } else if self.mounts.as_ref().is_none_or(|m| m.is_empty()) {
                ui.label("Nothing mounted yet. Use Auto-detect or add a folder above.");
            }
            ui.horizontal(|ui| {
                let (nm, nt) = self.mounts.as_ref().map(|m| (m.models.len(), m.materials.len())).unwrap_or((0, 0));
                if ui.selectable_label(self.content_tab_models, RichText::new(format!("Props ({nm})")).strong()).clicked() {
                    self.content_tab_models = true;
                }
                if ui.selectable_label(!self.content_tab_models, RichText::new(format!("Materials ({nt})")).strong()).clicked() {
                    self.content_tab_models = false;
                }
                ui.add(egui::TextEdit::singleline(&mut self.content_search).hint_text("search, e.g. props_c17 barrel").desired_width(220.0));
            });
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            let list_h = (ui.available_height() - 150.0).max(160.0);
            egui::ScrollArea::vertical().id_salt("content_list").max_height(list_h).auto_shrink([false, false]).show_rows(ui, row_h, self.content_filtered.len(), |ui, range| {
                for i in range {
                    let p = &self.content_filtered[i];
                    let sel = self.content_sel.as_deref() == Some(p.as_str());
                    let r = ui.selectable_label(sel, p);
                    if r.clicked() {
                        select = Some(p.clone());
                    }
                    if r.double_clicked() {
                        if self.content_tab_models {
                            import_model = Some(p.clone());
                        } else {
                            import_mat = Some((p.clone(), false, false));
                        }
                    }
                }
            });
            ui.separator();
            if let Some(sel) = self.content_sel.clone() {
                ui.label(RichText::new(&sel).monospace());
                if self.content_tab_models {
                    let thumb = self.source_prop_thumb(ctx, &sel);
                    ui.horizontal(|ui| {
                        match thumb {
                            Some(t) => {
                                ui.add(egui::Image::new((t, egui::vec2(96.0, 96.0))));
                            }
                            None => {
                                ui.add_sized([96.0, 96.0], egui::Label::new(RichText::new("no preview").weak()));
                            }
                        }
                        if ui.button(RichText::new("Import & place").strong()).on_hover_text("Converts it (with textures) to OBJ and picks the Prop tool").clicked() {
                            import_model = Some(sel.clone());
                        }
                        ui.label(RichText::new("double-click a prop to do the same").small().weak());
                    });
                } else {
                    ui.horizontal(|ui| {
                        if let Some((k, h)) = &self.content_preview {
                            if *k == sel {
                                ui.add(egui::Image::new((h.id(), egui::vec2(96.0, 96.0))));
                            }
                        }
                        ui.vertical(|ui| {
                            if ui.button("Import as texture").clicked() {
                                import_mat = Some((sel.clone(), false, false));
                            }
                            if ui.button("Import + apply to selection").clicked() {
                                import_mat = Some((sel.clone(), true, false));
                            }
                            if ui.button("Import + use as decal").clicked() {
                                import_mat = Some((sel.clone(), false, true));
                            }
                        });
                    });
                }
            } else {
                ui.label(RichText::new("Select a prop or material.").weak());
            }
        });
        if !open {
            self.show_content = false;
        }
        if let Some(i) = remove {
            self.settings.mounts.remove(i);
            let _ = self.settings.save();
            rescan = true;
        }
        if detect {
            let found = mount::detect_steam();
            let n = found.len();
            for f in found {
                if !self.settings.mounts.contains(&f) {
                    self.settings.mounts.push(f);
                }
            }
            let _ = self.settings.save();
            self.status = if n == 0 { "No Source games found in your Steam libraries".into() } else { format!("Found {n} Source game folder(s)") };
            rescan = true;
        }
        if add_folder {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                self.settings.mounts.push(d.to_string_lossy().to_string());
                let _ = self.settings.save();
                rescan = true;
            }
        }
        if add_file {
            if let Some(fs) = rfd::FileDialog::new().add_filter("VPK / GMA", &["vpk", "gma"]).pick_files() {
                for f in fs {
                    self.settings.mounts.push(f.to_string_lossy().to_string());
                }
                let _ = self.settings.save();
                rescan = true;
            }
        }
        if rescan && self.mounts_rx.is_none() {
            self.start_mount_scan();
        }
        if let Some(s) = select {
            if !self.content_tab_models {
                if let Some(m) = self.mounts.clone() {
                    match mount::preview_material(&m, &s, 256) {
                        Ok(img) => {
                            let ci = egui::ColorImage::from_rgba_unmultiplied([img.w as usize, img.h as usize], &img.data);
                            let h = ctx.load_texture(format!("content:{s}"), ci, egui::TextureOptions::LINEAR);
                            self.content_preview = Some((s.clone(), h));
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            self.content_sel = Some(s);
        }
        if let Some(rel) = import_model {
            self.import_model(&rel);
        }
        if let Some((rel, apply, decal)) = import_mat {
            self.import_material(&rel, apply, decal);
        }
    }
}
