//! UV editor: move / rotate / scale how a texture sits on an object or on
//! single faces, by dragging in a 2D canvas (or Alt+drag in the 3D view),
//! with snapping and "fit to face".
//!
//! The mapping (see `mesh::box_uv`) is
//! `uv = rotate((P + Off) / S, rotation)` where `P` is the point projected
//! onto the face's planar axes (in studs), `S = texture tile size * scale`
//! and `Off = texture offset + offset`.

use crate::app::{AnvilApp, Tool};
use crate::faces_ui::tex_params_ui;
use crate::map::{FaceTex, Object, TexParams, TextureDef};
use crate::mesh::{box_uv, local_mesh, rotate_uv, tri_face_key, ModelCache};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextureOptions};
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UvTarget {
    Object(u64),
    Face(u64, usize),
}

impl UvTarget {
    pub fn obj(&self) -> u64 {
        match *self {
            UvTarget::Object(id) | UvTarget::Face(id, _) => id,
        }
    }
    fn face(&self) -> Option<usize> {
        match *self {
            UvTarget::Face(_, f) => Some(f),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Move,
    /// Corner signs (-1/+1 per axis).
    Scale(Vec2),
    Rotate,
}

#[derive(Clone, Copy)]
struct CanvasDrag {
    handle: Handle,
    start: Vec2,
    pivot: Vec2,
    p0: TexParams,
}

/// Viewport (3D) drag state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewDrag {
    pub target: UvTarget,
    pub p0: TexParams,
    pub plane_p: Vec3,
    pub plane_n: Vec3,
    /// World -> object rotation and the face's planar U / V axes (object space).
    pub inv_rot: glam::Quat,
    pub u: Vec3,
    pub v: Vec3,
    pub ts: Vec2,
    pub toff: Vec2,
    pub start_x: f32,
    pub rotate: bool,
}

#[derive(Default)]
pub(crate) struct UvEditor {
    pub open: bool,
    drag: Option<CanvasDrag>,
    tex: Option<(String, egui::TextureHandle)>,
    view: Option<(UvTarget, Vec2, f32)>,
    pub vdrag: Option<ViewDrag>,
}

// ---------------------------------------------------------------------------
// The math (pure, tested)
// ---------------------------------------------------------------------------

fn nz(x: f32) -> f32 {
    if x.abs() < 1e-4 { 1.0 } else { x }
}

/// Tile size in studs for these settings.
pub fn tile(t: &TextureDef, p: &TexParams) -> Vec2 {
    let s = t.scale * p.scale;
    Vec2::new(nz(s.x), nz(s.y))
}

pub fn rot2(v: Vec2, deg: f32) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// UVs of the target's triangles exactly as the renderer computes them.
pub fn target_uvs(obj: &Object, face: Option<usize>, t: &TextureDef, p: &TexParams, models: &mut ModelCache) -> Vec<[Vec2; 3]> {
    let mesh = local_mesh(obj, models);
    let sc = obj.scale();
    let mut out = Vec::new();
    for (ti, tr) in mesh.tris.iter().enumerate() {
        if face.is_some() && tri_face_key(obj, &mesh, ti) != face {
            continue;
        }
        let lp = [mesh.pos[tr[0] as usize] * sc, mesh.pos[tr[1] as usize] * sc, mesh.pos[tr[2] as usize] * sc];
        let (uv, _, _) = box_uv(lp, t.scale * p.scale, t.offset + p.offset);
        out.push(rotate_uv(uv, p.rotation));
    }
    out
}

/// The face with the most area (what "fit" uses for a whole object).
pub fn largest_face(obj: &Object, models: &mut ModelCache) -> Option<usize> {
    let mesh = local_mesh(obj, models);
    let mut area: std::collections::HashMap<usize, f32> = Default::default();
    for (ti, tr) in mesh.tris.iter().enumerate() {
        let k = tri_face_key(obj, &mesh, ti)?;
        let (a, b, c) = (mesh.pos[tr[0] as usize], mesh.pos[tr[1] as usize], mesh.pos[tr[2] as usize]);
        *area.entry(k).or_default() += (b - a).cross(c - a).length();
    }
    area.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
}

/// Settings that stretch the texture exactly once over the face.
pub fn fit_params(obj: &Object, face: Option<usize>, t: &TextureDef, cur: &TexParams, models: &mut ModelCache) -> Option<TexParams> {
    let base = TexParams { scale: Vec2::ONE, offset: Vec2::ZERO, rotation: 0.0, tint: cur.tint };
    let uvs = target_uvs(obj, face, t, &base, models);
    let (a, b) = uvs.iter().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    if uvs.is_empty() {
        return None;
    }
    let size = b - a;
    let ts = tile(t, &base);
    Some(TexParams {
        scale: Vec2::new(if size.x > 1e-4 { size.x } else { 1.0 }, if size.y > 1e-4 { size.y } else { 1.0 }),
        offset: -a * ts,
        rotation: 0.0,
        tint: cur.tint,
    })
}

/// Slide the texture by `delta` (in UV units as shown).
pub fn moved(t: &TextureDef, p0: &TexParams, delta: Vec2) -> TexParams {
    let mut p = *p0;
    p.offset = p0.offset + rot2(delta, -p0.rotation) * tile(t, p0);
    p
}

/// Rescale so the UVs grow by `f` around `pivot` (UV units as shown).
pub fn scaled(t: &TextureDef, p0: &TexParams, pivot: Vec2, f: Vec2) -> TexParams {
    let s0 = tile(t, p0);
    let off0 = t.offset + p0.offset;
    let c_pre = rot2(pivot, -p0.rotation);
    let pc = c_pre * s0 - off0;
    let f = Vec2::new(f.x.clamp(0.01, 100.0), f.y.clamp(0.01, 100.0));
    let mut p = *p0;
    p.scale = p0.scale / f;
    let s1 = tile(t, &p);
    p.offset = c_pre * s1 - pc - t.offset;
    p
}

/// Turn the texture by `deg` around `pivot` (UV units as shown).
pub fn rotated(t: &TextureDef, p0: &TexParams, pivot: Vec2, deg: f32) -> TexParams {
    let s0 = tile(t, p0);
    let off0 = t.offset + p0.offset;
    let pc = rot2(pivot, -p0.rotation) * s0 - off0;
    let mut p = *p0;
    p.rotation = p0.rotation + deg;
    p.offset = rot2(pivot, -p.rotation) * s0 - pc - t.offset;
    p
}

fn snap_to(x: f32, step: f32) -> f32 {
    if step > 0.0 { (x / step).round() * step } else { x }
}

/// Offsets on 1/8 tiles, scales on 1/8 steps, rotation on 15 degrees.
pub fn snapped(t: &TextureDef, p: TexParams, what: char) -> TexParams {
    let mut p = p;
    match what {
        'm' => {
            let s = tile(t, &p) / 8.0;
            p.offset = Vec2::new(snap_to(p.offset.x, s.x.abs()), snap_to(p.offset.y, s.y.abs()));
        }
        's' => {
            for k in 0..2 {
                if p.scale[k].abs() >= 0.125 {
                    p.scale[k] = snap_to(p.scale[k], 0.125);
                }
            }
        }
        _ => p.rotation = snap_to(p.rotation, 15.0),
    }
    p
}

fn norm_deg(d: f32) -> f32 {
    let r = d % 360.0;
    if r > 180.0 { r - 360.0 } else if r <= -180.0 { r + 360.0 } else { r }
}

// ---------------------------------------------------------------------------
// App side
// ---------------------------------------------------------------------------

impl AnvilApp {
    /// What the UV editor works on: selected faces (Faces tool) or objects.
    pub(crate) fn uv_targets(&self) -> Vec<UvTarget> {
        if self.tool == Tool::Face && !self.face_sel.is_empty() {
            return self.face_sel.iter().map(|f| UvTarget::Face(f.0, f.1)).collect();
        }
        self.selection.iter().filter(|id| self.map.get(**id).is_some_and(|o| o.texture.is_some() || o.face_tex.iter().any(|f| f.is_some()))).map(|id| UvTarget::Object(*id)).collect()
    }

