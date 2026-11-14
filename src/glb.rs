//! Minimal binary glTF (.glb) writer. Roblox's Open Cloud doesn't accept
//! .obj, so props are converted to .glb before upload. Textured models get
//! one primitive per material with the texture embedded.

use crate::mesh::TriMesh;
use glam::{Vec2, Vec3};
use serde_json::{json, Value};
use std::collections::BTreeMap;

struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
}

impl Bin {
    fn push(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
        let off = self.data.len();
        self.data.extend_from_slice(bytes);
        let mut v = json!({ "buffer": 0, "byteOffset": off, "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }
}

fn floats(v: impl IntoIterator<Item = f32>) -> Vec<u8> {
    v.into_iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn mesh_to_glb(m: &TriMesh, scale: f32, name: &str) -> Vec<u8> {
    let textured = !m.materials.is_empty() && m.uv.len() == m.pos.len();
    let mut groups: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
    for i in 0..m.tris.len() {
        let key = if textured { m.tri_mat.get(i).copied().unwrap_or(0) } else { 0 };
        groups.entry(key).or_default().push(i);
    }
    let mut bin = Bin { data: Vec::new(), views: Vec::new() };
    let mut accessors = Vec::new();
    let mut primitives = Vec::new();
    let mut images = Vec::new();
    let mut gl_textures = Vec::new();
    let mut materials = Vec::new();
    let mut mat_index: BTreeMap<u16, Option<usize>> = BTreeMap::new();

    for (key, tris) in &groups {
        let (mut pos, mut nrm, mut uvs): (Vec<Vec3>, Vec<Vec3>, Vec<Vec2>) = (Vec::new(), Vec::new(), Vec::new());
        for &ti in tris {
            let t = m.tris[ti];
            let (a, b, c) = (m.pos[t[0] as usize] * scale, m.pos[t[1] as usize] * scale, m.pos[t[2] as usize] * scale);
            let n = (b - a).cross(c - a);
            if n.length_squared() < 1e-14 {
                continue;
            }
            let n = n.normalize();
            pos.extend([a, b, c]);
            nrm.extend([n, n, n]);
            if textured {
                uvs.extend([m.uv[t[0] as usize], m.uv[t[1] as usize], m.uv[t[2] as usize]]);
            }
        }
        if pos.is_empty() {
            continue;
        }
        let (mn, mx) = pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let pv = bin.push(&floats(pos.iter().flat_map(|p| [p.x, p.y, p.z])), Some(34962));
        let nv = bin.push(&floats(nrm.iter().flat_map(|p| [p.x, p.y, p.z])), Some(34962));
        accessors.push(json!({ "bufferView": pv, "componentType": 5126, "count": pos.len(), "type": "VEC3", "min": [mn.x, mn.y, mn.z], "max": [mx.x, mx.y, mx.z] }));
        let pa = accessors.len() - 1;
        accessors.push(json!({ "bufferView": nv, "componentType": 5126, "count": nrm.len(), "type": "VEC3" }));
        let na = accessors.len() - 1;
        let mut attrs = json!({ "POSITION": pa, "NORMAL": na });
        if textured {
            let uv = bin.push(&floats(uvs.iter().flat_map(|p| [p.x, p.y])), Some(34962));
            accessors.push(json!({ "bufferView": uv, "componentType": 5126, "count": uvs.len(), "type": "VEC2" }));
            attrs["TEXCOORD_0"] = json!(accessors.len() - 1);
        }
        let mut prim = json!({ "attributes": attrs, "mode": 4 });
        if textured {
            let mi = *mat_index.entry(*key).or_insert_with(|| {
                let path = m.materials.get(*key as usize)?;
                let bytes = std::fs::read(path).ok()?;
                let mime = if path.to_ascii_lowercase().ends_with(".jpg") || path.to_ascii_lowercase().ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
                let iv = bin.push(&bytes, None);
                images.push(json!({ "bufferView": iv, "mimeType": mime }));
                gl_textures.push(json!({ "source": images.len() - 1 }));
                materials.push(json!({
                    "name": format!("mat{key}"),
                    "pbrMetallicRoughness": { "baseColorTexture": { "index": gl_textures.len() - 1 }, "metallicFactor": 0.0, "roughnessFactor": 1.0 }
                }));
                Some(materials.len() - 1)
            });
            if let Some(mi) = mi {
                prim["material"] = json!(mi);
            }
        }
        primitives.push(prim);
    }
    while bin.data.len() % 4 != 0 {
        bin.data.push(0);
    }
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "Anvil" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0, "name": name }],
        "meshes": [{ "name": name, "primitives": primitives }],
        "accessors": accessors,
        "bufferViews": bin.views,
        "buffers": [{ "byteLength": bin.data.len() }],
    });
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(gl_textures);
        doc["materials"] = json!(materials);
    }
    let mut js = doc.to_string().into_bytes();
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend(0x4654_6C67u32.to_le_bytes()); // "glTF"
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((js.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend(&js);
    out.extend((bin.data.len() as u32).to_le_bytes());
    out.extend(0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend(&bin.data);
    out
}
