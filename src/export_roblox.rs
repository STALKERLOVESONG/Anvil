//! Roblox export. Brushes become native parts (Part / WedgePart / cylinder /
//! ball / stair models), boolean holes become a model of the remaining
//! pieces, editable meshes become wedge-pair triangles, lights become
//! invisible parts with a PointLight, spawns become SpawnLocations, decals
//! become thin parts with a Decal, textures become Texture faces or a
//! MaterialVariant (normal map / metal), folders become Models.
//!
//! Two formats are produced from the same instance tree:
//! * `.rbxmx` – drag into Studio, "Insert from File…", or upload via Open Cloud
//! * `.lua`   – paste into Studio's Command Bar

use crate::csg::box_minus;
use crate::map::{Kind, Map, Material, Object, Shape, TexParams, TextureDef};
use crate::mesh::{stair_boxes, ModelCache};
use crate::polymesh::triangulate;
use crate::settings::Settings;
use glam::{Mat3, Vec3};
use std::f32::consts::FRAC_PI_2;
use std::fmt::Write as _;
use std::path::Path;

pub enum Child {
    SpecialMesh { mesh_type: u32, mesh_id: Option<String>, scale: Vec3 },
    PointLight { range: f32, brightness: f32, color: [u8; 3] },
    StringValue { name: String, value: String },
    /// face: NormalId (0 Right, 1 Top, 2 Back, 3 Left, 4 Bottom, 5 Front)
    Texture { face: u32, id: String, su: f32, sv: f32, ou: f32, ov: f32, tint: [u8; 3] },
    Decal { face: u32, id: String, transparency: f32 },
    ProximityPrompt(crate::map::PromptSpec),
    Script { name: String, source: String },
}

pub struct Part {
    pub class: &'static str,
    pub name: String,
    pub size: Vec3,
    pub pos: Vec3,
    pub rot: Mat3,
    pub color: [u8; 3],
    pub material: Material,
    pub material_variant: Option<String>,
    /// PartType: 0 = Ball, 1 = Block, 2 = Cylinder
    pub shape: Option<u32>,
    pub transparency: f32,
    pub anchored: bool,
    pub collide: bool,
    pub children: Vec<Child>,
}

pub enum Node {
    Part(Part),
    Model { name: String, children: Vec<Node> },
    MaterialVariant { name: String, base: Material, color_id: String, normal_id: String, roughness_id: String, metalness_id: String, studs: f32 },
    /// A model uploaded through Open Cloud, inserted with InsertService (Luau only).
    Insert { name: String, asset_id: String, pos: Vec3, rot: Mat3, scale: f32 },
}

const MESH_SPHERE: u32 = 3;
const MESH_FILE: u32 = 5;
const NORMAL_IDS: [&str; 6] = ["Right", "Top", "Back", "Left", "Bottom", "Front"];

pub fn asset_url(id: &str) -> String {
    let t = id.trim();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        format!("rbxassetid://{t}")
    } else {
        t.to_string()
    }
}

pub fn variant_name(t: &TextureDef) -> String {
    let clean: String = t.name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("Anvil_{clean}")
}

fn variant_base(t: &TextureDef) -> Material {
    if t.metallic && !matches!(t.base_material, Material::Metal | Material::CorrodedMetal | Material::DiamondPlate | Material::Foil) {
        Material::Metal
    } else {
        t.base_material
    }
}

/// Splits a triangle into the two right-angle WedgeParts that fill it
/// (the classic Roblox "3D triangle" technique). Returns (size, center, rotation).
pub fn tri_wedges(a: Vec3, b: Vec3, c: Vec3, thick: f32) -> Vec<(Vec3, Vec3, Mat3)> {
    let (mut a, mut b, mut c) = (a, b, c);
    let (ab, ac, bc) = (b - a, c - a, c - b);
    let (abd, acd, bcd) = (ab.dot(ab), ac.dot(ac), bc.dot(bc));
    if abd > acd && abd > bcd {
        std::mem::swap(&mut c, &mut a);
    } else if acd > bcd && acd > abd {
        std::mem::swap(&mut a, &mut b);
    }
    let (ab, ac, bc) = (b - a, c - a, c - b);
    let right = ac.cross(ab);
    if right.length_squared() < 1e-12 || bc.length_squared() < 1e-12 {
        return Vec::new();
    }
    let right = right.normalize();
    let up = bc.cross(right).normalize();
    let back = bc.normalize();
    let height = ab.dot(up).abs();
    let mut out = Vec::new();
    let d1 = ab.dot(back).abs();
    if height > 1e-5 && d1 > 1e-5 {
        out.push((Vec3::new(thick, height, d1), (a + b) * 0.5, Mat3::from_cols(right, up, back)));
    }
    let d2 = ac.dot(back).abs();
    if height > 1e-5 && d2 > 1e-5 {
        out.push((Vec3::new(thick, height, d2), (a + c) * 0.5, Mat3::from_cols(-right, up, -back)));
    }
    out
}

struct Ctx<'a> {
    map: &'a Map,
    settings: &'a Settings,
    s: f32,
}

