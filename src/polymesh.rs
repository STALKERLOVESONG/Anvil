//! Editable polygon meshes: triangulation, primitives, extrusion and the
//! vertex / face operations used by mesh edit mode.

use crate::mesh::TriMesh;
use glam::Vec3;
use std::collections::{HashMap, HashSet};

/// Polygon normal (Newell's method), not normalized.
pub fn newell(verts: &[Vec3], face: &[u32]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..face.len() {
        let a = verts[face[i] as usize];
        let b = verts[face[(i + 1) % face.len()] as usize];
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    n
}

pub fn face_center(verts: &[Vec3], face: &[u32]) -> Vec3 {
    face.iter().map(|&i| verts[i as usize]).sum::<Vec3>() / face.len().max(1) as f32
}

/// Ear-clipping triangulation (handles concave footprints like L-shaped rooms).
pub fn triangulate(verts: &[Vec3], face: &[u32]) -> Vec<[u32; 3]> {
    let n = face.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[face[0], face[1], face[2]]];
    }
    let fan = || (1..n - 1).map(|i| [face[0], face[i], face[i + 1]]).collect::<Vec<_>>();
    let normal = newell(verts, face);
    if normal.length_squared() < 1e-12 {
        return fan();
    }
    let ax = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
        0
    } else if normal.y.abs() >= normal.z.abs() {
        1
    } else {
        2
    };
    let (u, v) = match ax {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let s = normal[ax].signum();
    let p: Vec<(f32, f32)> = face.iter().map(|&i| (verts[i as usize][u], verts[i as usize][v])).collect();
    let cross = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let inside = |pt: (f32, f32), a, b, c| {
        let d1 = cross(a, b, pt) * s;
        let d2 = cross(b, c, pt) * s;
        let d3 = cross(c, a, pt) * s;
        d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
    };
    let mut rem: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while rem.len() > 3 && guard < n * n {
        guard += 1;
        let m = rem.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (rem[(i + m - 1) % m], rem[i], rem[(i + 1) % m]);
            let (a, b, c) = (p[ia], p[ib], p[ic]);
            if cross(a, b, c) * s <= 1e-9 {
                continue; // reflex or degenerate corner
            }
            let blocked = rem.iter().any(|&j| j != ia && j != ib && j != ic && inside(p[j], a, b, c));
            if blocked {
                continue;
            }
            out.push([face[ia], face[ib], face[ic]]);
            rem.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if rem.len() == 3 {
        out.push([face[rem[0]], face[rem[1]], face[rem[2]]]);
    } else if rem.len() > 3 {
        // self-intersecting polygon: fan the remainder rather than dropping it
        for i in 1..rem.len() - 1 {
            out.push([face[rem[0]], face[rem[i]], face[rem[i + 1]]]);
        }
    }
    out
}

/// Renderable triangle mesh with face ids per triangle and polygon outline edges.
pub fn poly_trimesh(verts: &[Vec3], faces: &[Vec<u32>]) -> TriMesh {
    let mut m = TriMesh { pos: verts.to_vec(), ..Default::default() };
    let mut seen = HashSet::new();
    for (fi, f) in faces.iter().enumerate() {
        if f.iter().any(|&i| i as usize >= verts.len()) {
            continue;
        }
        for t in triangulate(verts, f) {
            m.tris.push(t);
            m.tri_face.push(fi as u32);
        }
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            if seen.insert(if a < b { (a, b) } else { (b, a) }) {
                m.edges.push([a, b]);
            }
        }
    }
    m
}

/// Box as 8 shared vertices and 6 quads.
pub fn box_poly(size: Vec3) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let h = size * 0.5;
    let verts = (0..8)
        .map(|i| Vec3::new(if i & 1 != 0 { h.x } else { -h.x }, if i & 2 != 0 { h.y } else { -h.y }, if i & 4 != 0 { h.z } else { -h.z }))
        .collect();
    let faces = vec![
        vec![5, 1, 3, 7],
        vec![0, 4, 6, 2],
        vec![6, 7, 3, 2],
        vec![0, 1, 5, 4],
        vec![4, 5, 7, 6],
        vec![1, 0, 2, 3],
    ];
    (verts, faces)
}

