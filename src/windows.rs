//! Tool windows: Settings, Textures, Plugins and Export (incl. Roblox Open Cloud).

use crate::app::{file_name, material_combo, shape_combo, vec2_edit, vec3_edit, AnvilApp, Tool};
use crate::cloud::{self, CloudMsg, Creator, Job};
use crate::map::TextureDef;
use crate::plugins::PluginKind;
use crate::settings::Settings;
use crate::texture::TexCache;
use eframe::egui::{self, RichText};
use std::collections::HashMap;

/// Small egui thumbnail for an image path.
pub(crate) fn preview(ctx: &egui::Context, cache: &mut TexCache, previews: &mut HashMap<String, egui::TextureHandle>, path: &str) -> Option<egui::TextureId> {
    if path.trim().is_empty() {
        return None;
    }
    if let Some(h) = previews.get(path) {
        return Some(h.id());
    }
    let img = cache.get(path)?;
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba);
    let h = ctx.load_texture(format!("preview:{path}"), ci, egui::TextureOptions::LINEAR);
    let id = h.id();
    previews.insert(path.to_string(), h);
    Some(id)
}

fn pick_image() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "tga"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

impl AnvilApp {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let before = self.settings.clone();
        let mut add_model = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(580.0)
            .default_height(660.0)
            .vscroll(true)
            .show(ctx, |ui| {
                add_model = settings_ui(ui, &mut self.settings);
                egui::CollapsingHeader::new("Experimental: live sync to Roblox Studio / Blender").default_open(false).show(ui, |ui| {
                    self.live_settings_ui(ui);
                });
            });
        if !open {
            self.show_settings = false;
        }
        if add_model {
            self.add_models();
        }
        if self.settings != before {
            if let Err(e) = self.settings.save() {
                self.status = e;
            }
            if self.active_preset.is_some_and(|i| i >= self.settings.presets.len()) {
                self.active_preset = None;
            }
        }
    }

    pub(crate) fn textures_window(&mut self, ctx: &egui::Context) {
        if !self.show_textures {
            return;
        }
        let mut open = true;
        let mut add = false;
        let mut apply: Option<u64> = None;
        let mut use_decal: Option<u64> = None;
        let mut delete: Option<u64> = None;
        let mut pick: Option<(u64, bool)> = None;
        let mut clear_normal: Option<u64> = None;
        let mut pbr_pick: Option<(u64, u8)> = None;
        let mut pbr_clear: Option<(u64, u8)> = None;
        let mut changed = false;
        let sel_count = self.selection.len();
        let active = self.active_texture;
        egui::Window::new("Textures").open(&mut open).default_width(500.0).default_height(560.0).vscroll(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Add texture…").clicked() {
                    add = true;
                }
                ui.label(RichText::new(format!("{sel_count} object(s) selected")).weak());
            });
            ui.label(
                RichText::new(
                    "Scale = units per tile, offset = units. Normal maps / metal export to Roblox as a MaterialVariant; \
                     plain images export as Texture faces (which support offset).",
                )
                .small()
                .weak(),
            );
            ui.separator();
            for t in &mut self.map.textures {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        if let Some(id) = preview(ctx, &mut self.textures, &mut self.tex_previews, &t.image) {
                            ui.dnd_drag_source(egui::Id::new(("texdrag_win", t.id)), crate::faces_ui::TexDrop(t.id), |ui| {
                                ui.add(egui::Image::new((id, egui::vec2(72.0, 72.0))));
                            })
                            .response
                            .on_hover_text("Drag onto an object in a view (hold Alt for just one face)");
                        } else {
                            ui.add_sized([72.0, 72.0], egui::Label::new(RichText::new("no image").weak()));
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("Name");
                                changed |= ui.text_edit_singleline(&mut t.name).changed();
                                if active == Some(t.id) {
                                    ui.label(RichText::new("(active)").strong());
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Image");
                                ui.label(RichText::new(file_name(&t.image)).monospace()).on_hover_text(&t.image);
                                if ui.small_button("…").clicked() {
                                    pick = Some((t.id, false));
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Normal map");
                                if t.normal_map.is_empty() {
                                    ui.label(RichText::new("none").weak());
                                } else {
                                    ui.label(RichText::new(file_name(&t.normal_map)).monospace()).on_hover_text(&t.normal_map);
                                    if ui.small_button("x").clicked() {
                                        clear_normal = Some(t.id);
                                    }
                                }
                                if ui.small_button("…").clicked() {
                                    pick = Some((t.id, true));
                                }
                            });
                            for (slot, label) in [(2u8, "Roughness map"), (3u8, "Metalness map")] {
                                let path = if slot == 2 { &t.roughness_map } else { &t.metalness_map };
                                ui.horizontal(|ui| {
                                    ui.label(label).on_hover_text("Roblox MaterialVariant PBR map (uploaded with the other images)");
                                    if path.is_empty() {
                                        ui.label(RichText::new("none").weak());
                                    } else {
                                        ui.label(RichText::new(file_name(path)).monospace()).on_hover_text(path.as_str());
                                        if ui.small_button("x").clicked() {
                                            pbr_clear = Some((t.id, slot));
                                        }
                                    }
                                    if ui.small_button("…").clicked() {
                                        pbr_pick = Some((t.id, slot));
                                    }
                                });
                            }
                            ui.horizontal(|ui| {
                                changed |= ui.checkbox(&mut t.metallic, "Metallic").changed();
                                egui::ComboBox::from_id_salt(("tex_alpha", t.id))
                                    .selected_text(["Solid", "Cut out", "See-through"][t.alpha.min(2) as usize])
                                    .show_ui(ui, |ui| {
                                        changed |= ui.selectable_value(&mut t.alpha, 0, "Solid").changed();
                                        changed |= ui.selectable_value(&mut t.alpha, 1, "Cut out").on_hover_text("The picture's see-through parts are holes (fences, leaves, grates)").changed();
                                        changed |= ui.selectable_value(&mut t.alpha, 2, "See-through").on_hover_text("Blended by the picture's alpha (glass, smoke)").changed();
                                    })
                                    .response
                                    .on_hover_text("Transparency of this picture");
                                ui.label("Base material");
                                changed |= material_combo(ui, ("tex_base", t.id), &mut t.base_material);
                            });
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Scale");
                        changed |= vec2_edit(ui, &mut t.scale, 0.05, Some(0.01), ["u", "v"]);
                        ui.label("Offset");
                        changed |= vec2_edit(ui, &mut t.offset, 0.05, None, ["u", "v"]);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Roblox image id");
                        changed |= ui.add(egui::TextEdit::singleline(&mut t.roblox_image_id).desired_width(110.0).hint_text("upload in Export")).changed();
                        ui.label("normal id");
                        changed |= ui.add(egui::TextEdit::singleline(&mut t.roblox_normal_id).desired_width(110.0)).changed();
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Apply to selection").clicked() {
                            apply = Some(t.id);
                        }
                        if ui.button("Use for decals").clicked() {
                            use_decal = Some(t.id);
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(t.id);
                        }
                    });
                });
            }
            if self.map.textures.is_empty() {
                ui.label(RichText::new("No textures yet. Click 'Add texture…' and pick an image.").weak());
            }
        });
        if !open {
            self.show_textures = false;
        }
        if changed {
            self.dirty = true;
        }
        if add {
            if let Some(path) = pick_image() {
                self.push_undo();
                let name = std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("texture").to_string();
                let id = self.map.add_texture(TextureDef::new(name, path));
                self.active_texture = Some(id);
            }
        }
        if let Some((id, normal)) = pick {
            if let Some(path) = pick_image() {
                self.push_undo();
                self.textures.forget(&path);
                self.tex_previews.remove(&path);
                if let Some(t) = self.map.textures.iter_mut().find(|t| t.id == id) {
                    if normal {
                        t.normal_map = path;
                        t.roblox_normal_id.clear();
                    } else {
                        t.image = path;
                        t.roblox_image_id.clear();
                    }
                }
            }
        }
        if let Some((id, slot)) = pbr_pick {
            if let Some(path) = pick_image() {
                self.push_undo();
                if let Some(t) = self.map.textures.iter_mut().find(|t| t.id == id) {
                    if slot == 2 {
                        t.roughness_map = path;
                        t.roblox_roughness_id.clear();
                    } else {
                        t.metalness_map = path;
                        t.roblox_metalness_id.clear();
                    }
                }
            }
        }
        if let Some((id, slot)) = pbr_clear {
            self.push_undo();
            if let Some(t) = self.map.textures.iter_mut().find(|t| t.id == id) {
                if slot == 2 {
                    t.roughness_map.clear();
                    t.roblox_roughness_id.clear();
                } else {
                    t.metalness_map.clear();
                    t.roblox_metalness_id.clear();
                }
            }
        }
        if let Some(id) = clear_normal {
            self.push_undo();
            if let Some(t) = self.map.textures.iter_mut().find(|t| t.id == id) {
                t.normal_map.clear();
                t.roblox_normal_id.clear();
            }
        }
        if let Some(id) = apply {
            if self.selection.is_empty() {
                self.status = "Select objects to texture first".into();
            } else {
                self.push_undo();
                for sid in self.selection.clone() {
                    if let Some(o) = self.map.get_mut(sid) {
                        if o.texture.is_none() {
                            o.color = [255, 255, 255];
                        }
                        o.texture = Some(id);
                    }
                }
                self.active_texture = Some(id);
            }
        }
        if let Some(id) = use_decal {
            self.active_texture = Some(id);
            self.tool = Tool::Decal;
            self.status = "Decal tool: click a wall or floor".into();
        }
        if let Some(id) = delete {
            self.push_undo();
            self.map.textures.retain(|t| t.id != id);
            for o in &mut self.map.objects {
                if o.texture == Some(id) {
                    o.texture = None;
                }
            }
            if self.active_texture == Some(id) {
                self.active_texture = None;
            }
        }
    }

    pub(crate) fn plugins_window(&mut self, ctx: &egui::Context) {
        if !self.show_plugins {
            return;
        }
        let mut open = true;
        let mut run: Option<usize> = None;
        let mut use_tool: Option<usize> = None;
        let mut reload = false;
        let mut new_plugin = false;
        let mut toggles: Vec<(String, bool)> = Vec::new();
        egui::Window::new("Plugins").open(&mut open).default_width(520.0).default_height(520.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Reload").clicked() {
                    reload = true;
                }
                if ui.button("Open plugins folder").clicked() {
                    let _ = std::fs::create_dir_all(&self.plugins.dir);
                    let _ = std::process::Command::new("explorer").arg(&self.plugins.dir).spawn();
                }
                if ui.button("New plugin…").clicked() {
                    new_plugin = true;
                }
            });
            ui.label(
                RichText::new("Plugins are Rhai scripts (.rhai) in the plugins folder. The template lists the editor API. Reload after editing.")
                    .small()
                    .weak(),
            );
            ui.separator();
            egui::ScrollArea::vertical().id_salt("plugin_list").max_height(280.0).show(ui, |ui| {
                for (i, p) in self.plugins.plugins.iter().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.name).strong());
                            ui.label(RichText::new(format!("[{}]  {}", p.kind.label(), p.file)).weak());
                        });
                        ui.label(&p.description);
                        ui.horizontal(|ui| match p.kind {
                            PluginKind::Command => {
                                if ui.button("Run").clicked() {
                                    run = Some(i);
                                }
                            }
                            PluginKind::Tool => {
                                if ui.selectable_label(self.tool == Tool::Plugin(i), "Use tool").clicked() {
                                    use_tool = Some(i);
                                }
                            }
                            PluginKind::Panel => {
                                let mut open = self.open_panels.contains(&p.file);
                                if ui.toggle_value(&mut open, "Open window").clicked() {
                                    toggles.push((format!("panel:{}", p.file), open));
                                }
                            }
                            PluginKind::Move | PluginKind::Gizmo => {
                                let mut on = self.settings.enabled_plugins.contains(&p.file);
                                let label = if p.kind == PluginKind::Gizmo { "Enabled (shows on the selection)" } else { "Enabled while moving" };
                                if ui.checkbox(&mut on, label).changed() {
                                    toggles.push((p.file.clone(), on));
                                }
                            }
                        });
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Log").strong());
                if ui.small_button("Clear").clicked() {
                    self.plugins.log.clear();
                }
            });
            egui::ScrollArea::vertical().id_salt("plugin_log").stick_to_bottom(true).show(ui, |ui| {
                for line in &self.plugins.log {
                    ui.label(RichText::new(line).monospace().small());
                }
            });
        });
        if !open {
            self.show_plugins = false;
        }
        if reload {
            self.plugins.reload();
            if let Tool::Plugin(i) = self.tool {
                if i >= self.plugins.plugins.len() {
                    self.tool = Tool::Select;
                }
            }
        }
        if new_plugin {
            match self.plugins.new_plugin_file() {
                Ok(p) => {
                    let _ = std::process::Command::new("notepad").arg(&p).spawn();
                    self.plugins.log.push(format!("Created {} — edit it, then press Reload", p.display()));
                }
                Err(e) => self.plugins.log.push(e),
            }
        }
        for (file, on) in toggles.iter().filter(|t| t.0.starts_with("panel:")).cloned().collect::<Vec<_>>() {
            let f = file.trim_start_matches("panel:").to_string();
            self.open_panels.retain(|x| *x != f);
            if on {
                self.open_panels.push(f);
            }
        }
        toggles.retain(|t| !t.0.starts_with("panel:"));
        for (file, on) in toggles {
            self.settings.enabled_plugins.retain(|f| *f != file);
            if on {
                self.settings.enabled_plugins.push(file);
            }
            let _ = self.settings.save();
        }
        if let Some(i) = run {
            self.run_plugin_command(i);
        }
        if let Some(i) = use_tool {
            self.tool = Tool::Plugin(i);
            if let Some(f) = self.plugins.plugins.get(i).map(|p| p.file.clone()) {
                self.plugins.clear_state_for(&f);
            }
        }
    }

    // ----- export --------------------------------------------------------

    pub(crate) fn poll_cloud(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.cloud_rx else { return };
        let mut done = false;
        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        for m in msgs {
            match m {
                CloudMsg::Log(s) => self.cloud_log.push(s),
                CloudMsg::TextureId { tex, slot, id } => {
                    if let Some(t) = self.map.textures.iter_mut().find(|t| t.id == tex) {
                        *crate::cloud::texture_slot_id(t, slot) = id;
                        self.dirty = true;
                    }
                }
                CloudMsg::ModelId { path, id } => {
                    if let Some(e) = self.settings.model_library.iter_mut().find(|e| e.path == path) {
                        e.roblox_model_id = id;
                    }
                    let _ = self.settings.save();
                }
                CloudMsg::MapId(id) => {
                    self.status = format!("Map uploaded to Roblox as asset {id}");
                    self.last_map_asset = Some(id);
                }
                CloudMsg::Finished => done = true,
            }
        }
        if done {
            self.cloud_rx = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn start_upload(&mut self) {
        if self.cloud_rx.is_some() {
            return;
        }
        if self.remember_key {
            if let Err(e) = cloud::save_key(&self.cloud_key) {
                self.cloud_log.push(format!("Could not save the key: {e}"));
            }
        }
        self.cloud_log.push("──────── upload started ────────".into());
        let job = Job {
            map: self.map.clone(),
            settings: self.settings.clone(),
            models: self.models.clone(),
            key: self.cloud_key.clone(),
            creator: Creator { group: self.settings.roblox_creator_group, id: self.settings.roblox_creator_id.clone() },
            textures: self.cloud_textures,
            props: self.cloud_props,
            upload_map: self.cloud_map,
            force: self.cloud_force,
        };
        self.cloud_rx = Some(cloud::spawn(job));
    }

    pub(crate) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.show_export {
            return;
        }
        let mut open = true;
        let mut action: Option<&'static str> = None;
        let busy = self.cloud_rx.is_some();
        egui::Window::new("Export").open(&mut open).default_width(560.0).default_height(600.0).vscroll(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(!self.export_tab_roblox, RichText::new("Unity").strong()).clicked() {
                    self.export_tab_roblox = false;
                }
                if ui.selectable_label(self.export_tab_roblox, RichText::new("Roblox").strong()).clicked() {
                    self.export_tab_roblox = true;
                }
            });
            ui.separator();
            if !self.export_tab_roblox {
                ui.label("Writes one .obj + .mtl with a child object per brush, mesh, prop and decal (folders become name prefixes). Textures and normal maps are copied next to it.");
                ui.label("Lights and spawns go into <name>_AnvilEntities.cs: put it in Assets/Editor and run Anvil > Create entities.");
                ui.horizontal(|ui| {
                    ui.label("Meters per unit");
                    if ui.add(egui::DragValue::new(&mut self.settings.unity_scale).speed(0.01).range(0.0001..=1000.0)).changed() {
                        let _ = self.settings.save();
                    }
                });
                if ui.button("Export for Unity (.obj)…").clicked() {
                    action = Some("unity");
                }
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Blender").strong());
                ui.label("The whole scene with textures and lights, one object per Anvil object. With Blender installed, Anvil makes a real .blend where folders are collections.");
                let blender = crate::blender_export::find_blender(&self.settings.blender_path);
                ui.horizontal(|ui| {
                    if ui.add_enabled(blender.is_some(), egui::Button::new("Blender file (.blend)…")).on_disabled_hover_text("Blender wasn't found. Set its path below, or use .glb").clicked() {
                        action = Some("blend");
                    }
                    if ui.button("Scene (.glb)…").on_hover_text("Blender: File > Import > glTF 2.0").clicked() {
                        action = Some("glb");
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("blender.exe");
                    let hint = blender.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not found".into());
                    if ui.add(egui::TextEdit::singleline(&mut self.settings.blender_path).hint_text(hint).desired_width(260.0)).changed() {
                        let _ = self.settings.save();
                    }
                });
                return;
            }

            ui.label(RichText::new("Files").strong());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Roblox model (.rbxmx)…").clicked() {
                    action = Some("rbxmx");
                }
                if ui.button("Command bar script (.lua)…").clicked() {
                    action = Some("lua");
                }
                if ui.button("Copy script to clipboard").clicked() {
                    action = Some("copy");
                }
            });
            ui.horizontal(|ui| {
                ui.label("Studs per unit");
                if ui.add(egui::DragValue::new(&mut self.settings.roblox_scale).speed(0.01).range(0.0001..=1000.0)).changed() {
                    let _ = self.settings.save();
                }
            });
            ui.separator();

            ui.label(RichText::new("Upload with Roblox Open Cloud").strong());
            ui.label(
                RichText::new(
                    "Create an API key at create.roblox.com > Open Cloud > API Keys, add the 'Assets' API with Read and Write, \
                     and allow your IP. Textures upload as Decals, props as Models (.glb), and the whole map as a Model (.rbxmx).",
                )
                .small()
                .weak(),
            );
            egui::Grid::new("cloud_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("API key");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.cloud_key).password(true).desired_width(260.0).hint_text("paste your Open Cloud key"));
                    if ui.small_button("Forget").on_hover_text("Clear the key and delete the saved copy").clicked() {
                        self.cloud_key.clear();
                        self.remember_key = false;
                        cloud::forget_key();
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.remember_key, "Remember on this PC (saved next to anvil.exe, not in maps)");
                ui.end_row();
                ui.label("Creator");
                ui.horizontal(|ui| {
                    let mut ch = false;
                    ch |= ui.radio_value(&mut self.settings.roblox_creator_group, false, "User").changed();
                    ch |= ui.radio_value(&mut self.settings.roblox_creator_group, true, "Group").changed();
                    ch |= ui
                        .add(egui::TextEdit::singleline(&mut self.settings.roblox_creator_id).desired_width(140.0).hint_text(if self.settings.roblox_creator_group {
                            "group id"
                        } else {
                            "your user id"
                        }))
                        .changed();
                    if ch {
                        let _ = self.settings.save();
                    }
                });
                ui.end_row();
                ui.label("Upload");
                ui.vertical(|ui| {
                    ui.checkbox(&mut self.cloud_textures, "Textures & decal images (Decal assets)");
                    ui.checkbox(&mut self.cloud_props, "Prop models used in this map (Model assets, .glb)");
                    ui.checkbox(&mut self.cloud_map, "The map itself (Model asset, .rbxmx)");
                    ui.checkbox(&mut self.cloud_force, "Re-upload items that already have an id");
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let can = !busy && !self.cloud_key.trim().is_empty() && !self.settings.roblox_creator_id.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new(RichText::new("Upload to Roblox").strong())).clicked() {
                    action = Some("upload");
                }
                if busy {
                    ui.spinner();
                    ui.label("Uploading…");
                }
            });
            if let Some(id) = self.last_map_asset.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!("Map asset: {id}"));
                    if ui.button("Copy insert snippet").on_hover_text("Paste into Studio's Command Bar").clicked() {
                        ctx.copy_text(format!(
                            "local m = game:GetService(\"InsertService\"):LoadAsset({id}) for _, c in ipairs(m:GetChildren()) do c.Parent = workspace end m:Destroy()"
                        ));
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Upload log").strong());
                if ui.small_button("Clear").clicked() {
                    self.cloud_log.clear();
                }
            });
            egui::ScrollArea::vertical().id_salt("cloud_log").max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                for l in &self.cloud_log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        });
        if !open {
            self.show_export = false;
        }
        match action {
            Some("unity") => self.export_unity(),
            Some("glb") => self.export_blender(false),
            Some("blend") => self.export_blender(true),
            Some("rbxmx") => self.export_rbxmx(),
            Some("lua") => self.export_luau(),
            Some("copy") => self.copy_luau(ctx),
            Some("upload") => self.start_upload(),
            _ => {}
        }
    }
}

