//! House / building tools: closed-loop layouts that become walls + floor,
//! extruded polygons, door & window openings (boolean holes) and carving.

use crate::app::{snap, AnvilApp, Tool};
use crate::map::{Hole, Kind, Material, Object, Shape};
use crate::mesh::world_mesh;
use crate::polymesh::prism;
use glam::Vec3;
use std::f32::consts::FRAC_PI_2;

/// A box wall from a to b (XZ), `t` thick and `h` tall, standing on `base_y`.
pub fn wall_box(a: Vec3, b: Vec3, t: f32, h: f32, base_y: f32, color: [u8; 3], material: Material) -> Option<Object> {
    let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let len = d.length();
    if len < 0.05 {
        return None;
    }
    let mid = (a + b) * 0.5;
    let pos = Vec3::new(mid.x, base_y + h * 0.5, mid.z);
    let (size, yaw) = if d.z.abs() < 1e-4 {
        (Vec3::new(len, h, t), 0.0)
    } else if d.x.abs() < 1e-4 {
        (Vec3::new(t, h, len), 0.0)
    } else {
        (Vec3::new(len, h, t), (-d.z).atan2(d.x).to_degrees())
    };
    let mut o = Object::brush(Shape::Box, size, pos, color, material);
    o.rot.y = yaw;
    Some(o)
}

/// Convex hull of points on the XZ plane (monotone chain).
fn hull_xz(pts: &[Vec3]) -> Vec<Vec3> {
    let mut p: Vec<Vec3> = pts.to_vec();
    p.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal).then(a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal)));
    p.dedup_by(|a, b| (*a - *b).length() < 1e-5);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec3, a: Vec3, b: Vec3| (a.x - o.x) * (b.z - o.z) - (a.z - o.z) * (b.x - o.x);
    let mut lower: Vec<Vec3> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 1e-7 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<Vec3> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 1e-7 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Walls for a closed outline with clean corners and no overlaps:
/// right-angle corners are butt-jointed (one wall runs through the corner,
/// the other stops at its face); any other angle gets both walls trimmed back
/// to the miter and a small corner piece that fills the gap exactly.
pub fn house_walls(pts: &[Vec3], t: f32, h: f32, base_y: f32, color: [u8; 3], material: Material) -> Vec<Object> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    let dirs: Vec<Vec3> = (0..n).map(|i| Vec3::new(pts[(i + 1) % n].x - pts[i].x, 0.0, pts[(i + 1) % n].z - pts[i].z).normalize_or_zero()).collect();
    let lens: Vec<f32> = (0..n).map(|i| Vec3::new(pts[(i + 1) % n].x - pts[i].x, 0.0, pts[(i + 1) % n].z - pts[i].z).length()).collect();
    let mut ext_start = vec![0.0f32; n];
    let mut ext_end = vec![0.0f32; n];
    let mut out = Vec::new();
    let half = t * 0.5;
    for c in 0..n {
        let (prev, next) = ((c + n - 1) % n, c);
        let (dp, dn) = (dirs[prev], dirs[next]);
        let alpha = dp.dot(dn).clamp(-1.0, 1.0).acos(); // how far the outline turns here
        if alpha < 0.5f32.to_radians() {
            continue; // straight through
        }
        if (alpha - FRAC_PI_2).abs() < 0.5f32.to_radians() {
            ext_end[prev] = -half;
            ext_start[next] = half;
            continue;
        }
        let k = (half * (alpha * 0.5).tan()).min(0.45 * lens[prev].min(lens[next]));
        ext_end[prev] = -k;
        ext_start[next] = -k;
        let p = Vec3::new(pts[c].x, 0.0, pts[c].z);
        let na = Vec3::new(-dp.z, 0.0, dp.x);
        let nb = Vec3::new(-dn.z, 0.0, dn.x);
        let ea = p - dp * k;
        let sb = p + dn * k;
        let outward = (dp - dn).normalize_or_zero();
        let q = p + outward * (half / (alpha * 0.5).cos().max(0.05));
        let hull = hull_xz(&[ea + na * half, ea - na * half, sb + nb * half, sb - nb * half, q]);
        if hull.len() >= 3 {
            let (verts, faces, center) = prism(&hull, base_y, h);
            let mut j = Object::new(Kind::Mesh { verts, faces }, center, color, material);
            j.name = format!("Corner{}", c + 1);
            out.push(j);
        }
    }
    for i in 0..n {
        let a = pts[i] - dirs[i] * ext_start[i];
        let b = pts[(i + 1) % n] + dirs[i] * ext_end[i];
        if let Some(mut w) = wall_box(a, b, t, h, base_y, color, material) {
            w.name = format!("Wall{}", i + 1);
            out.push(w);
        }
    }
    out
}

