//! Mesh edit mode: select and move vertices or faces of an editable mesh,
//! extrude / delete / flip faces and merge vertices.

use std::sync::Arc;
use crate::app::{fmt_num, paint_cmds, snap, snap2, vec3_edit, AnvilApp, Drag};
use crate::gizmo::{self, GDrag};
use crate::plugins::ViewInfo;
use crate::map::{Kind, Shape};
use crate::mesh::{brush_mesh, WorldMesh};
use crate::polymesh::{self, box_poly, triangulate, weld};
use crate::view2d::{in_triangle, View2D};
use crate::view3d::{pick_ray_where, ray_plane};
use eframe::egui::{self, Color32, Painter, PointerButton, Pos2, Rect, RichText, Stroke};
use glam::{Mat3, Vec3};

#[derive(Clone, Debug)]
pub(crate) struct EditState {
    pub obj: u64,
    pub face_mode: bool,
    pub verts: Vec<u32>,
    pub faces: Vec<usize>,
}

type Proj<'a> = &'a dyn Fn(Vec3) -> Option<Pos2>;

fn pick_vert(world: &[Vec3], proj: Proj, p: Pos2) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for (i, w) in world.iter().enumerate() {
        if let Some(s) = proj(*w) {
            let d = (s - p).length();
            if d < 8.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, i as u32));
            }
        }
    }
    best.map(|b| b.1)
}

fn pick_face(world: &[Vec3], faces: &[Vec<u32>], proj: Proj, p: Pos2) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (fi, f) in faces.iter().enumerate() {
        if f.iter().any(|&i| i as usize >= world.len()) {
            continue;
        }
        let pts: Vec<Option<Pos2>> = f.iter().map(|&i| proj(world[i as usize])).collect();
        if pts.iter().any(|q| q.is_none()) {
            continue;
        }
        let inside = triangulate(world, f).iter().any(|t| {
            let q = |i: u32| proj(world[i as usize]).unwrap_or(Pos2::ZERO);
            in_triangle(p, q(t[0]), q(t[1]), q(t[2]))
        });
        if inside {
            let r = Rect::from_points(&pts.iter().flatten().copied().collect::<Vec<_>>());
            let area = r.area().max(1.0);
            if best.is_none_or(|b| area < b.0) {
                best = Some((area, fi));
            }
        }
    }
    best.map(|b| b.1)
}

impl AnvilApp {
    /// Turns a brush into an editable polygon mesh.
    pub(crate) fn convert_to_mesh(&mut self, id: u64) -> bool {
        let Some(o) = self.map.get(id).cloned() else { return false };
        let Kind::Brush { shape, size, steps, holes } = &o.kind else {
            self.status = "Only brushes can be converted to editable meshes".into();
            return false;
        };
        let (verts, faces) = if *shape == Shape::Box && holes.is_empty() {
            box_poly(*size)
        } else {
            weld(&brush_mesh(*shape, *size, *steps, holes))
        };
        self.push_undo();
        if let Some(obj) = self.map.get_mut(id) {
            obj.kind = Kind::Mesh { verts, faces };
        }
        self.status = "Converted to an editable mesh. Tab enters edit mode.".into();
        true
    }

    pub(crate) fn toggle_edit(&mut self) {
        if self.edit.is_some() {
            self.edit = None;
            self.drag = Drag::None;
            self.status = "Left mesh edit mode".into();
            return;
        }
        let Some(&id) = self.selection.first() else {
            self.status = "Select a mesh (or a brush to convert) first".into();
            return;
        };
        let is_brush = matches!(self.map.get(id).map(|o| &o.kind), Some(Kind::Brush { .. }));
        if is_brush && !self.convert_to_mesh(id) {
            return;
        }
        if !matches!(self.map.get(id).map(|o| &o.kind), Some(Kind::Mesh { .. })) {
            self.status = "Only meshes and brushes can be edited by vertex / face".into();
            return;
        }
        self.selection = vec![id];
        self.edit = Some(EditState { obj: id, face_mode: false, verts: Vec::new(), faces: Vec::new() });
        self.drag = Drag::None;
        self.status = "Mesh edit: 1 vertices, 2 faces, E extrude, M merge, N flip, Del delete, Tab done".into();
    }

