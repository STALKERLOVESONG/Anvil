//! Edges of a polygon mesh, and the loops Alt+click selects in mesh edit
//! mode: an edge loop (straight on through four-way vertices, or around a
//! hole's border), and a face ring (the strip of quads across an edge).

use std::collections::HashMap;

/// An edge with its smaller vertex first.
pub fn key(a: u32, b: u32) -> [u32; 2] {
    if a < b { [a, b] } else { [b, a] }
}

/// Every edge of the faces, once.
pub fn unique_edges(faces: &[Vec<u32>]) -> Vec<[u32; 2]> {
    let mut out: Vec<[u32; 2]> = faces.iter().flat_map(|f| (0..f.len()).map(move |i| key(f[i], f[(i + 1) % f.len()]))).filter(|e| e[0] != e[1]).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The faces each edge belongs to.
pub fn edge_faces(faces: &[Vec<u32>]) -> HashMap<[u32; 2], Vec<usize>> {
    let mut m: HashMap<[u32; 2], Vec<usize>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            let e = key(f[i], f[(i + 1) % f.len()]);
            if e[0] != e[1] {
                m.entry(e).or_default().push(fi);
            }
        }
    }
    m
}

fn vert_edges(edges: &HashMap<[u32; 2], Vec<usize>>) -> HashMap<u32, Vec<[u32; 2]>> {
    let mut m: HashMap<u32, Vec<[u32; 2]>> = HashMap::new();
    for e in edges.keys() {
        m.entry(e[0]).or_default().push(*e);
        m.entry(e[1]).or_default().push(*e);
    }
    m
}

/// The edge loop through `start`: at each vertex the loop goes straight on
/// (a vertex with four edges: the one edge sharing no face with the way it
/// came), or, along a border, to the next border edge. It stops where
/// neither works (a corner of three or five edges) or where it closes.
pub fn edge_loop(faces: &[Vec<u32>], start: [u32; 2]) -> Vec<[u32; 2]> {
    let start = key(start[0], start[1]);
    let ef = edge_faces(faces);
    if !ef.contains_key(&start) {
        return Vec::new();
    }
    let ve = vert_edges(&ef);
    let mut out = vec![start];
    for (from, at) in [(start[0], start[1]), (start[1], start[0])] {
        let (mut cur, mut at, mut _from) = (start, at, from);
        loop {
            let here = ve.get(&at).cloned().unwrap_or_default();
            let cur_faces = ef.get(&cur).cloned().unwrap_or_default();
            let next = if cur_faces.len() == 2 && here.len() == 4 {
                // straight on: the edge at `at` in neither of cur's faces
                let others: Vec<[u32; 2]> = here.iter().copied().filter(|e| *e != cur && !ef.get(e).is_some_and(|fs| fs.iter().any(|f| cur_faces.contains(f)))).collect();
                (others.len() == 1).then(|| others[0])
            } else if cur_faces.len() == 1 {
                // along the border
                let border: Vec<[u32; 2]> = here.iter().copied().filter(|e| *e != cur && ef.get(e).is_some_and(|fs| fs.len() == 1)).collect();
                (border.len() == 1).then(|| border[0])
            } else {
                None
            };
            let Some(n) = next else { break };
            if out.contains(&n) {
                break; // closed
            }
            out.push(n);
            _from = at;
            at = if n[0] == at { n[1] } else { n[0] };
            cur = n;
        }
    }
    out
}

/// The faces across `start` and on along the strip of quads: from each quad,
/// across its edge opposite the one it was entered by. Stops at a face that
/// isn't a quad, at the border, or where it closes.
pub fn face_ring(faces: &[Vec<u32>], start: [u32; 2]) -> Vec<usize> {
    let start = key(start[0], start[1]);
    let ef = edge_faces(faces);
    let Some(first) = ef.get(&start).cloned() else { return Vec::new() };
    let mut out: Vec<usize> = Vec::new();
    for f0 in first {
        let (mut f, mut entered) = (f0, start);
        loop {
            if out.contains(&f) {
                break;
            }
            out.push(f);
            let face = &faces[f];
            if face.len() != 4 {
                break;
            }
            // the edge sharing no vertex with the one we came in by
            let Some(opp) = (0..4).map(|i| key(face[i], face[(i + 1) % 4])).find(|e| !e.contains(&entered[0]) && !e.contains(&entered[1])) else { break };
            let Some(next) = ef.get(&opp).and_then(|fs| fs.iter().copied().find(|x| *x != f)) else { break };
            entered = opp;
            f = next;
        }
    }
    out
}

/// The vertices of some edges, once.
pub fn edge_verts(edges: &[[u32; 2]]) -> Vec<u32> {
    let mut v: Vec<u32> = edges.iter().flatten().copied().collect();
    v.sort_unstable();
    v.dedup();
    v
}
