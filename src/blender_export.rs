//! One-click Blender export: the whole scene as a textured .glb (one object
//! per Anvil object, folders kept), and, when Blender is installed, a real
//! .blend made by running Blender in the background (folders become
//! collections).

use crate::map::{Kind, Map, Object};
use crate::mesh::{world_mesh, ModelCache, NO_TEX};
use glam::{Vec2, Vec3};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

fn lin(c: u8) -> f32 {
    (c as f32 / 255.0).powf(2.2)
}

#[derive(Default)]
struct Doc {
    bin: Option<Bin>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    mat_keys: HashMap<String, usize>,
    images: Vec<Value>,
    textures: Vec<Value>,
    image_keys: HashMap<String, Option<usize>>,
    lights: Vec<Value>,
    /// Write every picture as PNG (for readers without JPEG).
    png_only: bool,
}

impl Doc {
    fn bin(&mut self) -> &mut Bin {
        self.bin.get_or_insert(Bin { data: Vec::new(), views: Vec::new() })
    }

    fn texture(&mut self, path: &str) -> Option<usize> {
        if let Some(t) = self.image_keys.get(path) {
            return *t;
        }
        let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let t = std::fs::read(path).ok().and_then(|bytes| {
            // glTF only takes PNG / JPEG: convert anything else
            let (bytes, mime) = match ext.as_str() {
                "png" => (bytes, "image/png"),
                "jpg" | "jpeg" if !self.png_only => (bytes, "image/jpeg"),
                _ => {
                    let img = crate::texture::open_image(path)?;
                    let mut out = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
                    (out.into_inner(), "image/png")
                }
            };
            let iv = self.bin().push(&bytes, None);
            self.images.push(json!({ "bufferView": iv, "mimeType": mime }));
            self.textures.push(json!({ "source": self.images.len() - 1 }));
            Some(self.textures.len() - 1)
        });
        self.image_keys.insert(path.to_string(), t);
        t
    }