    pub(crate) fn set_edit_mode(&mut self, face: bool) {
        if let Some(e) = &mut self.edit {
            e.face_mode = face;
        }
    }

    /// (rotation, world verts, local verts, faces) of the mesh being edited.
    fn edit_geometry(&self) -> Option<(Mat3, Vec<Vec3>, Vec<Vec3>, Vec<Vec<u32>>)> {
        let e = self.edit.as_ref()?;
        let o = self.map.get(e.obj)?;
        let Kind::Mesh { verts, faces } = &o.kind else { return None };
        let m = o.matrix();
        let world = verts.iter().map(|v| m.transform_point3(*v)).collect();
        Some((Mat3::from_quat(o.rotation()), world, verts.clone(), faces.clone()))
    }

    fn moving_verts(&self, faces: &[Vec<u32>]) -> Vec<u32> {
        let Some(e) = &self.edit else { return Vec::new() };
        if e.face_mode {
            let mut v: Vec<u32> = e.faces.iter().filter_map(|f| faces.get(*f)).flatten().copied().collect();
            v.sort_unstable();
            v.dedup();
            v
        } else {
            e.verts.clone()
        }
    }

    fn edit_apply_pick(&mut self, vert: Option<u32>, face: Option<usize>, add: bool) {
        let Some(e) = self.edit.as_mut() else { return };
        if e.face_mode {
            match face {
                Some(f) if add => {
                    if let Some(i) = e.faces.iter().position(|x| *x == f) {
                        e.faces.remove(i);
                    } else {
                        e.faces.push(f);
                    }
                }
                Some(f) => e.faces = vec![f],
                None if !add => e.faces.clear(),
                None => {}
            }
        } else {
            match vert {
                Some(v) if add => {
                    if let Some(i) = e.verts.iter().position(|x| *x == v) {
                        e.verts.remove(i);
                    } else {
                        e.verts.push(v);
                    }
                }
                Some(v) => e.verts = vec![v],
                None if !add => e.verts.clear(),
                None => {}
            }
        }
    }

    /// Makes sure the element under the cursor is selected before a drag.
    fn edit_ensure_selected(&mut self, vert: Option<u32>, face: Option<usize>, add: bool) {
        let Some(e) = self.edit.as_mut() else { return };
        if e.face_mode {
            if let Some(f) = face {
                if !e.faces.contains(&f) {
                    if !add {
                        e.faces.clear();
                    }
                    e.faces.push(f);
                }
            }
        } else if let Some(v) = vert {
            if !e.verts.contains(&v) {
                if !add {
                    e.verts.clear();
                }
                e.verts.push(v);
            }
        }
    }

    fn edit_set_verts(&mut self, rot: Mat3, origs: &[(u32, Vec3)], wd: Vec3) {
        let ld = rot.transpose() * wd;
        let Some(e) = &self.edit else { return };
        if let Some(o) = self.map.get_mut(e.obj) {
            if let Kind::Mesh { verts, .. } = &mut o.kind {
                for (i, p) in origs {
                    if let Some(v) = verts.get_mut(*i as usize) {
                        *v = *p + ld;
                    }
                }
            }
        }
    }

    fn edit_marquee_select(&mut self, r: Rect, proj: Proj, add: bool) {
        let Some((_, world, _, faces)) = self.edit_geometry() else { return };
        let inside = |i: u32| world.get(i as usize).and_then(|w| proj(*w)).is_some_and(|s| r.contains(s));
        let Some(e) = self.edit.as_mut() else { return };
        if e.face_mode {
            if !add {
                e.faces.clear();
            }
            for (fi, f) in faces.iter().enumerate() {
                if f.iter().all(|&i| inside(i)) && !e.faces.contains(&fi) {
                    e.faces.push(fi);
                }
            }
        } else {
            if !add {
                e.verts.clear();
            }
            for i in 0..world.len() as u32 {
                if inside(i) && !e.verts.contains(&i) {
                    e.verts.push(i);
                }
            }
        }
    }