/// The outline moved inward by `d` (sharp, mitered corners).
pub fn inset_outline(pts: &[Vec3], d: f32) -> Vec<Vec3> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let area: f32 = (0..n).map(|i| pts[i].x * pts[(i + 1) % n].z - pts[(i + 1) % n].x * pts[i].z).sum();
    let inward = |a: Vec3, b: Vec3| {
        let e = Vec3::new(b.x - a.x, 0.0, b.z - a.z).normalize_or_zero();
        if area > 0.0 { Vec3::new(-e.z, 0.0, e.x) } else { Vec3::new(e.z, 0.0, -e.x) }
    };
    (0..n)
        .map(|i| {
            let (prev, cur, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let (n1, n2) = (inward(prev, cur), inward(cur, next));
            cur + (n1 + n2) * d / (1.0 + n1.dot(n2)).max(0.1)
        })
        .collect()
}

/// Baseboard / crown strips along the inside of a building's walls.
#[allow(clippy::too_many_arguments)]
pub fn house_trim(pts: &[Vec3], wall_t: f32, wall_h: f32, base_y: f32, trim_h: f32, trim_d: f32, baseboard: bool, crown: bool, color: [u8; 3]) -> Vec<Object> {
    let inner = inset_outline(pts, wall_t * 0.5 + trim_d * 0.5);
    let h = trim_h.min(wall_h * 0.45);
    let mut out = Vec::new();
    for (on, y, label) in [(baseboard, base_y, "Baseboard"), (crown, base_y + wall_h - h, "Crown")] {
        if !on {
            continue;
        }
        for mut o in house_walls(&inner, trim_d, h, y, color, Material::Wood) {
            o.name = o.name.replace("Wall", label).replace("Corner", &format!("{label}Corner"));
            out.push(o);
        }
    }
    out
}

/// Floor / ceiling slab for a footprint: a Box for axis-aligned
/// rectangles (one Roblox part), otherwise an extruded polygon mesh.
pub fn slab(pts: &[Vec3], base_y: f32, thickness: f32, color: [u8; 3], material: Material) -> Object {
    let n = pts.len();
    let rect = n == 4
        && (0..4).all(|i| {
            let d = pts[(i + 1) % 4] - pts[i];
            d.x.abs() < 1e-4 || d.z.abs() < 1e-4
        });
    if rect {
        let mn = pts.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
        let mx = pts.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
        let size = Vec3::new(mx.x - mn.x, thickness, mx.z - mn.z);
        let pos = Vec3::new((mn.x + mx.x) * 0.5, base_y + thickness * 0.5, (mn.z + mx.z) * 0.5);
        Object::brush(Shape::Box, size, pos, color, material)
    } else {
        let (verts, faces, center) = prism(pts, base_y, thickness);
        Object::new(Kind::Mesh { verts, faces }, center, color, material)
    }
}

/// Everything the Building tool makes for an outline: walls (+ corner
/// pieces), trim, floor and ceiling.
pub fn house_objects(pts: &[Vec3], s: &crate::settings::Settings, work_y: f32, wall_tex: Option<u64>, floor_tex: Option<u64>) -> Vec<Object> {
    let mut out = Vec::new();
    for mut o in house_walls(pts, s.wall_thickness, s.wall_height, work_y, s.wall_color, s.wall_material) {
        o.texture = wall_tex;
        out.push(o);
    }
    out.extend(house_trim(pts, s.wall_thickness, s.wall_height, work_y, s.trim_height, s.trim_depth, s.trim_baseboard, s.trim_crown, s.trim_color));
    // the walls stand centered on the outline, so the floor and ceiling use
    // the walls' outer edge: flush with their outside faces, no step
    let outer = inset_outline(pts, -s.wall_thickness * 0.5);
    let pts = &outer[..];
    if s.layout_floor {
        let mut f = slab(pts, work_y - s.floor_thickness, s.floor_thickness, s.floor_color, s.floor_material);
        f.name = "Floor".into();
        f.texture = floor_tex;
        out.push(f);
    }
    if s.layout_ceiling {
        let mut c = slab(pts, work_y + s.wall_height, s.floor_thickness, s.wall_color, s.wall_material);
        c.name = "Ceiling".into();
        out.push(c);
    }
    out
}

