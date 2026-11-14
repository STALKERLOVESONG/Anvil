//! Triangle meshes: built-in primitives, model loading (.obj / .stl) and
//! world-space scene building shared by the viewports and exporters.

use crate::csg::box_minus;
use crate::map::{Hole, Kind, Map, Object, Shape, TextureDef};
use crate::polymesh::poly_trimesh;
use glam::{Mat3, Vec2, Vec3};
use std::collections::{HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::path::Path;
use std::sync::Arc;

/// Triangles wound counter-clockwise when seen from outside, plus the
/// "feature" edges used for wireframes (so boxes don't show diagonals).
#[derive(Clone, Default, Debug)]
pub struct TriMesh {
    pub pos: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub edges: Vec<[u32; 2]>,
    /// Polygon face index per triangle (editable meshes only).
    pub tri_face: Vec<u32>,
    /// Per-vertex texture coordinates (image space, v down) for textured models.
    pub uv: Vec<Vec2>,
    /// Material index per triangle (textured models).
    pub tri_mat: Vec<u16>,
    /// Diffuse image path per material ("" = untextured).
    pub materials: Vec<String>,
    /// Normal map path per material ("" = none).
    pub material_normals: Vec<String>,
}

impl TriMesh {
    pub fn v(&mut self, p: Vec3) -> u32 {
        self.pos.push(p);
        (self.pos.len() - 1) as u32
    }

    pub fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        let (i, j, k, l) = (self.v(a), self.v(b), self.v(c), self.v(d));
        self.tris.push([i, j, k]);
        self.tris.push([i, k, l]);
        self.edges.extend([[i, j], [j, k], [k, l], [l, i]]);
    }

    pub fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let (i, j, k) = (self.v(a), self.v(b), self.v(c));
        self.tris.push([i, j, k]);
        self.edges.extend([[i, j], [j, k], [k, i]]);
    }

    pub fn add_box(&mut self, mn: Vec3, mx: Vec3) {
        let p = |x: bool, y: bool, z: bool| {
            Vec3::new(if x { mx.x } else { mn.x }, if y { mx.y } else { mn.y }, if z { mx.z } else { mn.z })
        };
        let (f, t) = (false, true);
        self.quad(p(t, f, t), p(t, f, f), p(t, t, f), p(t, t, t)); // +X
        self.quad(p(f, f, f), p(f, f, t), p(f, t, t), p(f, t, f)); // -X
        self.quad(p(f, t, t), p(t, t, t), p(t, t, f), p(f, t, f)); // +Y
        self.quad(p(f, f, f), p(t, f, f), p(t, f, t), p(f, f, t)); // -Y
        self.quad(p(f, f, t), p(t, f, t), p(t, t, t), p(f, t, t)); // +Z
        self.quad(p(t, f, f), p(f, f, f), p(f, t, f), p(t, t, f)); // -Z
    }

    pub fn scaled(mut self, s: Vec3) -> Self {
        for p in &mut self.pos {
            *p *= s;
        }
        self
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        if self.pos.is_empty() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        for p in &self.pos {
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        (mn, mx)
    }
}

pub fn unit_box() -> TriMesh {
    let mut m = TriMesh::default();
    m.add_box(Vec3::splat(-0.5), Vec3::splat(0.5));
    m
}

/// Matches Roblox's WedgePart: full height at the back (+Z), slope facing front (-Z).
pub fn unit_wedge() -> TriMesh {
    let h = 0.5;
    let a = Vec3::new(-h, -h, -h);
    let b = Vec3::new(h, -h, -h);
    let c = Vec3::new(h, -h, h);
    let d = Vec3::new(-h, -h, h);
    let e = Vec3::new(-h, h, h);
    let f = Vec3::new(h, h, h);
    let mut m = TriMesh::default();
    m.quad(a, b, c, d); // bottom
    m.quad(d, c, f, e); // back
    m.quad(b, a, e, f); // slope
    m.tri(a, d, e); // -X side
    m.tri(b, f, c); // +X side
    m
}

