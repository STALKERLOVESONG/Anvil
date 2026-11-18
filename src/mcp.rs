//! `anvil.exe --mcp`: a Model Context Protocol server (JSON-RPC over stdin /
//! stdout) so an AI assistant can build maps: make and save maps, lay out
//! buildings, cut doors and windows, add roofs, place props, search and
//! import textures and models from the mounted Half-Life / GMod content, and
//! render a view to check its work. It edits a map file; open that file in
//! Anvil to see it (File > Open, or reopen after changes).

use crate::map::{Hole, Kind, Map, Material, Object, Shape, TextureDef};
use crate::mesh::{world_mesh, ModelCache};
use crate::settings::Settings;
use glam::Vec3;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

struct Server {
    map: Map,
    path: Option<PathBuf>,
    models: ModelCache,
    textures: crate::texture::TexCache,
    mounts: Option<crate::mount::Mounts>,
    settings: Settings,
}

type R = Result<Value, String>;

const ALPHA_NAMES: [&str; 3] = ["solid", "cutout", "see_through"];

fn v3(v: &Value, key: &str) -> Option<Vec3> {
    let a = v.get(key)?.as_array()?;
    (a.len() == 3).then(|| Vec3::new(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32))
}

fn f(v: &Value, key: &str, d: f32) -> f32 {
    v.get(key).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

fn color(v: &Value, d: [u8; 3]) -> [u8; 3] {
    match v.get("color").and_then(|c| c.as_array()) {
        Some(a) if a.len() == 3 => [0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0).clamp(0.0, 255.0) as u8),
        _ => d,
    }
}

fn material(v: &Value, d: Material) -> Material {
    let want = s(v, "material").to_ascii_lowercase().replace([' ', '_'], "");
    Material::ALL.into_iter().find(|m| m.label().to_ascii_lowercase().replace(' ', "") == want).unwrap_or(d)
}

fn shape(name: &str) -> Option<Shape> {
    Shape::ALL.into_iter().find(|s| s.label().eq_ignore_ascii_case(name))
}

impl Server {
    fn folder(&mut self, v: &Value) -> Option<u64> {
        let name = s(v, "folder");
        if name.is_empty() {
            return None;
        }
        Some(match self.map.folders.iter().find(|f| f.name == name) {
            Some(f) => f.id,
            None => self.map.add_folder(name, None),
        })
    }

    fn texture(&self, v: &Value, key: &str) -> Option<u64> {
        let id = v.get(key)?.as_u64()?;
        self.map.textures.iter().any(|t| t.id == id).then_some(id)
    }

    fn mounts(&mut self) -> Result<&crate::mount::Mounts, String> {
        if self.mounts.is_none() {
            let m = crate::mount::Mounts::build(&self.settings.mounts);
            if m.is_empty() {
                return Err("No Half-Life / GMod content is mounted (Anvil: Half-Life window > Add folder)".into());
            }
            self.mounts = Some(m);
        }
        Ok(self.mounts.as_ref().unwrap())
    }

    fn autosave(&mut self) -> Option<String> {
        let p = self.path.clone()?;
        self.map.save(&p).err()
    }

    fn summary(&mut self) -> Value {
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut objs = Vec::new();
        for o in &self.map.objects {
            let wm = world_mesh(o, &self.map.textures, &mut self.models);
            if !wm.pos.is_empty() {
                mn = mn.min(wm.min);
                mx = mx.max(wm.max);
            }
            objs.push(json!({ "id": o.id, "name": o.name, "kind": o.type_label(), "pos": [o.pos.x, o.pos.y, o.pos.z], "rot_y": o.rot.y, "bounds_min": [wm.min.x, wm.min.y, wm.min.z], "bounds_max": [wm.max.x, wm.max.y, wm.max.z], "folder": o.folder.and_then(|f| self.map.folder(f)).map(|f| f.name.clone()) }));
        }
        json!({
            "path": self.path,
            "name": self.map.name,
            "objects": self.map.objects.len(),
            "bounds": if mn.x <= mx.x { json!([[mn.x, mn.y, mn.z], [mx.x, mx.y, mx.z]]) } else { Value::Null },
            "folders": self.map.folders.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
            "textures": self.map.textures.iter().map(|t| json!({ "id": t.id, "name": t.name })).collect::<Vec<_>>(),
            "list": objs.into_iter().rev().take(300).collect::<Vec<_>>(),
        })
    }