    pub(crate) fn edit_input_2d(&mut self, vi: usize, rect: Rect, resp: &egui::Response, _scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        let view: View2D = self.views[vi];
        let mods = ctx.input(|i| i.modifiers);
        let add = mods.command || mods.shift;
        let g = self.snap_step();
        if self.edit_gizmo_input(vi + 1, &ViewInfo::Ortho { view, rect }, resp, ctx) {
            return;
        }
        let Some((rot, world, local, faces)) = self.edit_geometry() else { return };
        let face_mode = self.edit.as_ref().is_some_and(|e| e.face_mode);
        let proj = |w: Vec3| Some(view.world_to_screen(rect, w));

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (v, f) = if face_mode { (None, pick_face(&world, &faces, &proj, p)) } else { (pick_vert(&world, &proj, p), None) };
                self.edit_apply_pick(v, f, add);
            }
        }
        if resp.drag_started_by(PointerButton::Primary) {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
            self.drag_cur = start;
            let (v, f) = if face_mode { (None, pick_face(&world, &faces, &proj, start)) } else { (pick_vert(&world, &proj, start), None) };
            if v.is_some() || f.is_some() {
                self.edit_ensure_selected(v, f, add);
                self.push_undo();
                let origs = self.moving_verts(&faces).into_iter().filter_map(|i| local.get(i as usize).map(|p| (i, *p))).collect();
                self.drag = Drag::EditMove2D { view: vi, start_uv: view.screen_to_uv(rect, start), origs };
            } else {
                self.drag = Drag::EditMarquee { view: Some(vi), start };
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            if let Drag::EditMove2D { view: dv, start_uv, origs } = &self.drag {
                if *dv == vi {
                    let d = snap2(view.screen_to_uv(rect, self.drag_cur) - *start_uv, g);
                    let origs = origs.clone();
                    self.edit_set_verts(rot, &origs, view.uv_delta_to_world(d));
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let Drag::EditMarquee { view: Some(dv), start } = std::mem::replace(&mut self.drag, Drag::None) {
                if dv == vi {
                    self.edit_marquee_select(Rect::from_two_pos(start, self.drag_cur), &proj, add);
                }
            }
        }
    }

    pub(crate) fn edit_input_3d(&mut self, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        let fov = self.settings.fov;
        let cam = self.cam;
        let mods = ctx.input(|i| i.modifiers);
        let add = mods.command || mods.shift;
        let g = self.snap_step();
        if self.edit_gizmo_input(0, &ViewInfo::Persp { cam, rect, fov }, resp, ctx) {
            return;
        }
        let Some((rot, world, local, faces)) = self.edit_geometry() else { return };
        let Some(obj) = self.edit.as_ref().map(|e| e.obj) else { return };
        let face_mode = self.edit.as_ref().is_some_and(|e| e.face_mode);
        let proj = |w: Vec3| cam.project(rect, fov, w);
        let pick = |p: Pos2| -> (Option<u32>, Option<usize>, Option<Vec3>) {
            if face_mode {
                let (o, d) = cam.ray(rect, fov, p);
                let hit = pick_ray_where(scene, o, d, |w| w.id != obj);
                let wm = scene.iter().find(|w| w.id == obj);
                match (hit, wm) {
                    (Some(h), Some(wm)) => (None, wm.mesh.tri_face.get(h.tri).map(|f| *f as usize), Some(h.point)),
                    _ => (None, None, None),
                }
            } else {
                let v = pick_vert(&world, &proj, p);
                (v, None, v.and_then(|i| world.get(i as usize).copied()))
            }
        };

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (v, f, _) = pick(p);
                self.edit_apply_pick(v, f, add);
            }
        }
        if resp.drag_started_by(PointerButton::Primary) {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
            self.drag_cur = start;
            let (v, f, at) = pick(start);
            match at {
                Some(at) if v.is_some() || f.is_some() => {
                    self.edit_ensure_selected(v, f, add);
                    self.push_undo();
                    let origs = self.moving_verts(&faces).into_iter().filter_map(|i| local.get(i as usize).map(|p| (i, *p))).collect();
                    self.drag = Drag::EditMove3D { plane_y: at.y, start: at, origs };
                }
                _ => self.drag = Drag::EditMarquee { view: None, start },
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            if let Drag::EditMove3D { plane_y, start, origs } = &self.drag {
                let (o, d) = cam.ray(rect, fov, self.drag_cur);
                if let Some(p) = ray_plane(o, d, *plane_y) {
                    let dd = p - *start;
                    let wd = Vec3::new(snap(dd.x, g), 0.0, snap(dd.z, g));
                    let origs = origs.clone();
                    self.edit_set_verts(rot, &origs, wd);
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let Drag::EditMarquee { view: None, start } = std::mem::replace(&mut self.drag, Drag::None) {
                self.edit_marquee_select(Rect::from_two_pos(start, self.drag_cur), &proj, add);
            }
        }
    }

    fn paint_edit(&self, painter: &Painter, proj: Proj) {
        let Some((_, world, _, faces)) = self.edit_geometry() else { return };
        let Some(e) = &self.edit else { return };
        let fill = Color32::from_rgba_unmultiplied(255, 150, 40, 90);
        for &fi in &e.faces {
            let Some(f) = faces.get(fi) else { continue };
            for t in triangulate(&world, f) {
                let q: Vec<Option<Pos2>> = t.iter().map(|&i| world.get(i as usize).and_then(|w| proj(*w))).collect();
                if let [Some(a), Some(b), Some(c)] = q[..] {
                    painter.add(egui::Shape::convex_polygon(vec![a, b, c], fill, Stroke::NONE));
                }
            }
        }
        if e.face_mode {
            for (fi, f) in faces.iter().enumerate() {
                if f.iter().any(|&i| i as usize >= world.len()) {
                    continue;
                }
                if let Some(c) = proj(polymesh::face_center(&world, f)) {
                    let sel = e.faces.contains(&fi);
                    painter.circle_filled(c, if sel { 4.0 } else { 2.5 }, if sel { Color32::from_rgb(255, 170, 40) } else { Color32::from_gray(200) });
                }
            }
        } else {
            for (i, w) in world.iter().enumerate() {
                if let Some(s) = proj(*w) {
                    let sel = e.verts.contains(&(i as u32));
                    let r = Rect::from_center_size(s, egui::vec2(6.0, 6.0));
                    painter.rect_filled(r, egui::CornerRadius::ZERO, if sel { Color32::from_rgb(255, 210, 50) } else { Color32::WHITE });
                    painter.rect_stroke(r, egui::CornerRadius::ZERO, Stroke::new(1.0f32, Color32::BLACK), egui::StrokeKind::Inside);
                }
            }
        }
    }

    pub(crate) fn paint_edit_2d(&self, painter: &Painter, view: &View2D, rect: Rect) {
        let proj = |w: Vec3| Some(view.world_to_screen(rect, w));
        self.paint_edit(painter, &proj);
        self.paint_edit_gizmo(painter, ViewInfo::Ortho { view: *view, rect });
    }

    /// Center and axes for the vertex / face gizmo (None = nothing selected).
    fn edit_gizmo_frame(&self) -> Option<(Vec3, [Vec3; 3])> {
        let (rot, world, _, faces) = self.edit_geometry()?;
        let ids = self.moving_verts(&faces);
        if ids.is_empty() {
            return None;
        }
        let pts: Vec<Vec3> = ids.iter().filter_map(|i| world.get(*i as usize).copied()).collect();
        if pts.is_empty() {
            return None;
        }
        let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
        let axes = if self.gizmo_local { [rot.col(0).normalize(), rot.col(1).normalize(), rot.col(2).normalize()] } else { [Vec3::X, Vec3::Y, Vec3::Z] };
        Some((c, axes))
    }

    fn paint_edit_gizmo(&self, painter: &Painter, view: ViewInfo) {
        let Some((c, axes)) = self.edit_gizmo_frame() else { return };
        let (c, hot) = match &self.drag {
            Drag::EditGizmo { g, .. } => (c, Some(g.axis)),
            _ => (c, None),
        };
        paint_cmds(painter, view, &gizmo::draw(&view, self.gizmo_mode, c, axes, hot));
    }

    /// Dragging the vertex / face gizmo. Returns true when it owns the input.
    fn edit_gizmo_input(&mut self, slot: usize, view: &ViewInfo, resp: &egui::Response, ctx: &egui::Context) -> bool {
        let here = matches!(&self.drag, Drag::EditGizmo { slot: s, .. } if *s == slot);
        if resp.drag_started_by(PointerButton::Primary) && !here {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(resp.rect.center());
            let Some((c, axes)) = self.edit_gizmo_frame() else { return false };
            let Some(axis) = gizmo::hit(view, self.gizmo_mode, c, axes, start) else { return false };
            let Some((_, world, _, faces)) = self.edit_geometry() else { return false };
            let starts: Vec<(u32, Vec3)> = self.moving_verts(&faces).into_iter().filter_map(|i| world.get(i as usize).map(|w| (i, *w))).collect();
            self.push_undo();
            self.drag = Drag::EditGizmo { slot, g: GDrag::begin(view, self.gizmo_mode, axis, c, axes, start), starts };
            self.drag_cur = start;
            return true;
        }
        if !here {
            return false;
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            let snap_on = !ctx.input(|i| i.modifiers.ctrl);
            let grid = self.snap_step();
            if let Drag::EditGizmo { g, starts, .. } = &self.drag {
                let xf = g.xf(view, self.drag_cur, grid, snap_on);
                let starts = starts.clone();
                if let Some(e) = &self.edit {
                    if let Some(o) = self.map.get_mut(e.obj) {
                        let inv = o.rotation().inverse();
                        let pos = o.pos;
                        if let Kind::Mesh { verts, .. } = &mut o.kind {
                            for (i, w0) in starts {
                                if let Some(v) = verts.get_mut(i as usize) {
                                    *v = inv * (xf.apply(w0) - pos);
                                }
                            }
                        }
                    }
                }
                self.status = match xf {
                    gizmo::Xf::Move(d) => format!("Move  x {} y {} z {}  (Ctrl = free)", fmt_num(d.x), fmt_num(d.y), fmt_num(d.z)),
                    gizmo::Xf::Rotate { deg, .. } => format!("Rotate {} degrees  (Ctrl = free)", fmt_num(deg)),
                    gizmo::Xf::Scale { f, .. } => format!("Scale x{}", fmt_num(f)),
                };
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            self.drag = Drag::None;
        }
        true
    }

    pub(crate) fn paint_edit_3d(&self, painter: &Painter, rect: Rect) {
        let cam = self.cam;
        let fov = self.settings.fov;
        let proj = |w: Vec3| cam.project(rect, fov, w);
        self.paint_edit(painter, &proj);
        self.paint_edit_gizmo(painter, ViewInfo::Persp { cam, rect, fov });
    }

    fn with_mesh(&mut self, f: impl FnOnce(&mut Vec<Vec3>, &mut Vec<Vec<u32>>, &mut EditState)) {
        let Some(e) = self.edit.as_mut() else { return };
        let Some(o) = self.map.objects.iter_mut().find(|o| o.id == e.obj) else { return };
        if let Kind::Mesh { verts, faces } = &mut o.kind {
            f(verts, faces, e);
        }
    }

    pub(crate) fn edit_select_all(&mut self) {
        let Some((_, world, _, faces)) = self.edit_geometry() else { return };
        if let Some(e) = &mut self.edit {
            if e.face_mode {
                e.faces = (0..faces.len()).collect();
            } else {
                e.verts = (0..world.len() as u32).collect();
            }
        }
    }

    pub(crate) fn edit_extrude(&mut self) {
        let ok = self.edit.as_ref().is_some_and(|e| e.face_mode && !e.faces.is_empty());
        if !ok {
            self.status = "Extrude: switch to face mode (2) and select faces".into();
            return;
        }
        self.push_undo();
        let dist = self.settings.grid.max(0.01);
        self.with_mesh(|v, f, e| polymesh::extrude(v, f, &e.faces, dist));
        self.status = format!("Extruded by {} (one grid step). Drag to move it further.", fmt_num(dist));
    }

    pub(crate) fn edit_delete(&mut self) {
        let Some(e) = &self.edit else { return };
        if e.verts.is_empty() && e.faces.is_empty() {
            return;
        }
        self.push_undo();
        if let Some(e) = self.edit.clone() {
            if e.face_mode {
                self.drop_face_textures(e.obj, &e.faces);
            }
        }
        self.with_mesh(|v, f, e| {
            if e.face_mode {
                polymesh::delete_faces(f, &e.faces);
                polymesh::cleanup(v, f);
            } else {
                polymesh::delete_verts(v, f, &e.verts);
            }
            e.verts.clear();
            e.faces.clear();
        });
    }

    pub(crate) fn edit_flip(&mut self) {
        if self.edit.as_ref().is_none_or(|e| e.faces.is_empty()) {
            return;
        }
        self.push_undo();
        self.with_mesh(|_, f, e| polymesh::flip(f, &e.faces));
    }

    pub(crate) fn edit_merge(&mut self) {
        if self.edit.as_ref().is_none_or(|e| e.verts.len() < 2) {
            self.status = "Merge: select two or more vertices".into();
            return;
        }
        self.push_undo();
        self.with_mesh(|v, f, e| {
            polymesh::merge_verts(v, f, &e.verts);
            e.verts.clear();
            e.faces.clear();
        });
    }

    pub(crate) fn edit_panel(&mut self, ui: &mut egui::Ui) {
        let Some(e) = self.edit.clone() else { return };
        let name = self.map.get(e.obj).map(|o| o.name.clone()).unwrap_or_default();
        let (nv, nf) = match self.map.get(e.obj).map(|o| &o.kind) {
            Some(Kind::Mesh { verts, faces }) => (verts.len(), faces.len()),
            _ => (0, 0),
        };
        ui.label(RichText::new(format!("Editing {name}  ({nv} vertices, {nf} faces)")).strong());
        ui.horizontal(|ui| {
            if ui.selectable_label(!e.face_mode, "1 Vertices").clicked() {
                self.set_edit_mode(false);
            }
            if ui.selectable_label(e.face_mode, "2 Faces").clicked() {
                self.set_edit_mode(true);
            }
        });
        ui.label(
            RichText::new(if e.face_mode {
                format!("{} face(s) selected. Drag a face to move it; E extrudes by one grid step.", e.faces.len())
            } else {
                format!("{} vertex(es) selected. Drag in 2D views to move (Front/Side for height).", e.verts.len())
            })
            .small()
            .weak(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Select all").clicked() {
                self.edit_select_all();
            }
            if e.face_mode {
                if ui.button("Extrude (E)").clicked() {
                    self.edit_extrude();
                }
                if ui.button("Flip (N)").clicked() {
                    self.edit_flip();
                }
            } else if ui.button("Merge (M)").clicked() {
                self.edit_merge();
            }
            if ui.button("Delete (Del)").clicked() {
                self.edit_delete();
            }
            if ui.button("Done (Tab)").clicked() {
                self.toggle_edit();
            }
        });
        if e.face_mode && !e.faces.is_empty() {
            ui.separator();
            let targets: Vec<(u64, usize)> = e.faces.iter().map(|f| (e.obj, *f)).collect();
            self.face_tex_panel(ui, targets);
        }
        // exact position of a single vertex
        if !e.face_mode && e.verts.len() == 1 {
            let vi = e.verts[0] as usize;
            let before: Vec<crate::map::Object> = self.map.get(e.obj).cloned().into_iter().collect();
            let mut changed = false;
            if let Some(o) = self.map.get_mut(e.obj) {
                if let Kind::Mesh { verts, .. } = &mut o.kind {
                    if let Some(v) = verts.get_mut(vi) {
                        ui.label("Vertex position (local)");
                        changed = vec3_edit(ui, v, 0.05, None);
                    }
                }
            }
            if changed && !self.inspector_editing {
                self.push_undo_restoring(before);
                self.inspector_editing = true;
            }
        }
    }
}