/// Cylinder along the local Y axis.
pub fn unit_cylinder(seg: u32) -> TriMesh {
    let mut m = TriMesh::default();
    let mut bot = Vec::new();
    let mut top = Vec::new();
    for i in 0..seg {
        let a = i as f32 / seg as f32 * TAU;
        let (s, c) = a.sin_cos();
        bot.push(m.v(Vec3::new(0.5 * c, -0.5, 0.5 * s)));
        top.push(m.v(Vec3::new(0.5 * c, 0.5, 0.5 * s)));
    }
    let cb = m.v(Vec3::new(0.0, -0.5, 0.0));
    let ct = m.v(Vec3::new(0.0, 0.5, 0.0));
    let every = (seg / 8).max(1);
    for i in 0..seg as usize {
        let j = (i + 1) % seg as usize;
        m.tris.push([bot[j], bot[i], top[i]]);
        m.tris.push([bot[j], top[i], top[j]]);
        m.tris.push([ct, top[j], top[i]]);
        m.tris.push([cb, bot[i], bot[j]]);
        m.edges.push([bot[i], bot[j]]);
        m.edges.push([top[i], top[j]]);
        if i as u32 % every == 0 {
            m.edges.push([bot[i], top[i]]);
        }
    }
    m
}

pub fn unit_sphere(stacks: u32, slices: u32) -> TriMesh {
    let mut m = TriMesh::default();
    for i in 0..=stacks {
        let th = PI * i as f32 / stacks as f32;
        for j in 0..=slices {
            let ph = TAU * j as f32 / slices as f32;
            m.v(Vec3::new(0.5 * th.sin() * ph.cos(), 0.5 * th.cos(), 0.5 * th.sin() * ph.sin()));
        }
    }
    let idx = |i: u32, j: u32| i * (slices + 1) + j;
    for i in 0..stacks {
        for j in 0..slices {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            if i != 0 {
                m.tris.push([a, d, c]);
            }
            if i != stacks - 1 {
                m.tris.push([a, c, b]);
            }
            if j % 2 == 0 {
                m.edges.push([a, b]);
            }
            if i > 0 && i % 2 == 0 {
                m.edges.push([a, d]);
            }
        }
    }
    // equator ring for a clean silhouette in the side views
    let eq = stacks / 2;
    for j in 0..slices {
        m.edges.push([idx(eq, j), idx(eq, j + 1)]);
    }
    m
}

/// Per-step boxes (in unit space) for a staircase that climbs towards +Z.
pub fn stair_boxes(steps: u32) -> Vec<(Vec3, Vec3)> {
    let n = steps.max(1);
    (0..n)
        .map(|i| {
            let f0 = i as f32 / n as f32;
            let f1 = (i + 1) as f32 / n as f32;
            (Vec3::new(-0.5, -0.5, -0.5 + f0), Vec3::new(0.5, -0.5 + f1, -0.5 + f1))
        })
        .collect()
}

pub fn unit_stairs(steps: u32) -> TriMesh {
    let mut m = TriMesh::default();
    for (mn, mx) in stair_boxes(steps) {
        m.add_box(mn, mx);
    }
    m
}

pub fn octahedron() -> TriMesh {
    let mut m = TriMesh::default();
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let x = Vec3::new(0.5 * sx, 0.0, 0.0);
                let y = Vec3::new(0.0, 0.5 * sy, 0.0);
                let z = Vec3::new(0.0, 0.0, 0.5 * sz);
                if sx * sy * sz > 0.0 {
                    m.tri(x, y, z);
                } else {
                    m.tri(x, z, y);
                }
            }
        }
    }
    m
}

pub fn primitive(shape: Shape, size: Vec3, steps: u32) -> TriMesh {
    let m = match shape {
        Shape::Box => unit_box(),
        Shape::Wedge => unit_wedge(),
        Shape::Cylinder => unit_cylinder(24),
        Shape::Sphere => unit_sphere(12, 16),
        Shape::Stairs => unit_stairs(steps),
    };
    m.scaled(size)
}

/// Brush mesh including any boolean holes (Box brushes only).
pub fn brush_mesh(shape: Shape, size: Vec3, steps: u32, holes: &[Hole]) -> TriMesh {
    if shape == Shape::Box && !holes.is_empty() {
        let mut m = TriMesh::default();
        for (mn, mx) in box_minus(size, holes) {
            m.add_box(mn, mx);
        }
        m
    } else {
        primitive(shape, size, steps)
    }
}

pub fn decal_quad(size: Vec2) -> TriMesh {
    let (w, h) = (size.x * 0.5, size.y * 0.5);
    let mut m = TriMesh::default();
    m.quad(Vec3::new(-w, -h, 0.0), Vec3::new(w, -h, 0.0), Vec3::new(w, h, 0.0), Vec3::new(-w, h, 0.0));
    m
}

// ---------------------------------------------------------------------------
// Model loading
// ---------------------------------------------------------------------------

