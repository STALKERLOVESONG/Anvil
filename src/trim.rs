//! Trim tool: click a wall to run a trim strip (molding, a stripe, a
//! baseboard...) along it at that height. On a building made with the
//! Building tool the strip wraps the whole building, outside or inside
//! (whichever face you clicked), with clean corners.

use crate::app::{snap, AnvilApp};
use crate::building::{house_walls, inset_outline, opening_reach, trim_hole};
use crate::map::{Kind, Map, Material, Object, Shape};
use glam::Vec3;

/// The outline (wall center line corners), wall thickness, base y and
/// height of the building whose walls are in `folder` (named Wall1..WallN
/// by the Building tool). None if the walls were changed too much to tell.
pub fn building_outline(map: &Map, folder: Option<u64>) -> Option<(Vec<Vec3>, f32, f32, f32)> {
    let folder = folder?;
    let mut walls: Vec<(usize, &Object)> = map
        .objects
        .iter()
        .filter(|o| o.folder == Some(folder))
        .filter_map(|o| o.name.strip_prefix("Wall").and_then(|n| n.parse::<usize>().ok()).map(|n| (n, o)))
        .collect();
    walls.sort_by_key(|w| w.0);
    let n = walls.len();
    if n < 3 || walls.iter().enumerate().any(|(i, w)| w.0 != i + 1) {
        return None;
    }
    // each wall's center line, flat
    let mut segs = Vec::with_capacity(n);
    let mut thick = 0.0f32;
    let (mut base, mut height) = (0.0f32, 0.0f32);
    for (i, (_, w)) in walls.iter().enumerate() {
        let Kind::Brush { shape: Shape::Box, size, .. } = w.kind else { return None };
        if w.rot.x.abs() > 0.01 || w.rot.z.abs() > 0.01 {
            return None;
        }
        let along_x = size.x >= size.z;
        let len = if along_x { size.x } else { size.z };
        let t = if along_x { size.z } else { size.x };
        let axis = if along_x { Vec3::X } else { Vec3::Z };
        let flat = |p: Vec3| Vec3::new(p.x, 0.0, p.z);
        segs.push((flat(w.to_world(-axis * len * 0.5)), flat(w.to_world(axis * len * 0.5))));
        if i == 0 {
            thick = t;
            base = w.pos.y - size.y * 0.5;
            height = size.y;
        } else if (t - thick).abs() > 1e-3 {
            return None;
        }
    }
    // point each wall from its corner to the next one
    for i in 0..n {
        let next = segs[(i + 1) % n];
        let d = |p: Vec3| (p - next.0).length().min((p - next.1).length());
        if d(segs[i].0) < d(segs[i].1) {
            segs[i] = (segs[i].1, segs[i].0);
        }
    }
    let mut pts = Vec::with_capacity(n);
    for c in 0..n {
        let (a, b) = (segs[(c + n - 1) % n], segs[c]);
        let (da, db) = (a.1 - a.0, b.1 - b.0);
        let den = da.x * db.z - da.z * db.x;
        let p = if den.abs() < 1e-6 * da.length() * db.length() {
            (a.1 + b.0) * 0.5
        } else {
            let t = ((b.0.x - a.0.x) * db.z - (b.0.z - a.0.z) * db.x) / den;
            a.0 + da * t
        };
        pts.push(Vec3::new(p.x, base, p.z));
    }
    Some((pts, thick, base, height))
}

/// Is `p` inside the outline (XZ)?
fn inside(pts: &[Vec3], p: Vec3) -> bool {
    let mut c = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a.z > p.z) != (b.z > p.z) && p.x < (b.x - a.x) * (p.z - a.z) / (b.z - a.z) + a.x {
            c = !c;
        }
    }
    c
}

