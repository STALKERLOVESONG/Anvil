//! glTF 2.0 model import (.glb and .gltf with external or embedded data),
//! e.g. Sketchfab downloads. Meshes from the whole node tree are flattened
//! into one model with their transforms applied; base-color and normal
//! textures are kept (embedded images are saved next to anvil.exe).

use crate::mesh::TriMesh;
use glam::{Mat4, Quat, Vec2, Vec3};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_TRIS: usize = 3_000_000;

struct Gltf {
    json: Value,
    bin: Option<Vec<u8>>,
    dir: PathBuf,
    buffers: Vec<Vec<u8>>,
}

fn data_uri(uri: &str) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix("data:")?;
    let (_, b64) = rest.split_once(";base64,")?;
    crate::team::b64_decode(b64)
}

/// Relative file next to the .gltf (never an absolute or ../ path).
fn safe_join(dir: &Path, uri: &str) -> Option<PathBuf> {
    let uri = uri.replace("%20", " ");
    let p = Path::new(&uri);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_))) || crate::texture::is_network_path(&uri) {
        return None;
    }
    Some(dir.join(p))
}

impl Gltf {
    fn open(path: &str) -> Result<Gltf, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
        let dir = Path::new(path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let (json, bin) = if bytes.len() >= 12 && &bytes[0..4] == b"glTF" {
            let mut off = 12;
            let (mut json, mut bin) = (None, None);
            while off + 8 <= bytes.len() {
                let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
                let kind = &bytes[off + 4..off + 8];
                let body = bytes.get(off + 8..off + 8 + len).ok_or("glb chunk runs past the end")?;
                match kind {
                    b"JSON" => json = Some(serde_json::from_slice::<Value>(body).map_err(|e| format!("Bad glTF JSON: {e}"))?),
                    b"BIN\0" => bin = Some(body.to_vec()),
                    _ => {}
                }
                off += 8 + len;
            }
            (json.ok_or("glb has no JSON")?, bin)
        } else {
            (serde_json::from_slice::<Value>(&bytes).map_err(|e| format!("Bad glTF JSON: {e}"))?, None)
        };
        let mut g = Gltf { json, bin, dir, buffers: Vec::new() };
        let bufs = g.json["buffers"].as_array().cloned().unwrap_or_default();
        for (i, b) in bufs.iter().enumerate() {
            let data = match b["uri"].as_str() {
                Some(u) if u.starts_with("data:") => data_uri(u).ok_or("bad embedded buffer")?,
                Some(u) => std::fs::read(safe_join(&g.dir, u).ok_or("buffer path not allowed")?).map_err(|e| format!("Missing {u}: {e}"))?,
                None if i == 0 => g.bin.clone().ok_or("glb has no binary chunk")?,
                None => Vec::new(),
            };
            g.buffers.push(data);
        }
        Ok(g)
    }

    fn view(&self, i: usize) -> Option<(&[u8], usize)> {
        let v = &self.json["bufferViews"][i];
        let buf = self.buffers.get(v["buffer"].as_u64()? as usize)?;
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = v["byteLength"].as_u64()? as usize;
        Some((buf.get(off..off + len)?, v["byteStride"].as_u64().unwrap_or(0) as usize))
    }