impl Ctx<'_> {
    fn part(&self, o: &Object, class: &'static str, size: Vec3, pos: Vec3, rot: Mat3) -> Part {
        let mut p = Part {
            class,
            name: o.name.clone(),
            size: size * self.s,
            pos: pos * self.s,
            rot,
            color: o.color,
            material: o.material,
            material_variant: None,
            shape: None,
            transparency: o.transparency,
            anchored: o.anchored,
            collide: o.collide,
            children: Vec::new(),
        };
        self.apply_texture(o, &mut p);
        p
    }

    /// Texture + settings that show on one face (face override or the object's).
    fn face_texture(&self, o: &Object, face: Option<usize>) -> (Option<&TextureDef>, TexParams) {
        match face.and_then(|f| o.face_tex.get(f)).and_then(|f| f.as_ref()) {
            Some(ft) => (self.map.texture(ft.texture), ft.params),
            None => (self.map.texture(o.texture), o.tex_params),
        }
    }

    fn apply_texture(&self, o: &Object, p: &mut Part) {
        self.apply_texture_on(o, p, None);
    }

    /// `mesh_face`: for wedge triangles of an editable mesh, the polygon face
    /// they came from (its texture goes on the wedge's two flat sides).
    fn apply_texture_on(&self, o: &Object, p: &mut Part, mesh_face: Option<usize>) {
        if matches!(o.kind, Kind::Decal { .. }) {
            return;
        }
        let s = self.s;
        // our box faces (+X -X +Y -Y +Z -Z) -> Roblox NormalId
        const NORMAL_ID: [u32; 6] = [0, 3, 1, 4, 2, 5];
        let per_face = mesh_face.is_none() && o.face_tex.iter().any(|f| f.is_some());
        let whole = self.map.texture(o.texture);
        // one texture everywhere with a normal map / metal: MaterialVariant
        if !per_face && mesh_face.is_none() {
            if let Some(t) = whole {
                if t.needs_material_variant() {
                    p.material = variant_base(t);
                    p.material_variant = Some(variant_name(t));
                    if o.tex_params.tint != [255, 255, 255] {
                        p.color = o.tex_params.tint; // MaterialVariants are tinted by the part color
                    }
                    return;
                }
            }
        }
        let faces: Vec<(u32, Option<usize>)> = match mesh_face {
            Some(f) => vec![(0, Some(f)), (3, Some(f))],
            None => (0..6).map(|d| (NORMAL_ID[d], Some(d))).collect(),
        };
        let mut missing_id = None;
        for (normal_id, key) in faces {
            let (def, params) = self.face_texture(o, key);
            let Some(t) = def else { continue };
            // a face with its own texture axes (from a Source map): Roblox
            // textures can't rotate or shear, so keep the tile size right and
            // let the offset start at the part's corner
            let axes = key.and_then(|f| o.face_tex.get(f)).and_then(|f| f.as_ref()).and_then(|f| f.uv);
            if t.roblox_image_id.trim().is_empty() {
                missing_id = Some(t.name.clone());
                continue;
            }
            p.children.push(Child::Texture {
                face: normal_id,
                id: asset_url(&t.roblox_image_id),
                su: t.scale.x * params.scale.x * s / axes.map(|a| a.u.length().max(1e-4)).unwrap_or(1.0),
                sv: t.scale.y * params.scale.y * s / axes.map(|a| a.v.length().max(1e-4)).unwrap_or(1.0),
                ou: if axes.is_some() { 0.0 } else { (t.offset.x + params.offset.x) * s },
                ov: if axes.is_some() { 0.0 } else { (t.offset.y + params.offset.y) * s },
                tint: params.tint,
            });
        }
        if let Some(name) = missing_id {
            p.children.push(Child::StringValue { name: "AnvilTexture".into(), value: name });
        }
    }

    fn object_nodes(&self, o: &Object, cache: &mut ModelCache) -> Vec<Node> {
        let rot = Mat3::from_quat(o.rotation());
        let s = self.s;
        let mut out = Vec::new();
        match &o.kind {
            Kind::Brush { shape, size, steps, holes } => match shape {
                Shape::Box if !holes.is_empty() => {
                    let children: Vec<Node> = box_minus(*size, holes)
                        .into_iter()
                        .enumerate()
                        .map(|(i, (mn, mx))| {
                            let c = (mn + mx) * 0.5;
                            let mut p = self.part(o, "Part", mx - mn, o.pos + rot * c, rot);
                            p.name = format!("Piece{}", i + 1);
                            p.shape = Some(1);
                            Node::Part(p)
                        })
                        .collect();
                    out.push(Node::Model { name: o.name.clone(), children });
                }
                Shape::Box => {
                    let mut p = self.part(o, "Part", *size, o.pos, rot);
                    p.shape = Some(1);
                    out.push(Node::Part(p));
                }
                Shape::Wedge => out.push(Node::Part(self.part(o, "WedgePart", *size, o.pos, rot))),
                Shape::Cylinder => {
                    // Roblox cylinders run along local X; ours run along Y.
                    let r = rot * Mat3::from_rotation_z(FRAC_PI_2);
                    let mut p = self.part(o, "Part", Vec3::new(size.y, size.x, size.z), o.pos, r);
                    p.shape = Some(2);
                    out.push(Node::Part(p));
                }
                Shape::Sphere => {
                    let uniform = (size.x - size.y).abs() < 1e-3 && (size.x - size.z).abs() < 1e-3;
                    let mut p = self.part(o, "Part", *size, o.pos, rot);
                    if uniform {
                        p.shape = Some(0);
                    } else {
                        p.shape = Some(1);
                        p.children.push(Child::SpecialMesh { mesh_type: MESH_SPHERE, mesh_id: None, scale: Vec3::ONE });
                    }
                    out.push(Node::Part(p));
                }
                Shape::Stairs => {
                    let children = stair_boxes(*steps)
                        .into_iter()
                        .enumerate()
                        .map(|(i, (mn, mx))| {
                            let c = (mn + mx) * 0.5 * *size;
                            let mut p = self.part(o, "Part", (mx - mn) * *size, o.pos + rot * c, rot);
                            p.name = format!("Step{}", i + 1);
                            p.shape = Some(1);
                            Node::Part(p)
                        })
                        .collect();
                    out.push(Node::Model { name: o.name.clone(), children });
                }
            },
            Kind::Mesh { verts, faces } => {
                let m = o.matrix();
                let world: Vec<Vec3> = verts.iter().map(|v| m.transform_point3(*v)).collect();
                let thick = self.settings.mesh_wedge_thickness.max(0.001);
                let mut children = Vec::new();
                for (fi, f) in faces.iter().enumerate() {
                    if f.iter().any(|&i| i as usize >= verts.len()) {
                        continue;
                    }
                    for t in triangulate(verts, f) {
                        let (a, b, c) = (world[t[0] as usize], world[t[1] as usize], world[t[2] as usize]);
                        for (size, pos, r) in tri_wedges(a, b, c, thick) {
                            let mut p = self.part(o, "WedgePart", size, pos, r);
                            if o.face_tex.get(fi).is_some_and(|f| f.is_some()) {
                                // this face has its own texture: replace the object's
                                p.children.retain(|c| !matches!(c, Child::Texture { .. } | Child::StringValue { .. }));
                                p.material_variant = None;
                                p.material = o.material;
                                self.apply_texture_on(o, &mut p, Some(fi));
                            }
                            p.name = "Tri".into();
                            children.push(Node::Part(p));
                        }
                    }
                }
                out.push(Node::Model { name: o.name.clone(), children });
            }
            Kind::Prop { model, scale, roblox_mesh_id } => {
                let entry = self.settings.model_library.iter().find(|m| m.path == *model);
                let mesh_id = if roblox_mesh_id.trim().is_empty() {
                    entry.map(|m| m.roblox_mesh_id.clone()).unwrap_or_default()
                } else {
                    roblox_mesh_id.clone()
                };
                let model_id = entry.map(|m| m.roblox_model_id.clone()).unwrap_or_default();
                let (mn, mx) = cache.get(model).map(|m| m.bounds()).unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0)));
                let center = (mn + mx) * 0.5 * *scale;
                let size = ((mx - mn) * *scale).abs().max(Vec3::splat(0.05));
                if mesh_id.trim().is_empty() && !model_id.trim().is_empty() {
                    out.push(Node::Insert {
                        name: o.name.clone(),
                        asset_id: model_id.trim().to_string(),
                        pos: (o.pos + rot * center) * s,
                        rot,
                        scale: scale.x,
                    });
                } else {
                    let mut p = self.part(o, "Part", size, o.pos + rot * center, rot);
                    p.shape = Some(1);
                    if mesh_id.trim().is_empty() {
                        p.transparency = p.transparency.max(0.5);
                        let file = Path::new(model).file_name().and_then(|f| f.to_str()).unwrap_or(model).to_string();
                        p.children.push(Child::StringValue { name: "SourceModel".into(), value: file });
                    } else {
                        p.children.push(Child::SpecialMesh { mesh_type: MESH_FILE, mesh_id: Some(asset_url(&mesh_id)), scale: *scale * s });
                    }
                    out.push(Node::Part(p));
                }
            }
            Kind::Decal { size } => {
                let mut p = self.part(o, "Part", Vec3::new(size.x, size.y, 0.05 / s.max(1e-4)), o.pos, rot);
                p.shape = Some(1);
                p.transparency = 1.0;
                p.collide = false;
                match self.map.texture(o.texture) {
                    Some(t) if !t.roblox_image_id.trim().is_empty() => {
                        // our decal faces local +Z, which is Roblox's Back face
                        p.children.push(Child::Decal { face: 2, id: asset_url(&t.roblox_image_id), transparency: o.transparency });
                    }
                    Some(t) => p.children.push(Child::StringValue { name: "AnvilTexture".into(), value: t.name.clone() }),
                    None => {}
                }
                out.push(Node::Part(p));
            }
            Kind::Light { range, brightness } => {
                let mut p = self.part(o, "Part", Vec3::splat(1.0), o.pos, rot);
                p.shape = Some(1);
                p.transparency = 1.0;
                p.collide = false;
                p.children.clear();
                p.children.push(Child::PointLight { range: (range * s).min(60.0), brightness: *brightness, color: o.color });
                out.push(Node::Part(p));
            }
            Kind::Prompt { prompt } => {
                let mut p = self.part(o, "Part", Vec3::ONE, o.pos, rot);
                p.transparency = 1.0;
                p.collide = false;
                p.anchored = true;
                p.children.clear();
                p.children.push(Child::ProximityPrompt(prompt.clone()));
                out.push(Node::Part(p));
            }
            Kind::Teleport { size, touch, dest, cooldown, exit_offset, prompt } => {
                let mut p = self.part(o, "Part", *size, o.pos, rot);
                p.anchored = true;
                if !*touch {
                    p.children.push(Child::ProximityPrompt(prompt.clone()));
                }
                match dest.and_then(|d| self.map.get(d)) {
                    Some(t) => {
                        let cf = lua_cframe((t.pos + *exit_offset) * s, Mat3::from_quat(t.rotation()));
                        p.children.push(Child::Script { name: "Teleport".into(), source: teleport_script(&cf, *cooldown, *touch, &t.name) });
                    }
                    None => p.children.push(Child::StringValue { name: "AnvilNote".into(), value: "This teleport has no destination yet".into() }),
                }
                out.push(Node::Part(p));
            }
            Kind::Target {} => {
                let mut p = self.part(o, "Part", Vec3::ONE, o.pos, rot);
                p.transparency = 1.0;
                p.collide = false;
                p.anchored = true;
                p.children.clear();
                out.push(Node::Part(p));
            }
            Kind::Spawn { size } => {
                // invisible in the game: players just appear here
                let mut p = self.part(o, "SpawnLocation", *size, o.pos, rot);
                p.shape = Some(1);
                p.transparency = 1.0;
                p.children.clear();
                out.push(Node::Part(p));
            }
        }
        out
    }

    fn tree(&self, parent: Option<u64>, cache: &mut ModelCache, depth: usize) -> Vec<Node> {
        let mut out = Vec::new();
        if depth < 32 {
            for f in self.map.folders.iter().filter(|f| f.parent == parent) {
                let children = self.tree(Some(f.id), cache, depth + 1);
                out.push(Node::Model { name: f.name.clone(), children });
            }
        }
        for o in self.map.objects.iter().filter(|o| !o.hidden && self.map.folder_of(o) == parent) {
            out.extend(self.object_nodes(o, cache));
        }
        out
    }
}