    fn material(&mut self, color: [u8; 3], alpha: f32, image: Option<&str>, metallic: bool) -> usize {
        let key = format!("{color:?}|{alpha:.3}|{image:?}|{metallic}");
        if let Some(i) = self.mat_keys.get(&key) {
            return *i;
        }
        let tex = image.and_then(|p| self.texture(p));
        let mut pbr = json!({ "baseColorFactor": [lin(color[0]), lin(color[1]), lin(color[2]), alpha], "metallicFactor": if metallic { 0.9 } else { 0.0 }, "roughnessFactor": if metallic { 0.35 } else { 0.85 } });
        if let Some(t) = tex {
            pbr["baseColorTexture"] = json!({ "index": t });
        }
        let name = match image {
            Some(p) => format!("{}_{:02x}{:02x}{:02x}", Path::new(p).file_stem().and_then(|s| s.to_str()).unwrap_or("tex"), color[0], color[1], color[2]),
            None => format!("Anvil_{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
        };
        let mut m = json!({ "name": name, "pbrMetallicRoughness": pbr, "doubleSided": false });
        if alpha < 0.999 {
            m["alphaMode"] = json!("BLEND");
        }
        self.materials.push(m);
        self.mat_keys.insert(key, self.materials.len() - 1);
        self.materials.len() - 1
    }

    /// The object's mesh in its own space, one primitive per texture slot.
    fn mesh(&mut self, o: &Object, map: &Map, models: &mut ModelCache, k: f32) -> Option<usize> {
        let wm = world_mesh(o, &map.textures, models);
        if wm.mesh.tris.is_empty() {
            return None;
        }
        let inv = o.rotation().inverse();
        let sc = o.scale();
        let sc = Vec3::new(if sc.x.abs() > 1e-6 { sc.x } else { 1.0 }, if sc.y.abs() > 1e-6 { sc.y } else { 1.0 }, if sc.z.abs() > 1e-6 { sc.z } else { 1.0 });
        let local = |p: Vec3| (inv * (p - o.pos)) / sc * k;
        let mut groups: Vec<(u16, Vec<usize>)> = Vec::new();
        for ti in 0..wm.mesh.tris.len() {
            let slot = wm.tri_tex.get(ti).copied().unwrap_or(NO_TEX);
            match groups.iter_mut().find(|g| g.0 == slot) {
                Some(g) => g.1.push(ti),
                None => groups.push((slot, vec![ti])),
            }
        }
        let decal = matches!(o.kind, Kind::Decal { .. });
        let mut prims = Vec::new();
        for (slot, tris) in groups {
            let (mut pos, mut nrm, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
            let tex = (slot != NO_TEX).then(|| wm.texs.get(slot as usize)).flatten();
            for &ti in &tris {
                let t = wm.mesh.tris[ti];
                let (a, b, c) = (local(wm.pos[t[0] as usize]), local(wm.pos[t[1] as usize]), local(wm.pos[t[2] as usize]));
                let n = (b - a).cross(c - a);
                if n.length_squared() < 1e-14 {
                    continue;
                }
                let n = n.normalize();
                pos.extend([a, b, c]);
                nrm.extend([n, n, n]);
                if tex.is_some() {
                    let uv = wm.tri_uv.get(ti).copied().unwrap_or([Vec2::ZERO; 3]);
                    uvs.extend(uv);
                }
            }
            if pos.is_empty() {
                continue;
            }
            let (mn, mx) = pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
            let pv = self.bin().push(&floats(pos.iter().flat_map(|p| [p.x, p.y, p.z])), Some(34962));
            let nv = self.bin().push(&floats(nrm.iter().flat_map(|p| [p.x, p.y, p.z])), Some(34962));
            self.accessors.push(json!({ "bufferView": pv, "componentType": 5126, "count": pos.len(), "type": "VEC3", "min": [mn.x, mn.y, mn.z], "max": [mx.x, mx.y, mx.z] }));
            let pa = self.accessors.len() - 1;
            self.accessors.push(json!({ "bufferView": nv, "componentType": 5126, "count": nrm.len(), "type": "VEC3" }));
            let mut attrs = json!({ "POSITION": pa, "NORMAL": self.accessors.len() - 1 });
            if !uvs.is_empty() {
                let uv = self.bin().push(&floats(uvs.iter().flat_map(|p| [p.x, p.y])), Some(34962));
                self.accessors.push(json!({ "bufferView": uv, "componentType": 5126, "count": uvs.len(), "type": "VEC2" }));
                attrs["TEXCOORD_0"] = json!(self.accessors.len() - 1);
            }
            let alpha = wm.alpha;
            let mat = match tex {
                Some(t) => {
                    let mi = self.material(t.tint, alpha, Some(&t.image), t.metallic);
                    if decal {
                        self.materials[mi]["alphaMode"] = json!("BLEND");
                    }
                    mi
                }
                None => self.material(o.color, alpha, None, false),
            };
            prims.push(json!({ "attributes": attrs, "mode": 4, "material": mat }));
        }
        if prims.is_empty() {
            return None;
        }
        self.meshes.push(json!({ "name": o.name, "primitives": prims }));
        Some(self.meshes.len() - 1)
    }
}

fn folder_path(map: &Map, mut cur: Option<u64>) -> String {
    let mut parts = Vec::new();
    while let Some(f) = cur.and_then(|id| map.folder(id)) {
        parts.push(f.name.clone());
        cur = f.parent;
        if parts.len() > 32 {
            break;
        }
    }
    parts.reverse();
    parts.join("/")
}

/// The whole map as a binary glTF. `k` = meters per stud (unit scale).
pub fn scene_glb(map: &Map, models: &mut ModelCache, k: f32) -> Vec<u8> {
    scene_glb_with(map, models, k, false)
}

/// `scene_glb`, optionally with every picture as PNG.
pub fn scene_glb_with(map: &Map, models: &mut ModelCache, k: f32, png_only: bool) -> Vec<u8> {
    let mut d = Doc { png_only, ..Default::default() };
    let mut nodes: Vec<Value> = Vec::new();
    let mut children_of: HashMap<Option<u64>, Vec<usize>> = HashMap::new();
    for o in map.objects.iter().filter(|o| !o.hidden) {
        let q = o.rotation();
        let mut node = json!({
            "name": o.name,
            "translation": [o.pos.x * k, o.pos.y * k, o.pos.z * k],
            "rotation": [q.x, q.y, q.z, q.w],
            "extras": { "anvil_id": o.id, "anvil_kind": o.type_label(), "anvil_folder": folder_path(map, o.folder) }
        });
        let sc = o.scale();
        if sc != Vec3::ONE {
            node["scale"] = json!([sc.x, sc.y, sc.z]);
        }
        match &o.kind {
            Kind::Light { range, brightness } => {
                d.lights.push(json!({ "type": "point", "color": [lin(o.color[0]), lin(o.color[1]), lin(o.color[2])], "intensity": brightness * 40.0, "range": range * k }));
                node["extensions"] = json!({ "KHR_lights_punctual": { "light": d.lights.len() - 1 } });
            }
            Kind::Spawn { .. } | Kind::Prompt { .. } | Kind::Target {} => {} // empties
            _ => {
                if let Some(m) = d.mesh(o, map, models, k) {
                    node["mesh"] = json!(m);
                }
            }
        }
        nodes.push(node);
        children_of.entry(o.folder.filter(|f| map.folder(*f).is_some())).or_default().push(nodes.len() - 1);
    }
    // folders: plain parent nodes with no transform (so removing them later moves nothing)
    let mut folder_node: HashMap<u64, usize> = HashMap::new();
    for f in &map.folders {
        nodes.push(json!({ "name": f.name, "extras": { "anvil_is_folder": 1, "anvil_folder": folder_path(map, Some(f.id)) } }));
        folder_node.insert(f.id, nodes.len() - 1);
    }
    for f in &map.folders {
        let ni = folder_node[&f.id];
        let mut kids: Vec<usize> = children_of.get(&Some(f.id)).cloned().unwrap_or_default();
        kids.extend(map.folders.iter().filter(|c| c.parent == Some(f.id)).map(|c| folder_node[&c.id]));
        if !kids.is_empty() {
            nodes[ni]["children"] = json!(kids);
        }
    }
    let mut roots: Vec<usize> = children_of.get(&None).cloned().unwrap_or_default();
    roots.extend(map.folders.iter().filter(|f| f.parent.is_none_or(|p| map.folder(p).is_none())).map(|f| folder_node[&f.id]));

    let mut bin = d.bin.take().unwrap_or(Bin { data: Vec::new(), views: Vec::new() });
    while bin.data.len() % 4 != 0 {
        bin.data.push(0);
    }
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "Anvil level editor" },
        "scene": 0,
        "scenes": [{ "name": map.name, "nodes": roots }],
        "nodes": nodes,
    });
    if !d.meshes.is_empty() {
        doc["meshes"] = json!(d.meshes);
        doc["accessors"] = json!(d.accessors);
        doc["materials"] = json!(d.materials);
    }
    if !bin.data.is_empty() {
        doc["bufferViews"] = json!(bin.views);
        doc["buffers"] = json!([{ "byteLength": bin.data.len() }]);
    }
    if !d.images.is_empty() {
        doc["images"] = json!(d.images);
        doc["textures"] = json!(d.textures);
    }
    if !d.lights.is_empty() {
        doc["extensionsUsed"] = json!(["KHR_lights_punctual"]);
        doc["extensions"] = json!({ "KHR_lights_punctual": { "lights": d.lights } });
    }
    let mut js = doc.to_string().into_bytes();
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let has_bin = !bin.data.is_empty();
    let total = 12 + 8 + js.len() + if has_bin { 8 + bin.data.len() } else { 0 };
    let mut out = Vec::with_capacity(total);
    out.extend(0x4654_6C67u32.to_le_bytes());
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((js.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes());
    out.extend(&js);
    if has_bin {
        out.extend((bin.data.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(&bin.data);
    }
    out
}

/// blender.exe: the path in Settings, the usual install folders, or Steam.
pub fn find_blender(configured: &str) -> Option<PathBuf> {
    let c = configured.trim();
    if !c.is_empty() && Path::new(c).is_file() {
        return Some(PathBuf::from(c));
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    for root in [r"C:\Program Files\Blender Foundation", r"C:\Program Files (x86)\Blender Foundation"] {
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            dirs.sort();
            dirs.reverse(); // newest version first
            cands.extend(dirs.into_iter().map(|d| d.join("blender.exe")));
        }
    }
    for lib in crate::mount::steam_libraries() {
        cands.push(lib.join("steamapps").join("common").join("Blender").join("blender.exe"));
    }
    cands.into_iter().find(|p| p.is_file())
}

/// Runs in Blender (background mode): import the .glb, put objects into
/// collections named after their Anvil folders, save the .blend.
pub const BLENDER_SCRIPT: &str = r#"
import sys, bpy
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst = argv[0], argv[1]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=src)
root = bpy.context.scene.collection
def coll_for(path):
    cur = root
    for name in [p for p in path.split("/") if p]:
        nxt = None
        for c in cur.children:
            if c.get("anvil_path") == name:
                nxt = c
        if nxt is None:
            nxt = bpy.data.collections.new(name)
            nxt["anvil_path"] = name
            cur.children.link(nxt)
        cur = nxt
    return cur
folders = [ob for ob in bpy.data.objects if ob.get("anvil_is_folder")]
for ob in list(bpy.data.objects):
    if ob.get("anvil_is_folder"):
        continue
    target = coll_for(ob.get("anvil_folder", ""))
    mw = ob.matrix_world.copy()
    ob.parent = None
    ob.matrix_world = mw
    for uc in list(ob.users_collection):
        uc.objects.unlink(ob)
    target.objects.link(ob)
for ob in folders:
    bpy.data.objects.remove(ob, do_unlink=True)
bpy.ops.wm.save_as_mainfile(filepath=dst)
print("ANVIL_BLEND_OK")
"#;

/// Makes a .blend from a .glb with Blender in the background.
pub fn make_blend(blender: &Path, glb: &Path, out: &Path) -> Result<(), String> {
    let res = std::process::Command::new(blender)
        .args(["-b", "--factory-startup", "--python-expr", BLENDER_SCRIPT, "--"])
        .arg(glb)
        .arg(out)
        .output()
        .map_err(|e| format!("Couldn't start Blender: {e}"))?;
    let text = String::from_utf8_lossy(&res.stdout);
    if text.contains("ANVIL_BLEND_OK") && out.is_file() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&res.stderr);
        let last: Vec<&str> = text.lines().chain(err.lines()).filter(|l| !l.trim().is_empty()).collect();
        Err(format!("Blender couldn't make the .blend: {}", last.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | ")))
    }
}

impl crate::app::AnvilApp {
    /// File > Export > Blender: writes the .glb, then (for .blend) runs
    /// Blender in the background so the editor doesn't freeze.
    pub(crate) fn export_blender(&mut self, blend: bool) {
        let ext = if blend { "blend" } else { "glb" };
        let Some(out) = rfd::FileDialog::new().set_file_name(format!("{}.{ext}", self.map.name)).add_filter(ext, &[ext]).save_file() else { return };
        let bytes = scene_glb(&self.map, &mut self.models, self.settings.unity_scale);
        if !blend {
            self.status = match std::fs::write(&out, bytes) {
                Ok(()) => format!("Wrote {} (Blender: File > Import > glTF 2.0)", out.display()),
                Err(e) => format!("Couldn't write: {e}"),
            };
            return;
        }
        let Some(blender) = find_blender(&self.settings.blender_path) else {
            self.status = "Blender wasn't found".into();
            return;
        };
        let glb = out.with_extension("anvil_export.glb");
        if let Err(e) = std::fs::write(&glb, bytes) {
            self.status = format!("Couldn't write: {e}");
            return;
        }
        self.status = "Making the .blend in Blender (in the background)…".into();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = make_blend(&blender, &glb, &out).map(|_| out.display().to_string());
            let _ = std::fs::remove_file(&glb);
            let _ = tx.send(r);
        });
        self.blend_rx = Some(rx);
    }

    pub(crate) fn poll_blend(&mut self, ctx: &eframe::egui::Context) {
        let Some(rx) = &self.blend_rx else { return };
        match rx.try_recv() {
            Ok(r) => {
                self.status = match r {
                    Ok(p) => format!("Saved {p}"),
                    Err(e) => e,
                };
                self.blend_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(300)),
            Err(_) => self.blend_rx = None,
        }
    }
}