    /// (settings, texture id) a target currently uses.
    pub(crate) fn uv_params(&self, t: UvTarget) -> Option<(TexParams, Option<u64>)> {
        let o = self.map.get(t.obj())?;
        if let Some(ft) = t.face().and_then(|f| o.face_tex.get(f)).and_then(|f| f.as_ref()) {
            return Some((ft.params, ft.texture));
        }
        Some((o.tex_params, o.texture))
    }

    pub(crate) fn set_uv_params(&mut self, t: UvTarget, p: TexParams) {
        let Some(o) = self.map.get_mut(t.obj()) else { return };
        match t.face() {
            Some(f) => {
                let tex = o.texture;
                let slot = o.face_tex_mut(f);
                match slot {
                    Some(ft) => ft.params = p,
                    None => *slot = Some(FaceTex { texture: tex, params: p }),
                }
            }
            None => o.tex_params = p,
        }
        self.dirty = true;
    }

    fn texdef(&self, id: Option<u64>) -> Option<TextureDef> {
        id.and_then(|id| self.map.textures.iter().find(|t| t.id == id).cloned())
    }

    /// Applies new settings to every target (relative to the first one's change).
    fn apply_uv_all(&mut self, targets: &[UvTarget], first_new: TexParams) {
        let Some((first_old, _)) = self.uv_params(targets[0]) else { return };
        for (i, t) in targets.iter().enumerate() {
            let p = if i == 0 {
                first_new
            } else if let Some((mut p, _)) = self.uv_params(*t) {
                p.scale = p.scale * first_new.scale / Vec2::new(nz(first_old.scale.x), nz(first_old.scale.y));
                p.offset += first_new.offset - first_old.offset;
                p.rotation += first_new.rotation - first_old.rotation;
                p.tint = first_new.tint;
                p
            } else {
                continue;
            };
            self.set_uv_params(*t, p);
        }
    }