/// What the Trim tool would add for a click at `point` (with surface
/// `normal`) on object `id`: the trim pieces, ready to add.
/// `h` / `d`: trim height and depth, `g`: snap step (0 = none).
pub fn trim_plan(map: &Map, id: u64, point: Vec3, normal: Vec3, h: f32, d: f32, g: f32, color: [u8; 3]) -> Result<Vec<Object>, String> {
    let o = map.get(id).ok_or("Click on a wall")?;
    if normal.y.abs() > 0.7 {
        return Err("Click the side of a wall, not its top".into());
    }
    let is_building_piece = o.name.starts_with("Wall") || o.name.starts_with("Corner");
    if let Some((pts, t, base, wall_h)) = is_building_piece.then(|| building_outline(map, o.folder)).flatten() {
        let bottom = snap(point.y - h * 0.5, g).clamp(base, (base + wall_h - h).max(base));
        let off = t * 0.5 + d * 0.5;
        let outside = !inside(&pts, point - normal * (t * 0.25));
        let band = inset_outline(&pts, if outside { -off } else { off });
        let side = if outside { "Outside" } else { "Inside" };
        let out: Vec<Object> = house_walls(&band, d, h, bottom, color, Material::Wood)
            .into_iter()
            .map(|mut p| {
                p.name = if p.name.starts_with("Corner") { format!("Trim{side}{}", p.name) } else { p.name.replace("Wall", &format!("Trim{side}")) };
                p.folder = o.folder;
                p
            })
            .collect();
        return Ok(out);
    }
    // any other box: a strip along the clicked face
    let Kind::Brush { shape: Shape::Box, size, .. } = o.kind else {
        return Err("Trim goes on walls (Box brushes or Building tool walls)".into());
    };
    let ln = o.to_local(point + normal) - o.to_local(point);
    let k = (0..3).max_by(|a, b| ln[*a].abs().total_cmp(&ln[*b].abs())).unwrap_or(2);
    if k == 1 {
        return Err("Click the side of a wall, not its top".into());
    }
    let len_axis = if k == 0 { 2 } else { 0 };
    let local = o.to_local(point);
    let bottom = (snap(local.y - h * 0.5 + size.y * 0.5, g) - size.y * 0.5).clamp(-size.y * 0.5, (size.y * 0.5 - h).max(-size.y * 0.5));
    let mut c = Vec3::ZERO;
    c[k] = ln[k].signum() * (size[k] * 0.5 + d * 0.5);
    c.y = bottom + h * 0.5;
    let mut s = Vec3::ZERO;
    s[len_axis] = size[len_axis];
    s.y = h;
    s[k] = d;
    let mut t = Object::brush(Shape::Box, s, o.to_world(c), color, Material::Wood);
    t.rot = o.rot;
    t.folder = o.folder;
    t.name = format!("Trim_{}", o.name);
    Ok(vec![t])
}

