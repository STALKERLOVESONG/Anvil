//! The map for an outside playtest (the TRP plugin): what a player collides
//! with as upright boxes, in a glTF that TRP's `--level` reads (each node
//! an upright box about its mesh's bounds, its material's name the sound it
//! makes underfoot, `start anvil` where the player begins). The look goes
//! separately, as the whole textured scene (`blender_export::scene_glb_with`).
//!
//! Box walls keep their door and window holes (split around them), wedges
//! become small steps to walk up, stairs go step by step, and everything else
//! is the upright box about it.

use crate::map::{Kind, Map, Material, Object, Shape};
use crate::mesh::{world_mesh, ModelCache};
use glam::{Quat, Vec3};
use serde_json::{json, Value};

/// One upright box, in meters (glTF / Bevy axes: Y up).
#[derive(Clone, Debug, PartialEq)]
pub struct SolidBox {
    pub name: String,
    pub centre: Vec3,
    pub size: Vec3,
    /// Turn about the up axis (radians, right-handed).
    pub yaw: f32,
    pub surface: &'static str,
}

/// What a material sounds of underfoot (one of TRP's 20 surface words).
pub fn surface_of(m: Material) -> &'static str {
    match m {
        Material::Wood | Material::WoodPlanks => "Wood",
        Material::Metal | Material::DiamondPlate | Material::CorrodedMetal => "Metal",
        Material::Foil => "MetalThin",
        Material::Grass => "Grass",
        Material::Sand => "Soil",
        Material::Pebble | Material::Cobblestone => "Gravel",
        Material::Slate | Material::Granite | Material::Marble => "Slate",
        Material::Glass | Material::Ice => "Glass",
        Material::Plastic | Material::SmoothPlastic | Material::Neon | Material::Fabric => "Plastic",
        Material::Concrete | Material::Brick => "Concrete",
    }
}

/// Is the rotation a turn about the up axis only?
fn upright(o: &Object) -> bool {
    o.rot.x.abs() < 0.5 && o.rot.z.abs() < 0.5
}

/// Local box (min, max) of `o` as a world box.
fn local_box(o: &Object, mn: Vec3, mx: Vec3, k: f32, name: String) -> SolidBox {
    let c = o.to_world((mn + mx) * 0.5);
    SolidBox { name, centre: c * k, size: (mx - mn) * k, yaw: o.rot.y.to_radians(), surface: surface_of(o.material) }
}

/// The solid boxes of the map. `props`: props too (each the box about it).
pub fn solid_boxes(map: &Map, models: &mut ModelCache, k: f32, props: bool) -> Vec<SolidBox> {
    let mut out = Vec::new();
    for o in map.objects.iter().filter(|o| !o.hidden && o.collide) {
        let name = format!("anvil {} {}", o.id, o.name);
        match &o.kind {
            Kind::Light { .. } | Kind::Decal { .. } | Kind::Spawn { .. } | Kind::Prompt { .. } | Kind::Target {} | Kind::Teleport { .. } => continue,
            Kind::Prop { .. } if !props => continue,
            Kind::Brush { shape: Shape::Box, size, holes, .. } if upright(o) => {
                let pieces = if holes.is_empty() { vec![(-*size * 0.5, *size * 0.5)] } else { crate::csg::box_minus(*size, holes) };
                for (i, (mn, mx)) in pieces.into_iter().enumerate() {
                    out.push(local_box(o, mn, mx, k, format!("{name} {i}")));
                }
                continue;
            }
            Kind::Brush { shape: Shape::Stairs, size, steps, .. } if upright(o) => {
                for (i, (mn, mx)) in crate::mesh::stair_boxes(*steps).into_iter().enumerate() {
                    out.push(local_box(o, mn * *size, mx * *size, k, format!("{name} {i}")));
                }
                continue;
            }
            Kind::Brush { shape: Shape::Wedge, size, .. } if upright(o) => {
                // rises from -Z (bottom) to +Z (top): steps of about 15 cm
                let n = ((size.y * k / 0.15).ceil() as usize).clamp(2, 32);
                let h = *size * 0.5;
                for i in 0..n {
                    let (z0, z1) = (-h.z + size.z * i as f32 / n as f32, -h.z + size.z * (i + 1) as f32 / n as f32);
                    let top = -h.y + size.y * (i + 1) as f32 / n as f32;
                    out.push(local_box(o, Vec3::new(-h.x, -h.y, z0), Vec3::new(h.x, top, z1), k, format!("{name} {i}")));
                }
                continue;
            }
            _ => {}
        }
        // anything else: the upright box about it
        let wm = world_mesh(o, &map.textures, models);
        if wm.pos.is_empty() {
            continue;
        }
        out.push(SolidBox { name, centre: (wm.min + wm.max) * 0.5 * k, size: (wm.max - wm.min) * k, yaw: 0.0, surface: surface_of(o.material) });
    }
    out.retain(|b| b.size.min_element() > 1e-4);
    out
}

