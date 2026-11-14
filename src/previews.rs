//! Previews before you click: the door / window opening outline, a ghost of
//! whatever the click would place, and the live height of a new polygon.

use crate::app::{AnvilApp, Tool};
use crate::mesh::{world_mesh, WorldMesh};
use crate::view2d::{View2D, ViewKind};
use crate::view3d::{pick_ray, ray_plane};
use eframe::egui::{vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use glam::Vec3;
use std::sync::Arc;

/// Polygon tool, second stage: the outline is done, the mouse sets the height.
#[derive(Clone, Debug)]
pub(crate) struct PolyExtrude {
    pub pts: Vec<Vec3>,
    pub base_y: f32,
    pub height: f32,
    /// Top-view drags: (screen y where it started, height then).
    pub anchor: Option<(f32, f32)>,
}

#[derive(Default)]
pub(crate) struct Ghost {
    pub mesh: Option<Arc<WorldMesh>>,
    key: String,
    pub touched: bool,
}

fn snap(v: f32, g: f32) -> f32 {
    if g > 0.0 { (v / g).round() * g } else { v }
}

/// Shift+drag outline: an exact rectangle (or square) on the ground.
pub fn outline_rect(a: Vec3, b: Vec3, square: bool) -> Vec<Vec3> {
    let mut b = b;
    if square {
        let m = (b.x - a.x).abs().max((b.z - a.z).abs());
        b.x = a.x + if b.x >= a.x { m } else { -m };
        b.z = a.z + if b.z >= a.z { m } else { -m };
    }
    if (b.x - a.x).abs() < 1e-4 || (b.z - a.z).abs() < 1e-4 {
        return Vec::new();
    }
    vec![Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, a.y, b.z), Vec3::new(a.x, a.y, b.z)]
}

/// Shift+drag block: the drag end that makes a perfect square.
pub fn square_end(start: glam::Vec2, end: glam::Vec2) -> glam::Vec2 {
    let d = end - start;
    let m = d.x.abs().max(d.y.abs());
    start + glam::Vec2::new(if d.x >= 0.0 { m } else { -m }, if d.y >= 0.0 { m } else { -m })
}

/// The 12 edges of a box given by its 8 corners (index bits: x, y, z).
pub fn box_edges(c: [Vec3; 8]) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    for i in 0..8usize {
        for b in [1usize, 2, 4] {
            if i & b == 0 {
                out.push((c[i], c[i | b]));
            }
        }
    }
    out
}

fn corners(min: Vec3, max: Vec3, f: impl Fn(Vec3) -> Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| f(Vec3::new(if i & 1 != 0 { max.x } else { min.x }, if i & 2 != 0 { max.y } else { min.y }, if i & 4 != 0 { max.z } else { min.z })))
}

/// Edges of a prism: outline at `base_y` extruded by `h`.
pub fn prism_edges(pts: &[Vec3], base_y: f32, h: f32) -> Vec<(Vec3, Vec3)> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (a0, b0) = (Vec3::new(a.x, base_y, a.z), Vec3::new(b.x, base_y, b.z));
        out.push((a0, b0));
        out.push((a0 + Vec3::Y * h, b0 + Vec3::Y * h));
        out.push((a0, a0 + Vec3::Y * h));
    }
    out
}

/// Height under the mouse for the polygon extrude in 3D: a vertical plane
/// through the outline's center, facing the camera.
pub fn extrude_height_3d(o: Vec3, d: Vec3, pts: &[Vec3], base_y: f32, cam_fwd: Vec3) -> Option<f32> {
    let c = pts.iter().fold(Vec3::ZERO, |a, p| a + *p) / pts.len().max(1) as f32;
    let n = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or_zero();
    if n == Vec3::ZERO {
        return None;
    }
    let denom = d.dot(n);
    if denom.abs() < 1e-5 {
        return None;
    }
    let t = (Vec3::new(c.x, base_y, c.z) - o).dot(n) / denom;
    (t > 0.0).then(|| (o + d * t).y - base_y)
}

const OPEN_COL: Color32 = Color32::from_rgb(255, 205, 70);
const GLASS_COL: Color32 = Color32::from_rgb(140, 210, 255);
const GHOST_COL: Color32 = Color32::from_rgb(120, 220, 255);

impl AnvilApp {
    /// Segments of the opening a door / window click at `at` would cut
    /// (plus the glass pane, flagged true).
    pub(crate) fn opening_preview(&self, id: u64, at: Vec3, snapping: bool) -> Vec<(Vec3, Vec3, bool)> {
        let window = self.tool == Tool::Window;
        let Ok((o, hole, glass)) = self.opening_plan(id, at, window, snapping) else { return Vec::new() };
        let mut out: Vec<(Vec3, Vec3, bool)> = box_edges(corners(hole.min, hole.max, |p| o.to_world(p))).into_iter().map(|(a, b)| (a, b, false)).collect();
        if let Some((gc, gs)) = glass {
            out.extend(box_edges(corners(gc - gs * 0.5, gc + gs * 0.5, |p| o.to_world(p))).into_iter().map(|(a, b)| (a, b, true)));
        }
        out
    }