const MAX_WIRE_EDGES: usize = 12_000;

pub fn load_model(path: &str) -> Result<TriMesh, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if crate::texture::is_network_path(path) {
        return Err("Network paths (\\\\server\\...) aren't opened, for safety. Copy the file to this PC.".into());
    }
    let mut m = match ext.as_str() {
        "obj" => load_obj(path)?,
        "stl" => load_stl(path)?,
        "glb" | "gltf" => crate::gltf::load(path)?,
        _ => return Err(format!("Unsupported model format '.{ext}' (use .obj, .stl, .glb or .gltf)")),
    };
    if m.tris.is_empty() {
        return Err("Model has no triangles".into());
    }
    match unique_edges(&m.tris, MAX_WIRE_EDGES) {
        Some(e) => m.edges = e,
        None => {
            // Too dense for a readable wireframe: show the bounding box instead.
            let (mn, mx) = m.bounds();
            let mut bb = TriMesh::default();
            bb.add_box(mn, mx);
            let base = m.pos.len() as u32;
            m.pos.extend(bb.pos);
            m.edges = bb.edges.iter().map(|e| [e[0] + base, e[1] + base]).collect();
        }
    }
    Ok(m)
}

fn load_obj(path: &str) -> Result<TriMesh, String> {
    let opts = tobj::LoadOptions { triangulate: true, single_index: true, ..Default::default() };
    let dir = Path::new(path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let file = std::fs::File::open(path).map_err(|e| format!("OBJ error: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    // .mtl files named inside the OBJ never resolve to network shares
    let (models, materials) = tobj::load_obj_buf(&mut reader, &opts, |p| {
        let full = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
        if crate::texture::is_network_path(&full.to_string_lossy()) {
            return Err(tobj::LoadError::OpenFileFailed);
        }
        tobj::load_mtl(&full)
    })
    .map_err(|e| format!("OBJ error: {e}"))?;
    let resolve = |t: &Option<String>| -> String {
        match t {
            Some(t) if !t.trim().is_empty() => {
                let p = Path::new(t.trim());
                let full = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                full.to_string_lossy().to_string()
            }
            _ => String::new(),
        }
    };
    let mut m = TriMesh::default();
    if let Ok(mats) = &materials {
        for mat in mats {
            m.materials.push(resolve(&mat.diffuse_texture));
            m.material_normals.push(resolve(&mat.normal_texture));
        }
    }
    let has_tex = m.materials.iter().any(|t| !t.is_empty());
    let all_uv = models.iter().all(|md| md.mesh.texcoords.len() / 2 == md.mesh.positions.len() / 3);
    for model in models {
        let base = m.pos.len() as u32;
        for p in model.mesh.positions.chunks_exact(3) {
            m.pos.push(Vec3::new(p[0], p[1], p[2]));
        }
        if has_tex && all_uv {
            for t in model.mesh.texcoords.chunks_exact(2) {
                m.uv.push(Vec2::new(t[0], 1.0 - t[1]));
            }
        }
        let mat = model.mesh.material_id.unwrap_or(0).min(u16::MAX as usize) as u16;
        for t in model.mesh.indices.chunks_exact(3) {
            m.tris.push([base + t[0], base + t[1], base + t[2]]);
            m.tri_mat.push(mat);
        }
    }
    if !(has_tex && all_uv) {
        m.uv.clear();
        m.materials.clear();
        m.material_normals.clear();
    }
    Ok(m)
}

fn load_stl(path: &str) -> Result<TriMesh, String> {
    let data = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let mut m = TriMesh::default();
    let binary_count = if data.len() >= 84 {
        let n = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
        (84 + n * 50 == data.len()).then_some(n)
    } else {
        None
    };
    let push = |m: &mut TriMesh, a: Vec3, b: Vec3, c: Vec3| {
        let (i, j, k) = (m.v(a), m.v(b), m.v(c));
        m.tris.push([i, j, k]);
    };
    if let Some(n) = binary_count {
        let rd = |o: usize| f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        for i in 0..n {
            let off = 84 + i * 50 + 12;
            let v = |k: usize| Vec3::new(rd(off + k * 12), rd(off + k * 12 + 4), rd(off + k * 12 + 8));
            push(&mut m, v(0), v(1), v(2));
        }
    } else {
        let text = String::from_utf8_lossy(&data);
        let mut verts = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("vertex") {
                let f: Vec<f32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if f.len() == 3 {
                    verts.push(Vec3::new(f[0], f[1], f[2]));
                }
            }
        }
        for c in verts.chunks_exact(3) {
            push(&mut m, c[0], c[1], c[2]);
        }
    }
    Ok(m)
}

fn unique_edges(tris: &[[u32; 3]], cap: usize) -> Option<Vec<[u32; 2]>> {
    let mut set = HashSet::new();
    for t in tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            set.insert(if a < b { (a, b) } else { (b, a) });
            if set.len() > cap {
                return None;
            }
        }
    }
    Some(set.into_iter().map(|(a, b)| [a, b]).collect())
}