pub fn build_nodes(map: &Map, cache: &mut ModelCache, settings: &Settings) -> Vec<Node> {
    let ctx = Ctx { map, settings, s: settings.roblox_scale };
    let mut out = Vec::new();
    // MaterialVariants for textures with normal maps / metal
    let mut seen = std::collections::HashSet::new();
    for o in map.objects.iter().filter(|o| !o.hidden) {
        if let Some(t) = map.texture(o.texture) {
            if t.needs_material_variant() && seen.insert(t.id) && !matches!(o.kind, Kind::Decal { .. }) {
                out.push(Node::MaterialVariant {
                    name: variant_name(t),
                    base: variant_base(t),
                    color_id: t.roblox_image_id.clone(),
                    normal_id: t.roblox_normal_id.clone(),
                    roughness_id: t.roblox_roughness_id.clone(),
                    metalness_id: t.roblox_metalness_id.clone(),
                    studs: ((t.scale.x + t.scale.y) * 0.5 * settings.roblox_scale).max(0.1),
                });
            }
        }
    }
    out.extend(ctx.tree(None, cache, 0));
    out
}

/// Roblox instances for one object (for live sync).
pub fn object_nodes_for(map: &Map, o: &Object, cache: &mut ModelCache, settings: &Settings) -> Vec<Node> {
    let ctx = Ctx { map, settings, s: settings.roblox_scale };
    ctx.object_nodes(o, cache)
}