/// Extruded footprint ("prism"). Points are on the XZ plane; returns local
/// vertices centered on the footprint centroid, plus that center (world).
pub fn prism(points: &[Vec3], base_y: f32, height: f32) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec3) {
    let n = points.len();
    let c2 = points.iter().copied().sum::<Vec3>() / n as f32;
    let center = Vec3::new(c2.x, base_y + height * 0.5, c2.z);
    let mut pts: Vec<Vec3> = points.iter().map(|p| Vec3::new(p.x - center.x, 0.0, p.z - center.z)).collect();
    // make the top face CCW seen from above (+Y normal)
    let idx: Vec<u32> = (0..n as u32).collect();
    if newell(&pts, &idx).y < 0.0 {
        pts.reverse();
    }
    let h = height * 0.5;
    let mut verts = Vec::with_capacity(n * 2);
    for p in &pts {
        verts.push(Vec3::new(p.x, -h, p.z));
    }
    for p in &pts {
        verts.push(Vec3::new(p.x, h, p.z));
    }
    let n32 = n as u32;
    let mut faces = Vec::new();
    faces.push((0..n32).map(|i| n32 + i).collect()); // top
    faces.push((0..n32).rev().collect()); // bottom
    for i in 0..n32 {
        let j = (i + 1) % n32;
        faces.push(vec![i, j, n32 + j, n32 + i]);
    }
    (verts, faces, center)
}

/// Welds a triangle soup into shared vertices and merges coplanar triangle
/// pairs back into quads (for converting brushes into editable meshes).
pub fn weld(m: &TriMesh) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let key = |p: Vec3| ((p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64, (p.z * 1000.0).round() as i64);
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut verts = Vec::new();
    let mut remap = Vec::with_capacity(m.pos.len());
    for p in &m.pos {
        let k = key(*p);
        let id = *map.entry(k).or_insert_with(|| {
            verts.push(*p);
            (verts.len() - 1) as u32
        });
        remap.push(id);
    }
    let tris: Vec<[u32; 3]> = m
        .tris
        .iter()
        .map(|t| [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]])
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    let normal = |t: &[u32; 3]| {
        (verts[t[1] as usize] - verts[t[0] as usize]).cross(verts[t[2] as usize] - verts[t[0] as usize]).normalize_or_zero()
    };
    // directed edge -> triangle
    let mut edge_tri: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for k in 0..3 {
            edge_tri.insert((t[k], t[(k + 1) % 3]), i);
        }
    }
    let mut merged = vec![false; tris.len()];
    let mut faces = Vec::new();
    for (i, t) in tris.iter().enumerate() {
        if merged[i] {
            continue;
        }
        merged[i] = true;
        let n = normal(t);
        let mut done = false;
        for k in 0..3 {
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            if let Some(&j) = edge_tri.get(&(b, a)) {
                if merged[j] || normal(&tris[j]).dot(n) < 0.9999 {
                    continue;
                }
                let tj = tris[j];
                let d = *tj.iter().find(|&&x| x != a && x != b).unwrap_or(&a);
                let quad = vec![a, d, b, c];
                // keep only convex quads
                let ok = (0..4).all(|q| {
                    let p0 = verts[quad[q] as usize];
                    let p1 = verts[quad[(q + 1) % 4] as usize];
                    let p2 = verts[quad[(q + 2) % 4] as usize];
                    (p1 - p0).cross(p2 - p1).dot(n) > -1e-6
                });
                if ok {
                    merged[j] = true;
                    faces.push(quad);
                    done = true;
                    break;
                }
            }
        }
        if !done {
            faces.push(t.to_vec());
        }
    }
    (verts, faces)
}

