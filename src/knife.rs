//! Knife tool: drag a line across walls / meshes to cut them in two
//! ("Split") or just add an edge across their faces ("Slice").

use crate::app::{AnvilApp, Drag};
use crate::map::{FaceTex, Hole, Kind, Map, Object, Shape};
use crate::csg::box_minus;
use crate::mesh::{box_face_dir, brush_mesh, stair_boxes, WorldMesh};
use crate::polymesh::{box_poly, newell, plane_cut, weld, Piece};
use crate::view2d::View2D;
use eframe::egui::{Pos2, Rect};
use glam::{Mat3, Vec3};
use std::sync::Arc;

/// Per-direction brush face textures -> per-polygon mesh face textures.
pub(crate) fn brush_faces_to_mesh(face_tex: &[Option<FaceTex>], verts: &[Vec3], faces: &[Vec<u32>]) -> Vec<Option<FaceTex>> {
    if face_tex.iter().all(|f| f.is_none()) {
        return Vec::new();
    }
    faces.iter().map(|f| face_tex.get(box_face_dir(newell(verts, f))).cloned().flatten()).collect()
}

/// An object as an editable mesh in its local space (with face textures).
pub(crate) fn as_mesh(o: &Object) -> Option<(Vec<Vec3>, Vec<Vec<u32>>, Vec<Option<FaceTex>>)> {
    match &o.kind {
        Kind::Mesh { verts, faces } => Some((verts.clone(), faces.clone(), o.face_tex.clone())),
        Kind::Brush { shape, size, steps, holes } => {
            let (v, f) = if *shape == Shape::Box && holes.is_empty() { box_poly(*size) } else { weld(&brush_mesh(*shape, *size, *steps, holes)) };
            let ft = brush_faces_to_mesh(&o.face_tex, &v, &f);
            Some((v, f, ft))
        }
        _ => None,
    }
}

pub(crate) type Part = (Vec<Vec3>, Vec<Vec<u32>>, Vec<Option<FaceTex>>);

/// An object as closed convex-ish pieces in local space. Walls with holes and
/// stairs are several touching boxes, so each box is cut on its own (welding
/// them together would leave inner edges shared by four faces).
pub(crate) fn components(o: &Object) -> Option<Vec<Part>> {
    let boxes: Vec<(Vec3, Vec3)> = match &o.kind {
        Kind::Brush { shape: Shape::Box, size, holes, .. } if !holes.is_empty() => box_minus(*size, holes),
        Kind::Brush { shape: Shape::Stairs, size, steps, .. } => stair_boxes(*steps).into_iter().map(|(a, b)| (a * *size, b * *size)).collect(),
        _ => return as_mesh(o).map(|m| vec![m]),
    };
    Some(
        boxes
            .into_iter()
            .map(|(mn, mx)| {
                let (mut v, f) = box_poly(mx - mn);
                let c = (mn + mx) * 0.5;
                for p in &mut v {
                    *p += c;
                }
                let ft = brush_faces_to_mesh(&o.face_tex, &v, &f);
                (v, f, ft)
            })
            .collect(),
    )
}

fn offset_src(mut p: Piece, base: usize) -> Piece {
    for s in &mut p.src {
        if *s != usize::MAX {
            *s += base;
        }
    }
    p
}

fn merge(pieces: Vec<Piece>) -> Piece {
    let mut out = Piece { verts: Vec::new(), faces: Vec::new(), src: Vec::new() };
    for p in pieces {
        let off = out.verts.len() as u32;
        out.verts.extend(p.verts);
        out.faces.extend(p.faces.into_iter().map(|f| f.into_iter().map(|i| i + off).collect()));
        out.src.extend(p.src);
    }
    out
}

/// Mesh object from one cut piece, with its pivot moved to the piece's center.
fn piece_object(base: &Object, piece: Piece, ft: &[Option<FaceTex>]) -> Object {
    let mut verts = piece.verts;
    let (mn, mx) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    let c = (mn + mx) * 0.5;
    for v in &mut verts {
        *v -= c;
    }
    let mut o = base.clone();
    o.pos = base.pos + base.rotation() * c;
    o.kind = Kind::Mesh { verts, faces: piece.faces };
    o.text = None;
    o.face_tex = piece.src.iter().map(|&s| if s == usize::MAX { None } else { ft.get(s).cloned().flatten() }).collect();
    while o.face_tex.last().is_some_and(|f| f.is_none()) {
        o.face_tex.pop();
    }
    o
}