fn cf_json(p: Vec3, r: Mat3) -> serde_json::Value {
    let m = |row: usize, col: usize| r.col(col)[row];
    serde_json::json!([p.x, p.y, p.z, m(0, 0), m(0, 1), m(0, 2), m(1, 0), m(1, 1), m(1, 2), m(2, 0), m(2, 1), m(2, 2)])
}

/// An instance tree as plain JSON (what the live-sync Studio plugin builds).
pub fn node_json(n: &Node) -> serde_json::Value {
    use serde_json::json;
    match n {
        Node::Model { name, children } => json!({"t": "Model", "name": name, "children": children.iter().map(node_json).collect::<Vec<_>>()}),
        Node::Part(p) => {
            let kids: Vec<serde_json::Value> = p
                .children
                .iter()
                .map(|c| match c {
                    Child::SpecialMesh { mesh_type, mesh_id, scale } => json!({"t": "SpecialMesh", "type": if *mesh_type == MESH_FILE { "FileMesh" } else { "Sphere" }, "id": mesh_id, "scale": [scale.x, scale.y, scale.z]}),
                    Child::PointLight { range, brightness, color } => json!({"t": "PointLight", "range": range, "brightness": brightness, "color": color}),
                    Child::StringValue { name, value } => json!({"t": "StringValue", "name": name, "value": value}),
                    Child::Texture { face, id, su, sv, ou, ov, tint } => json!({"t": "Texture", "face": NORMAL_IDS[*face as usize % 6], "id": id, "su": su, "sv": sv, "ou": ou, "ov": ov, "color": tint}),
                    Child::Decal { face, id, transparency } => json!({"t": "Decal", "face": NORMAL_IDS[*face as usize % 6], "id": id, "transparency": transparency}),
                    Child::ProximityPrompt(pp) => json!({"t": "ProximityPrompt", "action": pp.action_text, "object": pp.object_text, "key": safe_key(&pp.key, "E"), "pad": safe_key(&pp.gamepad, "ButtonX"), "hold": pp.hold, "distance": pp.distance, "los": pp.line_of_sight, "enabled": pp.enabled, "custom": pp.custom_style, "offset": [pp.ui_offset.x, pp.ui_offset.y]}),
                    Child::Script { name, source } => json!({"t": "Script", "name": name, "source": source}),
                })
                .collect();
            json!({
                "t": "Part", "class": p.class, "name": p.name, "size": [p.size.x, p.size.y, p.size.z], "cf": cf_json(p.pos, p.rot),
                "color": p.color, "material": p.material.label(), "variant": p.material_variant, "shape": p.shape.map(|s| match s { 0 => "Ball", 2 => "Cylinder", _ => "Block" }),
                "transparency": p.transparency, "anchored": p.anchored, "collide": p.collide, "children": kids
            })
        }
        // MaterialVariants live in MaterialService and uploaded models need InsertService: not mirrored live
        Node::MaterialVariant { .. } | Node::Insert { .. } => serde_json::Value::Null,
    }
}