    fn add_one(&mut self, o: &Value) -> Result<u64, String> {
        let kind = s(o, "kind").to_ascii_lowercase();
        let pos = v3(o, "pos").ok_or("each object needs pos [x, y, z]")?;
        let rot = v3(o, "rot").unwrap_or(Vec3::new(0.0, f(o, "rot_y", 0.0), 0.0));
        let folder = self.folder(o);
        let mut obj = match kind.as_str() {
            "prop" => {
                let model = s(o, "model");
                if model.is_empty() {
                    return Err("a prop needs model (a file path, e.g. from import_model)".into());
                }
                let sc = v3(o, "scale").unwrap_or(Vec3::splat(f(o, "scale", 1.0)));
                Object::new(Kind::Prop { model: model.to_string(), scale: sc, roblox_mesh_id: String::new() }, pos, color(o, [255; 3]), material(o, Material::SmoothPlastic))
            }
            "light" => {
                let mut l = Object::new(Kind::Light { range: f(o, "range", 30.0), brightness: f(o, "brightness", 1.0) }, pos, color(o, [255, 236, 190]), Material::Neon);
                l.collide = false;
                l
            }
            "spawn" => Object::new(Kind::Spawn { size: v3(o, "size").unwrap_or(Vec3::new(4.0, 0.2, 4.0)) }, pos, [90, 200, 110], Material::SmoothPlastic),
            k => {
                let sh = shape(if k.is_empty() { "box" } else { k }).ok_or(format!("unknown kind {k} (box, wedge, cylinder, sphere, stairs, prop, light, spawn)"))?;
                let size = v3(o, "size").ok_or("a brush needs size [x, y, z]")?;
                let mut b = Object::brush(sh, size, pos, color(o, [200, 200, 200]), material(o, Material::Plastic));
                if let Kind::Brush { steps, .. } = &mut b.kind {
                    *steps = o.get("steps").and_then(|x| x.as_u64()).unwrap_or(8).clamp(1, 128) as u32;
                }
                b
            }
        };
        obj.rot = rot;
        obj.name = s(o, "name").to_string();
        obj.folder = folder;
        obj.texture = self.texture(o, "texture");
        obj.transparency = f(o, "transparency", 0.0).clamp(0.0, 1.0);
        if let Some(c) = o.get("collide").and_then(|x| x.as_bool()) {
            obj.collide = c;
        }
        Ok(self.map.add(obj))
    }