/// Loaded models, keyed by file path. Failures are cached too so a missing
/// file doesn't get re-read every frame.
#[derive(Default, Clone)]
pub struct ModelCache {
    items: HashMap<String, Result<Arc<TriMesh>, String>>,
    /// Team Create: map path -> local copy (None = not downloaded yet, don't open).
    alias: HashMap<String, Option<String>>,
}

impl ModelCache {
    pub fn get(&mut self, path: &str) -> Result<Arc<TriMesh>, String> {
        if let Some(v) = self.items.get(path) {
            return v.clone();
        }
        let v = match self.alias.get(path) {
            Some(None) => return Err("Waiting for this model from the Team Create host".into()),
            Some(Some(real)) => load_model(real),
            None => load_model(path),
        }
        .map(Arc::new);
        self.items.insert(path.to_string(), v.clone());
        v
    }

    /// Makes `path` load from `real` instead (None: never open `path`).
    pub fn alias(&mut self, path: &str, real: Option<String>) {
        self.items.remove(path);
        self.alias.insert(path.to_string(), real);
    }

    /// Puts an in-memory mesh under a made-up path.
    pub fn insert(&mut self, path: &str, m: TriMesh) {
        self.items.insert(path.to_string(), Ok(Arc::new(m)));
    }

    pub fn peek(&self, path: &str) -> Option<&Result<Arc<TriMesh>, String>> {
        self.items.get(path)
    }

    pub fn forget(&mut self, path: &str) {
        self.items.remove(path);
    }
}

fn missing_model() -> TriMesh {
    unit_box().scaled(Vec3::splat(2.0))
}

/// Object-space mesh (before the object's transform is applied).
pub fn local_mesh(obj: &Object, cache: &mut ModelCache) -> Arc<TriMesh> {
    match &obj.kind {
        Kind::Brush { shape, size, steps, holes } => Arc::new(brush_mesh(*shape, *size, *steps, holes)),
        Kind::Mesh { verts, faces } => Arc::new(poly_trimesh(verts, faces)),
        Kind::Prop { model, .. } => cache.get(model).unwrap_or_else(|_| Arc::new(missing_model())),
        Kind::Decal { size } => Arc::new(decal_quad(*size)),
        Kind::Light { .. } => Arc::new(octahedron().scaled(Vec3::splat(1.2))),
        Kind::Spawn { size } => Arc::new(unit_box().scaled(*size)),
        Kind::Prompt { .. } => Arc::new(octahedron().scaled(Vec3::splat(0.9))),
        Kind::Teleport { size, .. } => Arc::new(unit_box().scaled(*size)),
        Kind::Target {} => Arc::new(octahedron().scaled(Vec3::splat(0.7))),
    }
}

/// Planar box mapping in object space. Returns per-corner UVs and the
/// surface tangent (U) / up (V) directions in object space.
pub fn box_uv(p: [Vec3; 3], scale: Vec2, offset: Vec2) -> ([Vec2; 3], Vec3, Vec3) {
    let n = (p[1] - p[0]).cross(p[2] - p[0]);
    let a = n.abs();
    let (u_axis, v_axis) = if a.x >= a.y && a.x >= a.z {
        (Vec3::new(0.0, 0.0, -n.x.signum()), Vec3::Y)
    } else if a.y >= a.z {
        (Vec3::X, Vec3::NEG_Z)
    } else {
        (Vec3::new(n.z.signum(), 0.0, 0.0), Vec3::Y)
    };
    let sx = if scale.x.abs() < 1e-4 { 1.0 } else { scale.x };
    let sy = if scale.y.abs() < 1e-4 { 1.0 } else { scale.y };
    let uv = p.map(|q| Vec2::new((q.dot(u_axis) + offset.x) / sx, (-q.dot(v_axis) + offset.y) / sy));
    (uv, u_axis, v_axis)
}