pub fn count_parts(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Part(_) | Node::Insert { .. } => 1,
            Node::Model { children, .. } => count_parts(children),
            Node::MaterialVariant { .. } => 0,
        })
        .sum()
}

// ---------------------------------------------------------------------------
// .rbxmx
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

struct Xml {
    out: String,
    refs: u32,
}

impl Xml {
    fn open(&mut self, class: &str) {
        self.refs += 1;
        let _ = writeln!(self.out, "<Item class=\"{class}\" referent=\"RBX{}\">\n<Properties>", self.refs);
    }
    fn end_props(&mut self) {
        self.out.push_str("</Properties>\n");
    }
    fn end(&mut self) {
        self.out.push_str("</Item>\n");
    }
    fn string(&mut self, n: &str, v: &str) {
        let _ = writeln!(self.out, "<string name=\"{n}\">{}</string>", esc(v));
    }
    fn boolean(&mut self, n: &str, v: bool) {
        let _ = writeln!(self.out, "<bool name=\"{n}\">{v}</bool>");
    }
    fn float(&mut self, n: &str, v: f32) {
        let _ = writeln!(self.out, "<float name=\"{n}\">{v}</float>");
    }
    fn token(&mut self, n: &str, v: u32) {
        let _ = writeln!(self.out, "<token name=\"{n}\">{v}</token>");
    }
    fn vec3(&mut self, n: &str, v: Vec3) {
        let _ = writeln!(self.out, "<Vector3 name=\"{n}\"><X>{}</X><Y>{}</Y><Z>{}</Z></Vector3>", v.x, v.y, v.z);
    }
    fn cframe(&mut self, n: &str, p: Vec3, r: Mat3) {
        let m = |row: usize, col: usize| r.col(col)[row];
        let _ = writeln!(
            self.out,
            "<CoordinateFrame name=\"{n}\"><X>{}</X><Y>{}</Y><Z>{}</Z>\
             <R00>{}</R00><R01>{}</R01><R02>{}</R02>\
             <R10>{}</R10><R11>{}</R11><R12>{}</R12>\
             <R20>{}</R20><R21>{}</R21><R22>{}</R22></CoordinateFrame>",
            p.x,
            p.y,
            p.z,
            m(0, 0),
            m(0, 1),
            m(0, 2),
            m(1, 0),
            m(1, 1),
            m(1, 2),
            m(2, 0),
            m(2, 1),
            m(2, 2)
        );
    }
    fn color3u8(&mut self, n: &str, c: [u8; 3]) {
        let v: u32 = 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        let _ = writeln!(self.out, "<Color3uint8 name=\"{n}\">{v}</Color3uint8>");
    }
    fn color3(&mut self, n: &str, c: [u8; 3]) {
        let _ = writeln!(
            self.out,
            "<Color3 name=\"{n}\"><R>{}</R><G>{}</G><B>{}</B></Color3>",
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0
        );
    }
    fn content(&mut self, n: &str, url: Option<&str>) {
        match url.filter(|u| !u.trim().is_empty()) {
            Some(u) => {
                let _ = writeln!(self.out, "<Content name=\"{n}\"><url>{}</url></Content>", esc(u));
            }
            None => {
                let _ = writeln!(self.out, "<Content name=\"{n}\"><null></null></Content>");
            }
        }
    }