    fn is_ghost_tool(&self) -> bool {
        matches!(self.tool, Tool::Prop | Tool::Light | Tool::Spawn | Tool::Prompt) || (self.tool == Tool::Block && self.active_preset.is_some())
    }

    /// Updates the placement ghost for a hover at `at` (call from viewport input).
    pub(crate) fn update_ghost(&mut self, at: Vec3, on_surface: bool) {
        if !self.is_ghost_tool() || self.poly_extrude.is_some() {
            return;
        }
        self.ghost.touched = true;
        let key = format!("{at:?}|{on_surface}|{:?}|{:?}|{:?}|{:?}", self.tool, self.active_model, self.active_preset, self.brush_shape);
        if key == self.ghost.key && self.ghost.mesh.is_some() {
            return;
        }
        self.ghost.key = key;
        self.ghost.mesh = self.placement(at, on_surface).ok().map(|o| Arc::new(world_mesh(&o, &self.map.textures, &mut self.models)));
    }

    /// End of frame: drop the ghost when no viewport is hovered.
    pub(crate) fn end_ghost_frame(&mut self) {
        if !self.ghost.touched || !self.is_ghost_tool() {
            self.ghost.mesh = None;
            self.ghost.key.clear();
        }
        self.ghost.touched = false;
    }

    /// Ghost / opening hover handling for the 3D view (call with the hover position).
    pub(crate) fn hover_3d(&mut self, rect: Rect, scene: &[Arc<WorldMesh>], hover: Option<Pos2>, free: bool) {
        let Some(h) = hover else { return };
        let fov = self.settings.fov;
        let (o, d) = self.cam.ray(rect, fov, h);
        if let Some(pe) = &mut self.poly_extrude {
            if let Some(y) = extrude_height_3d(o, d, &pe.pts, pe.base_y, self.cam.forward()) {
                let g = self.settings.grid;
                pe.height = if free { y } else { snap(y, if g > 0.0 { g } else { 0.0 }) };
            }
            return;
        }
        if self.is_ghost_tool() && matches!(self.drag, crate::app::Drag::None) {
            let g = self.snap_step();
            let pt = pick_ray(scene, o, d).map(|h| h.point).or_else(|| ray_plane(o, d, self.work_y));
            if let Some(pt) = pt {
                self.update_ghost(Vec3::new(snap(pt.x, g), pt.y, snap(pt.z, g)), true);
            }
        }
    }

    /// Ghost / extrude hover handling for a 2D view.
    pub(crate) fn hover_2d(&mut self, view: &View2D, rect: Rect, hover: Option<Pos2>, free: bool) {
        let Some(h) = hover.filter(|h| rect.contains(*h)) else { return };
        let g = self.snap_step();
        if let Some(pe) = &mut self.poly_extrude {
            let y = match view.kind {
                ViewKind::Top => {
                    let (sy, h0) = *pe.anchor.get_or_insert((h.y, pe.height));
                    h0 + (sy - h.y) / view.zoom.max(1e-4)
                }
                _ => view.uv_to_world(view.screen_to_uv(rect, h), 0.0).y - pe.base_y,
            };
            pe.height = if free { y } else { snap(y, g) };
            return;
        }
        if self.is_ghost_tool() && matches!(self.drag, crate::app::Drag::None) {
            let uv = view.screen_to_uv(rect, h);
            let uv = if g > 0.0 { (uv / g).round() * g } else { uv };
            let top = view.kind == ViewKind::Top;
            self.update_ghost(view.uv_to_world(uv, if top { self.work_y } else { 0.0 }), top);
        }
    }

    /// Clicks while the polygon height is being set finish it. True if used.
    pub(crate) fn extrude_click(&mut self) -> bool {
        let Some(pe) = self.poly_extrude.take() else { return false };
        let (base, h) = if pe.height < 0.0 { (pe.base_y + pe.height, -pe.height) } else { (pe.base_y, pe.height) };
        let h = h.max(0.05);
        self.settings.polygon_height = h;
        let _ = self.settings.save();
        self.finish_polygon(&pe.pts, base, h);
        true
    }