    fn call(&mut self, name: &str, a: &Value) -> Result<Vec<Value>, String> {
        let text = |v: Value| vec![json!({ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() })];
        let out: R = match name {
            "new_map" => {
                let p = PathBuf::from(s(a, "path"));
                if p.extension().and_then(|e| e.to_str()) != Some("anvil") {
                    return Err("path must end in .anvil".into());
                }
                self.map = Map { name: if s(a, "name").is_empty() { p.file_stem().and_then(|x| x.to_str()).unwrap_or("map").to_string() } else { s(a, "name").to_string() }, ..Map::default() };
                self.path = Some(p.clone());
                self.map.save(&p)?;
                Ok(json!({ "saved": p }))
            }
            "open_map" => {
                let p = PathBuf::from(s(a, "path"));
                self.map = Map::load(&p)?;
                self.path = Some(p);
                Ok(self.summary())
            }
            "save_map" => {
                if !s(a, "path").is_empty() {
                    let p = PathBuf::from(s(a, "path"));
                    if p.extension().and_then(|e| e.to_str()) != Some("anvil") {
                        return Err("path must end in .anvil".into());
                    }
                    self.path = Some(p);
                }
                let p = self.path.clone().ok_or("no path yet: give one")?;
                self.map.save(&p)?;
                Ok(json!({ "saved": p }))
            }
            "map_summary" => Ok(self.summary()),
            "add_objects" => {
                let list = a.get("objects").and_then(|x| x.as_array()).ok_or("objects: [...] needed")?.clone();
                // the good ones are added; each bad one is listed with why
                let (mut ids, mut errors) = (Vec::new(), Vec::new());
                for (i, o) in list.iter().enumerate() {
                    match self.add_one(o) {
                        Ok(id) => ids.push(json!(id)),
                        Err(e) => {
                            ids.push(Value::Null);
                            errors.push(json!({ "index": i, "error": e }));
                        }
                    }
                }
                Ok(json!({ "ids": ids, "errors": errors }))
            }
            "build_house" => {
                let pts: Vec<Vec3> = a.get("outline").and_then(|x| x.as_array()).ok_or("outline: [[x, z], ...] needed")?.iter().filter_map(|p| p.as_array()).filter(|p| p.len() == 2).map(|p| Vec3::new(p[0].as_f64().unwrap_or(0.0) as f32, 0.0, p[1].as_f64().unwrap_or(0.0) as f32)).collect();
                if pts.len() < 3 {
                    return Err("outline needs 3+ corners".into());
                }
                let mut st = Settings::default();
                st.wall_height = f(a, "wall_height", 10.0);
                st.wall_thickness = f(a, "wall_thickness", 1.0);
                st.layout_floor = a.get("floor").and_then(|x| x.as_bool()).unwrap_or(true);
                st.layout_ceiling = a.get("ceiling").and_then(|x| x.as_bool()).unwrap_or(true);
                st.floor_thickness = f(a, "floor_thickness", 1.0);
                st.wall_color = color(a, [220, 200, 170]);
                st.wall_material = material(a, Material::Concrete);
                st.floor_color = [200, 200, 200];
                st.floor_material = Material::Wood;
                st.trim_baseboard = a.get("baseboard").and_then(|x| x.as_bool()).unwrap_or(false);
                let base_y = f(a, "base_y", 0.0);
                let folder = self.map.add_folder(if s(a, "folder").is_empty() { "House" } else { s(a, "folder") }, None);
                let (wt, ft) = (self.texture(a, "wall_texture"), self.texture(a, "floor_texture"));
                let ceil_tex = self.texture(a, "ceiling_texture");
                let mut ids = Vec::new();
                for mut o in crate::building::house_objects(&pts, &st, base_y, wt, ft) {
                    o.folder = Some(folder);
                    if o.name == "Ceiling" {
                        o.texture = ceil_tex.or(o.texture);
                    }
                    ids.push(json!({ "id": self.map.add(o.clone()), "name": o.name }));
                }
                Ok(json!({ "objects": ids, "note": "Walls are named Wall1..WallN in outline order (Wall1 from corner 1 to corner 2). cut_opening cuts doors / windows into them." }))
            }
            "cut_opening" => {
                let id = a.get("object_id").and_then(|x| x.as_u64()).ok_or("object_id needed")?;
                let at = v3(a, "at").ok_or("at [x, y, z] needed: a point on the wall")?;
                let (w, h, bottom) = (f(a, "width", 4.0), f(a, "height", 7.0), f(a, "bottom", 0.0));
                let o = self.map.get(id).cloned().ok_or("no such object")?;
                let Kind::Brush { shape: Shape::Box, size, .. } = o.kind else { return Err("openings cut into Box brushes (walls)".into()) };
                let local = o.to_local(at);
                let thin = (0..3).min_by(|x, y| size[*x].total_cmp(&size[*y])).unwrap_or(2);
                let along = if thin == 0 { 2 } else { 0 };
                let half = size[along] * 0.5;
                let c = local[along].clamp(-half + w * 0.5, half - w * 0.5);
                let mut hole = Hole { min: Vec3::ZERO, max: Vec3::ZERO };
                hole.min[along] = c - w * 0.5;
                hole.max[along] = c + w * 0.5;
                hole.min.y = -size.y * 0.5 + bottom - if bottom <= 0.0 { 0.1 } else { 0.0 };
                hole.max.y = (-size.y * 0.5 + bottom + h).min(size.y * 0.5 + 0.1);
                hole.min[thin] = -size[thin] * 0.5 - 0.1;
                hole.max[thin] = size[thin] * 0.5 + 0.1;
                if let Some(Object { kind: Kind::Brush { holes, .. }, .. }) = self.map.get_mut(id) {
                    holes.push(hole);
                }
                // trim strips of the same building get the hole too
                let corners = crate::building::opening_reach(&o, &hole, 2.0);
                let trims: Vec<u64> = self.map.objects.iter().filter(|t| t.id != id && t.folder == o.folder && (t.name.starts_with("Baseboard") || t.name.starts_with("Crown") || t.name.starts_with("Trim"))).map(|t| t.id).collect();
                for tid in trims {
                    if let Some(t) = self.map.get_mut(tid) {
                        if let Some(hh) = crate::building::trim_hole(t, &corners) {
                            if let Kind::Brush { holes, .. } = &mut t.kind {
                                holes.push(hh);
                            }
                        }
                    }
                }
                let mut extra = Value::Null;
                if a.get("glass").and_then(|x| x.as_bool()).unwrap_or(false) {
                    let mut gc = Vec3::ZERO;
                    gc[along] = c;
                    gc.y = (hole.min.y.max(-size.y * 0.5) + hole.max.y) * 0.5;
                    let mut gs = Vec3::ZERO;
                    gs[along] = w;
                    gs.y = hole.max.y - hole.min.y.max(-size.y * 0.5);
                    gs[thin] = (size[thin] * 0.2).max(0.1);
                    let mut g = Object::brush(Shape::Box, gs, o.to_world(gc), [160, 205, 235], Material::Glass);
                    g.rot = o.rot;
                    g.transparency = 0.6;
                    g.folder = o.folder;
                    g.name = format!("{}_Glass", o.name);
                    extra = json!(self.map.add(g));
                }
                Ok(json!({ "cut": id, "hole_local": [[hole.min.x, hole.min.y, hole.min.z], [hole.max.x, hole.max.y, hole.max.z]], "glass_id": extra }))
            }
            "add_roof" => {
                let mn = a.get("min").and_then(|x| x.as_array()).filter(|x| x.len() == 2).ok_or("min [x, z] needed")?;
                let mx = a.get("max").and_then(|x| x.as_array()).filter(|x| x.len() == 2).ok_or("max [x, z] needed")?;
                let o = f(a, "overhang", 1.5);
                let (x0, z0) = (mn[0].as_f64().unwrap_or(0.0) as f32 - o, mn[1].as_f64().unwrap_or(0.0) as f32 - o);
                let (x1, z1) = (mx[0].as_f64().unwrap_or(0.0) as f32 + o, mx[1].as_f64().unwrap_or(0.0) as f32 + o);
                let (y0, h, t) = (f(a, "base_y", 10.0), f(a, "height", 5.0), f(a, "thickness", 0.6));
                let along_x = s(a, "ridge") != "z";
                let kind = s(a, "kind");
                let verts_faces = roof_mesh(kind, x0, z0, x1, z1, y0, h, t, along_x, f(a, "hip", -1.0));
                let (verts, faces) = verts_faces.ok_or("kind must be gable, hip or flat")?;
                let center = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
                let local: Vec<Vec3> = verts.iter().map(|v| *v - center).collect();
                let mut r = Object::new(Kind::Mesh { verts: local, faces }, center, color(a, [120, 80, 55]), material(a, Material::Slate));
                r.name = if s(a, "name").is_empty() { "Roof".into() } else { s(a, "name").into() };
                r.folder = self.folder(a);
                r.texture = self.texture(a, "texture");
                Ok(json!({ "id": self.map.add(r) }))
            }
            "delete_objects" => {
                let ids: Vec<u64> = a.get("ids").and_then(|x| x.as_array()).map(|x| x.iter().filter_map(|i| i.as_u64()).collect()).unwrap_or_default();
                self.map.remove(&ids);
                Ok(json!({ "deleted": ids.len() }))
            }
            "set_object" => {
                let id = a.get("id").and_then(|x| x.as_u64()).ok_or("id needed")?;
                let tex = if a.get("texture").is_some() { Some(self.texture(a, "texture")) } else { None };
                let folder = if a.get("folder").is_some() { Some(self.folder(a)) } else { None };
                let o = self.map.get_mut(id).ok_or("no such object")?;
                if let Some(p) = v3(a, "pos") {
                    o.pos = p;
                }
                if let Some(r) = v3(a, "rot") {
                    o.rot = r;
                } else if a.get("rot_y").is_some() {
                    o.rot.y = f(a, "rot_y", 0.0);
                }
                if let (Some(sz), Kind::Brush { size, .. }) = (v3(a, "size"), &mut o.kind) {
                    *size = sz;
                }
                if a.get("color").is_some() {
                    o.color = color(a, o.color);
                }
                if a.get("material").is_some() {
                    o.material = material(a, o.material);
                }
                if let Some(t) = tex {
                    o.texture = t;
                }
                if let Some(fo) = folder {
                    o.folder = fo;
                }
                if let Some(n) = a.get("name").and_then(|x| x.as_str()) {
                    o.name = n.to_string();
                }
                if let Some(t) = a.get("transparency").and_then(|x| x.as_f64()) {
                    o.transparency = t.clamp(0.0, 1.0) as f32;
                }
                if let Some(c) = a.get("collide").and_then(|x| x.as_bool()) {
                    o.collide = c;
                }
                Ok(json!({ "updated": id }))
            }
            "search_content" => {
                let q: Vec<String> = s(a, "query").to_ascii_lowercase().split_whitespace().map(|x| x.to_string()).collect();
                let models = s(a, "kind") == "models";
                let limit = a.get("limit").and_then(|x| x.as_u64()).unwrap_or(40).min(300) as usize;
                let m = self.mounts()?;
                let list = if models { &m.models } else { &m.materials };
                let hits: Vec<&String> = list.iter().filter(|n| q.iter().all(|w| n.to_ascii_lowercase().contains(w))).take(limit).collect();
                Ok(json!({ "kind": if models { "models" } else { "materials" }, "total_mounted": list.len(), "matches": hits }))
            }
            "import_texture" => {
                let rel = s(a, "material").to_string();
                let files = {
                    let m = self.mounts()?;
                    crate::mount::import_material(m, &rel)?
                };
                let id = match self.map.textures.iter().find(|t| t.image == files.image) {
                    Some(t) => t.id,
                    None => {
                        let mut t = TextureDef::new(if s(a, "name").is_empty() { files.name.clone() } else { s(a, "name").to_string() }, files.image.clone());
                        t.normal_map = files.normal.clone();
                        t.alpha = files.alpha;
                        self.map.add_texture(t)
                    }
                };
                Ok(json!({ "texture_id": id, "image": files.image, "alpha": ALPHA_NAMES[files.alpha.min(2) as usize] }))
            }
            "add_texture_file" => {
                let p = s(a, "path");
                if !std::path::Path::new(p).is_file() || crate::texture::is_network_path(p) {
                    return Err("path must be an image file on this PC".into());
                }
                let id = self.map.add_texture(TextureDef::new(if s(a, "name").is_empty() { "texture".into() } else { s(a, "name").into() }, p.to_string()));
                Ok(json!({ "texture_id": id }))
            }
            "set_texture_scale" => {
                let id = a.get("texture_id").and_then(|x| x.as_u64()).ok_or("texture_id needed")?;
                let t = self.map.textures.iter_mut().find(|t| t.id == id).ok_or("no such texture")?;
                if a.get("studs_per_tile").is_some() {
                    let sc = f(a, "studs_per_tile", 8.0).max(0.01);
                    t.scale = glam::Vec2::new(sc, sc);
                }
                match s(a, "alpha") {
                    "solid" => t.alpha = 0,
                    "cutout" => t.alpha = 1,
                    "see_through" => t.alpha = 2,
                    _ => {}
                }
                Ok(json!({ "texture_id": id, "studs_per_tile": t.scale.x, "alpha": ALPHA_NAMES[t.alpha.min(2) as usize] }))
            }
            "import_model" => {
                let rel = s(a, "model").to_string();
                let scale = f(a, "scale", self.settings.source_scale);
                let path = {
                    let m = self.mounts()?;
                    crate::mount::import_model(m, &rel, scale)?
                };
                let p = path.to_string_lossy().to_string();
                let size = self.models.get(&p).ok().map(|m| {
                    let (mn, mx) = m.pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
                    json!({ "min": [mn.x, mn.y, mn.z], "max": [mx.x, mx.y, mx.z] })
                });
                Ok(json!({ "model": p, "bounds_at_scale_1": size, "note": "place it with add_objects kind prop, model = this path; pos is the model's origin (usually its base)" }))
            }
            "import_bsp" => {
                let p = std::path::PathBuf::from(s(a, "path"));
                let ext_ok = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bsp"));
                if !ext_ok || !p.is_file() || crate::texture::is_network_path(&p.to_string_lossy()) {
                    return Err("path must be a .bsp file on this PC".into());
                }
                let opts = crate::bsp::Options {
                    scale: f(a, "scale", self.settings.source_scale),
                    spawn_size: self.settings.spawn_size,
                    props: a.get("props").and_then(|x| x.as_bool()).unwrap_or(true),
                };
                let _ = self.mounts();
                let imp = crate::bsp::import(&p, self.mounts.as_ref(), &opts, &|_| {})?;
                let report = imp.report.clone();
                let ids = crate::bsp::apply(&mut self.map, imp);
                Ok(json!({
                    "objects": ids.len(),
                    "summary": report.summary(),
                    "missing_materials": report.missing_materials.iter().take(40).collect::<Vec<_>>(),
                    "missing_models": report.missing_models.iter().take(40).collect::<Vec<_>>(),
                    "warnings": report.warnings,
                }))
            }
            "model_bounds" => {
                let p = s(a, "model").to_string();
                let m = self.models.get(&p)?;
                let (mn, mx) = m.pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
                Ok(json!({ "min": [mn.x, mn.y, mn.z], "max": [mx.x, mx.y, mx.z] }))
            }
            "render" => return self.render(a),
            _ => Err(format!("unknown tool {name}")),
        };
        let v = out?;
        let saved = if matches!(name, "map_summary" | "search_content" | "model_bounds" | "open_map" | "save_map" | "new_map") { None } else { self.autosave() };
        let mut content = text(v);
        if let Some(e) = saved {
            content.push(json!({ "type": "text", "text": format!("(couldn't save the map: {e})") }));
        }
        Ok(content)
    }

