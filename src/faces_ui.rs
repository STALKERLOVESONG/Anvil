//! Per-face texturing: the Faces tool (pick faces of walls / meshes) and the
//! panel that sets a face's texture, tile scale, offset, rotation and tint.

use crate::app::{vec2_edit, AnvilApp};
use crate::map::{FaceTex, Kind, Object, TexParams};
use crate::mesh::{tri_face_key, WorldMesh};
use eframe::egui::{self, Color32, Painter, Rect, RichText, Stroke};
use glam::Vec3;
use std::sync::Arc;

/// Texture settings editor shared by objects and faces. Returns true if changed.
pub(crate) fn tex_params_ui(ui: &mut egui::Ui, p: &mut TexParams, id: impl std::hash::Hash) -> bool {
    let mut ch = false;
    egui::Grid::new(id).num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
        ui.label("Tile size ×");
        ch |= vec2_edit(ui, &mut p.scale, 0.01, Some(0.01), ["u", "v"]);
        ui.end_row();
        ui.label("Offset");
        ch |= vec2_edit(ui, &mut p.offset, 0.05, None, ["u", "v"]);
        ui.end_row();
        ui.label("Rotation");
        ch |= ui.add(egui::DragValue::new(&mut p.rotation).speed(1.0).suffix("°")).on_hover_text("Preview + Unity. Roblox textures can't rotate.").changed();
        ui.end_row();
        ui.label("Tint");
        ui.horizontal(|ui| {
            ch |= ui.color_edit_button_srgb(&mut p.tint).changed();
            if ui.small_button("reset").clicked() {
                *p = TexParams::default();
                ch = true;
            }
        });
        ui.end_row();
    });
    ch
}

impl AnvilApp {
    /// Face key under a 3D hit (brush direction or mesh polygon).
    pub(crate) fn face_at(&self, scene: &[Arc<WorldMesh>], id: u64, tri: usize) -> Option<usize> {
        let o = self.map.get(id)?;
        let wm = scene.iter().find(|w| w.id == id)?;
        tri_face_key(o, &wm.mesh, tri)
    }

    pub(crate) fn click_face(&mut self, hit: Option<(u64, usize)>, add: bool) {
        match hit {
            Some(f) if add => {
                if let Some(i) = self.face_sel.iter().position(|x| *x == f) {
                    self.face_sel.remove(i);
                } else {
                    self.face_sel.push(f);
                }
            }
            Some(f) => self.face_sel = vec![f],
            None if !add => self.face_sel.clear(),
            None => {}
        }
        let objs: Vec<u64> = {
            let mut v: Vec<u64> = self.face_sel.iter().map(|f| f.0).collect();
            v.dedup();
            v
        };
        self.selection = objs;
        if !self.face_sel.is_empty() {
            self.status = format!("{} face(s) selected. Set their texture in the Inspector (Shift/Ctrl+click adds).", self.face_sel.len());
        }
    }

    /// Orange highlight over the selected faces in the 3D view.
    pub(crate) fn paint_face_selection(&self, painter: &Painter, rect: Rect, scene: &[Arc<WorldMesh>]) {
        if self.face_sel.is_empty() {
            return;
        }
        let fill = Color32::from_rgba_unmultiplied(255, 150, 40, 80);
        let fov = self.settings.fov;
        for wm in scene {
            if !self.face_sel.iter().any(|f| f.0 == wm.id) {
                continue;
            }
            let Some(o) = self.map.get(wm.id) else { continue };
            for (ti, t) in wm.mesh.tris.iter().enumerate() {
                let Some(k) = tri_face_key(o, &wm.mesh, ti) else { continue };
                if !self.face_sel.contains(&(wm.id, k)) {
                    continue;
                }
                let pts: Vec<_> = t.iter().map(|&i| self.cam.project(rect, fov, wm.pos[i as usize])).collect();
                if let [Some(a), Some(b), Some(c)] = pts[..] {
                    painter.add(egui::Shape::convex_polygon(vec![a, b, c], fill, Stroke::new(1.0f32, Color32::from_rgb(255, 170, 60))));
                }
            }
        }
    }