    fn uv_texture(&mut self, ctx: &egui::Context, path: &str) -> Option<egui::TextureId> {
        if let Some((p, h)) = &self.uv.tex {
            if p == path {
                return Some(h.id());
            }
        }
        let img = self.textures.get(path)?;
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba);
        let opts = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::LINEAR };
        let h = ctx.load_texture("uv_editor_tex", ci, opts);
        let id = h.id();
        self.uv.tex = Some((path.to_string(), h));
        Some(id)
    }

    pub(crate) fn uv_window(&mut self, ctx: &egui::Context) {
        if !self.uv.open {
            return;
        }
        let mut open = true;
        egui::Window::new("UV Editor").open(&mut open).default_width(380.0).resizable(false).show(ctx, |ui| self.uv_ui(ui, ctx));
        if !open {
            self.uv.open = false;
            self.uv.drag = None;
        }
    }

    fn uv_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let targets = self.uv_targets();
        if targets.is_empty() {
            ui.label(RichText::new("Select a textured object, or use the Faces tool and click faces, to edit how the texture sits.").weak());
            return;
        }
        let t0 = targets[0];
        let Some((p, texid)) = self.uv_params(t0) else { return };
        let Some(td) = self.texdef(texid) else {
            ui.label(RichText::new("This has no texture yet. Give it one in the Inspector (or the face panel) first.").weak());
            return;
        };
        let Some(obj) = self.map.get(t0.obj()).cloned() else { return };
        ui.label(match t0 {
            UvTarget::Face(..) => format!("{} face(s) · {}", targets.len(), td.name),
            UvTarget::Object(_) => format!("{} object(s) · {}", targets.len(), td.name),
        });
        let uvs = target_uvs(&obj, t0.face(), &td, &p, &mut self.models);

        // --- canvas ---
        let size = vec2(ui.available_width().clamp(260.0, 420.0), 300.0);
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = resp.rect;
        let (bmin, bmax) = uvs.iter().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
        let has = !uvs.is_empty();
        let center = if has { (bmin + bmax) * 0.5 } else { Vec2::splat(0.5) };
        // keep the view still while dragging, re-frame when the target changes
        let view = match self.uv.view {
            Some((k, c, z)) if k == t0 && (self.uv.drag.is_some() || resp.hovered()) => (c, z),
            _ => {
                let ext = if has { (bmax - bmin).max_element().max(1.0) } else { 1.0 };
                let z = rect.width().min(rect.height()) / (ext * 1.6);
                (center, z)
            }
        };
        let (vc, mut zoom) = view;
        if resp.hovered() {
            let sc = ui.input(|i| i.raw_scroll_delta.y);
            if sc != 0.0 {
                zoom = (zoom * (1.0 + sc * 0.002)).clamp(4.0, 4000.0);
            }
        }
        self.uv.view = Some((t0, vc, zoom));
        let to_s = |uv: Vec2| -> Pos2 { rect.center() + vec2((uv.x - vc.x) * zoom, (uv.y - vc.y) * zoom) };
        let to_uv = |s: Pos2| -> Vec2 { vc + Vec2::new((s.x - rect.center().x) / zoom, (s.y - rect.center().y) / zoom) };
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_gray(28));
        if let Some(tid) = self.uv_texture(ctx, &td.image) {
            let (a, b) = (to_uv(rect.min), to_uv(rect.max));
            let mut mesh = egui::Mesh::with_texture(tid);
            let tint = Color32::from_rgb(p.tint[0], p.tint[1], p.tint[2]).gamma_multiply(0.85);
            mesh.add_rect_with_uv(rect, Rect::from_min_max(pos2(a.x, a.y), pos2(b.x, b.y)), tint);
            painter.add(Shape::mesh(mesh));
        }
        // tile lines
        let (a, b) = (to_uv(rect.min), to_uv(rect.max));
        if zoom > 12.0 {
            for x in (a.x.floor() as i32)..=(b.x.ceil() as i32) {
                let sx = to_s(Vec2::new(x as f32, 0.0)).x;
                painter.line_segment([pos2(sx, rect.top()), pos2(sx, rect.bottom())], Stroke::new(1.0f32, Color32::from_white_alpha(40)));
            }
            for y in (a.y.floor() as i32)..=(b.y.ceil() as i32) {
                let sy = to_s(Vec2::new(0.0, y as f32)).y;
                painter.line_segment([pos2(rect.left(), sy), pos2(rect.right(), sy)], Stroke::new(1.0f32, Color32::from_white_alpha(40)));
            }
        }
        let orange = Color32::from_rgb(255, 170, 60);
        for tri in &uvs {
            let pts: Vec<Pos2> = tri.iter().map(|v| to_s(*v)).collect();
            painter.add(Shape::closed_line(pts, Stroke::new(1.0f32, orange.gamma_multiply(0.7))));
        }
        // handles around the outline
        let (smin, smax) = (to_s(bmin), to_s(bmax));
        let br = Rect::from_two_pos(smin, smax);
        let corners = [(br.left_top(), Vec2::new(-1.0, -1.0)), (br.right_top(), Vec2::new(1.0, -1.0)), (br.right_bottom(), Vec2::new(1.0, 1.0)), (br.left_bottom(), Vec2::new(-1.0, 1.0))];
        let ring_c = br.center();
        let ring_r = br.size().max_elem() * 0.5 + 18.0;
        if has {
            painter.rect_stroke(br, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::WHITE), StrokeKind::Middle);
            painter.circle_stroke(ring_c, ring_r, Stroke::new(1.5f32, Color32::from_rgb(120, 220, 255)));
            for (c, _) in corners {
                painter.rect_filled(Rect::from_center_size(c, vec2(8.0, 8.0)), CornerRadius::ZERO, Color32::WHITE);
            }
        }

        // --- canvas dragging ---
        let free = ui.input(|i| i.modifiers.command);
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(m) = resp.interact_pointer_pos() {
                let handle = corners
                    .iter()
                    .find(|(c, _)| (*c - m).length() < 9.0)
                    .map(|(_, s)| Handle::Scale(*s))
                    .or_else(|| (((m - ring_c).length() - ring_r).abs() < 7.0).then_some(Handle::Rotate))
                    .unwrap_or(Handle::Move);
                self.push_undo();
                self.uv.drag = Some(CanvasDrag { handle, start: to_uv(m), pivot: center, p0: p });
            }
        }
        if let (Some(d), Some(m)) = (self.uv.drag, resp.interact_pointer_pos()) {
            if resp.dragged_by(egui::PointerButton::Primary) {
                let cur = to_uv(m);
                let np = match d.handle {
                    Handle::Move => {
                        let np = moved(&td, &d.p0, cur - d.start);
                        if free { np } else { snapped(&td, np, 'm') }
                    }
                    Handle::Scale(sign) => {
                        let (a0, a1) = (d.start - d.pivot, cur - d.pivot);
                        let f = if norm_deg(d.p0.rotation).abs() < 0.01 {
                            Vec2::new(if a0.x.abs() > 1e-4 { a1.x / a0.x } else { 1.0 }, if a0.y.abs() > 1e-4 { a1.y / a0.y } else { 1.0 })
                        } else {
                            Vec2::splat(a1.length() / a0.length().max(1e-4))
                        };
                        let f = if ui.input(|i| i.modifiers.shift) { Vec2::splat(f.x.abs().max(f.y.abs())) } else { f.abs() };
                        let _ = sign;
                        let np = scaled(&td, &d.p0, d.pivot, f);
                        if free {
                            np
                        } else {
                            // snap the scale, then keep the pivot where it was
                            let s = snapped(&td, np, 's');
                            scaled(&td, &d.p0, d.pivot, d.p0.scale / Vec2::new(nz(s.scale.x), nz(s.scale.y)))
                        }
                    }
                    Handle::Rotate => {
                        let (a0, a1) = (d.start - d.pivot, cur - d.pivot);
                        let mut deg = (a1.y.atan2(a1.x) - a0.y.atan2(a0.x)).to_degrees();
                        if !free {
                            deg = snap_to(d.p0.rotation + deg, 15.0) - d.p0.rotation;
                        }
                        rotated(&td, &d.p0, d.pivot, deg)
                    }
                };
                self.apply_uv_all(&targets, np);
            }
        }
        if resp.drag_stopped() {
            self.uv.drag = None;
        }
        ui.label(RichText::new("Drag inside: move · corners: scale (Shift = even) · ring: rotate · Ctrl: no snap · scroll: zoom. In the 3D view, Alt+drag a face slides its texture, Alt+Shift+drag turns it.").small().weak());

        // --- buttons + numbers ---
        let mut np: Option<TexParams> = None;
        ui.horizontal(|ui| {
            if ui.button("Fit to face").on_hover_text("Stretch the texture exactly once across the face (the biggest face, for a whole object)").clicked() {
                let face = t0.face().or_else(|| largest_face(&obj, &mut self.models));
                np = fit_params(&obj, face, &td, &p, &mut self.models);
            }
            if ui.button("Rotate 90°").clicked() {
                np = Some(rotated(&td, &p, center, 90.0));
            }
            if ui.button("Reset").clicked() {
                np = Some(TexParams { tint: p.tint, ..TexParams::default() });
            }
        });
        let mut edit = p;
        if tex_params_ui(ui, &mut edit, "uv_params") {
            np = Some(edit);
        }
        if let Some(n) = np {
            if !self.inspector_editing {
                self.push_undo();
                self.inspector_editing = true;
            }
            self.apply_uv_all(&targets, n);
        }
    }

    /// Alt+drag in the 3D view: start sliding (or turning, with Shift) the
    /// texture of the face under the mouse. Returns true if it took the drag.
    pub(crate) fn uv_view_drag_start(&mut self, scene: &[std::sync::Arc<crate::mesh::WorldMesh>], o: Vec3, d: Vec3, mouse_x: f32, rotate: bool) -> bool {
        let Some(h) = crate::view3d::pick_ray(scene, o, d) else { return false };
        let Some(wm) = scene.iter().find(|w| w.id == h.id) else { return false };
        let Some(obj) = self.map.get(h.id).cloned() else { return false };
        let face = tri_face_key(&obj, &wm.mesh, h.tri);
        let target = match face {
            Some(f) if self.face_sel.contains(&(h.id, f)) || self.tool == Tool::Face => {
                if !self.face_sel.contains(&(h.id, f)) {
                    self.face_sel = vec![(h.id, f)];
                    self.selection = vec![h.id];
                }
                UvTarget::Face(h.id, f)
            }
            _ => {
                if !self.selection.contains(&h.id) {
                    self.selection = vec![h.id];
                }
                UvTarget::Object(h.id)
            }
        };
        let Some((p0, texid)) = self.uv_params(target) else { return false };
        let Some(td) = self.texdef(texid) else {
            self.status = "That has no texture to move".into();
            return false;
        };
        let tr = wm.mesh.tris[h.tri];
        let sc = obj.scale();
        let lp = [wm.mesh.pos[tr[0] as usize] * sc, wm.mesh.pos[tr[1] as usize] * sc, wm.mesh.pos[tr[2] as usize] * sc];
        let (_, u, v) = box_uv(lp, Vec2::ONE, Vec2::ZERO);
        self.push_undo();
        self.uv.vdrag = Some(ViewDrag { target, p0, plane_p: h.point, plane_n: h.normal, inv_rot: obj.rotation().inverse(), u, v, ts: tile(&td, &p0), toff: td.offset, start_x: mouse_x, rotate });
        true
    }

    /// Continues the Alt+drag with the mouse ray (o, d).
    pub(crate) fn uv_view_drag(&mut self, o: Vec3, d: Vec3, mouse_x: f32, free: bool) {
        let Some(vd) = self.uv.vdrag else { return };
        let Some((_, texid)) = self.uv_params(vd.target) else { return };
        let Some(td) = self.texdef(texid) else { return };
        let mut p = vd.p0;
        if vd.rotate {
            let mut deg = (mouse_x - vd.start_x) * 0.5;
            if !free {
                deg = snap_to(vd.p0.rotation + deg, 15.0) - vd.p0.rotation;
            }
            // turn around the grabbed point
            let lp = vd.inv_rot * (vd.plane_p - self.map.get(vd.target.obj()).map(|o| o.pos).unwrap_or(Vec3::ZERO));
            let pivot = rot2((Vec2::new(lp.dot(vd.u), -lp.dot(vd.v)) + vd.toff + vd.p0.offset) / vd.ts, vd.p0.rotation);
            p = rotated(&td, &vd.p0, pivot, deg);
        } else {
            let denom = d.dot(vd.plane_n);
            if denom.abs() < 1e-6 {
                return;
            }
            let t = (vd.plane_p - o).dot(vd.plane_n) / denom;
            if t <= 0.0 {
                return;
            }
            let dl = vd.inv_rot * (o + d * t - vd.plane_p);
            p.offset = vd.p0.offset - Vec2::new(dl.dot(vd.u), -dl.dot(vd.v));
            if !free {
                p = snapped(&td, p, 'm');
            }
        }
        self.set_uv_params(vd.target, p);
    }
}