    fn render(&mut self, a: &Value) -> Result<Vec<Value>, String> {
        use crate::view3d::{Camera, RenderOpts, Renderer};
        let eye = v3(a, "eye").ok_or("eye [x, y, z] needed")?;
        let target = v3(a, "target").ok_or("target [x, y, z] needed")?;
        let (w, h) = (f(a, "width", 960.0).clamp(64.0, 1920.0), f(a, "height", 540.0).clamp(64.0, 1080.0));
        let scene = crate::mesh::build_scene(&self.map, &mut self.models);
        let ctx = eframe::egui::Context::default();
        let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(w, h));
        let cam = Camera::looking_at(eye, target);
        let st = &self.settings;
        let sun = crate::lighting::sun_dir(st.sun_azimuth, st.sun_elevation);
        let q = a.get("lighting").and_then(|x| x.as_u64()).unwrap_or(2).min(4) as u8;
        let opts = RenderOpts { fov: f(a, "fov", 70.0), grid: 1.0, plane_y: 0.0, show_grid: false, outlines: false, bg: [140, 170, 210], scale: 1.0, filter: true, lighting: crate::lighting::preset(q, sun, st.fog_enabled, st.fog_distance.max(400.0), true) };
        let mut r = Renderer::default();
        r.render(&ctx, rect, &cam, &opts, &scene, &[], &std::collections::HashMap::new(), &mut self.textures);
        let (pw, ph, px) = r.pixels();
        let img = image::RgbaImage::from_raw(pw as u32, ph as u32, px.to_vec()).ok_or("render failed")?;
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let out = crate::autosave::dir().join("mcp_render.png");
        let _ = std::fs::create_dir_all(crate::autosave::dir());
        let _ = std::fs::write(&out, png.get_ref());
        Ok(vec![json!({ "type": "image", "data": crate::team::b64_encode(png.get_ref()), "mimeType": "image/png" }), json!({ "type": "text", "text": format!("Rendered {pw}x{ph} (also saved to {})", out.display()) })])
    }
}