    fn container(&mut self, class: &str, name: &str, children: &[Node]) {
        self.open(class);
        self.string("Name", name);
        self.end_props();
        for c in children {
            self.node(c);
        }
        self.end();
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::Model { name, children } => self.container("Model", name, children),
            Node::Part(p) => self.part(p),
            Node::MaterialVariant { name, base, color_id, normal_id, roughness_id, metalness_id, studs } => {
                self.open("MaterialVariant");
                self.string("Name", name);
                self.token("BaseMaterial", base.roblox_token());
                self.content("ColorMap", Some(&asset_url(color_id)));
                self.content("NormalMap", Some(&asset_url(normal_id)));
                self.content("RoughnessMap", Some(&asset_url(roughness_id)));
                self.content("MetalnessMap", Some(&asset_url(metalness_id)));
                self.float("StudsPerTile", *studs);
                self.end_props();
                self.end();
            }
            Node::Insert { name, asset_id, pos, rot, .. } => {
                // .rbxmx can't load assets; leave a marker the Luau script / user can replace
                self.open("Part");
                self.string("Name", name);
                self.boolean("Anchored", true);
                self.boolean("CanCollide", false);
                self.cframe("CFrame", *pos, *rot);
                self.float("Transparency", 0.5);
                self.vec3("size", Vec3::splat(1.0));
                self.end_props();
                self.open("StringValue");
                self.string("Name", "InsertAssetId");
                self.string("Value", asset_id);
                self.end_props();
                self.end();
                self.end();
            }
        }
    }

    fn part(&mut self, p: &Part) {
        self.open(p.class);
        self.string("Name", &p.name);
        self.boolean("Anchored", p.anchored);
        self.boolean("CanCollide", p.collide);
        self.cframe("CFrame", p.pos, p.rot);
        self.color3u8("Color3uint8", p.color);
        self.token("Material", p.material.roblox_token());
        if let Some(v) = &p.material_variant {
            self.string("MaterialVariantSerialized", v);
        }
        self.float("Transparency", p.transparency);
        self.vec3("size", p.size);
        if let Some(sh) = p.shape {
            self.token("shape", sh);
        }
        for face in ["TopSurface", "BottomSurface", "FrontSurface", "BackSurface", "LeftSurface", "RightSurface"] {
            self.token(face, 0);
        }
        self.end_props();
        for c in &p.children {
            match c {
                Child::SpecialMesh { mesh_type, mesh_id, scale } => {
                    self.open("SpecialMesh");
                    self.string("Name", "Mesh");
                    self.token("MeshType", *mesh_type);
                    self.content("MeshId", mesh_id.as_deref());
                    self.vec3("Scale", *scale);
                    self.end_props();
                    self.end();
                }
                Child::PointLight { range, brightness, color } => {
                    self.open("PointLight");
                    self.string("Name", "PointLight");
                    self.float("Range", *range);
                    self.float("Brightness", *brightness);
                    self.color3("Color", *color);
                    self.boolean("Shadows", true);
                    self.end_props();
                    self.end();
                }
                Child::StringValue { name, value } => {
                    self.open("StringValue");
                    self.string("Name", name);
                    self.string("Value", value);
                    self.end_props();
                    self.end();
                }
                Child::Texture { face, id, su, sv, ou, ov, tint } => {
                    self.open("Texture");
                    self.string("Name", "Texture");
                    self.token("Face", *face);
                    self.content("Texture", Some(id));
                    self.color3("Color3", *tint);
                    self.float("StudsPerTileU", *su);
                    self.float("StudsPerTileV", *sv);
                    self.float("OffsetStudsU", *ou);
                    self.float("OffsetStudsV", *ov);
                    self.end_props();
                    self.end();
                }
                Child::ProximityPrompt(pp) => {
                    self.open("ProximityPrompt");
                    self.string("Name", "ProximityPrompt");
                    self.string("ActionText", &pp.action_text);
                    self.string("ObjectText", &pp.object_text);
                    self.token("KeyboardKeyCode", crate::entities::keycode_value(&safe_key(&pp.key, "E")));
                    self.token("GamepadKeyCode", crate::entities::keycode_value(&safe_key(&pp.gamepad, "ButtonX")));
                    self.float("HoldDuration", pp.hold);
                    self.float("MaxActivationDistance", pp.distance);
                    self.boolean("RequiresLineOfSight", pp.line_of_sight);
                    self.boolean("Enabled", pp.enabled);
                    self.token("Style", if pp.custom_style { 1 } else { 0 });
                    let _ = writeln!(self.out, "<Vector2 name=\"UIOffset\"><X>{}</X><Y>{}</Y></Vector2>", pp.ui_offset.x, pp.ui_offset.y);
                    self.end_props();
                    self.end();
                }
                Child::Script { name, source } => {
                    self.open("Script");
                    self.string("Name", name);
                    let _ = writeln!(self.out, "<ProtectedString name=\"Source\"><![CDATA[{}]]></ProtectedString>", source.replace("]]>", "]] >"));
                    self.end_props();
                    self.end();
                }
                Child::Decal { face, id, transparency } => {
                    self.open("Decal");
                    self.string("Name", "Decal");
                    self.token("Face", *face);
                    self.content("Texture", Some(id));
                    self.float("Transparency", *transparency);
                    self.end_props();
                    self.end();
                }
            }
        }
        self.end();
    }
}

pub fn to_rbxmx(nodes: &[Node], root_name: &str) -> String {
    let mut w = Xml { out: String::new(), refs: 0 };
    w.out.push_str(
        "<roblox xmlns:xmime=\"http://www.w3.org/2005/05/xmlmime\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:noNamespaceSchemaLocation=\"http://www.roblox.com/roblox.xsd\" version=\"4\">\n",
    );
    // MaterialVariants only work inside MaterialService: group them for the user to move.
    let (variants, rest): (Vec<&Node>, Vec<&Node>) = nodes.iter().partition(|n| matches!(n, Node::MaterialVariant { .. }));
    w.open("Model");
    w.string("Name", root_name);
    w.end_props();
    if !variants.is_empty() {
        w.open("Folder");
        w.string("Name", "AnvilMaterials (move into MaterialService)");
        w.end_props();
        for v in variants {
            w.node(v);
        }
        w.end();
    }
    for n in rest {
        w.node(n);
    }
    w.end();
    w.out.push_str("</roblox>\n");
    w.out
}

// ---------------------------------------------------------------------------
// Luau (command bar script)
// ---------------------------------------------------------------------------

/// A key / button name that's safe to put after `Enum.KeyCode.` (letters and digits only).
fn safe_key(k: &str, fallback: &str) -> String {
    let k = k.trim();
    if !k.is_empty() && k.len() <= 24 && k.chars().all(|c| c.is_ascii_alphanumeric()) {
        k.to_string()
    } else {
        fallback.to_string()
    }
}