/// Settings window contents. Returns true when "Add model…" was clicked.
pub(crate) fn settings_ui(ui: &mut egui::Ui, s: &mut Settings) -> bool {
    let mut add_model = false;
    egui::CollapsingHeader::new("Grid").default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Grid size");
            ui.add(egui::DragValue::new(&mut s.grid).speed(0.05).range(0.01..=1024.0));
            ui.checkbox(&mut s.snap, "Snap to grid");
        });
    });
    egui::CollapsingHeader::new("Default brush").default_open(true).show(ui, |ui| {
        egui::Grid::new("set_brush").num_columns(2).show(ui, |ui| {
            ui.label("Shape");
            shape_combo(ui, "set_shape", &mut s.default_shape);
            ui.end_row();
            ui.label("Size (click placement)");
            vec3_edit(ui, &mut s.default_size, 0.1, Some(0.01));
            ui.end_row();
            ui.label("Color");
            ui.color_edit_button_srgb(&mut s.default_color);
            ui.end_row();
            ui.label("Material");
            material_combo(ui, "set_mat", &mut s.default_material);
            ui.end_row();
            ui.label("Stair steps");
            ui.add(egui::DragValue::new(&mut s.default_steps).range(1..=128));
            ui.end_row();
            ui.label("Polygon tool height");
            ui.add(egui::DragValue::new(&mut s.polygon_height).speed(0.1).range(0.01..=10000.0));
            ui.end_row();
        });
    });
    egui::CollapsingHeader::new("Look").default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Theme");
            egui::ComboBox::from_id_salt("set_theme").selected_text(crate::themes::THEME_NAMES[s.theme.min(4) as usize]).show_ui(ui, |ui| {
                for (i, name) in crate::themes::THEME_NAMES.iter().enumerate() {
                    ui.selectable_value(&mut s.theme, i as u8, *name);
                }
            });
            ui.label("Accent");
            ui.color_edit_button_srgb(&mut s.accent);
        });
    });
    ui.label(RichText::new("Wall, building, door, window, light, spawn and decal options are in the Tool settings box under the tool list (pick the tool to see them).").small().weak());
    egui::CollapsingHeader::new(format!("Model library ({})", s.model_library.len())).default_open(true).show(ui, |ui| {
        ui.label(
            RichText::new(
                "Mesh id: an uploaded Roblox mesh (used via SpecialMesh). Model id: filled in by Export > Upload \
                 (inserted with InsertService by the command bar script).",
            )
            .small()
            .weak(),
        );
        let mut remove = None;
        egui::Grid::new("set_models").num_columns(4).striped(true).show(ui, |ui| {
            for (i, m) in s.model_library.iter_mut().enumerate() {
                ui.label(file_name(&m.path)).on_hover_text(&m.path);
                ui.add(egui::TextEdit::singleline(&mut m.roblox_mesh_id).hint_text("mesh id").desired_width(110.0));
                ui.add(egui::TextEdit::singleline(&mut m.roblox_model_id).hint_text("model id").desired_width(110.0));
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            s.model_library.remove(i);
        }
        if ui.button("Add model…").clicked() {
            add_model = true;
        }
    });
    egui::CollapsingHeader::new("Export").default_open(false).show(ui, |ui| {
        egui::Grid::new("set_export").num_columns(2).show(ui, |ui| {
            ui.label("Unity: meters per unit");
            ui.add(egui::DragValue::new(&mut s.unity_scale).speed(0.01).range(0.0001..=1000.0));
            ui.end_row();
            ui.label("Roblox: studs per unit");
            ui.add(egui::DragValue::new(&mut s.roblox_scale).speed(0.01).range(0.0001..=1000.0));
            ui.end_row();
            ui.label("Roblox: mesh triangle thickness");
            ui.add(egui::DragValue::new(&mut s.mesh_wedge_thickness).speed(0.01).range(0.001..=10.0));
            ui.end_row();
        });
        ui.label(RichText::new("Default: 1 unit = 1 stud = 0.28 m. Editable meshes export to Roblox as pairs of WedgeParts per triangle.").small().weak());
    });
    egui::CollapsingHeader::new("Viewports & playtest").default_open(false).show(ui, |ui| {
        egui::Grid::new("set_view").num_columns(2).show(ui, |ui| {
            ui.label("3D fly speed");
            ui.add(egui::DragValue::new(&mut s.cam_speed).speed(0.5).range(1.0..=2000.0));
            ui.end_row();
            ui.label("3D field of view");
            ui.add(egui::Slider::new(&mut s.fov, 30.0..=110.0));
            ui.end_row();
            ui.label("3D render scale");
            ui.add(egui::Slider::new(&mut s.render_scale, 0.25..=2.0));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut s.fast_motion, "Faster while moving").on_hover_text("If the 3D view is slow, it draws at lower resolution while the camera moves and sharpens when it stops");
            ui.end_row();
            ui.label("Preview texture size");
            egui::ComboBox::from_id_salt("tex_quality").selected_text(format!("{} px", s.preview_texture_size)).show_ui(ui, |ui| {
                for v in [256u32, 512, 1024, 2048] {
                    ui.selectable_value(&mut s.preview_texture_size, v, format!("{v} px"));
                }
            });
            ui.end_row();
            ui.label("Smooth textures");
            ui.checkbox(&mut s.texture_filtering, "Bilinear filtering (exports always use the original images)");
            ui.end_row();
            ui.label("Lighting quality");
            egui::ComboBox::from_id_salt("light_q").selected_text(crate::lighting::QUALITY_NAMES[s.light_quality.min(4) as usize]).show_ui(ui, |ui| {
                for (i, name) in crate::lighting::QUALITY_NAMES.iter().enumerate() {
                    ui.selectable_value(&mut s.light_quality, i as u8, *name);
                }
            })
            .response
            .on_hover_text("Low: sky light, point lights, fog. Medium: + sun shadows. High: + soft shadows and ambient occlusion. Ultra: sharper, wider shadows.");
            ui.end_row();
            ui.label("Sun direction");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.sun_azimuth).speed(1.0).suffix("°").range(-360.0..=360.0)).on_hover_text("Compass angle");
                ui.add(egui::DragValue::new(&mut s.sun_elevation).speed(1.0).suffix("° up").range(1.0..=90.0)).on_hover_text("Height above the horizon");
            });
            ui.end_row();
            ui.label("Sky / fog");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.sky_enabled, "Sky gradient");
                ui.checkbox(&mut s.fog_enabled, "Fog");
                ui.add_enabled(s.fog_enabled, egui::DragValue::new(&mut s.fog_distance).speed(5.0).range(50.0..=20000.0).prefix("ends at "));
            });
            ui.end_row();
            ui.label("3D grid / outlines");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.show_grid_3d, "Grid");
                ui.checkbox(&mut s.outlines_3d, "Outlines");
            });
            ui.end_row();
            ui.label("Playtest mouse sensitivity");
            ui.add(egui::Slider::new(&mut s.mouse_sensitivity, 0.1..=4.0));
            ui.end_row();
            ui.label("2D background");
            ui.color_edit_button_srgb(&mut s.bg_2d);
            ui.end_row();
            ui.label("3D background");
            ui.color_edit_button_srgb(&mut s.bg_3d);
            ui.end_row();
        });
    });
    ui.separator();
    if ui.button("Reset all settings to defaults").clicked() {
        let lib = std::mem::take(&mut s.model_library);
        let creator = (s.roblox_creator_group, s.roblox_creator_id.clone());
        *s = Settings::default();
        s.model_library = lib;
        s.roblox_creator_group = creator.0;
        s.roblox_creator_id = creator.1;
    }
    add_model
}