/// A roof as a closed mesh over [x0, x1] x [z0, z1] (overhang included),
/// eaves at `y0`, `h` high, `t` thick. Gable: ridge along X (or Z) end to
/// end; hip: the ridge stops `hip` short of each end (default: half the
/// width, a pyramid-like hip); flat: a slab.
#[allow(clippy::too_many_arguments)]
fn roof_mesh(kind: &str, x0: f32, z0: f32, x1: f32, z1: f32, y0: f32, h: f32, t: f32, along_x: bool, hip: f32) -> Option<(Vec<Vec3>, Vec<Vec<u32>>)> {
    // work in (u along the ridge, w across it), then map back
    let (u0, u1, w0, w1) = if along_x { (x0, x1, z0, z1) } else { (z0, z1, x0, x1) };
    let to = |u: f32, y: f32, w: f32| if along_x { Vec3::new(u, y, w) } else { Vec3::new(w, y, u) };
    let wm = (w0 + w1) * 0.5;
    let (mut v, mut faces): (Vec<Vec3>, Vec<Vec<u32>>) = (Vec::new(), Vec::new());
    match kind {
        "flat" => {
            for (u, w) in [(u0, w0), (u1, w0), (u1, w1), (u0, w1)] {
                v.push(to(u, y0, w));
            }
            for (u, w) in [(u0, w0), (u1, w0), (u1, w1), (u0, w1)] {
                v.push(to(u, y0 + t, w));
            }
            faces = vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7], vec![0, 1, 5, 4], vec![1, 2, 6, 5], vec![2, 3, 7, 6], vec![3, 0, 4, 7]];
        }
        "gable" | "hip" => {
            let inset = if kind == "hip" { if hip >= 0.0 { hip } else { (w1 - w0) * 0.5 } } else { 0.0 };
            let (ra, rb) = ((u0 + inset).min((u0 + u1) * 0.5), (u1 - inset).max((u0 + u1) * 0.5));
            // top surface: eaves rectangle + ridge
            let e = [to(u0, y0, w0), to(u1, y0, w0), to(u1, y0, w1), to(u0, y0, w1)];
            let r = [to(ra, y0 + h, wm), to(rb, y0 + h, wm)];
            v.extend(e);
            v.extend(r); // 4, 5
            // a thin underside a little lower, so it has thickness when seen from below
            v.extend(e.map(|p| p - Vec3::Y * t)); // 6..9
            faces.push(vec![0, 1, 5, 4]); // slope on the w0 side
            faces.push(vec![2, 3, 4, 5]); // slope on the w1 side
            if kind == "hip" {
                faces.push(vec![1, 2, 5]);
                faces.push(vec![3, 0, 4]);
            } else {
                faces.push(vec![1, 2, 5]); // gable ends (vertical triangles)
                faces.push(vec![3, 0, 4]);
            }
            faces.push(vec![6, 7, 8, 9]); // underside
            for i in 0..4u32 {
                let j = (i + 1) % 4;
                faces.push(vec![i, j, 6 + j, 6 + i]); // fascia
            }
        }
        _ => return None,
    }
    // every face facing out from the middle (the renderer hides back faces)
    let c = v.iter().copied().sum::<Vec3>() / v.len() as f32;
    for fc in &mut faces {
        let n = crate::polymesh::newell(&v, fc);
        let fcen = fc.iter().map(|i| v[*i as usize]).sum::<Vec3>() / fc.len() as f32;
        if n.dot(fcen - c) < 0.0 {
            fc.reverse();
        }
    }
    Some((v, faces))
}

