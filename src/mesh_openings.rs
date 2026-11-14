//! Doors and windows on meshes (and on any brush that isn't a plain box):
//! the opening is lined up with the clicked face, sized through the wall's
//! thickness, and cut out with the mesh booleans.

use crate::app::AnvilApp;
use crate::map::{Kind, Material, Object, Shape};
use crate::mesh::WorldMesh;
use crate::mesh_csg::{csg, object_polys, to_mesh, Op};
use crate::polymesh::box_poly;
use crate::view3d::ray_tri;
use glam::{Mat3, Quat, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct MeshCut {
    pub center: Vec3,
    /// Along the wall, up, through the wall.
    pub size: Vec3,
    pub rot: Quat,
    /// Glass pane (center, size) in the same orientation.
    pub glass: Option<(Vec3, Vec3)>,
}

/// Nearest hit of a ray on this object: (distance, point, face normal).
pub fn ray_object(wm: &WorldMesh, o: Vec3, d: Vec3) -> Option<(f32, Vec3, Vec3)> {
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    for t in &wm.mesh.tris {
        let (a, b, c) = (wm.pos[t[0] as usize], wm.pos[t[1] as usize], wm.pos[t[2] as usize]);
        if let Some(s) = ray_tri(o, d, a, b, c) {
            if best.is_none_or(|x| s < x.0) {
                let mut n = (b - a).cross(c - a).normalize_or_zero();
                if n.dot(d) > 0.0 {
                    n = -n;
                }
                best = Some((s, o + d * s, n));
            }
        }
    }
    best
}

/// The 8-corner box of a cut as polygons (outward faces).
pub fn cutter_polys(center: Vec3, size: Vec3, rot: Quat) -> Vec<Vec<Vec3>> {
    let (v, f) = box_poly(size);
    f.iter().map(|face| face.iter().map(|i| center + rot * v[*i as usize]).collect()).collect()
}

/// Edges of a rotated box (for previews).
pub fn box_edges(center: Vec3, size: Vec3, rot: Quat) -> Vec<(Vec3, Vec3)> {
    let h = size * 0.5;
    let c: [Vec3; 8] = std::array::from_fn(|i| center + rot * Vec3::new(if i & 1 != 0 { h.x } else { -h.x }, if i & 2 != 0 { h.y } else { -h.y }, if i & 4 != 0 { h.z } else { -h.z }));
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

fn snap(v: f32, g: f32) -> f32 {
    if g > 0.0 { (v / g).round() * g } else { v }
}

/// A 2D view's "ray" through a point: straight along the view's depth axis.
pub fn view_ray(view: &crate::view2d::View2D, at: Vec3) -> (Vec3, Vec3) {
    let (_, _, _, _, da) = view.axes();
    let mut dir = Vec3::ZERO;
    dir[da] = -1.0;
    (at - dir * 10_000.0, dir)
}

impl AnvilApp {
    fn is_box_wall(&self, id: u64) -> bool {
        matches!(self.map.get(id).map(|o| &o.kind), Some(Kind::Brush { shape: Shape::Box, .. }))
    }

    /// Door / Window click: box walls get an editable hole, anything else
    /// (meshes, wedges, cylinders...) is cut with the mesh booleans.
    pub(crate) fn cut_opening_at(&mut self, id: u64, at: Vec3, ray: (Vec3, Vec3), window: bool, snapping: bool) {
        if self.is_box_wall(id) {
            self.cut_opening(id, at, window, snapping);
        } else {
            self.cut_mesh_opening(id, ray.0, ray.1, window, snapping);
        }
    }

    /// Preview segments for either kind (glass flagged true).
    pub(crate) fn any_opening_preview(&self, scene: &[std::sync::Arc<WorldMesh>], id: u64, at: Vec3, ray: (Vec3, Vec3), snapping: bool) -> Vec<(Vec3, Vec3, bool)> {
        if self.is_box_wall(id) {
            return self.opening_preview(id, at, snapping);
        }
        let Some(wm) = scene.iter().find(|w| w.id == id) else { return Vec::new() };
        if !matches!(self.map.get(id).map(|o| &o.kind), Some(Kind::Mesh { .. } | Kind::Brush { .. })) {
            return Vec::new();
        }
        let Ok(c) = self.mesh_cut_plan(wm, ray.0, ray.1, self.tool == crate::app::Tool::Window, snapping) else { return Vec::new() };
        let mut out: Vec<(Vec3, Vec3, bool)> = box_edges(c.center, c.size, c.rot).into_iter().map(|(a, b)| (a, b, false)).collect();
        if let Some((gc, gs)) = c.glass {
            out.extend(box_edges(gc, gs, c.rot).into_iter().map(|(a, b)| (a, b, true)));
        }
        out
    }

    /// Where a door / window goes on a mesh hit by the ray `(o, d)`.
    pub(crate) fn mesh_cut_plan(&self, wm: &WorldMesh, o: Vec3, d: Vec3, window: bool, snapping: bool) -> Result<MeshCut, String> {
        plan_mesh_cut(wm, o, d, window, &self.settings, if snapping { self.snap_step() } else { 0.0 })
    }
}

/// Where a door / window goes on a mesh hit by the ray `(o, d)` (grid 0 = no snapping).
pub fn plan_mesh_cut(wm: &WorldMesh, o: Vec3, d: Vec3, window: bool, settings: &crate::settings::Settings, grid: f32) -> Result<MeshCut, String> {
    {
        let (_, at, n) = ray_object(wm, o, d).ok_or("Click on the mesh")?;
        // thickness: go in from the face and find where we come out
        let inward = -n;
        let thick = ray_object(wm, at + inward * 0.01, inward).map(|h| h.0 + 0.01).filter(|t| *t > 0.02).unwrap_or(settings.wall_thickness);
        let s = settings;
        let g = grid * 0.5;
        if n.y.abs() > 0.7 {
            // floor / ceiling: a hatch
            let (w, dp) = if window { (s.window_width, s.window_height) } else { (s.door_width, s.door_height) };
            let c = Vec3::new(snap(at.x, g), at.y + inward.y * thick * 0.5, snap(at.z, g));
            return Ok(MeshCut { center: c, size: Vec3::new(w, thick + 0.4, dp), rot: Quat::IDENTITY, glass: None });
        }
        let nh = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        let t = Vec3::Y.cross(nh).normalize_or_zero();
        let rot = Quat::from_mat3(&Mat3::from_cols(t, Vec3::Y, nh));
        let (w, h, bottom) = if window { (s.window_width, s.window_height, s.window_sill) } else { (s.door_width, s.door_height, 0.0) };
        let floor = wm.min.y;
        let y0 = floor + bottom - if window { 0.0 } else { 0.1 };
        let y1 = (floor + bottom + h).min(wm.max.y + 0.1);
        if y1 <= y0 + 0.05 {
            return Err("This mesh is too short for that opening (see the Door / Window tool settings)".into());
        }
        // slide along the wall to the snapped spot
        let along = snap(at.dot(t), g);
        let mut c = at + t * (along - at.dot(t)) - nh * (thick * 0.5);
        c.y = (y0 + y1) * 0.5;
        let size = Vec3::new(w, y1 - y0, thick + 0.4);
        let glass = (window && s.window_glass).then(|| (c, Vec3::new(w, y1 - y0, (thick * 0.25).max(0.1))));
        Ok(MeshCut { center: c, size, rot, glass })
    }
}

impl AnvilApp {
    /// Cuts a door / window into a mesh along the ray `(o, d)`.
    pub(crate) fn cut_mesh_opening(&mut self, id: u64, o: Vec3, d: Vec3, window: bool, snapping: bool) {
        let Some(obj) = self.map.get(id).cloned() else { return };
        if !matches!(obj.kind, Kind::Mesh { .. } | Kind::Brush { .. }) {
            self.status = "Doors and windows cut into walls, floors and meshes".into();
            return;
        }
        let wm = crate::mesh::world_mesh(&obj, &self.map.textures, &mut self.models);
        let cut = match self.mesh_cut_plan(&wm, o, d, window, snapping) {
            Ok(c) => c,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let solid = object_polys(&obj, &mut self.models);
        let result = csg(solid, cutter_polys(cut.center, cut.size, cut.rot), Op::Subtract);
        let Some((verts, faces, c)) = to_mesh(&result) else {
            self.status = "The opening would remove the whole mesh".into();
            return;
        };
        self.push_undo();
        if let Some(x) = self.map.get_mut(id) {
            x.kind = Kind::Mesh { verts, faces };
            x.pos = c;
            x.rot = Vec3::ZERO;
            x.face_tex.clear();
            x.text = None;
        }
        if let Some((gc, gs)) = cut.glass {
            let mut g = Object::brush(Shape::Box, gs, gc, [160, 205, 235], Material::Glass);
            g.set_rotation(cut.rot);
            g.transparency = 0.6;
            g.folder = obj.folder;
            g.name = format!("{}_Glass", obj.name);
            self.map.add(g);
        }
        self.selection = vec![id];
        self.status = format!("Cut a {} into {}", if window { "window" } else { "door" }, obj.name);
    }
}