    pub(crate) fn paint_previews_3d(&self, painter: &Painter, rect: Rect, scene: &[Arc<WorldMesh>], hover: Option<Pos2>, pressed: bool, free: bool) {
        let fov = self.settings.fov;
        let seg = |a: Vec3, b: Vec3, s: Stroke| {
            if let Some((p, q)) = self.cam.project_segment(rect, fov, a, b) {
                painter.line_segment([p, q], s);
            }
        };
        if let Some(pe) = &self.poly_extrude {
            for (a, b) in prism_edges(&pe.pts, pe.base_y, pe.height) {
                seg(a, b, Stroke::new(1.5f32, GHOST_COL));
            }
            let top = pe.pts.iter().fold(Vec3::ZERO, |a, p| a + *p) / pe.pts.len() as f32;
            if let Some(s) = self.cam.project(rect, fov, Vec3::new(top.x, pe.base_y + pe.height, top.z)) {
                painter.text(s - vec2(0.0, 8.0), Align2::CENTER_BOTTOM, format!("height {:.2}", pe.height), FontId::proportional(13.0), GHOST_COL);
            }
            return;
        }
        if let crate::app::Drag::Outline3D { start, square } = self.drag {
            let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
            if let Some(p) = ray_plane(o, d, start.y) {
                let g = self.snap_step();
                let pts = outline_rect(start, Vec3::new(snap(p.x, g), start.y, snap(p.z, g)), square);
                for i in 0..pts.len() {
                    seg(pts[i], pts[(i + 1) % pts.len()], Stroke::new(2.0f32, GHOST_COL));
                }
            }
        }
        if let (Some(h), true) = (hover, matches!(self.tool, Tool::Door | Tool::Window)) {
            let (o, d) = self.cam.ray(rect, fov, h);
            if let Some(hit) = pick_ray(scene, o, d) {
                let a = if pressed { 255 } else { 110 };
                for (p, q, glass) in self.any_opening_preview(scene, hit.id, hit.point, (o, d), !free) {
                    let c = if glass { GLASS_COL } else { OPEN_COL };
                    seg(p, q, Stroke::new(if pressed { 2.0f32 } else { 1.0 }, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)));
                }
            }
        }
        self.paint_ghost(|a, b| seg(a, b, Stroke::new(1.0f32, GHOST_COL.gamma_multiply(0.8))));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_previews_2d(&self, painter: &Painter, vi: usize, view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>], hover: Option<Pos2>, pressed: bool, free: bool) {
        let seg = |a: Vec3, b: Vec3, s: Stroke| {
            painter.line_segment([view.world_to_screen(rect, a), view.world_to_screen(rect, b)], s);
        };
        if let crate::app::Drag::Outline2D { view: dv, start, square } = self.drag {
            if dv == vi {
                let g = self.snap_step();
                let uv = view.screen_to_uv(rect, self.drag_cur);
                let uv = if g > 0.0 && !free { (uv / g).round() * g } else { uv };
                let pts = outline_rect(start, view.uv_to_world(uv, start.y), square);
                for i in 0..pts.len() {
                    seg(pts[i], pts[(i + 1) % pts.len()], Stroke::new(2.0f32, GHOST_COL));
                }
            }
        }
        if let Some(pe) = &self.poly_extrude {
            for (a, b) in prism_edges(&pe.pts, pe.base_y, pe.height) {
                seg(a, b, Stroke::new(1.5f32, GHOST_COL));
            }
            return;
        }
        if let (Some(h), true) = (hover.filter(|h| rect.contains(*h)), matches!(self.tool, Tool::Door | Tool::Window)) {
            if let Some(id) = crate::view2d::pick(view, rect, scene, h) {
                let (_, _, _, _, da) = view.axes();
                let depth = self.map.get(id).map(|o| o.pos[da]).unwrap_or(0.0);
                let at = view.uv_to_world(view.screen_to_uv(rect, h), depth);
                let a = if pressed { 255 } else { 110 };
                for (p, q, glass) in self.any_opening_preview(scene, id, at, crate::mesh_openings::view_ray(view, at), !free) {
                    let c = if glass { GLASS_COL } else { OPEN_COL };
                    seg(p, q, Stroke::new(if pressed { 2.0f32 } else { 1.0 }, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)));
                }
            }
        }
        self.paint_ghost(|a, b| seg(a, b, Stroke::new(1.0f32, GHOST_COL.gamma_multiply(0.8))));
    }

    fn paint_ghost(&self, mut seg: impl FnMut(Vec3, Vec3)) {
        let Some(g) = &self.ghost.mesh else { return };
        if g.mesh.edges.is_empty() || g.mesh.edges.len() > 4000 {
            for (a, b) in box_edges(corners(g.min, g.max, |p| p)) {
                seg(a, b);
            }
        } else {
            for e in &g.mesh.edges {
                seg(g.pos[e[0] as usize], g.pos[e[1] as usize]);
            }
        }
    }
}