/// Script that moves the player to `cf` on touch or when the prompt fires.
/// A shared cooldown on the character stops two-way pads ping-ponging.
pub fn teleport_script(cf: &str, cooldown: f32, touch: bool, dest_name: &str) -> String {
    let dest: String = dest_name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ' ').collect();
    let head = format!(
        "-- Teleport made by Anvil. Sends players to {dest}.\n\
         -- If you move the destination, update TARGET (or re-export).\n\
         local TARGET = {cf}\n\
         local COOLDOWN = {cooldown}\n\n\
         local function send(char)\n\
         \x20   if not char then return end\n\
         \x20   local root = char:FindFirstChild(\"HumanoidRootPart\")\n\
         \x20   if not root or not char:FindFirstChildOfClass(\"Humanoid\") then return end\n\
         \x20   local now = os.clock()\n\
         \x20   if now - (char:GetAttribute(\"AnvilTeleportedAt\") or -math.huge) < COOLDOWN then return end\n\
         \x20   char:SetAttribute(\"AnvilTeleportedAt\", now)\n\
         \x20   char:PivotTo(TARGET * CFrame.new(0, 3, 0))\n\
         end\n\n"
    );
    let hook = if touch {
        "script.Parent.Touched:Connect(function(hit)\n    send(hit.Parent)\nend)\n"
    } else {
        "local prompt = script.Parent:WaitForChild(\"ProximityPrompt\")\nprompt.Triggered:Connect(function(player)\n    send(player.Character)\nend)\n"
    };
    head + hook
}

fn lua_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 32 => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn lua_cframe(p: Vec3, rot: Mat3) -> String {
    let r = |row: usize, col: usize| rot.col(col)[row];
    format!(
        "CFrame.new({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
        p.x,
        p.y,
        p.z,
        r(0, 0),
        r(0, 1),
        r(0, 2),
        r(1, 0),
        r(1, 1),
        r(1, 2),
        r(2, 0),
        r(2, 1),
        r(2, 2)
    )
}

fn lua_node(out: &mut String, n: &Node, parent: &str, depth: usize) {
    match n {
        Node::Model { name, children } => {
            let v = format!("m{depth}");
            let _ = writeln!(out, "do\nlocal {v} = Instance.new(\"Model\")\n{v}.Name = {}", lua_str(name));
            for c in children {
                lua_node(out, c, &v, depth + 1);
            }
            let _ = writeln!(out, "{v}.Parent = {parent}\nend");
        }
        Node::MaterialVariant { name, base, color_id, normal_id, roughness_id, metalness_id, studs } => {
            let _ = writeln!(
                out,
                "do\nlocal ms = game:GetService(\"MaterialService\")\nif not ms:FindFirstChild({n}) then\n\
                 local mv = Instance.new(\"MaterialVariant\")\nmv.Name = {n}\nmv.BaseMaterial = Enum.Material.{}\nmv.StudsPerTile = {studs}",
                base.label(),
                n = lua_str(name)
            );
            if !color_id.trim().is_empty() {
                let _ = writeln!(out, "mv.ColorMap = {}", lua_str(&asset_url(color_id)));
            }
            if !normal_id.trim().is_empty() {
                let _ = writeln!(out, "mv.NormalMap = {}", lua_str(&asset_url(normal_id)));
            }
            if !roughness_id.trim().is_empty() {
                let _ = writeln!(out, "mv.RoughnessMap = {}", lua_str(&asset_url(roughness_id)));
            }
            if !metalness_id.trim().is_empty() {
                let _ = writeln!(out, "mv.MetalnessMap = {}", lua_str(&asset_url(metalness_id)));
            }
            out.push_str("mv.Parent = ms\nend\nend\n");
        }
        Node::Insert { name, asset_id, pos, rot, scale } => {
            let _ = writeln!(
                out,
                "do\nlocal ok, c = pcall(function() return game:GetService(\"InsertService\"):LoadAsset({id}) end)\n\
                 if ok and c then\nlocal m = c:GetChildren()[1]\nif m then\nm.Name = {name}\n\
                 if m:IsA(\"Model\") then\nlocal bb = m:GetBoundingBox()\nm.WorldPivot = bb\n\
                 if {scale} ~= 1 then pcall(function() m:ScaleTo(m:GetScale() * {scale}) end) end\nm:PivotTo({cf})\n\
                 elseif m:IsA(\"BasePart\") then\nm.CFrame = {cf}\nend\nm.Parent = {parent}\nend\nc:Destroy()\n\
                 else warn(\"Anvil: could not insert asset {id}\") end\nend",
                id = asset_id,
                name = lua_str(name),
                scale = scale,
                cf = lua_cframe(*pos, *rot),
            );
        }
        Node::Part(p) => {
            let _ = writeln!(out, "do\nlocal p = Instance.new(\"{}\")\np.Name = {}", p.class, lua_str(&p.name));
            if let Some(sh) = p.shape {
                let t = match sh {
                    0 => "Ball",
                    2 => "Cylinder",
                    _ => "Block",
                };
                let _ = writeln!(out, "p.Shape = Enum.PartType.{t}");
            }
            let _ = writeln!(
                out,
                "p.Anchored = {}\np.CanCollide = {}\np.Size = Vector3.new({}, {}, {})\np.CFrame = {}\n\
                 p.Color = Color3.fromRGB({}, {}, {})\np.Material = Enum.Material.{}\np.Transparency = {}\n\
                 p.TopSurface = Enum.SurfaceType.Smooth\np.BottomSurface = Enum.SurfaceType.Smooth",
                p.anchored,
                p.collide,
                p.size.x,
                p.size.y,
                p.size.z,
                lua_cframe(p.pos, p.rot),
                p.color[0],
                p.color[1],
                p.color[2],
                p.material.label(),
                p.transparency
            );
            if let Some(v) = &p.material_variant {
                let _ = writeln!(out, "p.MaterialVariant = {}", lua_str(v));
            }
            for c in &p.children {
                match c {
                    Child::SpecialMesh { mesh_type, mesh_id, scale } => {
                        let t = if *mesh_type == MESH_FILE { "FileMesh" } else { "Sphere" };
                        let _ = writeln!(out, "local sm = Instance.new(\"SpecialMesh\")\nsm.MeshType = Enum.MeshType.{t}");
                        if let Some(id) = mesh_id {
                            let _ = writeln!(out, "sm.MeshId = {}", lua_str(id));
                        }
                        let _ = writeln!(out, "sm.Scale = Vector3.new({}, {}, {})\nsm.Parent = p", scale.x, scale.y, scale.z);
                    }
                    Child::PointLight { range, brightness, color } => {
                        let _ = writeln!(
                            out,
                            "local l = Instance.new(\"PointLight\")\nl.Range = {range}\nl.Brightness = {brightness}\n\
                             l.Color = Color3.fromRGB({}, {}, {})\nl.Shadows = true\nl.Parent = p",
                            color[0], color[1], color[2]
                        );
                    }
                    Child::StringValue { name, value } => {
                        let _ = writeln!(
                            out,
                            "local sv = Instance.new(\"StringValue\")\nsv.Name = {}\nsv.Value = {}\nsv.Parent = p",
                            lua_str(name),
                            lua_str(value)
                        );
                    }
                    Child::Texture { face, id, su, sv, ou, ov, tint } => {
                        let _ = writeln!(
                            out,
                            "local tx = Instance.new(\"Texture\")\ntx.Face = Enum.NormalId.{}\ntx.Texture = {}\n\
                             tx.StudsPerTileU = {su}\ntx.StudsPerTileV = {sv}\ntx.OffsetStudsU = {ou}\ntx.OffsetStudsV = {ov}\n\
                             tx.Color3 = Color3.fromRGB({}, {}, {})\ntx.Parent = p",
                            NORMAL_IDS[*face as usize % 6],
                            lua_str(id),
                            tint[0],
                            tint[1],
                            tint[2]
                        );
                    }
                    Child::ProximityPrompt(pp) => {
                        let _ = writeln!(
                            out,
                            "local pp = Instance.new(\"ProximityPrompt\")\npp.ActionText = {}\npp.ObjectText = {}\n\
                             pp.KeyboardKeyCode = Enum.KeyCode.{}\npp.GamepadKeyCode = Enum.KeyCode.{}\n\
                             pp.HoldDuration = {}\npp.MaxActivationDistance = {}\npp.RequiresLineOfSight = {}\npp.Enabled = {}\n\
                             pp.Style = Enum.ProximityPromptStyle.{}\npp.UIOffset = Vector2.new({}, {})\npp.Parent = p",
                            lua_str(&pp.action_text),
                            lua_str(&pp.object_text),
                            safe_key(&pp.key, "E"),
                            safe_key(&pp.gamepad, "ButtonX"),
                            pp.hold,
                            pp.distance,
                            pp.line_of_sight,
                            pp.enabled,
                            if pp.custom_style { "Custom" } else { "Default" },
                            pp.ui_offset.x,
                            pp.ui_offset.y
                        );
                    }
                    Child::Script { name, source } => {
                        let _ = writeln!(out, "local sc = Instance.new(\"Script\")\nsc.Name = {}\nsc.Source = {}\nsc.Parent = p", lua_str(name), lua_str(source));
                    }
                    Child::Decal { face, id, transparency } => {
                        let _ = writeln!(
                            out,
                            "local dc = Instance.new(\"Decal\")\ndc.Face = Enum.NormalId.{}\ndc.Texture = {}\ndc.Transparency = {transparency}\ndc.Parent = p",
                            NORMAL_IDS[*face as usize % 6],
                            lua_str(id)
                        );
                    }
                }
            }
            let _ = writeln!(out, "p.Parent = {parent}\nend");
        }
    }
}