/// Where the player starts: the first spawn pad (feet on its top, facing
/// the way it faces), or the middle of the map if there's none.
pub fn start(map: &Map, k: f32) -> (Vec3, f32) {
    for o in map.objects.iter().filter(|o| !o.hidden) {
        if let Kind::Spawn { size } = o.kind {
            return (Vec3::new(o.pos.x, o.pos.y + size.y * 0.5, o.pos.z) * k, o.rot.y.to_radians());
        }
    }
    (Vec3::ZERO, 0.0)
}

/// The boxes as a .gltf (JSON with its small buffer inside): a unit cube
/// per surface material, one node per box, and `start anvil`.
pub fn collision_gltf(map: &Map, models: &mut ModelCache, k: f32, props: bool) -> Vec<u8> {
    let boxes = solid_boxes(map, models, k, props);
    // a unit cube: 8 corners, 12 triangles
    let corners: Vec<f32> = (0..8).flat_map(|i| [if i & 1 != 0 { 0.5 } else { -0.5 }, if i & 2 != 0 { 0.5 } else { -0.5 }, if i & 4 != 0 { 0.5 } else { -0.5 }]).collect();
    let tris: [u16; 36] = [0, 2, 1, 1, 2, 3, 4, 5, 6, 5, 7, 6, 0, 1, 4, 1, 5, 4, 2, 6, 3, 3, 6, 7, 0, 4, 2, 2, 4, 6, 1, 3, 5, 3, 7, 5];
    let mut bin: Vec<u8> = corners.iter().flat_map(|f| f.to_le_bytes()).collect();
    let pos_len = bin.len();
    bin.extend(tris.iter().flat_map(|i| i.to_le_bytes()));
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let uri = format!("data:application/octet-stream;base64,{}", crate::team::b64_encode(&bin));
    let mut surfaces: Vec<&'static str> = boxes.iter().map(|b| b.surface).collect();
    surfaces.sort_unstable();
    surfaces.dedup();
    let materials: Vec<Value> = surfaces.iter().map(|s| json!({ "name": s, "pbrMetallicRoughness": { "baseColorFactor": [0.55, 0.55, 0.55, 1.0] } })).collect();
    let meshes: Vec<Value> = surfaces.iter().enumerate().map(|(i, s)| json!({ "name": format!("cube {s}"), "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "material": i }] })).collect();
    let mut nodes: Vec<Value> = boxes
        .iter()
        .map(|b| {
            let q = Quat::from_rotation_y(b.yaw);
            json!({
                "name": b.name,
                "mesh": surfaces.iter().position(|s| *s == b.surface).unwrap_or(0),
                "translation": [b.centre.x, b.centre.y, b.centre.z],
                "rotation": [q.x, q.y, q.z, q.w],
                "scale": [b.size.x, b.size.y, b.size.z],
            })
        })
        .collect();
    // the start: an empty whose -Z is the way the player looks
    let (feet, yaw) = start(map, k);
    let q = Quat::from_rotation_y(-yaw);
    nodes.push(json!({ "name": "start anvil", "translation": [feet.x, feet.y, feet.z], "rotation": [q.x, q.y, q.z, q.w] }));
    let doc = json!({
        "asset": { "version": "2.0", "generator": "Anvil level editor (collision for TRP)" },
        "scene": 0,
        "scenes": [{ "name": map.name, "nodes": (0..nodes.len()).collect::<Vec<_>>() }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3", "min": [-0.5, -0.5, -0.5], "max": [0.5, 0.5, 0.5] },
            { "bufferView": 1, "componentType": 5123, "count": 36, "type": "SCALAR" }
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": pos_len, "target": 34962 },
            { "buffer": 0, "byteOffset": pos_len, "byteLength": tris.len() * 2, "target": 34963 }
        ],
        "buffers": [{ "byteLength": bin.len(), "uri": uri }],
    });
    serde_json::to_vec_pretty(&doc).unwrap_or_default()
}