/// Region extrude: moves the selected faces along their normals and stitches
/// side walls along the region's boundary.
pub fn extrude(verts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[usize], dist: f32) {
    let sel: Vec<usize> = sel.iter().copied().filter(|&f| f < faces.len()).collect();
    if sel.is_empty() {
        return;
    }
    let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
    let mut vnorm: HashMap<u32, Vec3> = HashMap::new();
    for &f in &sel {
        let face = &faces[f];
        let n = newell(verts, face).normalize_or_zero();
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            *edge_count.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
            *vnorm.entry(a).or_default() += n;
        }
    }
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut keys: Vec<u32> = vnorm.keys().copied().collect();
    keys.sort();
    for v in keys {
        let n = vnorm[&v].normalize_or_zero();
        verts.push(verts[v as usize] + n * dist);
        map.insert(v, (verts.len() - 1) as u32);
    }
    for &f in &sel {
        let face = faces[f].clone();
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            if edge_count[&if a < b { (a, b) } else { (b, a) }] == 1 {
                faces.push(vec![a, b, map[&b], map[&a]]);
            }
        }
        faces[f] = face.iter().map(|v| map[v]).collect();
    }
}

pub fn flip(faces: &mut [Vec<u32>], sel: &[usize]) {
    for &f in sel {
        if let Some(face) = faces.get_mut(f) {
            face.reverse();
        }
    }
}

pub fn delete_faces(faces: &mut Vec<Vec<u32>>, sel: &[usize]) {
    let mut s: Vec<usize> = sel.to_vec();
    s.sort_unstable();
    s.dedup();
    for f in s.into_iter().rev() {
        if f < faces.len() {
            faces.remove(f);
        }
    }
}

/// Removes unused vertices and reindexes faces.
pub fn cleanup(verts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>) {
    let mut used = vec![false; verts.len()];
    for f in faces.iter() {
        for &v in f {
            if let Some(u) = used.get_mut(v as usize) {
                *u = true;
            }
        }
    }
    let mut remap = vec![u32::MAX; verts.len()];
    let mut nv = Vec::new();
    for (i, v) in verts.iter().enumerate() {
        if used[i] {
            remap[i] = nv.len() as u32;
            nv.push(*v);
        }
    }
    for f in faces.iter_mut() {
        for v in f.iter_mut() {
            *v = remap[*v as usize];
        }
    }
    faces.retain(|f| f.len() >= 3 && f.iter().all(|&v| v != u32::MAX));
    *verts = nv;
}

pub fn delete_verts(verts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[u32]) {
    faces.retain(|f| !f.iter().any(|v| sel.contains(v)));
    cleanup(verts, faces);
}

/// Collapses the selected vertices into their centroid.
pub fn merge_verts(verts: &mut Vec<Vec3>, faces: &mut Vec<Vec<u32>>, sel: &[u32]) {
    if sel.len() < 2 {
        return;
    }
    let c = sel.iter().map(|&i| verts[i as usize]).sum::<Vec3>() / sel.len() as f32;
    let keep = sel[0];
    verts[keep as usize] = c;
    for f in faces.iter_mut() {
        for v in f.iter_mut() {
            if sel.contains(v) {
                *v = keep;
            }
        }
        f.dedup();
        while f.len() > 1 && f.first() == f.last() {
            f.pop();
        }
    }
    faces.retain(|f| f.len() >= 3);
    cleanup(verts, faces);
}

/// One side of a plane cut: vertices, faces, and for every face the index
/// of the original face it came from (`usize::MAX` for the new cap faces).
pub struct Piece {
    pub verts: Vec<Vec3>,
    pub faces: Vec<Vec<u32>>,
    pub src: Vec<usize>,
}

