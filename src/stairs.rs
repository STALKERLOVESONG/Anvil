//! Stair maker: click the bottom and the top, get a staircase with side
//! trim (stringers) and a handrail (posts + rail). Steps and posts can be
//! swapped for prefabs saved from any selection.

use crate::app::{apply_scale, AnvilApp};
use crate::map::{Map, Material, Object, Shape};
use crate::mesh::{build_scene, ModelCache, WorldMesh};
use crate::view2d::{View2D, ViewKind};
use crate::view3d::{pick_ray, ray_plane};
use eframe::egui::{self, Color32, Painter, Pos2, Rect, RichText, Stroke};
use glam::{Quat, Vec3};
use std::sync::Arc;

pub const RAIL_LEFT: u8 = 1;
pub const RAIL_RIGHT: u8 = 2;

#[derive(Clone, Debug)]
pub struct StairSpec {
    pub width: f32,
    pub step_height: f32,
    /// Height used when both clicks are at the same level.
    pub rise: f32,
    /// Bit flags: RAIL_LEFT | RAIL_RIGHT.
    pub rail: u8,
    pub rail_height: f32,
    pub stringers: bool,
    pub color: [u8; 3],
    pub material: Material,
    pub trim_color: [u8; 3],
}

/// Where the stairs go: bottom front-center point, facing `yaw` (radians,
/// local +Z runs up the stairs), `run` long, `height` tall, `n` steps.
#[derive(Clone, Copy, Debug)]
pub struct StairLayout {
    pub bottom: Vec3,
    pub yaw: f32,
    pub run: f32,
    pub height: f32,
    pub n: usize,
}

impl StairLayout {
    pub fn depth(&self) -> f32 {
        self.run / self.n as f32
    }
    pub fn rise(&self) -> f32 {
        self.height / self.n as f32
    }
    fn qy(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }
    pub fn to_world(&self, local: Vec3) -> Vec3 {
        self.bottom + self.qy() * local
    }
    /// Top of the step under local distance `z`.
    fn top_at(&self, z: f32) -> f32 {
        let i = ((z / self.depth()).floor().max(0.0) as usize).min(self.n - 1);
        (i + 1) as f32 * self.rise()
    }
}

/// Bottom and top from two clicks (either order). Flat clicks go up by `rise`.
pub fn stair_layout(a: Vec3, b: Vec3, spec: &StairSpec) -> Option<StairLayout> {
    let (lo, mut hi) = if (b.y - a.y).abs() < 1e-3 {
        (a, b + Vec3::Y * spec.rise.max(0.1))
    } else if b.y > a.y {
        (a, b)
    } else {
        (b, a)
    };
    hi.y = hi.y.max(lo.y + 0.05);
    let flat = Vec3::new(hi.x - lo.x, 0.0, hi.z - lo.z);
    let run = flat.length();
    if run < 0.25 {
        return None;
    }
    let height = hi.y - lo.y;
    let n = ((height / spec.step_height.max(0.05)).round() as usize).clamp(1, 500);
    Some(StairLayout { bottom: lo, yaw: flat.x.atan2(flat.z), run, height, n })
}

fn part(name: &str, lay: &StairLayout, center: Vec3, size: Vec3, pitch: f32, color: [u8; 3], material: Material) -> Object {
    let mut o = Object::brush(Shape::Box, size, lay.to_world(center), color, material);
    o.set_rotation(lay.qy() * Quat::from_rotation_x(pitch));
    o.name = name.into();
    o
}

/// Copies of `prefab` stretched to fill the local box `nmin..nmax` of the stairs.
pub fn fit_prefab(prefab: &[Object], nmin: Vec3, nmax: Vec3, lay: &StairLayout, models: &mut ModelCache) -> Vec<Object> {
    let mut m = Map::default();
    for o in prefab {
        m.add(o.clone());
    }
    let scene = build_scene(&m, models);
    let Some((omin, omax)) = scene.iter().fold(None, |acc: Option<(Vec3, Vec3)>, w| Some(acc.map_or((w.min, w.max), |(a, b)| (a.min(w.min), b.max(w.max))))) else {
        return Vec::new();
    };
    let origs = m.objects.clone();
    apply_scale(&mut m, &origs, omin, omax, nmin, nmax, &[0, 1, 2]);
    m.objects
        .into_iter()
        .map(|mut o| {
            o.pos = lay.to_world(o.pos);
            o.set_rotation(lay.qy() * o.rotation());
            o.folder = None;
            o
        })
        .collect()
}