impl AnvilApp {
    /// A wall brush along a → b on the work plane, extended by `extend` at both ends.
    pub(crate) fn wall_object(&self, a: Vec3, b: Vec3, extend: f32) -> Option<Object> {
        let flat = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
        let len = flat.length();
        if len < 0.05 {
            return None;
        }
        let dir = flat / len;
        let s = &self.settings;
        wall_box(a - dir * extend, b + dir * extend, s.wall_thickness, s.wall_height, self.work_y, s.wall_color, s.wall_material)
    }

    pub(crate) fn layout_click(&mut self, p: Vec3, near_first: bool) {
        if near_first && self.layout_pts.len() >= 3 {
            self.close_layout();
            return;
        }
        if self.layout_pts.last().is_some_and(|q| (*q - p).length() < 1e-4) {
            return;
        }
        self.layout_pts.push(p);
        self.status = format!(
            "{} point(s). Click the first point, double-click or press Enter to close. Backspace undoes a point.",
            self.layout_pts.len()
        );
    }

    pub(crate) fn close_layout(&mut self) {
        let mut pts = std::mem::take(&mut self.layout_pts);
        pts.dedup_by(|a, b| (*a - *b).length() < 1e-4);
        if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).length() < 1e-4 {
            pts.pop();
        }
        if pts.len() < 3 {
            self.status = "An outline needs at least 3 corners".into();
            return;
        }
        match self.tool {
            Tool::Polygon => self.build_polygon(&pts),
            _ => self.build_house(&pts),
        }
    }

    fn build_house(&mut self, pts: &[Vec3]) {
        self.push_undo();
        let n = self.map.folders.iter().filter(|f| f.name.starts_with("Building")).count() + 1;
        let folder = self.map.add_folder(&format!("Building {n}"), None);
        let wall_tex = self.build_wall_texture.filter(|t| self.map.textures.iter().any(|d| d.id == *t));
        let floor_tex = self.build_floor_texture.filter(|t| self.map.textures.iter().any(|d| d.id == *t));
        let mut ids = Vec::new();
        for mut o in house_objects(pts, &self.settings, self.work_y, wall_tex, floor_tex) {
            o.folder = Some(folder);
            ids.push(self.map.add(o));
        }
        self.selection = ids;
        self.status = format!("Built 'Building {n}' with {} walls. Use the Door / Window tools to cut openings.", pts.len());
    }

    /// Outline closed: now the mouse sets the height (click to finish).
    fn build_polygon(&mut self, pts: &[Vec3]) {
        self.poly_extrude = Some(crate::previews::PolyExtrude { pts: pts.to_vec(), base_y: self.work_y, height: self.settings.polygon_height, anchor: None });
        self.status = "Move the mouse to set the height, click to finish (Ctrl = no snapping, Esc = cancel)".into();
    }

    pub(crate) fn finish_polygon(&mut self, pts: &[Vec3], base_y: f32, height: f32) {
        let s = &self.settings;
        let (verts, faces, center) = prism(pts, base_y, height.max(0.01));
        let mut o = Object::new(Kind::Mesh { verts, faces }, center, s.default_color, s.default_material);
        o.name = format!("Polygon_{}", self.map.next_id + 1);
        self.add_and_select(o);
        self.status = "Polygon created. Press Tab to edit its vertices / faces (E extrudes faces).".into();
    }

    /// Where a door / window opening would go on wall `id` for a click at
    /// `at`: the wall, the hole (wall space) and an optional glass pane
    /// (wall-space center, size). Used for both the preview and the cut.
    pub(crate) fn opening_plan(&self, id: u64, at: Vec3, window: bool, snapping: bool) -> Result<(Object, Hole, Option<(Vec3, Vec3)>), String> {
        let o = self.map.get(id).cloned().ok_or("Click on a wall")?;
        let Kind::Brush { shape: Shape::Box, size, .. } = o.kind else {
            return Err("Doors and windows cut into Box brushes (walls, floors). Use Edit > Boolean for others.".into());
        };
        let local = o.to_local(at);
        let thin = if size.x <= size.y && size.x <= size.z {
            0
        } else if size.y <= size.z {
            1
        } else {
            2
        };
        let s = &self.settings;
        let g = if snapping { self.snap_step() } else { 0.0 };
        let mut hole = Hole::default();
        let mut glass: Option<(Vec3, Vec3)> = None;
        if thin == 1 {
            // floor / ceiling: a hatch centered on the click
            let (w, d) = if window { (s.window_width, s.window_height) } else { (s.door_width, s.door_height) };
            let cx = snap(local.x, g * 0.5).clamp(-size.x * 0.5 + w * 0.5, size.x * 0.5 - w * 0.5);
            let cz = snap(local.z, g * 0.5).clamp(-size.z * 0.5 + d * 0.5, size.z * 0.5 - d * 0.5);
            hole.min = Vec3::new(cx - w * 0.5, -size.y * 0.5 - 0.1, cz - d * 0.5);
            hole.max = Vec3::new(cx + w * 0.5, size.y * 0.5 + 0.1, cz + d * 0.5);
        } else {
            let len_axis = if thin == 0 { 2 } else { 0 };
            let (w, h, bottom) = if window { (s.window_width, s.window_height, s.window_sill) } else { (s.door_width, s.door_height, 0.0) };
            let w = w.min(size[len_axis]);
            let half = size[len_axis] * 0.5;
            let c = snap(local[len_axis], g * 0.5).clamp(-half + w * 0.5, half - w * 0.5);
            let y0 = -size.y * 0.5 + bottom;
            let y1 = (y0 + h).min(size.y * 0.5 + 0.1);
            if y1 <= y0 {
                return Err("The wall is too short for that opening (see Settings > Building tools)".into());
            }
            hole.min[len_axis] = c - w * 0.5;
            hole.max[len_axis] = c + w * 0.5;
            hole.min.y = if window { y0 } else { y0 - 0.1 };
            hole.max.y = y1;
            hole.min[thin] = -size[thin] * 0.5 - 0.1;
            hole.max[thin] = size[thin] * 0.5 + 0.1;
            if window && s.window_glass {
                let mut gc = Vec3::ZERO;
                gc[len_axis] = c;
                gc.y = (y0 + y1) * 0.5;
                let mut gs = Vec3::ZERO;
                gs[len_axis] = w;
                gs.y = y1 - y0;
                gs[thin] = (size[thin] * 0.25).max(0.1);
                glass = Some((gc, gs));
            }
        }
        Ok((o, hole, glass))
    }

    /// Cuts a door or window opening into a Box brush at the clicked point.
    pub(crate) fn cut_opening(&mut self, id: u64, at: Vec3, window: bool, snapping: bool) {
        let (o, hole, glass) = match self.opening_plan(id, at, window, snapping) {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        self.push_undo();
        if let Some(Object { kind: Kind::Brush { holes, .. }, .. }) = self.map.get_mut(id) {
            holes.push(hole);
        }
        self.cut_trim(&o, &hole);
        if let Some((gc, gs)) = glass {
            let mut g = Object::brush(Shape::Box, gs, o.to_world(gc), [160, 205, 235], Material::Glass);
            g.rot = o.rot;
            g.transparency = 0.6;
            g.folder = o.folder;
            g.name = format!("{}_Glass", o.name);
            self.map.add(g);
        }
        self.selection = vec![id];
        self.status = format!("Cut a {} into {}", if window { "window" } else { "door" }, o.name);
    }

    /// Boolean carve: every selected Box brush cuts a hole into each other
    /// Box brush it overlaps, then the cutters are removed.
    pub(crate) fn carve_selection(&mut self) {
        let cutters: Vec<Object> = self
            .selection
            .iter()
            .filter_map(|id| self.map.get(*id))
            .filter(|o| matches!(o.kind, Kind::Brush { shape: Shape::Box, .. }))
            .cloned()
            .collect();
        if cutters.is_empty() {
            self.status = "Select one or more Box brushes to carve with".into();
            return;
        }
        let corners = |o: &Object| -> Vec<Vec3> {
            let Kind::Brush { size, .. } = o.kind else { return Vec::new() };
            let h = size * 0.5;
            (0..8)
                .map(|i| o.to_world(Vec3::new(if i & 1 != 0 { h.x } else { -h.x }, if i & 2 != 0 { h.y } else { -h.y }, if i & 4 != 0 { h.z } else { -h.z })))
                .collect()
        };
        let mut new_map = self.map.clone();
        let mut touched = Vec::new();
        let cutter_ids: Vec<u64> = cutters.iter().map(|c| c.id).collect();
        for target in self.map.objects.iter().filter(|o| !o.hidden && !cutter_ids.contains(&o.id)) {
            let Kind::Brush { shape: Shape::Box, size, .. } = target.kind else { continue };
            let tw = world_mesh(target, &self.map.textures, &mut self.models);
            for c in &cutters {
                let cw = corners(c);
                let (cmin, cmax) = cw.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
                if cmax.cmplt(tw.min).any() || cmin.cmpgt(tw.max).any() {
                    continue;
                }
                let local: Vec<Vec3> = cw.iter().map(|p| target.to_local(*p)).collect();
                let (lmin, lmax) = local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
                let h = size * 0.5;
                if lmax.cmple(-h).any() || lmin.cmpge(h).any() {
                    continue;
                }
                if let Some(Object { kind: Kind::Brush { holes, .. }, .. }) = new_map.get_mut(target.id) {
                    holes.push(Hole { min: lmin, max: lmax });
                    if !touched.contains(&target.id) {
                        touched.push(target.id);
                    }
                }
            }
        }
        if touched.is_empty() {
            self.status = "The selected boxes don't overlap any other Box brush".into();
            return;
        }
        self.push_undo();
        new_map.remove(&cutter_ids);
        self.map = new_map;
        self.selection = touched.clone();
        self.status = format!("Carved {} brush(es); the cutter(s) were removed", touched.len());
    }

    /// A door / window also cuts through any baseboard or crown trim it overlaps.
    fn cut_trim(&mut self, wall: &Object, hole: &Hole) {
        let corners = opening_reach(wall, hole, 2.0);
        let trims: Vec<u64> = self
            .map
            .objects
            .iter()
            .filter(|t| t.id != wall.id && t.folder == wall.folder && (t.name.starts_with("Baseboard") || t.name.starts_with("Crown") || t.name.starts_with("Trim")))
            .map(|t| t.id)
            .collect();
        for tid in trims {
            let Some(t) = self.map.get_mut(tid) else { continue };
            if let Some(h) = trim_hole(t, &corners) {
                if let Kind::Brush { holes, .. } = &mut t.kind {
                    holes.push(h);
                }
            }
        }
    }
}