/// Cuts objects with the plane through `p` (world) with normal `n`.
/// Returns how many objects were cut and the resulting object ids.
pub fn knife_cut(map: &mut Map, p: Vec3, n: Vec3, targets: &[u64], split: bool) -> (usize, Vec<u64>) {
    let mut count = 0;
    let mut result = Vec::new();
    for &id in targets {
        let Some(o) = map.get(id).cloned() else { continue };
        let rot = o.rotation();
        let inv = rot.inverse();
        let lp = inv * (p - o.pos);
        let ln = (inv * n).normalize_or_zero();
        if ln == Vec3::ZERO {
            continue;
        }
        // a box cut along its own axis stays two real boxes (doors keep working)
        if split {
            if let Kind::Brush { shape: Shape::Box, size, steps, holes } = &o.kind {
                if let Some(k) = (0..3).find(|&k| ln[k].abs() > 0.9999) {
                    let c = lp[k];
                    let h = size[k] * 0.5;
                    if c > -h + 1e-3 && c < h - 1e-3 {
                        for (pi, (a, b)) in [(-h, c), (c, h)].into_iter().enumerate() {
                            let mut sz = *size;
                            sz[k] = b - a;
                            let mut off = Vec3::ZERO;
                            off[k] = (a + b) * 0.5;
                            let new_holes: Vec<Hole> = holes
                                .iter()
                                .filter_map(|hh| {
                                    let (lo, hi) = (hh.min[k].max(a), hh.max[k].min(b));
                                    (hi > lo).then(|| {
                                        let (mut mn, mut mx) = (hh.min, hh.max);
                                        mn[k] = lo;
                                        mx[k] = hi;
                                        Hole { min: mn - off, max: mx - off }
                                    })
                                })
                                .collect();
                            let mut part = o.clone();
                            part.kind = Kind::Brush { shape: Shape::Box, size: sz, steps: *steps, holes: new_holes };
                            part.pos = o.pos + rot * off;
                            if pi == 0 {
                                if let Some(x) = map.get_mut(id) {
                                    *x = part;
                                }
                                result.push(id);
                            } else {
                                part.name = format!("{} (cut)", o.name);
                                result.push(map.add(part));
                            }
                        }
                        count += 1;
                        continue;
                    }
                }
            }
        }
        let Some(parts) = components(&o) else { continue };
        let mut fronts = Vec::new();
        let mut backs = Vec::new();
        let mut ft_all: Vec<Option<FaceTex>> = Vec::new();
        let mut faces_before = 0;
        for (v, f, ft) in parts {
            let base = ft_all.len();
            ft_all.extend((0..f.len()).map(|i| ft.get(i).cloned().flatten()));
            faces_before += f.len();
            let (a, b) = plane_cut(&v, &f, lp, ln, split);
            fronts.extend(a.map(|p| offset_src(p, base)));
            backs.extend(b.map(|p| offset_src(p, base)));
        }
        if split {
            if !fronts.is_empty() && !backs.is_empty() {
                let oa = piece_object(&o, merge(fronts), &ft_all);
                let mut ob = piece_object(&o, merge(backs), &ft_all);
                if let Some(x) = map.get_mut(id) {
                    *x = oa;
                }
                ob.name = format!("{} (cut)", o.name);
                result.push(id);
                result.push(map.add(ob));
                count += 1;
            }
        } else if !fronts.is_empty() {
            let a = merge(fronts);
            if a.faces.len() > faces_before {
                let mut sliced = piece_object(&o, a, &ft_all);
                // slicing keeps the pivot where it was
                if let Kind::Mesh { verts, .. } = &mut sliced.kind {
                    let d = Mat3::from_quat(o.rotation()).transpose() * (sliced.pos - o.pos);
                    for vv in verts.iter_mut() {
                        *vv += d;
                    }
                }
                sliced.pos = o.pos;
                if let Some(x) = map.get_mut(id) {
                    *x = sliced;
                }
                result.push(id);
                count += 1;
            }
        }
    }
    (count, result)
}

fn seg_hits_rect(a: Pos2, b: Pos2, r: Rect) -> bool {
    Rect::from_two_pos(a, b).expand(2.0).intersects(r)
}