/// Every object of the staircase.
pub fn build_stairs(lay: &StairLayout, spec: &StairSpec, step_prefab: &[Object], post_prefab: &[Object], models: &mut ModelCache) -> Vec<Object> {
    let (d, h, w, n) = (lay.depth(), lay.rise(), spec.width.max(0.2), lay.n);
    let mut out = Vec::new();
    for i in 0..n {
        let top = (i + 1) as f32 * h;
        let (z0, z1) = (i as f32 * d, (i + 1) as f32 * d);
        if step_prefab.is_empty() {
            out.push(part(&format!("Step {}", i + 1), lay, Vec3::new(0.0, top * 0.5, (z0 + z1) * 0.5), Vec3::new(w, top, d), 0.0, spec.color, spec.material));
        } else {
            let mut p = fit_prefab(step_prefab, Vec3::new(-w * 0.5, top - h, z0), Vec3::new(w * 0.5, top, z1), lay, models);
            for o in &mut p {
                o.name = format!("Step {} {}", i + 1, o.name);
            }
            out.extend(p);
        }
    }
    let slope = lay.height.atan2(lay.run);
    if spec.stringers {
        let t = 0.3;
        let len = (lay.run * lay.run + lay.height * lay.height).sqrt();
        let thick = (h + 0.5) * slope.cos();
        for (side, name) in [(1.0f32, "Trim left"), (-1.0, "Trim right")] {
            let c = Vec3::new(side * (w * 0.5 + t * 0.5), h * 0.5 + lay.height * 0.5, lay.run * 0.5);
            out.push(part(name, lay, c, Vec3::new(t, thick, len), -slope, spec.trim_color, Material::Wood));
        }
    }
    let rh = spec.rail_height.max(0.5);
    let posts = ((lay.run / 4.0).ceil() as usize + 1).max(2);
    let (za, zb) = if n == 1 { (d * 0.2, d * 0.8) } else { (d * 0.5, lay.run - d * 0.5) };
    for (bit, side, name) in [(RAIL_LEFT, 1.0f32, "left"), (RAIL_RIGHT, -1.0, "right")] {
        if spec.rail & bit == 0 {
            continue;
        }
        let x = side * (w * 0.5 - 0.3);
        for j in 0..posts {
            let z = za + (zb - za) * j as f32 / (posts - 1) as f32;
            let base = lay.top_at(z);
            if post_prefab.is_empty() {
                out.push(part(&format!("Post {name} {}", j + 1), lay, Vec3::new(x, base + rh * 0.5, z), Vec3::new(0.3, rh, 0.3), 0.0, spec.trim_color, Material::Wood));
            } else {
                let mut p = fit_prefab(post_prefab, Vec3::new(x - 0.15, base, z - 0.15), Vec3::new(x + 0.15, base + rh, z + 0.15), lay, models);
                for o in &mut p {
                    o.name = format!("Post {name} {}", j + 1);
                }
                out.extend(p);
            }
        }
        let p0 = Vec3::new(x, lay.top_at(za) + rh, za);
        let p1 = Vec3::new(x, lay.top_at(zb) + rh, zb);
        let dv = p1 - p0;
        let pitch = -dv.y.atan2(dv.z);
        out.push(part(&format!("Handrail {name}"), lay, (p0 + p1) * 0.5, Vec3::new(0.35, 0.3, dv.length() + 0.3), pitch, spec.trim_color, Material::Wood));
    }
    out
}