fn tools() -> Value {
    let obj = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req });
    let xyz = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 });
    let xz = json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 });
    let rgb = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "0-255" });
    let mats = Material::ALL.iter().map(|m| m.label()).collect::<Vec<_>>().join(", ");
    json!([
        { "name": "new_map", "description": "Start a new empty map saved at path (.anvil). Every edit after this saves the map. Units: 1 unit ≈ 0.28 m (Roblox stud); Y is up; a person is about 6 units tall.", "inputSchema": obj(json!({ "path": { "type": "string" }, "name": { "type": "string" } }), &["path"]) },
        { "name": "open_map", "description": "Open an existing .anvil map (edits save back to it).", "inputSchema": obj(json!({ "path": { "type": "string" } }), &["path"]) },
        { "name": "save_map", "description": "Save the map (optionally to a new .anvil path).", "inputSchema": obj(json!({ "path": { "type": "string" } }), &[]) },
        { "name": "map_summary", "description": "The map's objects (id, name, kind, position, world bounds), folders and textures.", "inputSchema": obj(json!({}), &[]) },
        { "name": "add_objects", "description": format!("Add objects. Each: kind (box | wedge | cylinder | sphere | stairs | prop | light | spawn), pos [x,y,z] (center for brushes; a prop's origin), size [x,y,z] for brushes, rot_y (degrees) or rot [x,y,z], color [r,g,b], material ({mats}), texture (texture id), name, folder (name; made if new), transparency 0-1, collide (bool). Props: model (file path from import_model), scale. Lights: range, brightness. Wedges rise toward +Z; stairs climb toward +Z (steps). Returns ids."), "inputSchema": obj(json!({ "objects": { "type": "array", "items": { "type": "object" } } }), &["objects"]) },
        { "name": "build_house", "description": "Walls (with clean corners) along a closed outline of [x, z] corners at their center line, plus floor and ceiling flush with the walls' outside. Returns ids; walls are Wall1..WallN in outline order (WallK runs from corner K to K+1).", "inputSchema": obj(json!({ "outline": { "type": "array", "items": xz }, "base_y": { "type": "number" }, "wall_height": { "type": "number" }, "wall_thickness": { "type": "number" }, "floor": { "type": "boolean" }, "ceiling": { "type": "boolean" }, "floor_thickness": { "type": "number" }, "color": rgb, "material": { "type": "string" }, "wall_texture": { "type": "integer" }, "floor_texture": { "type": "integer" }, "ceiling_texture": { "type": "integer" }, "baseboard": { "type": "boolean" }, "folder": { "type": "string" } }), &["outline"]) },
        { "name": "cut_opening", "description": "Cut a door or window hole through a Box wall at a point on it: width along the wall, height, bottom = height of the hole's bottom above the wall's bottom (0 for a door). glass: add a glass pane.", "inputSchema": obj(json!({ "object_id": { "type": "integer" }, "at": xyz, "width": { "type": "number" }, "height": { "type": "number" }, "bottom": { "type": "number" }, "glass": { "type": "boolean" } }), &["object_id", "at"]) },
        { "name": "add_roof", "description": "A roof over the rectangle min [x,z]..max [x,z] (overhang added around it): kind gable | hip | flat, base_y (eaves height), height (eaves to ridge), ridge x | z (the ridge's direction), hip (hip only: how far the ridge stops short of each end), thickness, color, material, texture, folder, name.", "inputSchema": obj(json!({ "kind": { "type": "string" }, "min": xz, "max": xz, "base_y": { "type": "number" }, "height": { "type": "number" }, "overhang": { "type": "number" }, "ridge": { "type": "string" }, "hip": { "type": "number" }, "thickness": { "type": "number" }, "color": rgb, "material": { "type": "string" }, "texture": { "type": "integer" }, "folder": { "type": "string" }, "name": { "type": "string" } }), &["kind", "min", "max"]) },
        { "name": "set_object", "description": "Change an object: pos, rot / rot_y, size (brushes), color, material, texture (id, or null to clear), name, folder, transparency, collide.", "inputSchema": obj(json!({ "id": { "type": "integer" } }), &["id"]) },
        { "name": "delete_objects", "description": "Delete objects by id.", "inputSchema": obj(json!({ "ids": { "type": "array", "items": { "type": "integer" } } }), &["ids"]) },
        { "name": "search_content", "description": "Search the mounted Half-Life / GMod content: kind materials (paths under materials/, no extension) or models (.mdl paths). All query words must appear in the path.", "inputSchema": obj(json!({ "kind": { "type": "string", "enum": ["materials", "models"] }, "query": { "type": "string" }, "limit": { "type": "integer" } }), &["kind", "query"]) },
        { "name": "import_texture", "description": "Import a mounted material (from search_content) into the map's textures (PNG + normal map). Returns texture_id to use on objects.", "inputSchema": obj(json!({ "material": { "type": "string" }, "name": { "type": "string" } }), &["material"]) },
        { "name": "set_texture_scale", "description": "A texture's tiling (studs_per_tile: units one tile covers, default 8) and transparency (alpha: solid | cutout | see_through; imported GMod materials get theirs set already).", "inputSchema": obj(json!({ "texture_id": { "type": "integer" }, "studs_per_tile": { "type": "number" }, "alpha": { "type": "string", "enum": ["solid", "cutout", "see_through"] } }), &["texture_id"]) },
        { "name": "add_texture_file", "description": "Add an image file on this PC as a texture.", "inputSchema": obj(json!({ "path": { "type": "string" }, "name": { "type": "string" } }), &["path"]) },
        { "name": "import_model", "description": "Convert a mounted .mdl (from search_content) into a textured OBJ and return its path (and size) for add_objects kind prop.", "inputSchema": obj(json!({ "model": { "type": "string" }, "scale": { "type": "number" } }), &["model"]) },
        { "name": "import_bsp", "description": "Import a Source engine map (.bsp from Garry's Mod / Half-Life 2) into the open map: brushes become editable meshes with their textures, plus terrain, props, lights and spawns, under one folder named after the map.", "inputSchema": obj(json!({ "path": { "type": "string" }, "scale": { "type": "number" }, "props": { "type": "boolean" } }), &["path"]) },
        { "name": "model_bounds", "description": "A model file's size (at scale 1).", "inputSchema": obj(json!({ "model": { "type": "string" } }), &["model"]) },
        { "name": "render", "description": "Render the map from eye [x,y,z] looking at target [x,y,z] (Anvil's 3D view, lit) and return the picture.", "inputSchema": obj(json!({ "eye": xyz, "target": xyz, "width": { "type": "number" }, "height": { "type": "number" }, "fov": { "type": "number" }, "lighting": { "type": "integer", "description": "0 flat .. 4 best" } }), &["eye", "target"]) },
    ])
}