    /// An accessor as f32 rows (`n` components each), normalizing ints.
    fn floats(&self, i: usize) -> Option<Vec<Vec<f32>>> {
        let a = &self.json["accessors"][i];
        let count = a["count"].as_u64()? as usize;
        let n = match a["type"].as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => return None,
        };
        let ct = a["componentType"].as_u64()?;
        let size = match ct {
            5126 | 5125 => 4,
            5123 | 5122 => 2,
            5121 | 5120 => 1,
            _ => return None,
        };
        let norm = a["normalized"].as_bool().unwrap_or(false);
        let Some(bv) = a["bufferView"].as_u64() else { return Some(vec![vec![0.0; n]; count]) };
        let (data, stride) = self.view(bv as usize)?;
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = if stride == 0 { n * size } else { stride };
        let mut out = Vec::with_capacity(count);
        for k in 0..count {
            let base = off + k * stride;
            let mut row = Vec::with_capacity(n);
            for c in 0..n {
                let p = base + c * size;
                let b = data.get(p..p + size)?;
                row.push(match ct {
                    5126 => f32::from_le_bytes(b.try_into().ok()?),
                    5125 => u32::from_le_bytes(b.try_into().ok()?) as f32,
                    5123 => {
                        let v = u16::from_le_bytes(b.try_into().ok()?) as f32;
                        if norm { v / 65535.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes(b.try_into().ok()?) as f32;
                        if norm { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = b[0] as f32;
                        if norm { v / 255.0 } else { v }
                    }
                    _ => {
                        let v = b[0] as i8 as f32;
                        if norm { (v / 127.0).max(-1.0) } else { v }
                    }
                });
            }
            out.push(row);
        }
        Some(out)
    }

    fn indices(&self, i: usize) -> Option<Vec<u32>> {
        let a = &self.json["accessors"][i];
        let count = a["count"].as_u64()? as usize;
        let ct = a["componentType"].as_u64()?;
        let (data, _) = self.view(a["bufferView"].as_u64()? as usize)?;
        let off = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let size = match ct {
            5125 => 4,
            5123 => 2,
            5121 => 1,
            _ => return None,
        };
        (0..count)
            .map(|k| {
                let p = off + k * size;
                let b = data.get(p..p + size)?;
                Some(match size {
                    4 => u32::from_le_bytes(b.try_into().ok()?),
                    2 => u16::from_le_bytes(b.try_into().ok()?) as u32,
                    _ => b[0] as u32,
                })
            })
            .collect()
    }

    /// A texture index -> an image file on disk (embedded ones are saved).
    fn image_file(&self, tex: Option<u64>, model: &str, cache: &Path) -> String {
        let Some(t) = tex else { return String::new() };
        let Some(src) = self.json["textures"][t as usize]["source"].as_u64() else { return String::new() };
        let img = &self.json["images"][src as usize];
        if let Some(u) = img["uri"].as_str() {
            if let Some(bytes) = u.starts_with("data:").then(|| data_uri(u)).flatten() {
                return save_embedded(&bytes, model, src, cache);
            }
            return safe_join(&self.dir, u).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        }
        if let Some((bytes, _)) = img["bufferView"].as_u64().and_then(|bv| self.view(bv as usize)) {
            return save_embedded(bytes, model, src, cache);
        }
        String::new()
    }
}

fn save_embedded(bytes: &[u8], model: &str, idx: u64, cache: &Path) -> String {
    let ext = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => "jpg",
        Ok(image::ImageFormat::Png) => "png",
        _ => return String::new(),
    };
    let h = model.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let _ = std::fs::create_dir_all(cache);
    let p = cache.join(format!("{h:016x}_{idx}.{ext}"));
    if !p.is_file() {
        let _ = std::fs::write(&p, bytes);
    }
    p.to_string_lossy().to_string()
}

pub fn cache_dir() -> PathBuf {
    crate::mount::content_dir().join("gltf_textures")
}

fn node_matrix(n: &Value) -> Mat4 {
    if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
        let f: Vec<f32> = m.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
        return Mat4::from_cols_slice(&f);
    }
    let v3 = |k: &str, d: f32| -> Vec3 { n[k].as_array().filter(|a| a.len() == 3).map(|a| Vec3::new(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32)).unwrap_or(Vec3::splat(d)) };
    let r = n["rotation"].as_array().filter(|a| a.len() == 4).map(|a| Quat::from_xyzw(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32, a[3].as_f64().unwrap_or(1.0) as f32).normalize()).unwrap_or(Quat::IDENTITY);
    Mat4::from_scale_rotation_translation(v3("scale", 1.0), r, v3("translation", 0.0))
}