impl AnvilApp {
    /// Trim tool click on a wall.
    pub(crate) fn place_trim(&mut self, id: u64, point: Vec3, normal: Vec3, snapping: bool) {
        let s = &self.settings;
        let g = if snapping { self.snap_step() } else { 0.0 };
        let mut pieces = match trim_plan(&self.map, id, point, normal, s.trim_height, s.trim_depth, g, s.trim_color) {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        // doors and windows already in the walls cut through the new trim too
        let folder = self.map.get(id).and_then(|o| o.folder);
        let walls: Vec<Object> = self.map.objects.iter().filter(|w| w.id == id || (folder.is_some() && w.folder == folder)).cloned().collect();
        let reach = self.settings.trim_depth + 1.0;
        for w in &walls {
            let Kind::Brush { holes, .. } = &w.kind else { continue };
            for hole in holes {
                let corners = opening_reach(w, hole, reach);
                for p in &mut pieces {
                    if let Some(h) = trim_hole(p, &corners) {
                        if let Kind::Brush { holes, .. } = &mut p.kind {
                            holes.push(h);
                        }
                    }
                }
            }
        }
        self.push_undo();
        let n = pieces.len();
        self.selection = pieces.into_iter().map(|p| self.map.add(p)).collect();
        self.status = if n > 1 { format!("Added trim around the building ({n} pieces). Change its size and color in the Trim settings.") } else { "Added a trim strip along the wall".into() };
    }

    /// Edges of the trim a click here would add (for the hover preview).
    pub(crate) fn trim_preview(&self, id: u64, point: Vec3, normal: Vec3, snapping: bool) -> Vec<(Vec3, Vec3)> {
        let s = &self.settings;
        let g = if snapping { self.snap_step() } else { 0.0 };
        let Ok(pieces) = trim_plan(&self.map, id, point, normal, s.trim_height, s.trim_depth, g, s.trim_color) else { return Vec::new() };
        let mut out = Vec::new();
        for p in &pieces {
            match &p.kind {
                Kind::Brush { size, .. } => {
                    let hs = *size * 0.5;
                    let c: Vec<Vec3> = (0..8).map(|i| p.to_world(Vec3::new(if i & 1 != 0 { hs.x } else { -hs.x }, if i & 2 != 0 { hs.y } else { -hs.y }, if i & 4 != 0 { hs.z } else { -hs.z }))).collect();
                    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
                        out.push((c[a], c[b]));
                    }
                }
                Kind::Mesh { verts, faces } => {
                    for f in faces {
                        for i in 0..f.len() {
                            let (a, b) = (f[i] as usize, f[(i + 1) % f.len()] as usize);
                            if let (Some(a), Some(b)) = (verts.get(a), verts.get(b)) {
                                out.push((p.to_world(*a), p.to_world(*b)));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Buildings made before 0.2.4 have a floor and ceiling that stop at the
/// middle of the walls (a step on the outside). Refits those to the walls'
/// outer edge, keeping their look. Returns how many slabs were fixed.
pub fn refit_old_slabs(map: &mut Map) -> usize {
    let folders: Vec<u64> = map.folders.iter().map(|f| f.id).collect();
    let mut fixed = 0;
    for f in folders {
        let Some((pts, t, _, _)) = building_outline(map, Some(f)) else { continue };
        let outer = inset_outline(&pts, -t * 0.5);
        for o in map.objects.iter_mut().filter(|o| o.folder == Some(f) && (o.name == "Floor" || o.name == "Ceiling")) {
            // its footprint (flat corners), and its thickness and bottom
            let (corners, bottom, thick) = match &o.kind {
                Kind::Mesh { verts, .. } => {
                    let world: Vec<Vec3> = verts.iter().map(|v| o.to_world(*v)).collect();
                    let (lo, hi) = world.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
                    let mut c: Vec<Vec3> = world.iter().filter(|p| (p.y - lo).abs() < 1e-3).map(|p| Vec3::new(p.x, 0.0, p.z)).collect();
                    c.dedup_by(|a, b| (*a - *b).length() < 1e-4);
                    (c, lo, hi - lo)
                }
                Kind::Brush { shape: Shape::Box, size, .. } if o.rot.length() < 1e-3 => {
                    let h = *size * 0.5;
                    let c = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|(sx, sz)| Vec3::new(o.pos.x + sx * h.x, 0.0, o.pos.z + sz * h.z)).collect();
                    (c, o.pos.y - h.y, size.y)
                }
                _ => continue,
            };
            // only if it's the old one: its corners are the walls' center line
            let on_center = corners.len() == pts.len() && corners.iter().all(|c| pts.iter().any(|p| (Vec3::new(p.x, 0.0, p.z) - *c).length() < 1e-2));
            if !on_center || thick <= 0.0 {
                continue;
            }
            let new = crate::building::slab(&outer, bottom, thick, o.color, o.material);
            o.kind = new.kind;
            o.pos = new.pos;
            o.rot = new.rot;
            fixed += 1;
        }
    }
    fixed
}
