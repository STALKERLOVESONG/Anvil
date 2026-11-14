//! Unity export: one OBJ + MTL holding every brush and prop (one child
//! object per Anvil object), plus an optional editor script that recreates
//! lights and spawn points.
//!
//! Geometry is written right-handed (Y up); Unity's OBJ importer mirrors X
//! on import, so entity positions in the script are mirrored the same way.

use crate::map::{Kind, Map, Material, TextureDef};
use crate::mesh::{world_mesh, ModelCache};
use crate::settings::Settings;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if out.is_empty() {
        "object".into()
    } else {
        out
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn mat_name(m: Material, c: [u8; 3], transparency: f32) -> String {
    let mut s = format!("{}_{:02x}{:02x}{:02x}", m.label(), c[0], c[1], c[2]);
    if transparency > 0.001 {
        let _ = write!(s, "_t{}", (transparency * 100.0).round() as u32);
    }
    s
}

fn f(v: f32) -> String {
    format!("{v:.4}f")
}

pub fn export(map: &Map, cache: &mut ModelCache, settings: &Settings, obj_path: &Path) -> Result<String, String> {
    let s = settings.unity_scale;
    let stem = obj_path.file_stem().and_then(|x| x.to_str()).unwrap_or("map").to_string();
    let dir = obj_path.parent().unwrap_or(Path::new("."));
    let mtl_file = format!("{stem}.mtl");
    let tex_dir_name = format!("{}_textures", sanitize(&stem));

    struct Mat {
        color: [u8; 3],
        transparency: f32,
        material: Material,
        tex: Option<TextureDef>,
        decal: bool,
    }
    let mut obj = String::new();
    let mut mats: BTreeMap<String, Mat> = BTreeMap::new();
    let _ = writeln!(obj, "# Exported by Anvil\nmtllib {mtl_file}");

    let (mut vbase, mut nbase, mut tbase) = (1usize, 1usize, 1usize);
    let mut count = 0;
    let mut used_names = std::collections::HashSet::new();

    for o in map.objects.iter().filter(|o| !o.hidden) {
        if matches!(o.kind, Kind::Light { .. } | Kind::Spawn { .. } | Kind::Prompt { .. } | Kind::Target {}) {
            continue;
        }
        let wm = world_mesh(o, &map.textures, cache);
        let decal = matches!(o.kind, Kind::Decal { .. });
        // untextured faces use a plain color material
        let mat = mat_name(o.material, o.color, o.transparency);
        mats.entry(mat.clone()).or_insert(Mat { color: o.color, transparency: o.transparency, material: o.material, tex: None, decal: false });
        // one material per texture slot (object texture, face textures, model textures), tinted
        let own_mats: Vec<String> = wm
            .texs
            .iter()
            .map(|t| {
                let stem = Path::new(&t.image).file_stem().and_then(|s| s.to_str()).unwrap_or("tex").to_string();
                let key = format!(
                    "{}_{}{}",
                    sanitize(&stem),
                    fnv(&format!("{}|{:?}|{}", t.image, t.tint, t.metallic)) % 100_000,
                    if decal { "_decal" } else { "" }
                );
                let mut def = TextureDef::new(stem, t.image.clone());
                def.normal_map = t.normal.clone();
                def.metallic = t.metallic;
                mats.entry(key.clone()).or_insert(Mat { color: t.tint, transparency: o.transparency, material: o.material, tex: Some(def), decal });
                key
            })
            .collect();

        let mut name = sanitize(&o.name);
        if let Some(f) = map.folder_of(o) {
            name = format!("{}__{name}", sanitize(&map.folder_path(f).replace('/', "__")));
        }
        if !used_names.insert(name.clone()) {
            name = format!("{name}_{}", o.id);
            used_names.insert(name.clone());
        }
        let _ = writeln!(obj, "o {name}\ng {name}");
        for p in &wm.pos {
            let _ = writeln!(obj, "v {} {} {}", p.x * s, p.y * s, p.z * s);
        }
        let textured = wm.tri_uv.len() == wm.mesh.tris.len() && !wm.tri_uv.is_empty();
        let mut faces: BTreeMap<String, String> = BTreeMap::new();
        let (mut nn, mut tn) = (0usize, 0usize);
        for (ti, t) in wm.mesh.tris.iter().enumerate() {
            let (a, b, c) = (wm.pos[t[0] as usize], wm.pos[t[1] as usize], wm.pos[t[2] as usize]);
            let n = (b - a).cross(c - a);
            if n.length_squared() < 1e-12 {
                continue;
            }
            let n = n.normalize();
            let _ = writeln!(obj, "vn {} {} {}", n.x, n.y, n.z);
            let ni = nbase + nn;
            nn += 1;
            let vi = [vbase + t[0] as usize, vbase + t[1] as usize, vbase + t[2] as usize];
            let key = match wm.tri_tex.get(ti) {
                Some(k) => own_mats.get(*k as usize).cloned().unwrap_or_else(|| mat.clone()),
                None => own_mats.first().cloned().unwrap_or_else(|| mat.clone()),
            };
            let faces = faces.entry(key).or_default();
            if textured {
                for uv in wm.tri_uv[ti] {
                    let _ = writeln!(obj, "vt {} {}", uv.x, 1.0 - uv.y);
                }
                let t0 = tbase + tn;
                tn += 3;
                let _ = writeln!(faces, "f {}/{}/{ni} {}/{}/{ni} {}/{}/{ni}", vi[0], t0, vi[1], t0 + 1, vi[2], t0 + 2);
            } else {
                let _ = writeln!(faces, "f {}//{ni} {}//{ni} {}//{ni}", vi[0], vi[1], vi[2]);
            }
        }
        for (m, f) in &faces {
            let _ = writeln!(obj, "usemtl {m}");
            obj.push_str(f);
        }
        vbase += wm.pos.len();
        nbase += nn;
        tbase += tn;
        count += 1;
    }

    // copy texture images next to the export
    let mut copied: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut used_files = std::collections::HashSet::new();
    let mut copy_tex = |src: &str| -> Option<String> {
        if src.trim().is_empty() {
            return None;
        }
        if let Some(r) = copied.get(src) {
            return Some(r.clone());
        }
        let mut fname = Path::new(src).file_name()?.to_str()?.to_string();
        if !used_files.insert(fname.to_ascii_lowercase()) {
            fname = format!("{}_{}", fnv(src) % 100_000, fname);
            used_files.insert(fname.to_ascii_lowercase());
        }
        let tdir = dir.join(&tex_dir_name);
        std::fs::create_dir_all(&tdir).ok()?;
        std::fs::copy(src, tdir.join(&fname)).ok()?;
        let rel = format!("{tex_dir_name}/{fname}");
        copied.insert(src.to_string(), rel.clone());
        Some(rel)
    };

    let mut mtl = String::from("# Exported by Anvil\n");
    for (name, m) in &mats {
        let c = m.color; // textured materials carry their tint here
        let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
        let _ = writeln!(mtl, "\nnewmtl {name}\nKa 0 0 0\nKd {r:.4} {g:.4} {b:.4}");
        let metal = m.tex.as_ref().is_some_and(|t| t.metallic)
            || matches!(m.material, Material::Metal | Material::Foil | Material::DiamondPlate | Material::CorrodedMetal);
        let spec = if metal || matches!(m.material, Material::Glass | Material::Ice) { 0.6 } else { 0.08 };
        let _ = writeln!(mtl, "Ks {spec} {spec} {spec}\nNs 40");
        if metal {
            let _ = writeln!(mtl, "Pm 1");
        }
        if m.material == Material::Neon {
            let _ = writeln!(mtl, "Ke {r:.4} {g:.4} {b:.4}");
        }
        if let Some(t) = &m.tex {
            if let Some(rel) = copy_tex(&t.image) {
                let _ = writeln!(mtl, "map_Kd {rel}");
                if m.decal {
                    let _ = writeln!(mtl, "map_d {rel}");
                }
            }
            if let Some(rel) = copy_tex(&t.normal_map) {
                let _ = writeln!(mtl, "map_Bump {rel}\nnorm {rel}");
            }
        }
        let _ = writeln!(mtl, "d {:.4}\nillum 2", 1.0 - m.transparency);
    }

    std::fs::write(obj_path, obj).map_err(|e| format!("Could not write {}: {e}", obj_path.display()))?;
    std::fs::write(dir.join(&mtl_file), mtl).map_err(|e| format!("Could not write {mtl_file}: {e}"))?;

    let mut msg = format!("Unity: wrote {count} objects to {} + {mtl_file}", obj_path.display());
    if !copied.is_empty() {
        let _ = write!(msg, " + {} texture(s) in {tex_dir_name}/", copied.len());
    }
    if let Some(script) = entity_script(map, settings, &stem) {
        let cs_name = format!("{}_AnvilEntities.cs", sanitize(&stem));
        std::fs::write(dir.join(&cs_name), script).map_err(|e| format!("Could not write {cs_name}: {e}"))?;
        let _ = write!(msg, " + {cs_name} (put it in Assets/Editor)");
    }
    Ok(msg)
}

/// Editor script that adds a menu item creating lights and spawn points.
fn entity_script(map: &Map, settings: &Settings, stem: &str) -> Option<String> {
    let s = settings.unity_scale;
    let ents: Vec<_> = map
        .objects
        .iter()
        .filter(|o| !o.hidden && matches!(o.kind, Kind::Light { .. } | Kind::Spawn { .. } | Kind::Prompt { .. } | Kind::Teleport { .. } | Kind::Target {}))
        .collect();
    if ents.is_empty() {
        return None;
    }
    let cls = sanitize(stem).replace('-', "_");
    let mut cs = String::new();
    let _ = writeln!(
        cs,
        "// Generated by Anvil. Put this file in Assets/Editor, then use the menu\n\
         // \"Anvil > Create entities for {stem}\" with the imported OBJ in the scene.\n\
         #if UNITY_EDITOR\n\
         using UnityEngine;\n\
         using UnityEditor;\n\n\
         public static class Anvil_{cls}_Entities\n{{\n    \
         [MenuItem(\"Anvil/Create entities for {stem}\")]\n    \
         public static void Create()\n    {{\n        \
         var root = new GameObject(\"{stem}_Entities\");\n        \
         Undo.RegisterCreatedObjectUndo(root, \"Create Anvil entities\");"
    );
    for o in ents {
        // Mirror X to match Unity's OBJ import.
        let (x, y, z) = (-o.pos.x * s, o.pos.y * s, o.pos.z * s);
        let name = o.name.replace('"', "'");
        let _ = writeln!(
            cs,
            "        {{\n            var go = new GameObject(\"{name}\");\n            \
             go.transform.SetParent(root.transform, false);\n            \
             go.transform.localPosition = new Vector3({}, {}, {});\n            \
             go.transform.localRotation = Quaternion.Euler({}, {}, {});",
            f(x),
            f(y),
            f(z),
            f(o.rot.x),
            f(-o.rot.y),
            f(-o.rot.z)
        );
        match &o.kind {
            Kind::Light { range, brightness } => {
                let c = o.color;
                let _ = writeln!(
                    cs,
                    "            var light = go.AddComponent<Light>();\n            \
                     light.type = LightType.Point;\n            \
                     light.range = {};\n            \
                     light.intensity = {};\n            \
                     light.color = new Color({}, {}, {});",
                    f(range * s),
                    f(*brightness),
                    f(c[0] as f32 / 255.0),
                    f(c[1] as f32 / 255.0),
                    f(c[2] as f32 / 255.0)
                );
            }
            Kind::Spawn { size } => {
                let _ = writeln!(
                    cs,
                    "            go.tag = \"Respawn\";\n            \
                     var box = go.AddComponent<BoxCollider>();\n            \
                     box.size = new Vector3({}, {}, {});",
                    f(size.x * s),
                    f(size.y * s),
                    f(size.z * s)
                );
            }
            _ => {}
        }
        cs.push_str("        }\n");
    }
    cs.push_str("        Selection.activeGameObject = root;\n    }\n}\n#endif\n");
    Some(cs)
}
