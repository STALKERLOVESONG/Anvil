//! Mesh booleans (union / subtract / intersect) for any solid shapes, using
//! the classic BSP-tree method (Laidlaw / Naylor, as popularised by csg.js).
//! Used to sculpt models: drop a cube or sphere on a mesh and union it in or
//! cut it out.

use crate::app::AnvilApp;
use crate::map::{Kind, Object};
use crate::mesh::{local_mesh, ModelCache};
use glam::Vec3;
use std::collections::HashMap;

const EPS: f32 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Union,
    Subtract,
    Intersect,
}

#[derive(Clone, Copy, Debug)]
struct Plane {
    n: Vec3,
    w: f32,
}

impl Plane {
    fn from(p: &[Vec3]) -> Option<Plane> {
        // Newell normal: robust for any planar polygon
        let mut n = Vec3::ZERO;
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        let len = n.length();
        if len < 1e-9 {
            return None;
        }
        let n = n / len;
        Some(Plane { n, w: n.dot(p[0]) })
    }

    fn flip(&mut self) {
        self.n = -self.n;
        self.w = -self.w;
    }

    /// Sorts `poly` into the four lists (coplanar ones by facing).
    fn split(&self, poly: &Poly, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, f: &mut Vec<Poly>, b: &mut Vec<Poly>) {
        const COP: u8 = 0;
        const FRONT: u8 = 1;
        const BACK: u8 = 2;
        let mut kind = 0u8;
        let types: Vec<u8> = poly
            .v
            .iter()
            .map(|v| {
                let t = self.n.dot(*v) - self.w;
                let k = if t < -EPS { BACK } else if t > EPS { FRONT } else { COP };
                kind |= k;
                k
            })
            .collect();
        match kind {
            COP => {
                if self.n.dot(poly.plane.n) > 0.0 {
                    cf.push(poly.clone())
                } else {
                    cb.push(poly.clone())
                }
            }
            FRONT => f.push(poly.clone()),
            BACK => b.push(poly.clone()),
            _ => {
                let (mut fv, mut bv) = (Vec::new(), Vec::new());
                let n = poly.v.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (ti, tj) = (types[i], types[j]);
                    let (vi, vj) = (poly.v[i], poly.v[j]);
                    if ti != BACK {
                        fv.push(vi);
                    }
                    if ti != FRONT {
                        bv.push(vi);
                    }
                    if (ti | tj) == 3 {
                        let t = (self.w - self.n.dot(vi)) / self.n.dot(vj - vi);
                        let v = vi.lerp(vj, t);
                        fv.push(v);
                        bv.push(v);
                    }
                }
                if fv.len() >= 3 {
                    f.push(Poly { v: fv, plane: poly.plane });
                }
                if bv.len() >= 3 {
                    b.push(Poly { v: bv, plane: poly.plane });
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Poly {
    v: Vec<Vec3>,
    plane: Plane,
}

impl Poly {
    fn new(v: Vec<Vec3>) -> Option<Poly> {
        Plane::from(&v).map(|plane| Poly { v, plane })
    }
    fn flip(&mut self) {
        self.v.reverse();
        self.plane.flip();
    }
}

#[derive(Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polys: Vec<Poly>,
}

impl Node {
    fn new(polys: Vec<Poly>) -> Node {
        let mut n = Node::default();
        n.build(polys);
        n
    }

    fn invert(&mut self) {
        for p in &mut self.polys {
            p.flip();
        }
        if let Some(pl) = &mut self.plane {
            pl.flip();
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// Removes the parts of `polys` inside this solid.
    fn clip_polys(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let Some(pl) = self.plane else { return polys };
        let (mut f, mut b) = (Vec::new(), Vec::new());
        for p in &polys {
            let (mut cf, mut cb) = (Vec::new(), Vec::new());
            pl.split(p, &mut cf, &mut cb, &mut f, &mut b);
            f.extend(cf);
            b.extend(cb);
        }
        let mut f = match &self.front {
            Some(n) => n.clip_polys(f),
            None => f,
        };
        let b = match &self.back {
            Some(n) => n.clip_polys(b),
            None => Vec::new(),
        };
        f.extend(b);
        f
    }

    fn clip_to(&mut self, other: &Node) {
        self.polys = other.clip_polys(std::mem::take(&mut self.polys));
        if let Some(f) = &mut self.front {
            f.clip_to(other);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other);
        }
    }

    fn all(&self) -> Vec<Poly> {
        let mut out = self.polys.clone();
        if let Some(f) = &self.front {
            out.extend(f.all());
        }
        if let Some(b) = &self.back {
            out.extend(b.all());
        }
        out
    }

    fn build(&mut self, polys: Vec<Poly>) {
        if polys.is_empty() {
            return;
        }
        let pl = *self.plane.get_or_insert(polys[0].plane);
        let (mut f, mut b) = (Vec::new(), Vec::new());
        let (mut cf, mut cb) = (Vec::new(), Vec::new());
        for p in &polys {
            pl.split(p, &mut cf, &mut cb, &mut f, &mut b);
        }
        self.polys.extend(cf);
        self.polys.extend(cb);
        if !f.is_empty() {
            self.front.get_or_insert_with(Default::default).build(f);
        }
        if !b.is_empty() {
            self.back.get_or_insert_with(Default::default).build(b);
        }
    }
}

fn polys(v: Vec<Vec<Vec3>>) -> Vec<Poly> {
    v.into_iter().filter_map(Poly::new).collect()
}

fn run(a: Vec<Vec<Vec3>>, b: Vec<Vec<Vec3>>, op: Op) -> Vec<Vec<Vec3>> {
    let mut a = Node::new(polys(a));
    let mut b = Node::new(polys(b));
    match op {
        Op::Union => {
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            a.build(b.all());
        }
        Op::Subtract => {
            a.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            a.build(b.all());
            a.invert();
        }
        Op::Intersect => {
            a.invert();
            b.clip_to(&a);
            b.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            a.build(b.all());
            a.invert();
        }
    }
    a.all().into_iter().map(|p| p.v).collect()
}

/// Boolean of two closed solids given as world-space polygons. Runs on a
/// thread with a big stack (the BSP tree recursion can get deep).
pub fn csg(a: Vec<Vec<Vec3>>, b: Vec<Vec<Vec3>>, op: Op) -> Vec<Vec<Vec3>> {
    std::thread::Builder::new().stack_size(256 * 1024 * 1024).spawn(move || run(a, b, op)).ok().and_then(|h| h.join().ok()).unwrap_or_default()
}

/// An object as world-space polygons (a closed solid). Walls with openings
/// and stairs are several boxes, merged into one solid first.
pub fn object_polys(o: &Object, models: &mut ModelCache) -> Vec<Vec<Vec3>> {
    let world = |p: Vec3| o.pos + o.rotation() * (p * o.scale());
    match &o.kind {
        Kind::Brush { .. } | Kind::Mesh { .. } => {
            let parts = crate::knife::components(o).unwrap_or_default();
            let mut acc: Option<Vec<Vec<Vec3>>> = None;
            for (v, f, _) in parts {
                let ps: Vec<Vec<Vec3>> = f.iter().filter(|f| f.len() >= 3).map(|f| f.iter().map(|i| world(v[*i as usize])).collect()).collect();
                acc = Some(match acc {
                    None => ps,
                    Some(a) => csg(a, ps, Op::Union),
                });
            }
            acc.unwrap_or_default()
        }
        Kind::Prop { .. } => {
            let m = local_mesh(o, models);
            m.tris.iter().map(|t| t.iter().map(|i| world(m.pos[*i as usize])).collect()).collect()
        }
        _ => Vec::new(),
    }
}

/// Polygons -> shared vertices + faces, centered (returns the center too).
pub fn to_mesh(polys: &[Vec<Vec3>]) -> Option<(Vec<Vec3>, Vec<Vec<u32>>, Vec3)> {
    let all: Vec<Vec3> = polys.iter().flatten().copied().collect();
    if all.is_empty() {
        return None;
    }
    let (mn, mx) = all.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let c = (mn + mx) * 0.5;
    let key = |p: Vec3| ((p.x * 2000.0).round() as i64, (p.y * 2000.0).round() as i64, (p.z * 2000.0).round() as i64);
    let mut index: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut verts = Vec::new();
    let mut faces = Vec::new();
    for p in polys {
        let mut f: Vec<u32> = Vec::new();
        for v in p {
            let i = *index.entry(key(*v)).or_insert_with(|| {
                verts.push(*v - c);
                (verts.len() - 1) as u32
            });
            if f.last() != Some(&i) && f.first() != Some(&i) || f.is_empty() {
                f.push(i);
            }
        }
        f.dedup();
        if f.len() >= 3 {
            faces.push(f);
        }
    }
    (!faces.is_empty()).then_some((verts, faces, c))
}

impl AnvilApp {
    /// Edit > Boolean: the first selected object is the base, the others are
    /// the tools (they're used up).
    pub(crate) fn boolean_selection(&mut self, op: Op) {
        if self.selection.len() < 2 {
            self.status = "Boolean: select the object to change first, then the shapes to add / cut (Shift+click)".into();
            return;
        }
        let ids = self.selection.clone();
        let Some(base) = self.map.get(ids[0]).cloned() else { return };
        let ok = |o: &Object| matches!(o.kind, Kind::Brush { .. } | Kind::Mesh { .. } | Kind::Prop { .. });
        if !ok(&base) {
            self.status = "Boolean works on brushes, meshes and props".into();
            return;
        }
        let tools: Vec<Object> = ids[1..].iter().filter_map(|id| self.map.get(*id).cloned()).filter(ok).collect();
        if tools.is_empty() {
            return;
        }
        let mut acc = object_polys(&base, &mut self.models);
        for t in &tools {
            let tp = object_polys(t, &mut self.models);
            acc = csg(acc, tp, op);
        }
        self.push_undo();
        let tool_ids: Vec<u64> = tools.iter().map(|t| t.id).collect();
        self.map.remove(&tool_ids);
        match to_mesh(&acc) {
            Some((verts, faces, c)) => {
                if let Some(o) = self.map.get_mut(base.id) {
                    o.kind = Kind::Mesh { verts, faces };
                    o.pos = c;
                    o.rot = Vec3::ZERO;
                    o.face_tex.clear();
                    o.text = None;
                }
                self.selection = vec![base.id];
                self.status = format!("Boolean {}: done ({} tool shape(s) used up). Tab edits the result.", match op {
                    Op::Union => "union",
                    Op::Subtract => "subtract",
                    Op::Intersect => "intersect",
                }, tools.len());
            }
            None => {
                self.map.remove(&[base.id]);
                self.selection.clear();
                self.status = "Boolean: nothing was left".into();
            }
        }
    }
}