    /// Texture panel for a set of faces (object id, face key).
    pub(crate) fn face_tex_panel(&mut self, ui: &mut egui::Ui, targets: Vec<(u64, usize)>) {
        if targets.is_empty() {
            return;
        }
        let textures: Vec<(u64, String)> = self.map.textures.iter().map(|t| (t.id, t.name.clone())).collect();
        let first = targets[0];
        let cur: Option<FaceTex> = self.map.get(first.0).and_then(|o| o.face_tex.get(first.1).cloned().flatten());
        let mut state = cur.clone();
        let mut changed = false;
        ui.label(RichText::new(format!("{} face(s) selected", targets.len())).strong());
        ui.label(RichText::new("Give just these faces their own texture. 'Object's texture' = same as the rest of the object.").small().weak());
        let label = match &state {
            None => "(object's texture)".to_string(),
            Some(FaceTex { texture: None, .. }) => "(no texture)".to_string(),
            Some(FaceTex { texture: Some(id), .. }) => textures.iter().find(|t| t.0 == *id).map(|t| t.1.clone()).unwrap_or_else(|| "(missing)".into()),
        };
        ui.horizontal(|ui| {
            ui.label("Texture");
            egui::ComboBox::from_id_salt("face_tex").selected_text(label).width(170.0).show_ui(ui, |ui| {
                if ui.selectable_label(state.is_none(), "(object's texture)").clicked() {
                    state = None;
                    changed = true;
                }
                if ui.selectable_label(matches!(state, Some(FaceTex { texture: None, .. })), "(no texture)").clicked() {
                    state = Some(FaceTex { texture: None, params: TexParams::default(), uv: None });
                    changed = true;
                }
                for (tid, name) in &textures {
                    let on = matches!(state, Some(FaceTex { texture: Some(x), .. }) if x == *tid);
                    if ui.selectable_label(on, name).clicked() {
                        let params = state.as_ref().map(|s| s.params).unwrap_or_default();
                        let uv = state.as_ref().and_then(|s| s.uv);
                        state = Some(FaceTex { texture: Some(*tid), params, uv });
                        changed = true;
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(self.active_texture.is_some(), egui::Button::new("Apply active texture")).clicked() {
                let params = state.as_ref().map(|s| s.params).unwrap_or_default();
                let uv = state.as_ref().and_then(|s| s.uv);
                state = Some(FaceTex { texture: self.active_texture, params, uv });
                changed = true;
            }
            if ui.button("Use object's texture").clicked() {
                state = None;
                changed = true;
            }
        });
        if let Some(ft) = &mut state {
            if ft.texture.is_some() {
                changed |= tex_params_ui(ui, &mut ft.params, "face_params");
            }
        }
        if ui.small_button("Open UV editor").on_hover_text("Drag the texture into place on these faces (Ctrl+U)").clicked() {
            self.uv.open = true;
        }
        if changed {
            let mut ids: Vec<u64> = targets.iter().map(|t| t.0).collect();
            ids.sort_unstable();
            ids.dedup();
            let originals: Vec<Object> = ids.iter().filter_map(|id| self.map.get(*id).cloned()).collect();
            for (id, f) in &targets {
                if let Some(o) = self.map.get_mut(*id) {
                    *o.face_tex_mut(*f) = state.clone();
                    while o.face_tex.last().is_some_and(|x| x.is_none()) {
                        o.face_tex.pop();
                    }
                }
            }
            if !self.inspector_editing {
                self.push_undo_restoring(originals);
                self.inspector_editing = true;
            }
            self.dirty = true;
        }
    }

    /// Faces tool click in a 2D view: the brush face pointing at the viewer.
    pub(crate) fn face_click_2d(&mut self, id: Option<u64>, viewer_face: usize, add: bool) {
        match id.and_then(|i| self.map.get(i)) {
            Some(o) if matches!(o.kind, Kind::Brush { .. }) => self.click_face(Some((o.id, viewer_face)), add),
            Some(_) => self.status = "Pick faces of meshes in the 3D view".into(),
            None => self.click_face(None, add),
        }
    }

    /// Keeps per-face textures lined up after mesh faces are deleted.
    pub(crate) fn drop_face_textures(&mut self, obj: u64, deleted: &[usize]) {
        if let Some(o) = self.map.get_mut(obj) {
            let mut d = deleted.to_vec();
            d.sort_unstable();
            d.dedup();
            for f in d.into_iter().rev() {
                if f < o.face_tex.len() {
                    o.face_tex.remove(f);
                }
            }
        }
        let _ = Vec3::ZERO;
    }
}

/// Drag-and-drop payload: a texture id from the library or Textures window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TexDrop(pub u64);

impl AnvilApp {
    fn drop_texture(&mut self, id: u64, face: Option<usize>, tex: u64) {
        let Some(name) = self.map.textures.iter().find(|t| t.id == tex).map(|t| t.name.clone()) else { return };
        self.push_undo();
        let Some(o) = self.map.get_mut(id) else { return };
        match face {
            Some(f) => {
                let old = o.face_tex.get(f).cloned().flatten();
                let params = old.as_ref().map(|x| x.params).unwrap_or_default();
                let uv = old.and_then(|x| x.uv);
                *o.face_tex_mut(f) = Some(FaceTex { texture: Some(tex), params, uv });
                self.status = format!("Put '{name}' on one face of {}", o.name);
            }
            None => {
                o.texture = Some(tex);
                self.status = format!("Put '{name}' on {} (hold Alt while dropping for one face)", o.name);
            }
        }
        self.selection = vec![id];
    }

    /// A texture dropped on the 3D view lands on the object (or face, with Alt) under the mouse.
    pub(crate) fn texture_drop_3d(&mut self, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        let Some(t) = resp.dnd_release_payload::<TexDrop>() else { return };
        let Some(p) = ctx.pointer_latest_pos() else { return };
        let (o, d) = self.cam.ray(rect, self.settings.fov, p);
        let Some(h) = crate::view3d::pick_ray(scene, o, d) else {
            self.status = "Drop textures onto an object".into();
            return;
        };
        let face = if ctx.input(|i| i.modifiers.alt) { self.face_at(scene, h.id, h.tri) } else { None };
        self.drop_texture(h.id, face, t.0);
    }

    /// Same for a 2D view (Alt picks the brush face facing you).
    pub(crate) fn texture_drop_2d(&mut self, vi: usize, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        let Some(t) = resp.dnd_release_payload::<TexDrop>() else { return };
        let Some(p) = ctx.pointer_latest_pos() else { return };
        let view = self.views[vi];
        let Some(id) = crate::view2d::pick(&view, rect, scene, p) else {
            self.status = "Drop textures onto an object".into();
            return;
        };
        let brush = self.map.get(id).is_some_and(|o| matches!(o.kind, Kind::Brush { .. }));
        let face = (ctx.input(|i| i.modifiers.alt) && brush).then(|| match view.kind {
            crate::view2d::ViewKind::Top => 2,
            crate::view2d::ViewKind::Front => 4,
            crate::view2d::ViewKind::Side => 1,
        });
        self.drop_texture(id, face, t.0);
    }

    /// While dragging a texture: outline what it would land on.
    pub(crate) fn paint_texture_drop_hover(&self, painter: &Painter, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>]) {
        if resp.dnd_hover_payload::<TexDrop>().is_none() {
            return;
        }
        let Some(p) = resp.hover_pos() else { return };
        let (o, d) = self.cam.ray(rect, self.settings.fov, p);
        if let Some(wm) = crate::view3d::pick_ray(scene, o, d).and_then(|h| scene.iter().find(|w| w.id == h.id)) {
            let fov = self.settings.fov;
            for e in wm.mesh.edges.iter().take(4000) {
                if let Some((a, b)) = self.cam.project_segment(rect, fov, wm.pos[e[0] as usize], wm.pos[e[1] as usize]) {
                    painter.line_segment([a, b], Stroke::new(2.0f32, Color32::from_rgb(120, 255, 160)));
                }
            }
        }
    }
}