/// Outline of the stairs for the placement preview (world-space segments).
pub fn preview_lines(lay: &StairLayout, spec: &StairSpec) -> Vec<(Vec3, Vec3)> {
    let (d, h, w) = (lay.depth(), lay.rise(), spec.width * 0.5);
    let mut out = Vec::new();
    for side in [-w, w] {
        let mut prev = Vec3::new(side, 0.0, 0.0);
        for i in 0..lay.n {
            let a = Vec3::new(side, (i + 1) as f32 * h, i as f32 * d);
            let b = Vec3::new(side, (i + 1) as f32 * h, (i + 1) as f32 * d);
            out.push((lay.to_world(prev), lay.to_world(a)));
            out.push((lay.to_world(a), lay.to_world(b)));
            prev = b;
        }
        out.push((lay.to_world(prev), lay.to_world(Vec3::new(side, 0.0, lay.run))));
        out.push((lay.to_world(Vec3::new(side, 0.0, lay.run)), lay.to_world(Vec3::new(side, 0.0, 0.0))));
    }
    for i in 0..lay.n {
        let y = (i + 1) as f32 * h;
        out.push((lay.to_world(Vec3::new(-w, y, i as f32 * d)), lay.to_world(Vec3::new(w, y, i as f32 * d))));
    }
    out
}

impl AnvilApp {
    pub(crate) fn stair_spec(&self) -> StairSpec {
        let s = &self.settings;
        StairSpec {
            width: s.stair_width,
            step_height: s.stair_step_height,
            rise: s.stair_rise,
            rail: s.stair_rail,
            rail_height: s.stair_rail_height,
            stringers: s.stair_stringers,
            color: s.stair_color,
            material: Material::Concrete,
            trim_color: s.stair_trim_color,
        }
    }

    /// Point for a stair click in the 3D view: on whatever is under the
    /// mouse (a floor, the top of a wall) or on the work plane.
    pub(crate) fn stair_point_3d(&self, rect: Rect, scene: &[Arc<WorldMesh>], p: Pos2) -> Option<Vec3> {
        let (o, d) = self.cam.ray(rect, self.settings.fov, p);
        let g = self.snap_step();
        let s = |v: f32| if g > 0.0 { (v / g).round() * g } else { v };
        let hit = pick_ray(scene, o, d).map(|h| h.point).or_else(|| ray_plane(o, d, self.work_y))?;
        Some(Vec3::new(s(hit.x), if g > 0.0 { (hit.y / (g * 0.5)).round() * g * 0.5 } else { hit.y }, s(hit.z)))
    }

    pub(crate) fn stair_point_2d(&self, view: &View2D, rect: Rect, p: Pos2) -> Vec3 {
        let g = self.snap_step();
        let uv = view.screen_to_uv(rect, p);
        let uv = if g > 0.0 { (uv / g).round() * g } else { uv };
        let (_, _, _, _, da) = view.axes();
        match view.kind {
            ViewKind::Top => view.uv_to_world(uv, self.stair_start.map_or(self.work_y, |s| s.y)),
            _ => view.uv_to_world(uv, self.stair_start.map_or(0.0, |s| s[da])),
        }
    }

    pub(crate) fn stair_click(&mut self, p: Vec3) {
        match self.stair_start.take() {
            None => {
                self.stair_start = Some(p);
                self.status = "Stairs: now click where they should end (the top). Esc cancels.".into();
            }
            Some(a) => self.place_stairs(a, p),
        }
    }

    pub(crate) fn place_stairs(&mut self, a: Vec3, b: Vec3) {
        let spec = self.stair_spec();
        let Some(lay) = stair_layout(a, b, &spec) else {
            self.status = "Stairs: the two points are too close together".into();
            return;
        };
        let step = self.settings.stair_step_prefab.clone();
        let post = self.settings.stair_post_prefab.clone();
        let objs = build_stairs(&lay, &spec, &step, &post, &mut self.models);
        self.push_undo();
        let folder = self.map.add_folder("Stairs", None);
        self.selection.clear();
        for mut o in objs {
            o.folder = Some(folder);
            let id = self.map.add(o);
            self.selection.push(id);
        }
        self.status = format!("Stairs: {} steps, {:.1} high, {:.1} long", lay.n, lay.height, lay.run);
    }

    pub(crate) fn paint_stair_preview_3d(&self, painter: &Painter, rect: Rect, scene: &[Arc<WorldMesh>], hover: Option<Pos2>) {
        let Some(a) = self.stair_start else { return };
        let fov = self.settings.fov;
        let col = Color32::from_rgb(120, 220, 255);
        if let Some(s) = self.cam.project(rect, fov, a) {
            painter.circle_filled(s, 5.0, Color32::from_rgb(255, 205, 70));
        }
        let Some(b) = hover.and_then(|h| self.stair_point_3d(rect, scene, h)) else { return };
        let Some(lay) = stair_layout(a, b, &self.stair_spec()) else { return };
        for (p, q) in preview_lines(&lay, &self.stair_spec()) {
            if let Some((sa, sb)) = self.cam.project_segment(rect, fov, p, q) {
                painter.line_segment([sa, sb], Stroke::new(1.5f32, col));
            }
        }
    }