/// World corners of an opening, stretched `reach` studs out past both faces
/// of the wall (so it reaches trim fixed to the wall's sides).
pub fn opening_reach(wall: &Object, hole: &Hole, reach: f32) -> Vec<Vec3> {
    let Kind::Brush { size, .. } = &wall.kind else { return Vec::new() };
    let thin = (0..3).min_by(|a, b| size[*a].total_cmp(&size[*b])).unwrap_or(2);
    let (mut mn, mut mx) = (hole.min, hole.max);
    mn[thin] = -size[thin] * 0.5 - reach;
    mx[thin] = size[thin] * 0.5 + reach;
    (0..8).map(|i| wall.to_world(Vec3::new(if i & 1 != 0 { mx.x } else { mn.x }, if i & 2 != 0 { mx.y } else { mn.y }, if i & 4 != 0 { mx.z } else { mn.z }))).collect()
}

/// The hole a door / window (given by its 8 world corners) cuts into a
/// trim strip, if they overlap. Goes through the strip's whole thickness.
pub fn trim_hole(t: &Object, corners: &[Vec3]) -> Option<Hole> {
    let Kind::Brush { shape: Shape::Box, size, .. } = &t.kind else { return None };
    let local: Vec<Vec3> = corners.iter().map(|p| t.to_local(*p)).collect();
    let (mut mn, mut mx) = local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let half = *size * 0.5;
    if (0..3).any(|k| mx[k] <= -half[k] || mn[k] >= half[k]) {
        return None;
    }
    let thin = (0..3).min_by(|a, b| size[*a].total_cmp(&size[*b])).unwrap_or(2);
    mn[thin] = -half[thin] - 0.1;
    mx[thin] = half[thin] + 0.1;
    Some(Hole { min: mn, max: mx })
}