impl AnvilApp {
    /// Objects the knife line should cut: the selection, or (if nothing is
    /// selected) every wall / mesh the plane passes through under the line.
    fn knife_targets(&self, scene: &[Arc<WorldMesh>], p: Vec3, n: Vec3, screen_rect: impl Fn(&WorldMesh) -> Option<Rect>, a: Pos2, b: Pos2) -> Vec<u64> {
        if !self.selection.is_empty() {
            return self.selection.clone();
        }
        scene
            .iter()
            .filter(|w| matches!(self.map.get(w.id).map(|o| &o.kind), Some(Kind::Brush { .. } | Kind::Mesh { .. })))
            .filter(|w| {
                let (mn, mx) = (w.min, w.max);
                let corners = (0..8).map(|i| Vec3::new(if i & 1 != 0 { mx.x } else { mn.x }, if i & 2 != 0 { mx.y } else { mn.y }, if i & 4 != 0 { mx.z } else { mn.z }));
                let side: Vec<f32> = corners.map(|c| (c - p).dot(n)).collect();
                side.iter().any(|s| *s > 1e-4) && side.iter().any(|s| *s < -1e-4)
            })
            .filter(|w| screen_rect(w).is_some_and(|r| seg_hits_rect(a, b, r)))
            .map(|w| w.id)
            .collect()
    }

    fn knife_finish(&mut self, p: Vec3, n: Vec3, targets: Vec<u64>) {
        if targets.is_empty() {
            self.status = "Knife: the line doesn't cross any wall or mesh".into();
            return;
        }
        let mut map = self.map.clone();
        let (count, ids) = knife_cut(&mut map, p, n, &targets, self.settings.knife_split);
        if count == 0 {
            self.status = "Knife: the line doesn't cut through the selected objects".into();
            return;
        }
        self.push_undo();
        self.map = map;
        self.selection = ids;
        self.status = format!("Knife: {} {count} object(s)", if self.settings.knife_split { "split" } else { "sliced" });
    }

    /// Releases a knife drag in a 2D view (line from `a` to `b` in screen space).
    pub(crate) fn knife_release_2d(&mut self, view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>], a: Pos2, b: Pos2) {
        let (_, _, _, _, da) = view.axes();
        let wa = view.uv_to_world(view.screen_to_uv(rect, a), 0.0);
        let wb = view.uv_to_world(view.screen_to_uv(rect, b), 0.0);
        let mut depth = Vec3::ZERO;
        depth[da] = 1.0;
        let n = (wb - wa).cross(depth).normalize_or_zero();
        if n == Vec3::ZERO || (b - a).length() < 3.0 {
            return;
        }
        let v = *view;
        let targets = self.knife_targets(scene, wa, n, |w| Some(v.screen_rect(rect, w.min, w.max)), a, b);
        self.knife_finish(wa, n, targets);
    }

    /// Releases a knife drag in the 3D view.
    pub(crate) fn knife_release_3d(&mut self, rect: Rect, scene: &[Arc<WorldMesh>], a: Pos2, b: Pos2) {
        if (b - a).length() < 3.0 {
            return;
        }
        let fov = self.settings.fov;
        let (o0, d0) = self.cam.ray(rect, fov, a);
        let (o1, d1) = self.cam.ray(rect, fov, b);
        let n = if self.cam.is_ortho() { (o1 - o0).cross(d0) } else { d0.cross(d1) }.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let cam = self.cam;
        let targets = self.knife_targets(
            scene,
            o0,
            n,
            |w| {
                let pts: Vec<Pos2> = (0..8)
                    .filter_map(|i| {
                        let c = Vec3::new(if i & 1 != 0 { w.max.x } else { w.min.x }, if i & 2 != 0 { w.max.y } else { w.min.y }, if i & 4 != 0 { w.max.z } else { w.min.z });
                        cam.project(rect, fov, c)
                    })
                    .collect();
                (!pts.is_empty()).then(|| Rect::from_points(&pts))
            },
            a,
            b,
        );
        self.knife_finish(o0, n, targets);
    }

    pub(crate) fn is_knife_drag(&self, slot: usize) -> Option<Pos2> {
        match self.drag {
            Drag::Knife { slot: s, start } if s == slot => Some(start),
            _ => None,
        }
    }
}