/// Cuts a mesh with the plane through `p` with normal `n`.
/// * `split = true`: returns the two halves (front = +n side), each closed
///   with a cap face where it was cut.
/// * `split = false`: "slice" — one mesh whose faces are divided along the
///   cut (returned as `front`), nothing removed.
pub fn plane_cut(verts: &[Vec3], faces: &[Vec<u32>], p: Vec3, n: Vec3, split: bool) -> (Option<Piece>, Option<Piece>) {
    let n = n.normalize_or_zero();
    let eps = 1e-5;
    let d: Vec<f32> = verts.iter().map(|v| {
        let x = (*v - p).dot(n);
        if x.abs() < eps { 0.0 } else { x }
    }).collect();
    let mut out = verts.to_vec();
    let mut dist = d.clone();
    let mut cuts: HashMap<(u32, u32), u32> = HashMap::new();
    let mut cut = |a: u32, b: u32, out: &mut Vec<Vec3>, dist: &mut Vec<f32>| -> u32 {
        let key = if a < b { (a, b) } else { (b, a) };
        *cuts.entry(key).or_insert_with(|| {
            let (da, db) = (d[a as usize], d[b as usize]);
            let t = da / (da - db);
            out.push(verts[a as usize] + (verts[b as usize] - verts[a as usize]) * t);
            dist.push(0.0);
            (out.len() - 1) as u32
        })
    };
    let (mut front, mut back): (Vec<(Vec<u32>, usize)>, Vec<(Vec<u32>, usize)>) = (Vec::new(), Vec::new());
    let mut segs: Vec<(u32, u32)> = Vec::new(); // cap edges for the front piece
    for (fi, f) in faces.iter().enumerate() {
        if f.len() < 3 || f.iter().any(|&i| i as usize >= verts.len()) {
            continue;
        }
        let pos = f.iter().any(|&i| d[i as usize] > 0.0);
        let neg = f.iter().any(|&i| d[i as usize] < 0.0);
        if !neg && !pos {
            // lies on the plane: belongs to the side it faces away from
            if newell(verts, f).dot(n) < 0.0 {
                front.push((f.clone(), fi));
            } else {
                back.push((f.clone(), fi));
            }
            continue;
        }
        if !neg {
            // an existing edge lying on the plane is part of the cap outline
            for k in 0..f.len() {
                let (u, v) = (f[k], f[(k + 1) % f.len()]);
                if d[u as usize] == 0.0 && d[v as usize] == 0.0 {
                    segs.push((v, u));
                }
            }
            front.push((f.clone(), fi));
            continue;
        }
        if !pos {
            back.push((f.clone(), fi));
            continue;
        }
        let (mut fp, mut bp) = (Vec::new(), Vec::new());
        for k in 0..f.len() {
            let (a, b) = (f[k], f[(k + 1) % f.len()]);
            let (da, db) = (d[a as usize], d[b as usize]);
            if da >= 0.0 {
                fp.push(a);
            }
            if da <= 0.0 {
                bp.push(a);
            }
            if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                let m = cut(a, b, &mut out, &mut dist);
                fp.push(m);
                bp.push(m);
            }
        }
        // the chord on the plane: consecutive on-plane vertices in the front polygon
        for k in 0..fp.len() {
            let (u, v) = (fp[k], fp[(k + 1) % fp.len()]);
            if dist[u as usize] == 0.0 && dist[v as usize] == 0.0 && u != v {
                segs.push((v, u));
                break;
            }
        }
        if fp.len() >= 3 {
            front.push((fp, fi));
        }
        if bp.len() >= 3 {
            back.push((bp, fi));
        }
    }
    let mk = |list: Vec<(Vec<u32>, usize)>, caps: Vec<Vec<u32>>, out: &Vec<Vec3>| -> Option<Piece> {
        if list.is_empty() {
            return None;
        }
        let mut faces: Vec<Vec<u32>> = list.iter().map(|x| x.0.clone()).collect();
        let mut src: Vec<usize> = list.iter().map(|x| x.1).collect();
        for c in caps {
            faces.push(c);
            src.push(usize::MAX);
        }
        let mut v = out.clone();
        cleanup(&mut v, &mut faces);
        Some(Piece { verts: v, faces, src })
    };
    if !split {
        let mut all = front;
        all.extend(back);
        return (mk(all, Vec::new(), &out), None);
    }
    // chain cap edges into loops
    let mut next: HashMap<u32, u32> = segs.iter().copied().collect();
    let mut caps = Vec::new();
    loop {
        let Some(&start) = next.keys().next() else { break };
        let mut lp = vec![start];
        let mut cur = start;
        let mut guard = 0;
        while let Some(nx) = next.remove(&cur) {
            if nx == start {
                break;
            }
            lp.push(nx);
            cur = nx;
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        if lp.len() >= 3 {
            caps.push(lp);
        }
    }
    let back_caps: Vec<Vec<u32>> = caps.iter().map(|c| c.iter().rev().copied().collect()).collect();
    (mk(front, caps, &out), mk(back, back_caps, &out))
}