/// Which box face (0..6 = +X -X +Y -Y +Z -Z) a local-space normal points at.
pub fn box_face_dir(n: Vec3) -> usize {
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        if n.x >= 0.0 {
            0
        } else {
            1
        }
    } else if a.y >= a.z {
        if n.y >= 0.0 {
            2
        } else {
            3
        }
    } else if n.z >= 0.0 {
        4
    } else {
        5
    }
}

/// The face key a triangle belongs to (for per-face textures):
/// brushes use the box direction, meshes their polygon index.
pub fn tri_face_key(obj: &Object, mesh: &TriMesh, ti: usize) -> Option<usize> {
    match obj.kind {
        Kind::Mesh { .. } => mesh.tri_face.get(ti).map(|f| *f as usize),
        Kind::Brush { .. } => {
            let t = mesh.tris.get(ti)?;
            let (a, b, c) = (mesh.pos[t[0] as usize], mesh.pos[t[1] as usize], mesh.pos[t[2] as usize]);
            Some(box_face_dir((b - a).cross(c - a)))
        }
        _ => None,
    }
}

pub fn rotate_uv(uv: [Vec2; 3], deg: f32) -> [Vec2; 3] {
    if deg.abs() < 1e-4 {
        return uv;
    }
    let (s, c) = deg.to_radians().sin_cos();
    uv.map(|v| Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c))
}

/// Texture info carried into the renderer.
#[derive(Clone)]
pub struct SceneTex {
    pub image: String,
    pub normal: String,
    pub metallic: bool,
    pub tint: [u8; 3],
}

/// Marks an untextured triangle in `WorldMesh::tri_tex`.
pub const NO_TEX: u16 = u16::MAX;

/// An object's mesh transformed into world space.
pub struct WorldMesh {
    pub id: u64,
    pub mesh: Arc<TriMesh>,
    pub pos: Vec<Vec3>,
    pub min: Vec3,
    pub max: Vec3,
    pub color: [u8; 3],
    pub alpha: f32,
    /// Texture slots used by this object.
    pub texs: Vec<SceneTex>,
    /// Slot per triangle (`NO_TEX` = untextured); empty when `texs` is empty.
    pub tri_tex: Vec<u16>,
    /// Per-triangle corner UVs (textured objects only).
    pub tri_uv: Vec<[Vec2; 3]>,
    /// Per-triangle world tangent / up / normal (normal-mapped objects only).
    pub tri_tbn: Vec<Mat3>,
    pub decal: bool,
    pub solid: bool,
    /// Throws a sun shadow (not lights, decals or spawn pads).
    pub cast_shadow: bool,
}