    pub(crate) fn paint_stair_preview_2d(&self, painter: &Painter, view: &View2D, rect: Rect, hover: Option<Pos2>) {
        let Some(a) = self.stair_start else { return };
        let col = Color32::from_rgb(120, 220, 255);
        painter.circle_filled(view.world_to_screen(rect, a), 5.0, Color32::from_rgb(255, 205, 70));
        let Some(h) = hover.filter(|h| rect.contains(*h)) else { return };
        let b = self.stair_point_2d(view, rect, h);
        let Some(lay) = stair_layout(a, b, &self.stair_spec()) else { return };
        for (p, q) in preview_lines(&lay, &self.stair_spec()) {
            painter.line_segment([view.world_to_screen(rect, p), view.world_to_screen(rect, q)], Stroke::new(1.0f32, col));
        }
    }

    /// Stairs settings in the tool panel.
    pub(crate) fn stairs_panel(&mut self, ui: &mut egui::Ui) {
        let mut ch = false;
        let s = &mut self.settings;
        egui::Grid::new("stairs_opts").num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
            ui.label("Width");
            ch |= ui.add(egui::DragValue::new(&mut s.stair_width).speed(0.1).range(0.5..=200.0)).changed();
            ui.end_row();
            ui.label("Step height");
            ch |= ui.add(egui::DragValue::new(&mut s.stair_step_height).speed(0.05).range(0.1..=20.0)).changed();
            ui.end_row();
            ui.label("Height if flat");
            ch |= ui.add(egui::DragValue::new(&mut s.stair_rise).speed(0.1).range(0.5..=500.0)).on_hover_text("Used when both clicks are at the same height").changed();
            ui.end_row();
            ui.label("Handrail");
            ui.horizontal(|ui| {
                for (v, l) in [(0u8, "None"), (RAIL_LEFT, "Left"), (RAIL_RIGHT, "Right"), (RAIL_LEFT | RAIL_RIGHT, "Both")] {
                    ch |= ui.selectable_value(&mut s.stair_rail, v, l).changed();
                }
            });
            ui.end_row();
            ui.label("Rail height");
            ch |= ui.add(egui::DragValue::new(&mut s.stair_rail_height).speed(0.1).range(0.5..=20.0)).changed();
            ui.end_row();
            ui.label("Side trim");
            ch |= ui.checkbox(&mut s.stair_stringers, "stringers").changed();
            ui.end_row();
            ui.label("Colors");
            ui.horizontal(|ui| {
                ch |= ui.color_edit_button_srgb(&mut s.stair_color).on_hover_text("Steps").changed();
                ch |= ui.color_edit_button_srgb(&mut s.stair_trim_color).on_hover_text("Trim and handrail").changed();
            });
            ui.end_row();
        });
        ui.label(RichText::new("Prefabs: build a step or a post however you like, select it, then save it here. It gets stretched to fit each step / post.").small().weak());
        let sel: Vec<Object> = self.selection.iter().filter_map(|id| self.map.get(*id).cloned()).collect();
        for which in 0..2 {
            let (label, list) = if which == 0 { ("Step", &mut self.settings.stair_step_prefab) } else { ("Post", &mut self.settings.stair_post_prefab) };
            ui.horizontal(|ui| {
                ui.label(format!("{label}: {}", if list.is_empty() { "plain box".to_string() } else { format!("prefab ({} parts)", list.len()) }));
                if ui.add_enabled(!sel.is_empty(), egui::Button::new("Use selection").small()).clicked() {
                    *list = sel
                        .iter()
                        .cloned()
                        .map(|mut o| {
                            o.folder = None;
                            o
                        })
                        .collect();
                    ch = true;
                }
                if !list.is_empty() && ui.small_button("Clear").clicked() {
                    list.clear();
                    ch = true;
                }
            });
        }
        if ch {
            let _ = self.settings.save();
        }
    }
}