impl AnvilApp {
    /// Asks before any new or changed plugin runs.
    pub(crate) fn plugin_trust_window(&mut self, ctx: &egui::Context) {
        if self.plugins.pending.is_empty() {
            return;
        }
        let mut act: Option<(String, bool)> = None;
        egui::Window::new("New or changed plugins").collapsible(false).resizable(true).default_width(520.0).show(ctx, |ui| {
            ui.label(RichText::new("These plugin scripts are new or were changed since you last allowed them. Plugins can change your map, but can't touch files, the network or other programs, except: websites a plugin lists under hosts, and (with programs: true) folders you pick for it and programs in them, after asking you. Only allow ones you trust.").small());
            for p in &self.plugins.pending {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&p.file).strong().monospace());
                    if ui.button("Allow").clicked() {
                        act = Some((p.key.clone(), true));
                    }
                    if ui.button("Don't run").clicked() {
                        act = Some((p.key.clone(), false));
                    }
                });
                egui::ScrollArea::vertical().id_salt(("plugin_preview", &p.key)).max_height(140.0).show(ui, |ui| {
                    ui.label(RichText::new(&p.preview).monospace().small());
                });
            }
        });
        if let Some((key, allow)) = act {
            if allow {
                let k = self.plugins.approve(&key);
                if !self.settings.trusted_plugins.contains(&k) {
                    self.settings.trusted_plugins.push(k);
                    let _ = self.settings.save();
                }
                let keep = self.plugins.pending.clone();
                self.plugins.reload();
                // keep anything still waiting that wasn't answered yet
                self.plugins.pending.retain(|p| keep.iter().any(|k| k.key == p.key));
            } else {
                self.plugins.dismiss(&key);
            }
        }
    }
}