pub fn world_mesh(obj: &Object, textures: &[TextureDef], cache: &mut ModelCache) -> WorldMesh {
    let mesh = local_mesh(obj, cache);
    let m = obj.matrix();
    let pos: Vec<Vec3> = mesh.pos.iter().map(|p| m.transform_point3(*p)).collect();
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);
    for p in &pos {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    if pos.is_empty() {
        mn = obj.pos;
        mx = obj.pos;
    }
    let decal = matches!(obj.kind, Kind::Decal { .. });
    let find = |id: Option<u64>| id.and_then(|id| textures.iter().find(|t| t.id == id));
    let texdef = find(obj.texture);
    let mut tri_uv = Vec::new();
    let mut tri_tbn = Vec::new();
    let mut texs: Vec<SceneTex> = Vec::new();
    let mut tri_tex = Vec::new();
    if texdef.is_none() && obj.face_tex.is_empty() && !mesh.materials.is_empty() && mesh.uv.len() == mesh.pos.len() {
        // a model with its own textures (e.g. an imported Half-Life prop)
        for (i, img) in mesh.materials.iter().enumerate() {
            texs.push(SceneTex {
                image: img.clone(),
                normal: mesh.material_normals.get(i).cloned().unwrap_or_default(),
                metallic: false,
                tint: obj.tex_params.tint,
            });
        }
        for t in &mesh.tris {
            tri_uv.push([mesh.uv[t[0] as usize], mesh.uv[t[1] as usize], mesh.uv[t[2] as usize]]);
        }
        tri_tex = mesh.tri_mat.clone();
    } else if texdef.is_some() || obj.face_tex.iter().any(|f| f.is_some()) {
        let rot = Mat3::from_quat(obj.rotation());
        let sc = obj.scale();
        let decal_size = match obj.kind {
            Kind::Decal { size } => size,
            _ => Vec2::ONE,
        };
        let mut slots: Vec<(u64, crate::map::TexParams)> = Vec::new();
        let mut any_normal = false;
        let mut tbns = Vec::with_capacity(mesh.tris.len());
        for (ti, tr) in mesh.tris.iter().enumerate() {
            let lp = [mesh.pos[tr[0] as usize] * sc, mesh.pos[tr[1] as usize] * sc, mesh.pos[tr[2] as usize] * sc];
            let face = tri_face_key(obj, &mesh, ti);
            let (def, params) = match face.and_then(|f| obj.face_tex.get(f)).and_then(|f| f.as_ref()) {
                Some(ft) => (find(ft.texture), ft.params),
                None => (texdef, obj.tex_params),
            };
            let Some(t) = def else {
                tri_tex.push(NO_TEX);
                tri_uv.push([Vec2::ZERO; 3]);
                tbns.push(Mat3::IDENTITY);
                continue;
            };
            let slot = match slots.iter().position(|(id, p)| *id == t.id && *p == params) {
                Some(i) => i,
                None => {
                    slots.push((t.id, params));
                    texs.push(SceneTex { image: t.image.clone(), normal: t.normal_map.clone(), metallic: t.metallic, tint: params.tint });
                    any_normal |= !t.normal_map.trim().is_empty();
                    slots.len() - 1
                }
            };
            tri_tex.push(slot as u16);
            if decal {
                let uv = lp.map(|q| Vec2::new(q.x / decal_size.x.max(1e-4) + 0.5, 0.5 - q.y / decal_size.y.max(1e-4)));
                tri_uv.push(uv);
                tbns.push(Mat3::from_cols(rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z));
            } else {
                let (uv, ua, va) = box_uv(lp, t.scale * params.scale, t.offset + params.offset);
                tri_uv.push(rotate_uv(uv, params.rotation));
                let n = (lp[1] - lp[0]).cross(lp[2] - lp[0]).normalize_or_zero();
                tbns.push(Mat3::from_cols(rot * ua, rot * va, rot * n));
            }
        }
        if any_normal {
            tri_tbn = tbns;
        }
    }
    let solid = obj.collide && !matches!(obj.kind, Kind::Light { .. } | Kind::Decal { .. });
    WorldMesh {
        id: obj.id,
        mesh,
        pos,
        min: mn,
        max: mx,
        color: obj.color,
        alpha: if decal { (1.0 - obj.transparency).clamp(0.05, 1.0) } else { (1.0 - obj.transparency).clamp(0.15, 1.0) },
        texs,
        tri_tex,
        tri_uv,
        tri_tbn,
        decal,
        solid,
        cast_shadow: !matches!(obj.kind, Kind::Light { .. } | Kind::Decal { .. } | Kind::Spawn { .. } | Kind::Prompt { .. } | Kind::Target {}),
    }
}

pub fn build_scene(map: &Map, cache: &mut ModelCache) -> Vec<Arc<WorldMesh>> {
    map.objects.iter().filter(|o| !o.hidden).map(|o| Arc::new(world_mesh(o, &map.textures, cache))).collect()
}

/// Rebuilds only the objects that changed since last frame.
#[derive(Default)]
pub struct SceneCache {
    items: HashMap<u64, (Object, u64, Arc<WorldMesh>)>,
}

fn textures_hash(t: &[TextureDef]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{t:?}").hash(&mut h);
    h.finish()
}

impl SceneCache {
    pub fn build(&mut self, map: &Map, cache: &mut ModelCache) -> Vec<Arc<WorldMesh>> {
        let th = textures_hash(&map.textures);
        let mut out = Vec::with_capacity(map.objects.len());
        let mut keep = std::collections::HashSet::with_capacity(map.objects.len());
        for o in map.objects.iter().filter(|o| !o.hidden) {
            keep.insert(o.id);
            let textured = o.texture.is_some() || !o.face_tex.is_empty();
            if let Some((old, h, wm)) = self.items.get(&o.id) {
                if old == o && (!textured || *h == th) {
                    out.push(wm.clone());
                    continue;
                }
            }
            let wm = Arc::new(world_mesh(o, &map.textures, cache));
            self.items.insert(o.id, (o.clone(), th, wm.clone()));
            out.push(wm);
        }
        self.items.retain(|id, _| keep.contains(id));
        out
    }

    /// Forget everything (e.g. after a model file changed on disk).
    pub fn clear(&mut self) {
        self.items.clear();
    }
}