/// Runs the server on stdin / stdout until stdin closes.
pub fn serve() {
    let mut sv = Server { map: Map::default(), path: None, models: ModelCache::default(), textures: crate::texture::TexCache::default(), mounts: None, settings: Settings::load() };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } }));
            let _ = out.flush();
            continue;
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "anvil", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Anvil level editor: build maps. Start with new_map or open_map; then build_house / add_objects / add_roof / cut_opening; search_content + import_texture / import_model use the user's mounted GMod content; render to look at the result. Units are studs (about 0.28 m), Y up."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match sv.call(&name, &args) {
                    Ok(content) => Ok(json!({ "content": content })),
                    Err(e) => Ok(json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
                }
            }
            m if m.starts_with("notifications/") => continue,
            _ => Err((-32601, format!("unknown method {method}"))),
        };
        let Some(id) = id else { continue };
        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, msg)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } }),
        };
        let _ = writeln!(out, "{resp}");
        let _ = out.flush();
    }
}

#[cfg(test)]
pub(crate) fn test_call(name: &str, args: Value, state: &mut Option<Box<dyn std::any::Any>>) -> Result<Vec<Value>, String> {
    let sv = state.get_or_insert_with(|| Box::new(Server { map: Map::default(), path: None, models: ModelCache::default(), textures: crate::texture::TexCache::default(), mounts: None, settings: Settings::default() }) as Box<dyn std::any::Any>);
    sv.downcast_mut::<Server>().unwrap().call(name, &args)
}

#[cfg(test)]
pub(crate) fn test_roof(kind: &str) -> Option<(Vec<Vec3>, Vec<Vec<u32>>)> {
    roof_mesh(kind, 0.0, 0.0, 20.0, 10.0, 10.0, 5.0, 0.5, true, -1.0)
}