pub fn to_luau(nodes: &[Node], root_name: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Generated by Anvil. Paste into Roblox Studio's Command Bar (View > Command Bar) and press Enter.\n\
         local CHS = game:GetService(\"ChangeHistoryService\")\n\
         local okRec, rec = pcall(function() return CHS:TryBeginRecording(\"Anvil import\") end)\n\
         local root = Instance.new(\"Model\")\nroot.Name = {}",
        lua_str(root_name)
    );
    for n in nodes {
        lua_node(&mut out, n, "root", 1);
    }
    out.push_str(
        "root.Parent = workspace\n\
         pcall(function() game:GetService(\"Selection\"):Set({root}) end)\n\
         if okRec and rec then CHS:FinishRecording(rec, Enum.FinishRecordingOperation.Commit) end\n\
         print(\"Anvil: imported \" .. #root:GetDescendants() .. \" instances into \" .. root:GetFullName())\n",
    );
    out
}

pub fn export_rbxmx(map: &Map, cache: &mut ModelCache, settings: &Settings, path: &Path) -> Result<String, String> {
    let nodes = build_nodes(map, cache, settings);
    std::fs::write(path, to_rbxmx(&nodes, &map.name)).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(format!("Roblox: wrote {} parts to {}", count_parts(&nodes), path.display()))
}

pub fn export_luau(map: &Map, cache: &mut ModelCache, settings: &Settings, path: &Path) -> Result<String, String> {
    let nodes = build_nodes(map, cache, settings);
    std::fs::write(path, to_luau(&nodes, &map.name)).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(format!("Roblox: wrote command bar script with {} parts to {}", count_parts(&nodes), path.display()))
}

pub fn luau_string(map: &Map, cache: &mut ModelCache, settings: &Settings) -> String {
    to_luau(&build_nodes(map, cache, settings), &map.name)
}