/// Loads a .glb / .gltf into one triangle mesh.
pub fn load(path: &str) -> Result<TriMesh, String> {
    let g = Gltf::open(path)?;
    let cache = cache_dir();
    let mut m = TriMesh::default();
    // materials -> texture slots
    let mats = g.json["materials"].as_array().cloned().unwrap_or_default();
    for mat in &mats {
        let pbr = &mat["pbrMetallicRoughness"];
        m.materials.push(g.image_file(pbr["baseColorTexture"]["index"].as_u64(), path, &cache));
        m.material_normals.push(g.image_file(mat["normalTexture"]["index"].as_u64(), path, &cache));
        m.material_alpha.push(match mat["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        });
    }
    let no_mat = m.materials.len() as u16;
    m.materials.push(String::new());
    m.material_normals.push(String::new());
    m.material_alpha.push(0);
    // walk the scene
    let scene = g.json["scene"].as_u64().unwrap_or(0) as usize;
    let mut stack: Vec<(usize, Mat4, u32)> = g.json["scenes"][scene]["nodes"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|i| (i as usize, Mat4::IDENTITY, 0)).collect()).unwrap_or_default();
    if stack.is_empty() {
        let n = g.json["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
        stack = (0..n).map(|i| (i, Mat4::IDENTITY, 0)).collect();
    }
    let mut uvs: Vec<Vec2> = Vec::new();
    while let Some((ni, parent, depth)) = stack.pop() {
        if depth > 64 {
            continue;
        }
        let node = &g.json["nodes"][ni];
        let world = parent * node_matrix(node);
        for c in node["children"].as_array().into_iter().flatten().filter_map(|x| x.as_u64()) {
            stack.push((c as usize, world, depth + 1));
        }
        let Some(mi) = node["mesh"].as_u64() else { continue };
        for prim in g.json["meshes"][mi as usize]["primitives"].as_array().into_iter().flatten() {
            if prim["mode"].as_u64().unwrap_or(4) != 4 {
                continue; // only triangle lists
            }
            let Some(pos) = prim["attributes"]["POSITION"].as_u64().and_then(|a| g.floats(a as usize)) else { continue };
            let uv = prim["attributes"]["TEXCOORD_0"].as_u64().and_then(|a| g.floats(a as usize));
            let base = m.pos.len() as u32;
            for p in &pos {
                m.pos.push(world.transform_point3(Vec3::new(p[0], p[1], p[2])));
            }
            match &uv {
                Some(u) if u.len() == pos.len() => uvs.extend(u.iter().map(|t| Vec2::new(t[0], t[1]))),
                _ => uvs.extend(std::iter::repeat_n(Vec2::ZERO, pos.len())),
            }
            let idx: Vec<u32> = match prim["indices"].as_u64() {
                Some(a) => g.indices(a as usize).ok_or("bad index data")?,
                None => (0..pos.len() as u32).collect(),
            };
            let flip = world.determinant() < 0.0;
            let mat = prim["material"].as_u64().map(|x| x as u16).unwrap_or(no_mat);
            for t in idx.chunks_exact(3) {
                if t.iter().any(|i| *i as usize >= pos.len()) {
                    continue;
                }
                let tri = if flip { [base + t[0], base + t[2], base + t[1]] } else { [base + t[0], base + t[1], base + t[2]] };
                m.tris.push(tri);
                m.tri_mat.push(mat);
            }
            if m.tris.len() > MAX_TRIS {
                return Err("This model has too many triangles (over 3 million)".into());
            }
        }
    }
    if m.tris.is_empty() {
        return Err("No triangle meshes in this glTF".into());
    }
    // glTF uv origin is top-left like images; Anvil's sampler is the same (v down)
    // parts without UVs just get no texture (their slot has no image)
    if m.materials.iter().any(|t| !t.is_empty()) {
        m.uv = uvs;
    } else {
        m.materials.clear();
        m.material_normals.clear();
        m.tri_mat.clear();
    }
    Ok(m)
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// An image as a png / jpg file, shrunk to fit `max_tex` (if given).
/// Other formats are converted; anything that can't be read is dropped.
fn portable_image(bytes: &[u8], max_tex: Option<u32>) -> Option<(Vec<u8>, &'static str)> {
    let fmt = image::guess_format(bytes).ok()?;
    let ext = match fmt {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        _ => "",
    };
    if !ext.is_empty() && max_tex.is_none() {
        return Some((bytes.to_vec(), ext));
    }
    let mut r = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    r.limits(crate::texture::image_limits());
    let img = r.decode().ok()?;
    let m = max_tex.unwrap_or(16384);
    if !ext.is_empty() && img.width() <= m && img.height() <= m {
        return Some((bytes.to_vec(), ext));
    }
    let img = if img.width() > m || img.height() > m { img.resize(m, m, image::imageops::FilterType::Triangle) } else { img };
    let mut out = Vec::new();
    if fmt != image::ImageFormat::Jpeg && img.color().has_alpha() {
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
        Some((out, "png"))
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img.to_rgb8()).ok()?;
        Some((out, "jpg"))
    }
}

/// A .glb / .gltf made portable, to send to Team Create friends or pack:
/// one .gltf (JSON) plus a single .bin and its textures as separate png / jpg
/// files, all with flat names (no folders). Textures bigger than `max_tex`
/// are shrunk. Every outside file it reads must pass `allow`.
/// Returns (gltf JSON, [(file name, contents)]).
pub fn bundle(path: &str, allow: &dyn Fn(&Path) -> bool, max_tex: Option<u32>) -> Result<(Vec<u8>, Vec<(String, Vec<u8>)>), String> {
    let g = Gltf::open(path)?;
    for b in g.json["buffers"].as_array().into_iter().flatten() {
        if let Some(u) = b["uri"].as_str().filter(|u| !u.starts_with("data:")) {
            if !safe_join(&g.dir, u).is_some_and(|p| allow(&p)) {
                return Err(format!("{u} can't be shared"));
            }
        }
    }
    let tag = format!("{:08x}", fnv(path) as u32);
    let mut json = g.json.clone();
    let mut files = Vec::new();
    let mut image_views = std::collections::HashSet::new();
    let images = g.json["images"].as_array().cloned().unwrap_or_default();
    for (i, img) in images.iter().enumerate() {
        let bytes = if let Some(u) = img["uri"].as_str() {
            if u.starts_with("data:") {
                data_uri(u)
            } else {
                safe_join(&g.dir, u).filter(|p| allow(p)).and_then(|p| std::fs::read(p).ok())
            }
        } else if let Some(bv) = img["bufferView"].as_u64() {
            image_views.insert(bv as usize);
            g.view(bv as usize).map(|(b, _)| b.to_vec())
        } else {
            None
        };
        json["images"][i] = match bytes.and_then(|b| portable_image(&b, max_tex)) {
            Some((b, ext)) => {
                let name = format!("{tag}_tex{i}.{ext}");
                files.push((name.clone(), b));
                serde_json::json!({ "uri": name })
            }
            None => serde_json::json!({}),
        };
    }
    // one new buffer with every view the mesh data uses (embedded images now
    // live in their own files, so their bytes are left out)
    let mut used = std::collections::HashSet::new();
    for a in g.json["accessors"].as_array().into_iter().flatten() {
        for v in [&a["bufferView"], &a["sparse"]["indices"]["bufferView"], &a["sparse"]["values"]["bufferView"]] {
            if let Some(v) = v.as_u64() {
                used.insert(v as usize);
            }
        }
    }
    let mut bin: Vec<u8> = vec![0];
    let views = g.json["bufferViews"].as_array().cloned().unwrap_or_default();
    for i in 0..views.len() {
        let keep = used.contains(&i) || !image_views.contains(&i);
        match g.view(i).filter(|_| keep) {
            Some((data, _)) => {
                while bin.len() % 4 != 0 {
                    bin.push(0);
                }
                json["bufferViews"][i]["buffer"] = 0.into();
                json["bufferViews"][i]["byteOffset"] = bin.len().into();
                bin.extend_from_slice(data);
            }
            None => json["bufferViews"][i] = serde_json::json!({ "buffer": 0, "byteOffset": 0, "byteLength": 1 }),
        }
    }
    let bin_name = format!("{tag}_mesh.bin");
    json["buffers"] = serde_json::json!([{ "uri": bin_name, "byteLength": bin.len() }]);
    files.push((bin_name, bin));
    Ok((serde_json::to_vec(&json).map_err(|e| e.to_string())?, files))
}

/// Every file a .gltf points at (buffer and image `uri`s that aren't data:).
pub fn referenced_files(json: &Value) -> Vec<String> {
    ["buffers", "images"]
        .iter()
        .flat_map(|k| json[*k].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v["uri"].as_str().map(|s| s.to_string()))
        .filter(|u| !u.starts_with("data:"))
        .collect()
}
