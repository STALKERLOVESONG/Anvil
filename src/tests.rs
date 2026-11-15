use crate::csg::box_minus;
use crate::export_roblox;
use crate::export_unity;
use crate::map::{Hole, Kind, Map, Material, Object, Shape, TextureDef};
use crate::mesh::{primitive, unit_wedge, ModelCache, TriMesh};
use crate::playtest::{Play, PlayInput};
use crate::plugins::{Hit, PluginHost};
use crate::polymesh;
use crate::settings::Settings;
use glam::{Mat3, Vec2, Vec3};
use std::collections::HashMap;

fn out_dir(sub: &str) -> std::path::PathBuf {
    let d = std::env::var("ANVIL_TEST_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("anvil_tests")).join(sub);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every non-degenerate triangle of a convex solid must face away from its interior.
fn assert_outward(name: &str, m: &TriMesh) {
    // vertex centroid: strictly inside every convex primitive (unlike the
    // bounding-box center, which lies on a wedge's slope)
    let inside = m.pos.iter().copied().sum::<Vec3>() / m.pos.len() as f32;
    for t in &m.tris {
        let (a, b, c) = (m.pos[t[0] as usize], m.pos[t[1] as usize], m.pos[t[2] as usize]);
        let n = (b - a).cross(c - a);
        if n.length() < 1e-7 {
            continue;
        }
        let centroid = (a + b + c) / 3.0;
        assert!(n.dot(centroid - inside) > 0.0, "{name}: inward-facing triangle {t:?}");
    }
}

#[test]
fn primitives_face_outward() {
    for shape in [Shape::Box, Shape::Wedge, Shape::Cylinder, Shape::Sphere] {
        let m = primitive(shape, Vec3::new(4.0, 2.0, 6.0), 1);
        assert!(!m.tris.is_empty());
        assert_outward(shape.label(), &m);
    }
    assert_outward("Stairs", &primitive(Shape::Stairs, Vec3::ONE, 1));
}

fn sample_map() -> Map {
    let mut map = Map::default();
    map.name = "Sample".into();
    for (i, shape) in Shape::ALL.into_iter().enumerate() {
        let mut o = Object::new(
            Kind::Brush { shape, size: Vec3::new(4.0, 6.0, 8.0), steps: 4, holes: Vec::new() },
            Vec3::new(i as f32 * 10.0, 3.0, 0.0),
            [200, 100, 50],
            Material::Brick,
        );
        o.rot.y = 30.0;
        map.add(o);
    }
    map.add(Object::new(Kind::Light { range: 20.0, brightness: 2.0 }, Vec3::new(0.0, 8.0, 0.0), [255, 240, 200], Material::Neon));
    map.add(Object::new(Kind::Spawn { size: Vec3::new(6.0, 1.0, 6.0) }, Vec3::new(0.0, 0.5, 10.0), [90, 200, 110], Material::SmoothPlastic));
    map.add(Object::new(
        Kind::Prop { model: "missing_model.obj".into(), scale: Vec3::ONE, roblox_mesh_id: "1234567".into() },
        Vec3::new(0.0, 0.0, -10.0),
        [255, 255, 255],
        Material::Plastic,
    ));
    map
}

#[test]
fn map_round_trips_through_json() {
    let mut map = sample_map();
    let f = map.add_folder("House", None);
    map.objects[0].folder = Some(f);
    map.add_texture(TextureDef::new("bricks".into(), "bricks.png".into()));
    let p = out_dir("roundtrip").join("m.anvil");
    map.save(&p).unwrap();
    let back = Map::load(&p).unwrap();
    assert_eq!(back.objects, map.objects);
    assert_eq!(back.folders, map.folders);
    assert_eq!(back.textures, map.textures);
    assert_eq!(back.next_id, map.next_id);
}

#[test]
fn door_hole_splits_wall_into_three_pieces() {
    // 10 wide, 10 tall, 1 thick wall; 2x6 door at the bottom middle, cut through
    let size = Vec3::new(10.0, 10.0, 1.0);
    let hole = Hole { min: Vec3::new(-1.0, -5.1, -0.6), max: Vec3::new(1.0, 1.0, 0.6) };
    let pieces = box_minus(size, &[hole]);
    assert_eq!(pieces.len(), 3, "left, right and lintel: {pieces:?}");
    let vol: f32 = pieces.iter().map(|(a, b)| (*b - *a).x * (*b - *a).y * (*b - *a).z).sum();
    assert!((vol - (100.0 - 2.0 * 6.0)).abs() < 1e-3, "volume {vol}");
    // a window (hole fully inside on X/Y) needs four pieces
    let win = Hole { min: Vec3::new(-1.0, -1.0, -0.6), max: Vec3::new(1.0, 1.0, 0.6) };
    assert_eq!(box_minus(size, &[win]).len(), 4);
    // no hole -> the box itself
    assert_eq!(box_minus(size, &[]).len(), 1);
}

#[test]
fn wedge_pairs_rebuild_the_triangle() {
    // Each wedge's sloped profile, in our WedgePart convention (tall side at +Z),
    // must lie inside the source triangle and together cover its area. This
    // checks our wedge orientation against the classic Roblox triangle method.
    let tris = [
        [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(3.0, 7.0, 2.0)],
        [Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, 5.0, 1.0), Vec3::new(2.0, -3.0, 6.0)],
    ];
    let wedge = unit_wedge();
    for t in tris {
        let area = (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5;
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize();
        let mut total = 0.0;
        for (size, pos, rot) in export_roblox::tri_wedges(t[0], t[1], t[2], 0.0) {
            assert!((rot.determinant() - 1.0).abs() < 1e-4, "rotation must be proper");
            // profile corners: unit wedge vertices on the x = -0.5 side, at x = 0 plane
            let mut corners: Vec<Vec3> = wedge.pos.iter().filter(|p| p.x < 0.0).map(|p| pos + rot * (Vec3::new(0.0, p.y, p.z) * size)).collect();
            corners.dedup_by(|a, b| (*a - *b).length() < 1e-4);
            for c in &corners {
                // inside the triangle: barycentric coords >= 0
                let (a, b, cc) = (t[0], t[1], t[2]);
                let w0 = (b - *c).cross(cc - *c).dot(n);
                let w1 = (cc - *c).cross(a - *c).dot(n);
                let w2 = (a - *c).cross(b - *c).dot(n);
                assert!(w0 >= -1e-3 && w1 >= -1e-3 && w2 >= -1e-3, "wedge corner {c} outside triangle {t:?}");
            }
            total += size.y * size.z * 0.5;
        }
        assert!((total - area).abs() < 1e-3 * area.max(1.0), "wedges cover {total}, triangle {area}");
    }
}

#[test]
fn concave_footprint_triangulates_and_extrudes() {
    // L-shaped room
    let pts = [(0.0, 0.0), (8.0, 0.0), (8.0, 4.0), (4.0, 4.0), (4.0, 8.0), (0.0, 8.0)].map(|(x, z)| Vec3::new(x, 0.0, z));
    let (verts, faces, center) = polymesh::prism(&pts, 0.0, 2.0);
    assert!((center.y - 1.0).abs() < 1e-5);
    let top = &faces[0];
    let tris = polymesh::triangulate(&verts, top);
    assert_eq!(tris.len(), 4);
    let area: f32 = tris.iter().map(|t| (verts[t[1] as usize] - verts[t[0] as usize]).cross(verts[t[2] as usize] - verts[t[0] as usize]).y * 0.5).sum();
    assert!((area - 48.0).abs() < 1e-3, "top area {area} (must be +Y facing and cover the L)");
    // every face points away from the solid (check against the prism's side midpoint direction)
    let m = polymesh::poly_trimesh(&verts, &faces);
    let total_vol: f32 = m.tris.iter().map(|t| {
        let (a, b, c) = (m.pos[t[0] as usize], m.pos[t[1] as usize], m.pos[t[2] as usize]);
        a.dot(b.cross(c)) / 6.0
    }).sum();
    assert!((total_vol - 96.0).abs() < 1e-2, "signed volume {total_vol} (positive = outward winding)");

    // extruding the top face keeps the mesh closed and adds 6 side quads
    let (mut v, mut f) = (verts.clone(), faces.clone());
    polymesh::extrude(&mut v, &mut f, &[0], 3.0);
    assert_eq!(f.len(), faces.len() + 6);
    let m2 = polymesh::poly_trimesh(&v, &f);
    let vol2: f32 = m2.tris.iter().map(|t| {
        let (a, b, c) = (m2.pos[t[0] as usize], m2.pos[t[1] as usize], m2.pos[t[2] as usize]);
        a.dot(b.cross(c)) / 6.0
    }).sum();
    assert!((vol2 - 48.0 * 5.0).abs() < 1e-2, "extruded volume {vol2}");
}

#[test]
fn box_poly_and_weld_are_closed() {
    let (v, f) = polymesh::box_poly(Vec3::new(2.0, 4.0, 6.0));
    let m = polymesh::poly_trimesh(&v, &f);
    let vol: f32 = m.tris.iter().map(|t| m.pos[t[0] as usize].dot(m.pos[t[1] as usize].cross(m.pos[t[2] as usize])) / 6.0).sum();
    assert!((vol - 48.0).abs() < 1e-3);
    let (wv, wf) = polymesh::weld(&primitive(Shape::Wedge, Vec3::new(2.0, 2.0, 2.0), 1));
    assert_eq!(wv.len(), 6);
    assert_eq!(wf.len(), 5, "wedge = 3 quads + 2 triangles after welding");
}

fn plugin_index(host: &PluginHost, file: &str) -> usize {
    host.plugins.iter().position(|p| p.file == file).unwrap_or_else(|| panic!("{file} not loaded; log: {:?}", host.log))
}

#[test]
fn snap_to_part_plugin_snaps_flush() {
    let mut host = PluginHost::new();
    let idx = plugin_index(&host, "snap_to_part.rhai");
    let mut map = Map::default();
    let a = map.add(Object::brush(Shape::Box, Vec3::new(2.0, 4.0, 2.0), Vec3::new(0.0, 2.0, 0.0), [1, 2, 3], Material::Plastic));
    let b = map.add(Object::brush(Shape::Box, Vec3::new(2.0, 4.0, 2.0), Vec3::new(10.0, 2.0, 0.0), [1, 2, 3], Material::Plastic));
    let mut bounds = HashMap::new();
    bounds.insert(a, (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 4.0, 1.0)));
    // drag b to x=2.6 (gap of 0.6 to a): should snap flush to x=2 (delta -7.4 -> -8)
    let (bmin, bmax) = (Vec3::new(9.0, 0.0, -1.0), Vec3::new(11.0, 4.0, 1.0));
    let d = host.adjust_move(idx, &map, &ModelCache::default(), &[b], 1.0, bounds.clone(), bmin, bmax, Vec3::new(-7.4, 0.0, 0.0));
    assert!((d.x + 8.0).abs() < 1e-4, "snapped delta {d}");
    // drag b into a (overlap) -> pushed out to flush
    let d = host.adjust_move(idx, &map, &ModelCache::default(), &[b], 1.0, bounds, bmin, bmax, Vec3::new(-9.5, 0.0, 0.0));
    assert!((d.x + 8.0).abs() < 1e-4 || (d.x + 12.0).abs() < 1e-4, "pushed-out delta {d}");
}

#[test]
fn align_sizes_plugin_stretches_to_edge() {
    let mut host = PluginHost::new();
    let idx = plugin_index(&host, "align_sizes.rhai");
    let mut map = Map::default();
    // floor 10 wide centered at 0 (x -5..5), wall at x = 8
    let floor = map.add(Object::brush(Shape::Box, Vec3::new(10.0, 1.0, 10.0), Vec3::new(0.0, -0.5, 0.0), [1, 2, 3], Material::Plastic));
    let wall = map.add(Object::brush(Shape::Box, Vec3::new(1.0, 10.0, 10.0), Vec3::new(8.0, 5.0, 0.0), [1, 2, 3], Material::Plastic));
    let models = ModelCache::default();
    // first click: the floor's +X top edge; second: the wall's -X bottom edge
    let h1 = Hit { id: floor, point: Vec3::new(5.0, 0.0, 0.0), normal: Vec3::X, edge: (Vec3::new(5.0, 0.0, -5.0), Vec3::new(5.0, 0.0, 5.0)) };
    let h2 = Hit { id: wall, point: Vec3::new(7.5, 0.0, 0.0), normal: Vec3::NEG_X, edge: (Vec3::new(7.5, 0.0, -5.0), Vec3::new(7.5, 0.0, 5.0)) };
    let o1 = host.tool_click(idx, &map, &models, &[], 1.0, HashMap::new(), &h1);
    assert!(!o1.has_changes());
    let o2 = host.tool_click(idx, &map, &models, &[], 1.0, HashMap::new(), &h2);
    assert!(o2.has_changes(), "no change; status {:?} log {:?}", o2.status, host.log);
    let mut m = map.clone();
    o2.apply_to(&mut m);
    let f = m.get(floor).unwrap();
    let Kind::Brush { size, .. } = f.kind else { panic!() };
    assert!((size.x - 12.5).abs() < 1e-3, "floor width {}", size.x);
    assert!((f.pos.x - 1.25).abs() < 1e-3, "floor center {}", f.pos.x); // -5 .. 7.5
}

#[test]
fn player_lands_on_floor_and_is_blocked_by_wall() {
    let mut map = Map::default();
    map.add(Object::brush(Shape::Box, Vec3::new(40.0, 1.0, 40.0), Vec3::new(0.0, -0.5, 0.0), [1, 2, 3], Material::Plastic));
    map.add(Object::brush(Shape::Box, Vec3::new(1.0, 10.0, 40.0), Vec3::new(5.0, 5.0, 0.0), [1, 2, 3], Material::Plastic));
    let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
    // spawn above the floor facing +X (yaw = pi/2 looks along +X)
    let mut p = Play::new(&scene, Vec3::new(0.0, 6.0, 0.0), std::f32::consts::FRAC_PI_2, 1.0);
    for _ in 0..120 {
        p.step(&PlayInput::default(), 1.0 / 60.0);
    }
    assert!(p.grounded, "should be standing");
    assert!(p.pos.y.abs() < 0.1, "feet at floor top, got {}", p.pos.y);
    let walk = PlayInput { forward: 1.0, ..Default::default() };
    for _ in 0..180 {
        p.step(&walk, 1.0 / 60.0);
    }
    assert!(p.pos.x < 4.5 && p.pos.x > 3.0, "stopped by the wall at x≈3.5, got {}", p.pos.x);
}

#[test]
fn glb_is_well_formed() {
    let m = primitive(Shape::Box, Vec3::ONE, 1);
    let g = crate::glb::mesh_to_glb(&m, 1.0, "box");
    assert_eq!(&g[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(g[8..12].try_into().unwrap()) as usize, g.len());
    let jlen = u32::from_le_bytes(g[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&g[20..20 + jlen]).unwrap();
    assert_eq!(json["accessors"][0]["count"], 36);
    let blen = u32::from_le_bytes(g[20 + jlen..24 + jlen].try_into().unwrap()) as usize;
    assert_eq!(blen, json["buffers"][0]["byteLength"].as_u64().unwrap() as usize);
}

#[test]
fn open_cloud_multipart_body() {
    let c = crate::cloud::Creator { group: false, id: "42".into() };
    let body = crate::cloud::multipart_body("BOUND", "Decal", "bricks", "desc", &c, "bricks.png", "image/png", b"PNGDATA");
    let s = String::from_utf8_lossy(&body);
    assert!(s.starts_with("--BOUND\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n"));
    let json_line = s.split("\r\n").nth(3).unwrap();
    let v: serde_json::Value = serde_json::from_str(json_line).unwrap();
    assert_eq!(v["assetType"], "Decal");
    assert_eq!(v["creationContext"]["creator"]["userId"], "42");
    assert!(s.contains("name=\"fileContent\"; filename=\"bricks.png\"\r\nContent-Type: image/png\r\n\r\nPNGDATA\r\n--BOUND--\r\n"));
}

fn rich_map() -> Map {
    let mut map = sample_map();
    let folder = map.add_folder("House", None);
    let sub = map.add_folder("Upstairs", Some(folder));
    let tex = map.add_texture(TextureDef { roblox_image_id: "111".into(), ..TextureDef::new("bricks".into(), "bricks.png".into()) });
    let mut metal = TextureDef::new("plate".into(), "plate.png".into());
    metal.metallic = true;
    metal.roblox_image_id = "222".into();
    let metal = map.add_texture(metal);
    let mut wall = Object::brush(Shape::Box, Vec3::new(10.0, 10.0, 1.0), Vec3::new(0.0, 5.0, 20.0), [255, 255, 255], Material::Brick);
    if let Kind::Brush { holes, .. } = &mut wall.kind {
        holes.push(Hole { min: Vec3::new(-1.0, -5.1, -0.6), max: Vec3::new(1.0, 1.0, 0.6) });
    }
    wall.texture = Some(tex);
    wall.folder = Some(folder);
    map.add(wall);
    let (v, f, c) = polymesh::prism(&[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 4.0)], 0.0, 1.0);
    let mut mesh = Object::new(Kind::Mesh { verts: v, faces: f }, c + Vec3::new(30.0, 0.0, 0.0), [100, 100, 100], Material::Metal);
    mesh.texture = Some(metal);
    mesh.folder = Some(sub);
    map.add(mesh);
    let mut decal = Object::new(Kind::Decal { size: Vec2::new(2.0, 2.0) }, Vec3::new(0.0, 5.0, 20.6), [255, 255, 255], Material::SmoothPlastic);
    decal.texture = Some(tex);
    map.add(decal);
    map
}

#[test]
fn roblox_export_is_complete() {
    let map = rich_map();
    let mut cache = ModelCache::default();
    let settings = Settings::default();
    let nodes = export_roblox::build_nodes(&map, &mut cache, &settings);

    let xml = export_roblox::to_rbxmx(&nodes, &map.name);
    assert!(xml.starts_with("<roblox"));
    assert_eq!(xml.matches("<Item ").count(), xml.matches("</Item>").count());
    for class in ["\"Part\"", "\"WedgePart\"", "\"SpawnLocation\"", "\"PointLight\"", "\"SpecialMesh\"", "\"MaterialVariant\"", "\"Texture\"", "\"Decal\""] {
        assert!(xml.contains(class), "missing {class}");
    }
    assert!(xml.contains("rbxassetid://1234567"));
    assert!(xml.contains("<string name=\"Name\">House</string>") && xml.contains("<string name=\"Name\">Upstairs</string>"));
    assert!(xml.contains("<string name=\"MaterialVariantSerialized\">Anvil_plate</string>"));
    assert!(xml.contains("<string name=\"Name\">Piece3</string>"), "door wall should be 3 pieces");

    let lua = export_roblox::to_luau(&nodes, &map.name);
    assert_eq!(lua.matches("\ndo\n").count() + lua.starts_with("do\n") as usize, lua.matches("\nend\n").count());
    assert!(lua.contains("Enum.PartType.Cylinder"));
    assert!(lua.contains("game:GetService(\"MaterialService\")"));
    assert!(lua.contains("Enum.NormalId.Back"));

    let dir = out_dir("roblox");
    std::fs::write(dir.join("sample.rbxmx"), &xml).unwrap();
    std::fs::write(dir.join("sample.lua"), &lua).unwrap();
}

#[test]
fn unity_export_writes_obj_mtl_and_script() {
    let map = rich_map();
    let mut cache = ModelCache::default();
    let settings = Settings::default();
    let dir = out_dir("unity");
    let msg = export_unity::export(&map, &mut cache, &settings, &dir.join("sample.obj")).unwrap();
    assert!(msg.contains("9 objects"), "{msg}"); // 5 brushes + prop + wall + mesh + decal
    let obj = std::fs::read_to_string(dir.join("sample.obj")).unwrap();
    let v = obj.lines().filter(|l| l.starts_with("v ")).count();
    let vn = obj.lines().filter(|l| l.starts_with("vn ")).count();
    let vt = obj.lines().filter(|l| l.starts_with("vt ")).count();
    assert!(vt > 0, "textured objects need UVs");
    for l in obj.lines().filter(|l| l.starts_with("f ")) {
        for part in l[2..].split_whitespace() {
            let idx: Vec<&str> = part.split('/').collect();
            let vi: usize = idx[0].parse().unwrap();
            let ni: usize = idx[2].parse().unwrap();
            assert!(vi >= 1 && vi <= v && ni >= 1 && ni <= vn, "bad face {l}");
            if !idx[1].is_empty() {
                let ti: usize = idx[1].parse().unwrap();
                assert!(ti >= 1 && ti <= vt, "bad uv index {l}");
            }
        }
    }
    assert!(obj.contains("o House__"), "folder prefix");
    let mtl = std::fs::read_to_string(dir.join("sample.mtl")).unwrap();
    assert!(mtl.contains("newmtl Brick_c86432"));
    assert!(mtl.contains("Pm 1"), "metallic texture");
    let cs = std::fs::read_to_string(dir.join("sample_AnvilEntities.cs")).unwrap();
    assert!(cs.contains("LightType.Point") && cs.contains("\"Respawn\""));
}

/// Minimal uncompressed PNG writer (stored deflate blocks) for test snapshots.
fn write_png(path: &std::path::Path, w: usize, h: usize, rgba: &[u8]) {
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec()).unwrap();
    img.save(path).unwrap();
}

#[test]
fn renders_textured_scene() {
    use crate::view3d::{Camera, RenderOpts, Renderer};
    use eframe::egui;
    let dir = out_dir("render");
    // checker texture + bumpy normal map + an alpha "paper" decal
    let checker = image::RgbaImage::from_fn(64, 64, |x, y| if (x / 8 + y / 8) % 2 == 0 { image::Rgba([200, 60, 40, 255]) } else { image::Rgba([240, 220, 200, 255]) });
    checker.save(dir.join("checker.png")).unwrap();
    let normal = image::RgbaImage::from_fn(64, 64, |x, _| {
        let s = ((x as f32 / 64.0) * std::f32::consts::TAU * 4.0).sin();
        image::Rgba([(128.0 + s * 90.0) as u8, 128, 230, 255])
    });
    normal.save(dir.join("normal.png")).unwrap();
    let paper = image::RgbaImage::from_fn(64, 64, |x, y| {
        let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt();
        if d < 28.0 { image::Rgba([30, 30, 30, 255]) } else { image::Rgba([0, 0, 0, 0]) }
    });
    paper.save(dir.join("paper.png")).unwrap();

    let mut map = Map::default();
    let mut t = TextureDef::new("checker".into(), dir.join("checker.png").to_string_lossy().to_string());
    t.normal_map = dir.join("normal.png").to_string_lossy().to_string();
    let tex = map.add_texture(t);
    let decal_tex = map.add_texture(TextureDef::new("paper".into(), dir.join("paper.png").to_string_lossy().to_string()));
    map.add(Object::brush(Shape::Box, Vec3::new(60.0, 1.0, 40.0), Vec3::new(10.0, -0.5, 0.0), [110, 112, 118], Material::Slate));
    let mut wall = Object::brush(Shape::Box, Vec3::new(16.0, 10.0, 1.0), Vec3::new(4.0, 5.0, -4.0), [255, 255, 255], Material::Brick);
    if let Kind::Brush { holes, .. } = &mut wall.kind {
        holes.push(Hole { min: Vec3::new(-2.0, -5.1, -0.6), max: Vec3::new(2.0, 2.0, 0.6) });
    }
    wall.texture = Some(tex);
    map.add(wall);
    let mut d = Object::new(Kind::Decal { size: Vec2::new(3.0, 3.0) }, Vec3::new(-1.0, 0.03, 3.0), [255, 255, 255], Material::SmoothPlastic);
    d.set_rotation(glam::Quat::from_mat3(&Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y)));
    d.texture = Some(decal_tex);
    map.add(d);
    let (v, f, c) = polymesh::prism(&[Vec3::new(14.0, 0.0, 0.0), Vec3::new(22.0, 0.0, 0.0), Vec3::new(22.0, 0.0, 6.0), Vec3::new(18.0, 0.0, 3.0), Vec3::new(14.0, 0.0, 6.0)], 0.0, 3.0);
    map.add(Object::new(Kind::Mesh { verts: v, faces: f }, c, [90, 160, 220], Material::Plastic));

    let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 450.0));
    let cam = Camera::looking_at(Vec3::new(-6.0, 14.0, 22.0), Vec3::new(8.0, 2.0, 0.0));
    let opts = RenderOpts { fov: 70.0, grid: 1.0, plane_y: 0.0, show_grid: true, outlines: true, bg: [58, 66, 80], scale: 1.0, filter: true, lighting: Default::default() };
    let mut r = Renderer::default();
    let mut tc = crate::texture::TexCache::default();
    r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut tc);
    let (w, h, px) = r.pixels();
    let bg = px.chunks_exact(4).filter(|p| p[0] == 58 && p[1] == 66 && p[2] == 80).count();
    assert!(bg < w * h * 9 / 10, "almost nothing was drawn");
    // the checker must actually show up (both of its colors' hues present)
    let reddish = px.chunks_exact(4).filter(|p| p[0] > 120 && p[1] < 80 && p[2] < 70).count();
    assert!(reddish > 200, "checker texture not visible ({reddish} px)");
    write_png(&dir.join("render3d.png"), w, h, px);
}

/// Drives the bundled Transform Gizmo plugin exactly like the editor does.
mod gizmo {
    use super::*;
    use crate::plugins::{event_map, DrawCmd, ViewInfo};
    use crate::view2d::{View2D, ViewKind};
    use eframe::egui::{pos2, vec2, Pos2, Rect};

    fn setup() -> (PluginHost, usize, Map, u64, ViewInfo) {
        let host = PluginHost::new();
        let idx = plugin_index(&host, "transform_gizmo.rhai");
        let mut map = Map::default();
        let id = map.add(Object::brush(Shape::Box, Vec3::new(2.0, 2.0, 2.0), Vec3::new(0.0, 1.0, 0.0), [1, 2, 3], Material::Plastic));
        let mut v = View2D::new(ViewKind::Top);
        v.center = Vec2::ZERO;
        v.zoom = 8.0;
        let view = ViewInfo::Ortho { view: v, rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)) };
        (host, idx, map, id, view)
    }

    /// pointer_down at `a`, drag to `b`, release. Returns the final map.
    fn drag(host: &mut PluginHost, idx: usize, map: Map, id: u64, view: ViewInfo, a: Pos2, b: Pos2) -> Map {
        let models = ModelCache::default();
        let mut map = map;
        let (out, taken) = host.gizmo_event(idx, "pointer_down", &map, &models, &[id], 1.0, HashMap::new(), view, event_map(&view, a, false, false, false));
        assert!(taken, "gizmo should grab the handle at {a:?}; log {:?}", host.log);
        out.apply_to(&mut map);
        for step in 1..=4 {
            let p = a + (b - a) * (step as f32 / 4.0);
            let (out, _) = host.gizmo_event(idx, "pointer_drag", &map, &models, &[id], 1.0, HashMap::new(), view, event_map(&view, p, false, false, false));
            out.apply_to(&mut map);
        }
        let (out, _) = host.gizmo_event(idx, "pointer_up", &map, &models, &[id], 1.0, HashMap::new(), view, event_map(&view, b, false, false, false));
        out.apply_to(&mut map);
        map
    }

    #[test]
    fn draws_handles() {
        let (mut host, idx, map, id, view) = setup();
        let cmds = host.gizmo_draw(idx, &map, &ModelCache::default(), &[id], 1.0, HashMap::new(), view);
        let lines = cmds.iter().filter(|c| matches!(c, DrawCmd::Line { .. })).count();
        assert!(lines >= 2, "expected X and Z arrows in the top view, got {cmds:?}; log {:?}", host.log);
    }

    #[test]
    fn misses_return_false() {
        let (mut host, idx, map, id, view) = setup();
        let (_, taken) = host.gizmo_event(idx, "pointer_down", &map, &ModelCache::default(), &[id], 1.0, HashMap::new(), view, event_map(&view, pos2(700.0, 100.0), false, false, false));
        assert!(!taken, "clicks away from the handles must fall through to the editor");
    }

    #[test]
    fn plugin_plane_square_moves_in_the_plane() {
        let (mut host, idx, map, id, view) = setup();
        let units = view.units_per_pixel(Vec3::new(0.0, 1.0, 0.0));
        let s = units * 85.0;
        // the XZ square (facing the top view) sits between the X and Z arrows
        let corner = |a: f32| view.project(Vec3::new(a * s, 1.0, a * s)).unwrap();
        let mid = corner(0.32);
        let target = mid + vec2(40.0, 24.0);
        let m = drag(&mut host, idx, map, id, view, mid, target);
        let p = m.get(id).unwrap().pos;
        let (o0, _) = view.ray(mid);
        let (o1, _) = view.ray(target);
        let want = o1 - o0;
        assert!(p.y == 1.0, "Y stays put: {p}");
        assert!((p.x - want.x.round()).abs() < 1e-4 && (p.z - want.z.round()).abs() < 1e-4, "moved {p}, mouse moved {want}");
    }

    #[test]
    fn move_arrow_moves_along_x_with_snapping() {
        let (mut host, idx, map, id, view) = setup();
        let c = pos2(400.0, 300.0);
        // grab the X arrow halfway, drag 80 px right = 10 units at zoom 8
        let m = drag(&mut host, idx, map, id, view, c + vec2(50.0, 0.0), c + vec2(131.0, 3.0));
        let p = m.get(id).unwrap().pos;
        assert!((p.x - 10.0).abs() < 1e-4 && p.y == 1.0 && p.z.abs() < 1e-4, "moved to {p}");
    }

    #[test]
    fn ring_rotates_around_y() {
        let (mut host, idx, map, id, view) = setup();
        host.set_gizmo("rotate", false);
        let c = pos2(400.0, 300.0);
        let r = 0.8 * 85.0;
        let at = |deg: f32| c + vec2(deg.to_radians().cos(), deg.to_radians().sin()) * r;
        // a quarter turn counter-clockwise on screen (seen from above) = +90 degrees about Y
        let m = drag(&mut host, idx, map, id, view, at(-45.0), at(-135.0));
        let rot = m.get(id).unwrap().rot;
        assert!((rot.y - 90.0).abs() < 1e-3 && rot.x.abs() < 1e-3 && rot.z.abs() < 1e-3, "rotation {rot}");
    }

    #[test]
    fn scale_handle_doubles_width() {
        let (mut host, idx, map, id, view) = setup();
        host.set_gizmo("scale", false);
        let c = pos2(400.0, 300.0);
        let m = drag(&mut host, idx, map, id, view, c + vec2(50.0, 0.0), c + vec2(135.0, 0.0));
        let o = m.get(id).unwrap();
        let Kind::Brush { size, .. } = o.kind else { panic!() };
        assert!((size.x - 4.0).abs() < 1e-4 && size.y == 2.0 && size.z == 2.0, "size {size}");
        assert!(o.pos.x.abs() < 1e-4, "scaling about the center keeps it in place: {}", o.pos);
    }
}

/// Building walls: no two pieces may overlap and the outline must be fully covered.
mod corners {
    use super::*;
    use crate::building::house_walls;

    fn contains(o: &Object, p: Vec3, shrink: f32) -> bool {
        match &o.kind {
            Kind::Brush { size, .. } => {
                let l = o.to_local(p);
                l.x.abs() < size.x * 0.5 - shrink && l.z.abs() < size.z * 0.5 - shrink
            }
            Kind::Mesh { verts, faces } => {
                // point in the (convex) top face, in world XZ
                let top: Vec<Vec3> = faces[0].iter().map(|&i| o.to_world(verts[i as usize])).collect();
                let n = top.len();
                let mut sign = 0.0f32;
                for i in 0..n {
                    let (a, b) = (top[i], top[(i + 1) % n]);
                    let c = (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x);
                    let len = ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt().max(1e-6);
                    if c.abs() / len < shrink {
                        return false;
                    }
                    if sign == 0.0 {
                        sign = c.signum();
                    } else if c.signum() != sign {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn check(name: &str, pts: &[Vec3]) {
        let t = 1.0;
        let parts = house_walls(pts, t, 10.0, 0.0, [1, 2, 3], Material::Plastic);
        // no overlaps: sample a grid over the whole area
        let (mn, mx) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let mut x = mn.x - 2.0;
        while x < mx.x + 2.0 {
            let mut z = mn.z - 2.0;
            while z < mx.z + 2.0 {
                let p = Vec3::new(x, 5.0, z);
                let n = parts.iter().filter(|o| contains(o, p, 1e-3)).count();
                assert!(n <= 1, "{name}: {n} pieces overlap at {p}");
                z += 0.071;
            }
            x += 0.073;
        }
        // no gaps: walk every edge of the outline across the wall's thickness
        let k = pts.len();
        for i in 0..k {
            let (a, b) = (pts[i], pts[(i + 1) % k]);
            let d = (b - a).normalize();
            let nrm = Vec3::new(-d.z, 0.0, d.x);
            let len = (b - a).length();
            let mut s = 0.0;
            while s <= len {
                for off in [-0.45 * t, 0.0, 0.45 * t] {
                    let p = a + d * s + nrm * off + Vec3::Y * 5.0;
                    assert!(parts.iter().any(|o| contains(o, p, -1e-3)), "{name}: gap at {p}");
                }
                s += 0.25;
            }
        }
    }

    #[test]
    fn rectangle_has_clean_corners() {
        check("rect", &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(16.0, 0.0, 0.0), Vec3::new(16.0, 0.0, 12.0), Vec3::new(0.0, 0.0, 12.0)]);
        // counter-wound outline too
        check("rect-rev", &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 12.0), Vec3::new(16.0, 0.0, 12.0), Vec3::new(16.0, 0.0, 0.0)]);
        let parts = house_walls(&[Vec3::ZERO, Vec3::new(16.0, 0.0, 0.0), Vec3::new(16.0, 0.0, 12.0), Vec3::new(0.0, 0.0, 12.0)], 1.0, 10.0, 0.0, [1, 2, 3], Material::Plastic);
        assert_eq!(parts.len(), 4, "right angles need no corner pieces");
    }

    #[test]
    fn l_shape_has_clean_corners() {
        let pts = [(0.0, 0.0), (12.0, 0.0), (12.0, 6.0), (6.0, 6.0), (6.0, 12.0), (0.0, 12.0)].map(|(x, z)| Vec3::new(x, 0.0, z));
        check("L", &pts);
    }

    #[test]
    fn angled_outline_gets_corner_pieces() {
        let pts = [(0.0, 0.0), (14.0, 0.0), (18.0, 7.0), (9.0, 13.0), (-2.0, 8.0)].map(|(x, z)| Vec3::new(x, 0.0, z));
        check("pentagon", &pts);
        let parts = house_walls(&pts, 1.0, 10.0, 0.0, [1, 2, 3], Material::Plastic);
        assert_eq!(parts.iter().filter(|o| matches!(o.kind, Kind::Mesh { .. })).count(), 5);
    }
}

#[test]
fn gizmo_scales_meshes() {
    use crate::plugins::{event_map, ViewInfo};
    use crate::view2d::{View2D, ViewKind};
    use eframe::egui::{pos2, vec2, Pos2, Rect};
    let mut host = PluginHost::new();
    let idx = plugin_index(&host, "transform_gizmo.rhai");
    let mut map = Map::default();
    let (v, f) = polymesh::box_poly(Vec3::new(2.0, 2.0, 2.0));
    let id = map.add(Object::new(Kind::Mesh { verts: v, faces: f }, Vec3::new(0.0, 1.0, 0.0), [1, 2, 3], Material::Plastic));
    let mut view = View2D::new(ViewKind::Top);
    view.center = Vec2::ZERO;
    view.zoom = 8.0;
    let view = ViewInfo::Ortho { view, rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)) };
    let models = ModelCache::default();
    host.set_gizmo("scale", false);
    let c = pos2(400.0, 300.0);
    let ev = |p: Pos2| event_map(&view, p, false, false, false);
    let (o, taken) = host.gizmo_event(idx, "pointer_down", &map, &models, &[id], 1.0, HashMap::new(), view, ev(c + vec2(50.0, 0.0)));
    assert!(taken);
    o.apply_to(&mut map);
    let (o, _) = host.gizmo_event(idx, "pointer_drag", &map, &models, &[id], 1.0, HashMap::new(), view, ev(c + vec2(135.0, 0.0)));
    assert!(o.has_changes(), "scaling a mesh must change it");
    o.apply_to(&mut map);
    let Kind::Mesh { verts, .. } = &map.get(id).unwrap().kind else { panic!() };
    let (mn, mx) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    assert!(((mx - mn).x - 4.0).abs() < 1e-4 && ((mx - mn).y - 2.0).abs() < 1e-4, "mesh extent {}", mx - mn);
}

/// Source engine content. Real-file tests use the Garry's Mod folder when it
/// exists (set ANVIL_GMOD to point elsewhere) and are skipped otherwise.
mod source_content {
    use super::*;
    use crate::mount::{import_model, Mounts};
    use crate::source::{decode_vtf, load_mdl, Gma, Vpk};

    fn gmod() -> Option<String> {
        let p = std::env::var("ANVIL_GMOD").unwrap_or_else(|_| r"C:\Program Files (x86)\Steam\steamapps\common\GarrysMod".into());
        std::path::Path::new(&p).is_dir().then_some(p)
    }

    #[test]
    fn vpk_v2_round_trip() {
        // build a tiny VPK v2: one file preloaded in the tree, one stored after it
        let mut tree = Vec::new();
        let entry = |tree: &mut Vec<u8>, ext: &str, dir: &str, name: &str, preload: &[u8], off: u32, len: u32| {
            for s in [ext, dir, name] {
                tree.extend(s.as_bytes());
                tree.push(0);
            }
            tree.extend(0u32.to_le_bytes());
            tree.extend((preload.len() as u16).to_le_bytes());
            tree.extend(0x7fffu16.to_le_bytes());
            tree.extend(off.to_le_bytes());
            tree.extend(len.to_le_bytes());
            tree.extend(0xffffu16.to_le_bytes());
            tree.extend(preload);
            tree.extend([0u8, 0, 0]); // end names, dirs, exts
        };
        entry(&mut tree, "vmt", "materials/test", "a", b"\"LightmappedGeneric\" {}", 0, 0);
        tree.pop(); // the next extension follows, so drop only the extension terminator
        let data = b"HELLO-VTF";
        let mut t2 = Vec::new();
        entry(&mut t2, "vtf", "materials/test", "b", b"", 0, data.len() as u32);
        tree.extend(t2);
        let mut f = Vec::new();
        f.extend(0x55aa1234u32.to_le_bytes());
        f.extend(2u32.to_le_bytes());
        f.extend((tree.len() as u32).to_le_bytes());
        f.extend([0u8; 16]);
        f.extend(&tree);
        f.extend(data);
        let p = out_dir("vpk").join("pak01_dir.vpk");
        std::fs::write(&p, f).unwrap();
        let v = Vpk::open(&p).unwrap();
        assert_eq!(v.read("materials/test/a.vmt").as_deref(), Some(&b"\"LightmappedGeneric\" {}"[..]));
        assert_eq!(v.read("materials/test/b.vtf").as_deref(), Some(&data[..]));
    }

    #[test]
    fn reads_real_hl2_oil_drum() {
        let Some(g) = gmod() else { return };
        let mounts = Mounts::build(&[g.clone()]);
        assert!(mounts.models.iter().any(|m| m == "models/props_c17/oildrum001.mdl"), "oil drum not indexed; mounts: {:?}", mounts.summary);
        assert!(mounts.materials.iter().any(|m| m == "models/props_c17/oil_drum001a"));

        let m = load_mdl(&|r| mounts.read(r), "models/props_c17/oildrum001.mdl").unwrap();
        assert!(m.tris.len() > 50, "{} triangles", m.tris.len());
        assert_eq!(m.materials[0], "models/props_c17/oil_drum001a", "{:?}", m.materials);
        // a closed drum must have positive volume with our counter-clockwise winding
        let vol: f32 = m.tris.iter().map(|t| m.pos[t[0] as usize].dot(m.pos[t[1] as usize].cross(m.pos[t[2] as usize])) / 6.0).sum();
        assert!(vol > 1000.0, "signed volume {vol} (negative = inside-out)");
        let (mn, mx) = m.pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        assert!((mx - mn).y > (mx - mn).x, "the drum should stand upright (Y up): {}", mx - mn);

        let vtf = mounts.read("materials/models/props_c17/oil_drum001a.vtf").unwrap();
        let img = decode_vtf(&vtf, 1024).unwrap();
        assert_eq!((img.w, img.h), (512, 512));
        let avg: u32 = img.data.chunks_exact(4).map(|p| p[0] as u32).sum::<u32>() / (img.w * img.h);
        assert!(avg > 10 && avg < 245, "decoded texture looks blank (avg red {avg})");
        let dir = out_dir("hl2");
        image::RgbaImage::from_raw(img.w, img.h, img.data).unwrap().save(dir.join("oil_drum001a.png")).unwrap();

        // import -> OBJ/MTL/PNG -> load like any library model, textures included
        let obj = import_model(&mounts, "models/props_c17/oildrum001.mdl", 0.07).unwrap();
        let tm = crate::mesh::load_model(&obj.to_string_lossy()).unwrap();
        assert_eq!(tm.uv.len(), tm.pos.len(), "imported prop lost its UVs");
        assert!(tm.materials.iter().any(|t| t.ends_with("oil_drum001a.png") && std::path::Path::new(t).is_file()), "{:?}", tm.materials);

        // render it with its texture
        use crate::view3d::{Camera, RenderOpts, Renderer};
        let mut map = Map::default();
        map.add(Object::brush(Shape::Box, Vec3::new(20.0, 1.0, 20.0), Vec3::new(0.0, -0.5, 0.0), [110, 112, 118], Material::Slate));
        for (i, x) in [-2.5f32, 0.0, 2.5].iter().enumerate() {
            let mut p = Object::new(Kind::Prop { model: obj.to_string_lossy().to_string(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::new(*x, 0.0, 0.0), [255, 255, 255], Material::Metal);
            p.rot.y = i as f32 * 40.0;
            map.add(p);
        }
        let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
        let ctx = eframe::egui::Context::default();
        let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(640.0, 400.0));
        let cam = Camera::looking_at(Vec3::new(0.0, 4.5, 8.0), Vec3::new(0.0, 1.5, 0.0));
        let opts = RenderOpts { fov: 60.0, grid: 1.0, plane_y: 0.0, show_grid: true, outlines: false, bg: [58, 66, 80], scale: 1.0, filter: true, lighting: Default::default() };
        let mut r = Renderer::default();
        r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut crate::texture::TexCache::default());
        let (w, h, px) = r.pixels();
        image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(dir.join("oildrums.png")).unwrap();
    }

    #[test]
    fn reads_real_gma_addon() {
        let Some(g) = gmod() else { return };
        let ws = std::path::Path::new(&g).join("garrysmod").join("cache").join("workshop");
        let Some(f) = std::fs::read_dir(&ws).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "gma"))) else { return };
        let a = Gma::open(&f).unwrap();
        let first = a.names().next().cloned().expect("empty addon");
        assert!(a.read(&first).is_some(), "could not read {first} from {}", f.display());
    }
}

#[test]
fn package_round_trip_works_on_another_pc() {
    let src = out_dir("pak_src").join("assets_here");
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(src.join("tex")).unwrap();
    // a texture, a normal map and a textured OBJ prop with its own .mtl + image
    image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255])).save(src.join("bricks.png")).unwrap();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([128, 128, 255, 255])).save(src.join("bricks_n.png")).unwrap();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([30, 200, 30, 255])).save(src.join("tex").join("crate.png")).unwrap();
    std::fs::write(src.join("crate.mtl"), "newmtl wood\nKd 1 1 1\nmap_Kd tex/crate.png\n").unwrap();
    std::fs::write(
        src.join("crate.obj"),
        "mtllib crate.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvn 0 0 1\nusemtl wood\nf 1/1/1 2/2/1 3/3/1\n",
    )
    .unwrap();
    let mut map = Map::default();
    map.name = "shared".into();
    let mut t = TextureDef::new("bricks".into(), src.join("bricks.png").to_string_lossy().to_string());
    t.normal_map = src.join("bricks_n.png").to_string_lossy().to_string();
    let tex = map.add_texture(t);
    let mut wall = Object::brush(Shape::Box, Vec3::ONE, Vec3::ZERO, [255, 255, 255], Material::Brick);
    wall.texture = Some(tex);
    map.add(wall);
    let model = src.join("crate.obj").to_string_lossy().to_string();
    map.add(Object::new(Kind::Prop { model: model.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255, 255, 255], Material::Wood));
    let mut settings = Settings::default();
    settings.model_library.push(crate::settings::ModelEntry { path: model, roblox_model_id: "999".into(), ..Default::default() });

    let pak = out_dir("pak").join("shared.anvilpak");
    let msg = crate::package::save_package(&map, &settings, &pak).unwrap();
    assert!(!msg.contains("missing"), "{msg}");

    // "another PC": the original files are gone
    std::fs::remove_dir_all(&src).unwrap();
    let o = crate::package::open_package(&pak).unwrap();
    let t = &o.map.textures[0];
    assert!(std::path::Path::new(&t.image).is_file() && std::path::Path::new(&t.normal_map).is_file(), "{t:?}");
    let Kind::Prop { model, .. } = &o.map.objects[1].kind else { panic!() };
    let mesh = crate::mesh::load_model(model).unwrap();
    assert_eq!(mesh.uv.len(), 3, "prop UVs survive");
    assert!(mesh.materials[0].ends_with("crate.png") && std::path::Path::new(&mesh.materials[0]).is_file(), "{:?}", mesh.materials);
    assert_eq!(o.library.len(), 1);
    assert_eq!(o.library[0].path, *model);
    assert_eq!(o.library[0].roblox_model_id, "999", "upload ids travel with the package");
}

#[test]
fn packages_keep_gltf_props_whole() {
    // a Sketchfab-style .gltf with a separate .bin and a texture in a subfolder
    let src = out_dir("pak_gltf_src").join("model");
    std::fs::create_dir_all(src.join("textures")).unwrap();
    image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba([x as u8 * 60, 0, 0, 255])).save(src.join("textures").join("c.png")).unwrap();
    let mut bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend([0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()));
    std::fs::write(src.join("scene.bin"), &bin).unwrap();
    let gltf = serde_json::json!({
        "asset": { "version": "2.0" }, "scenes": [{ "nodes": [0] }], "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "TEXCOORD_0": 1 }, "material": 0 }] }],
        "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }],
        "textures": [{ "source": 0 }], "images": [{ "uri": "textures/c.png" }],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
            { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" }
        ],
        "bufferViews": [{ "buffer": 0, "byteLength": 36 }, { "buffer": 0, "byteOffset": 36, "byteLength": 24 }],
        "buffers": [{ "uri": "scene.bin", "byteLength": 60 }]
    });
    std::fs::write(src.join("scene.gltf"), gltf.to_string()).unwrap();
    let model = src.join("scene.gltf").to_string_lossy().to_string();
    let mut map = Map::default();
    map.add(Object::new(Kind::Prop { model, scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Wood));
    let pak = out_dir("pak_gltf").join("g.anvilpak");
    let msg = crate::package::save_package(&map, &Settings::default(), &pak).unwrap();
    assert!(!msg.contains("missing"), "{msg}");
    std::fs::remove_dir_all(&src).unwrap();
    let o = crate::package::open_package(&pak).unwrap();
    let Kind::Prop { model, .. } = &o.map.objects[0].kind else { panic!() };
    let mesh = crate::mesh::load_model(model).unwrap();
    assert_eq!(mesh.tris.len(), 1);
    assert!(std::path::Path::new(&mesh.materials[0]).is_file(), "texture packed: {:?}", mesh.materials);
}

#[test]
fn textured_glb_embeds_images() {
    let dir = out_dir("glbtex");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).save(dir.join("t.png")).unwrap();
    let mut m = primitive(Shape::Box, Vec3::ONE, 1);
    m.uv = vec![Vec2::ZERO; m.pos.len()];
    m.tri_mat = vec![0; m.tris.len()];
    m.materials = vec![dir.join("t.png").to_string_lossy().to_string()];
    let g = crate::glb::mesh_to_glb(&m, 1.0, "box");
    let jlen = u32::from_le_bytes(g[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&g[20..20 + jlen]).unwrap();
    assert_eq!(json["images"][0]["mimeType"], "image/png");
    assert!(json["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"].is_number());
    assert_eq!(json["meshes"][0]["primitives"][0]["material"], 0);
    assert_eq!(u32::from_le_bytes(g[8..12].try_into().unwrap()) as usize, g.len());
}

/// Timing benchmark on a heavy textured map. Run with:
/// cargo test --release bench_heavy -- --nocapture --ignored
#[test]
#[ignore]
fn bench_heavy_scene() {
    use crate::view3d::{Camera, RenderOpts, Renderer};
    use std::time::Instant;
    let dir = out_dir("bench");
    let tex_path = dir.join("brick.png");
    image::RgbaImage::from_fn(512, 512, |x, y| image::Rgba([(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x ^ y) % 256) as u8, 255])).save(&tex_path).unwrap();
    let mut map = Map::default();
    let tex = map.add_texture(TextureDef::new("brick".into(), tex_path.to_string_lossy().to_string()));
    let mut floor = Object::brush(Shape::Box, Vec3::new(400.0, 1.0, 400.0), Vec3::new(0.0, -0.5, 0.0), [255, 255, 255], Material::Slate);
    floor.texture = Some(tex);
    map.add(floor);
    for i in 0..30 {
        for j in 0..30 {
            let mut o = Object::brush(if (i + j) % 5 == 0 { Shape::Cylinder } else { Shape::Box }, Vec3::new(3.0, 6.0, 3.0), Vec3::new(i as f32 * 8.0 - 120.0, 3.0, j as f32 * 8.0 - 120.0), [255, 255, 255], Material::Brick);
            if (i + j) % 2 == 0 {
                o.texture = Some(tex);
            }
            map.add(o);
        }
    }
    let mut models = ModelCache::default();
    let t = Instant::now();
    let scene = crate::mesh::build_scene(&map, &mut models);
    let full = t.elapsed();
    let mut sc = crate::mesh::SceneCache::default();
    sc.build(&map, &mut models);
    let t = Instant::now();
    let _ = sc.build(&map, &mut models);
    let cached = t.elapsed();
    let ctx = eframe::egui::Context::default();
    let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1280.0, 720.0));
    let cam = Camera::looking_at(Vec3::new(-150.0, 40.0, 150.0), Vec3::new(0.0, 0.0, 0.0));
    let opts = RenderOpts { fov: 70.0, grid: 1.0, plane_y: 0.0, show_grid: true, outlines: true, bg: [58, 66, 80], scale: 1.0, filter: true, lighting: Default::default() };
    let mut tc = crate::texture::TexCache::default();
    let mut r = Renderer::default();
    r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut tc); // warm up texture cache
    let frames = 10;
    let t = Instant::now();
    for _ in 0..frames {
        r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut tc);
    }
    let multi = t.elapsed() / frames;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let t = Instant::now();
    pool.install(|| {
        for _ in 0..frames {
            r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut tc);
        }
    });
    let single = t.elapsed() / frames;
    let t = Instant::now();
    for _ in 0..20 {
        let _ = map.clone();
    }
    let clone = t.elapsed() / 20;
    // Team Create: cost of one sync tick (hash every object) with nothing changed
    let mut hm = map.clone();
    let mut hs = crate::team::Session::host(&mut hm, "b", [0; 3], 0, false, dir.join("tc")).unwrap();
    let pr = crate::team::Presence::default();
    hs.pump(&mut hm, &pr);
    std::thread::sleep(std::time::Duration::from_millis(120));
    let t = Instant::now();
    hs.pump(&mut hm, &pr);
    let tick = t.elapsed();
    let mut lit_opts = RenderOpts { lighting: crate::lighting::preset(2, crate::lighting::sun_dir(-55.0, 45.0), true, 900.0, true), ..opts };
    r.render(&ctx, rect, &cam, &lit_opts, &scene, &[], &HashMap::new(), &mut tc);
    let t = Instant::now();
    for _ in 0..frames {
        r.render(&ctx, rect, &cam, &lit_opts, &scene, &[], &HashMap::new(), &mut tc);
    }
    let medium = t.elapsed() / frames;
    lit_opts.lighting = crate::lighting::preset(3, crate::lighting::sun_dir(-55.0, 45.0), true, 900.0, true);
    r.render(&ctx, rect, &cam, &lit_opts, &scene, &[], &HashMap::new(), &mut tc);
    let t = Instant::now();
    for _ in 0..frames {
        r.render(&ctx, rect, &cam, &lit_opts, &scene, &[], &HashMap::new(), &mut tc);
    }
    let high = t.elapsed() / frames;
    println!("BENCH lighting 1280x720: medium {medium:?}, high {high:?}");
    println!("BENCH objects={} | scene rebuild {:?} | cached scene {:?} | render 1280x720: {:?} all cores vs {:?} one core | whole-map clone {:?} | team sync tick {:?}", map.objects.len(), full, cached, multi, single, clone, tick);
    let (w, h, px) = r.pixels();
    image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(dir.join("bench.png")).unwrap();
}

#[test]
fn text_mesh_from_installed_fonts() {
    use crate::view3d::{Camera, RenderOpts, Renderer};
    let fonts = [r"C:\Windows\Fonts\OCRAEXT.TTF", r"C:\Windows\Fonts\arial.ttf"];
    let mut map = Map::default();
    let mut placed = 0;
    for (i, f) in fonts.iter().enumerate() {
        if !std::path::Path::new(f).is_file() {
            continue;
        }
        let (verts, faces) = crate::text::text_mesh(f, "ANVIL 8B", 4.0, 1.0).unwrap();
        // closed solid: signed volume == front area * depth, and positive (outward faces)
        let m = polymesh::poly_trimesh(&verts, &faces);
        let vol: f32 = m.tris.iter().map(|t| m.pos[t[0] as usize].dot(m.pos[t[1] as usize].cross(m.pos[t[2] as usize])) / 6.0).sum();
        let front: f32 = faces
            .iter()
            .filter(|f| f.len() == 3 && f.iter().all(|&i| verts[i as usize].z > 0.0))
            .map(|f| (verts[f[1] as usize] - verts[f[0] as usize]).cross(verts[f[2] as usize] - verts[f[0] as usize]).z * 0.5)
            .sum();
        assert!(front > 5.0, "{f}: front area {front}");
        assert!((vol - front * 1.0).abs() < front * 0.01, "{f}: volume {vol} vs area*depth {front} (holes / winding wrong)");
        let (mn, _) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(mn.y.abs() < 1e-4, "text should stand on y = 0");
        let mut o = Object::new(Kind::Mesh { verts, faces }, Vec3::new(0.0, i as f32 * 5.0, 0.0), [70, 110, 230], Material::Plastic);
        o.text = Some(crate::map::TextSpec { text: "ANVIL 8B".into(), font: f.to_string(), font_name: "test".into(), height: 4.0, depth: 1.0 });
        map.add(o);
        placed += 1;
    }
    if placed == 0 {
        return;
    }
    // saved as a plain mesh: reloads without needing the font
    let p = out_dir("text").join("text.anvil");
    map.save(&p).unwrap();
    assert_eq!(Map::load(&p).unwrap().objects.len(), placed);

    let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
    let ctx = eframe::egui::Context::default();
    let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(800.0, 360.0));
    let cam = Camera::looking_at(Vec3::new(4.0, 7.0, 22.0), Vec3::new(0.0, 4.5, 0.0));
    let opts = RenderOpts { fov: 60.0, grid: 1.0, plane_y: -1.0, show_grid: false, outlines: false, bg: [40, 44, 52], scale: 1.0, filter: true, lighting: Default::default() };
    let mut r = Renderer::default();
    r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut crate::texture::TexCache::default());
    let (w, h, px) = r.pixels();
    image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(out_dir("text").join("text.png")).unwrap();
}

mod faces_and_views {
    use super::*;
    use crate::map::{FaceTex, TexParams};
    use crate::plugins::ViewInfo;
    use crate::view3d::{Camera, RenderOpts, Renderer};
    use eframe::egui::{pos2, vec2, Pos2, Rect};

    fn textures(dir: &std::path::Path) -> (Map, u64, u64) {
        image::RgbaImage::from_fn(32, 32, |x, y| if (x / 4 + y / 4) % 2 == 0 { image::Rgba([230, 230, 230, 255]) } else { image::Rgba([120, 120, 120, 255]) }).save(dir.join("a.png")).unwrap();
        image::RgbaImage::from_fn(32, 32, |x, _| if x % 8 < 4 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([60, 60, 60, 255]) }).save(dir.join("b.png")).unwrap();
        let mut map = Map::default();
        let a = map.add_texture(TextureDef { roblox_image_id: "111".into(), ..TextureDef::new("plaster".into(), dir.join("a.png").to_string_lossy().to_string()) });
        let b = map.add_texture(TextureDef { roblox_image_id: "222".into(), ..TextureDef::new("stripes".into(), dir.join("b.png").to_string_lossy().to_string()) });
        (map, a, b)
    }

    #[test]
    fn face_textures_preview_and_export() {
        let dir = out_dir("faces");
        let (mut map, a, b) = textures(&dir);
        let mut wall = Object::brush(Shape::Box, Vec3::new(8.0, 6.0, 1.0), Vec3::new(0.0, 3.0, 0.0), [255, 255, 255], Material::Brick);
        wall.texture = Some(a);
        *wall.face_tex_mut(4) = Some(FaceTex { texture: Some(b), params: TexParams { tint: [220, 40, 40], ..TexParams::default() } });
        let id = map.add(wall);

        // preview: two texture slots, the +Z face uses the striped one
        let wm = crate::mesh::world_mesh(map.get(id).unwrap(), &map.textures, &mut ModelCache::default());
        assert_eq!(wm.texs.len(), 2);
        let o = map.get(id).unwrap();
        for ti in 0..wm.mesh.tris.len() {
            let face = crate::mesh::tri_face_key(o, &wm.mesh, ti).unwrap();
            let slot = wm.tri_tex[ti] as usize;
            assert_eq!(wm.texs[slot].image.ends_with("b.png"), face == 4, "tri {ti} face {face}");
        }
        assert_eq!(wm.texs.iter().find(|t| t.image.ends_with("b.png")).unwrap().tint, [220, 40, 40]);

        // Roblox: five faces with the object's texture, the Back face (+Z) striped and tinted
        let nodes = export_roblox::build_nodes(&map, &mut ModelCache::default(), &Settings::default());
        let xml = export_roblox::to_rbxmx(&nodes, "t");
        assert_eq!(xml.matches("rbxassetid://111").count(), 5);
        assert_eq!(xml.matches("rbxassetid://222").count(), 1);
        let i = xml.find("rbxassetid://222").unwrap();
        let block = &xml[xml[..i].rfind("<Item class=\"Texture\"").unwrap()..i + 200];
        assert!(block.contains("<token name=\"Face\">2</token>") && block.contains("<R>0.8627451</R>"), "{block}");
        let lua = export_roblox::to_luau(&nodes, "t");
        assert!(lua.contains("tx.Color3 = Color3.fromRGB(220, 40, 40)"));

        // Unity: two textured materials, the tinted one carries the tint
        let msg = export_unity::export(&map, &mut ModelCache::default(), &Settings::default(), &dir.join("faces.obj")).unwrap();
        assert!(msg.contains("2 texture(s)"), "{msg}");
        let mtl = std::fs::read_to_string(dir.join("faces.mtl")).unwrap();
        assert!(mtl.contains("Kd 0.8627 0.1569 0.1569"), "{mtl}");
    }

    #[test]
    fn ortho_camera_rays_hit_what_they_show() {
        for ortho in [0.0f32, 12.0] {
            let mut cam = Camera::looking_at(Vec3::new(10.0, 20.0, 30.0), Vec3::ZERO);
            cam.ortho = ortho;
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0));
            for p in [Vec3::ZERO, Vec3::new(3.0, 1.0, -2.0), Vec3::new(-5.0, 4.0, 2.0)] {
                let s = cam.project(rect, 60.0, p).unwrap();
                let (o, d) = cam.ray(rect, 60.0, s);
                let closest = o + d * (p - o).dot(d);
                assert!((closest - p).length() < 1e-3, "ortho={ortho}: ray misses {p} by {}", (closest - p).length());
            }
        }
    }

    #[test]
    fn vertex_gizmo_moves_rotates_scales() {
        use crate::gizmo::{hit, GDrag, MOVE, ROTATE, SCALE};
        let mut v = crate::view2d::View2D::new(crate::view2d::ViewKind::Top);
        v.center = Vec2::ZERO;
        v.zoom = 8.0;
        let view = ViewInfo::Ortho { view: v, rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)) };
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        let c = Vec3::ZERO;
        let pc = pos2(400.0, 300.0);
        // move: grab the X arrow, drag 80 px -> 10 units (grid 1)
        let a = pc + vec2(50.0, 0.0);
        assert_eq!(hit(&view, MOVE, c, axes, a), Some(0));
        let g = GDrag::begin(&view, MOVE, 0, c, axes, a);
        let p = g.xf(&view, a + vec2(80.0, 4.0), 1.0, true).apply(Vec3::new(1.0, 2.0, 3.0));
        assert!((p - Vec3::new(11.0, 2.0, 3.0)).length() < 1e-4, "{p}");
        // rotate a quarter turn about Y
        let r = 0.8 * 85.0;
        let at = |deg: f32| pc + vec2(deg.to_radians().cos(), deg.to_radians().sin()) * r;
        assert_eq!(hit(&view, ROTATE, c, axes, at(-45.0)), Some(1));
        let g = GDrag::begin(&view, ROTATE, 1, c, axes, at(-45.0));
        let p = g.xf(&view, at(-135.0), 1.0, true).apply(Vec3::new(1.0, 0.0, 0.0));
        assert!((p - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4, "{p}");
        // scale x2 along X around the center
        let g = GDrag::begin(&view, SCALE, 0, c, axes, a);
        let p = g.xf(&view, a + vec2(85.0, 0.0), 1.0, true).apply(Vec3::new(2.0, 1.0, 1.0));
        assert!((p - Vec3::new(4.0, 1.0, 1.0)).length() < 1e-3, "{p}");
    }

    #[test]
    fn renders_top_view_with_face_textures() {
        let dir = out_dir("faces");
        let (mut map, a, b) = textures(&dir);
        map.add(Object::brush(Shape::Box, Vec3::new(40.0, 1.0, 30.0), Vec3::new(0.0, -0.5, 0.0), [110, 112, 118], Material::Slate));
        let walls = crate::building::house_walls(&[Vec3::new(-10.0, 0.0, -8.0), Vec3::new(10.0, 0.0, -8.0), Vec3::new(10.0, 0.0, 8.0), Vec3::new(-10.0, 0.0, 8.0)], 1.0, 8.0, 0.0, [255, 255, 255], Material::Brick);
        for mut w in walls {
            w.texture = Some(a);
            *w.face_tex_mut(2) = Some(FaceTex { texture: Some(b), params: TexParams { tint: [220, 60, 50], ..TexParams::default() } });
            map.add(w);
        }
        let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
        let ctx = eframe::egui::Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0));
        let mut r = Renderer::default();
        let opts = RenderOpts { fov: 60.0, grid: 1.0, plane_y: 0.0, show_grid: true, outlines: true, bg: [58, 66, 80], scale: 1.0, filter: true, lighting: Default::default() };
        for (name, ortho, yaw, pitch) in [("top", 14.0, 0.0, -std::f32::consts::FRAC_PI_2), ("persp", 0.0, 0.6, -0.6)] {
            let mut cam = Camera::looking_at(Vec3::new(-16.0, 20.0, 24.0), Vec3::ZERO);
            if ortho > 0.0 {
                cam.yaw = yaw;
                cam.pitch = pitch;
                cam.ortho = ortho;
                cam.pos = Vec3::ZERO - cam.forward() * 5000.0;
            }
            r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut crate::texture::TexCache::default());
            let (w, h, px) = r.pixels();
            image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        }
    }
}

mod knife {
    use crate::knife::knife_cut;
    use crate::map::{FaceTex, Hole, Kind, Map, Material, Object, Shape, TexParams};
    use crate::polymesh;
    use glam::Vec3;
    use std::collections::HashSet;

    fn volume(o: &Object) -> f32 {
        match &o.kind {
            Kind::Brush { size, .. } => size.x * size.y * size.z,
            Kind::Mesh { verts, faces } => {
                let m = polymesh::poly_trimesh(verts, faces);
                m.tris.iter().map(|t| m.pos[t[0] as usize].dot(m.pos[t[1] as usize].cross(m.pos[t[2] as usize])) / 6.0).sum()
            }
            _ => 0.0,
        }
    }

    /// Every edge is used once in each direction: no holes, no flipped faces.
    fn closed(o: &Object) -> bool {
        let Kind::Mesh { faces, .. } = &o.kind else { return true };
        let mut e = HashSet::new();
        for f in faces {
            for i in 0..f.len() {
                if !e.insert((f[i], f[(i + 1) % f.len()])) {
                    return false;
                }
            }
        }
        e.iter().all(|(a, b)| e.contains(&(*b, *a)))
    }

    fn one(o: Object) -> (Map, u64) {
        let mut m = Map::default();
        let id = m.add(o);
        (m, id)
    }

    #[test]
    fn axis_cut_keeps_two_boxes_and_splits_holes() {
        let mut o = Object::brush(Shape::Box, Vec3::new(8.0, 4.0, 2.0), Vec3::new(10.0, 0.0, 0.0), [200; 3], Material::Concrete);
        if let Kind::Brush { holes, .. } = &mut o.kind {
            holes.push(Hole { min: Vec3::new(0.0, -2.0, -1.0), max: Vec3::new(2.0, 1.0, 1.0) });
        }
        let (mut m, id) = one(o);
        let (n, ids) = knife_cut(&mut m, Vec3::new(11.0, 0.0, 0.0), Vec3::X, &[id], true);
        assert_eq!(n, 1);
        assert_eq!(ids.len(), 2);
        let a = m.get(ids[0]).unwrap();
        let b = m.get(ids[1]).unwrap();
        for (p, w, x) in [(a, 5.0, 8.5), (b, 3.0, 12.5)] {
            let Kind::Brush { shape, size, holes, .. } = &p.kind else { panic!("not a brush") };
            assert_eq!(*shape, Shape::Box);
            assert!((size.x - w).abs() < 1e-4, "{size}");
            assert!((p.pos.x - x).abs() < 1e-4, "{}", p.pos);
            assert_eq!(holes.len(), 1, "the door spans the cut, so both halves get part of it");
            // hole stays in the same world place
            let hw = p.pos.x + holes[0].min.x;
            assert!(hw >= 9.999 && hw <= 11.001, "{hw}");
        }
        assert!((volume(a) + volume(b) - 64.0).abs() < 1e-3);
    }

    #[test]
    fn diagonal_cut_makes_two_closed_meshes_with_face_textures() {
        let mut o = Object::brush(Shape::Box, Vec3::new(4.0, 4.0, 4.0), Vec3::new(0.0, 2.0, 0.0), [200; 3], Material::Concrete);
        o.rot = Vec3::new(0.0, 30.0, 0.0);
        let tex = FaceTex { texture: Some(7), params: TexParams::default() };
        *o.face_tex_mut(2) = Some(tex.clone()); // +Y top
        let (mut m, id) = one(o.clone());
        let n = Vec3::new(1.0, 0.0, 1.0).normalize();
        let (count, ids) = knife_cut(&mut m, o.pos, n, &[id], true);
        assert_eq!(count, 1);
        assert_eq!(ids.len(), 2);
        let mut total = 0.0;
        for i in &ids {
            let p = m.get(*i).unwrap();
            assert!(matches!(p.kind, Kind::Mesh { .. }));
            assert!(closed(p), "piece {} is not closed", p.name);
            total += volume(p);
            assert!(p.face_tex.iter().any(|f| f.as_ref().is_some_and(|f| f.texture == Some(7))), "top texture kept on both halves");
        }
        assert!((total - 64.0).abs() < 1e-2, "volume {total}");
        // pieces stay where the box was
        let c = (m.get(ids[0]).unwrap().pos + m.get(ids[1]).unwrap().pos) * 0.5;
        assert!((c - o.pos).length() < 1.5, "{c}");
    }

    #[test]
    fn wedge_and_rotated_mesh_split() {
        let mut o = Object::brush(Shape::Wedge, Vec3::new(4.0, 2.0, 6.0), Vec3::ZERO, [200; 3], Material::Concrete);
        o.rot = Vec3::new(0.0, 90.0, 0.0);
        let v0 = 4.0 * 2.0 * 6.0 / 2.0;
        let (mut m, id) = one(o);
        let (count, ids) = knife_cut(&mut m, Vec3::new(0.0, 0.0, 0.5), Vec3::Z, &[id], true);
        assert_eq!(count, 1);
        let total: f32 = ids.iter().map(|i| volume(m.get(*i).unwrap())).sum();
        assert!((total - v0).abs() < 1e-2, "{total} vs {v0}");
        assert!(ids.iter().all(|i| closed(m.get(*i).unwrap())));
    }

    #[test]
    fn slice_keeps_one_object_with_more_faces() {
        let (v, f) = polymesh::box_poly(Vec3::splat(2.0));
        let mut o = Object::new(Kind::Mesh { verts: v, faces: f }, Vec3::new(5.0, 1.0, 0.0), [200; 3], Material::Plastic);
        o.rot = Vec3::new(0.0, 0.0, 0.0);
        let (mut m, id) = one(o.clone());
        let (count, ids) = knife_cut(&mut m, Vec3::new(5.3, 0.0, 0.0), Vec3::new(1.0, 0.2, 0.0).normalize(), &[id], false);
        assert_eq!(count, 1);
        assert_eq!(ids, vec![id]);
        assert_eq!(m.objects.len(), 1);
        let s = m.get(id).unwrap();
        let Kind::Mesh { faces, .. } = &s.kind else { panic!() };
        assert_eq!(faces.len(), 10, "4 side faces split in two + 2 untouched");
        assert!(closed(s));
        assert!((volume(s) - 8.0).abs() < 1e-3);
        assert!((s.pos - o.pos).length() < 1e-5, "slice keeps the pivot");
    }

    #[test]
    fn angled_cut_through_wall_with_door() {
        let mut o = Object::brush(Shape::Box, Vec3::new(12.0, 8.0, 1.0), Vec3::ZERO, [200; 3], Material::Concrete);
        if let Kind::Brush { holes, .. } = &mut o.kind {
            holes.push(Hole { min: Vec3::new(-2.0, -4.0, -0.5), max: Vec3::new(2.0, 2.0, 0.5) });
        }
        let solid = 12.0 * 8.0 - 4.0 * 6.0;
        let (mut m, id) = one(o);
        let (count, ids) = knife_cut(&mut m, Vec3::new(4.0, 0.0, 0.0), Vec3::new(1.0, 0.5, 0.0).normalize(), &[id], true);
        assert_eq!(count, 1);
        let total: f32 = ids.iter().map(|i| volume(m.get(*i).unwrap())).sum();
        assert!((total - solid).abs() < 0.05, "{total} vs {solid}");
        assert!(ids.iter().all(|i| closed(m.get(*i).unwrap())), "the cut runs exactly through box corners; caps must still close");
    }

    #[test]
    fn missing_cut_changes_nothing() {
        let o = Object::brush(Shape::Box, Vec3::splat(2.0), Vec3::ZERO, [200; 3], Material::Concrete);
        let (mut m, id) = one(o);
        let before = serde_json::to_string(&m).unwrap();
        let (count, _) = knife_cut(&mut m, Vec3::new(50.0, 0.0, 0.0), Vec3::new(1.0, 0.3, 0.0).normalize(), &[id], true);
        assert_eq!(count, 0);
        assert_eq!(before, serde_json::to_string(&m).unwrap());
        let (count, _) = knife_cut(&mut m, Vec3::new(50.0, 0.0, 0.0), Vec3::new(1.0, 0.3, 0.0).normalize(), &[id], false);
        assert_eq!(count, 0);
    }
}

mod team_create {
    use crate::map::{Kind, Map, Material, Object, Shape, TextureDef};
    use crate::team::*;
    use glam::Vec3;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct P {
        s: Session,
        m: Map,
        pr: Presence,
        got: Vec<Pumped>,
    }

    impl P {
        fn new(s: Session, m: Map) -> P {
            P { s, m, pr: Presence::default(), got: Vec::new() }
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = super::out_dir(&format!("team_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wait(ps: &mut [&mut P], what: &str, f: impl Fn(&[&mut P]) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            for p in ps.iter_mut() {
                let r = p.s.pump(&mut p.m, &p.pr);
                p.got.push(r);
            }
            if f(ps) {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!("timed out waiting for: {what}");
    }

    fn host_map(dir: &PathBuf) -> (Map, String, Vec<u8>) {
        let png = dir.join("brick.png");
        let img = image::RgbaImage::from_fn(8, 8, |x, y| image::Rgba([(x * 30) as u8, (y * 30) as u8, 99, 255]));
        img.save(&png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        let mut m = Map::default();
        let tid = m.add_texture(TextureDef::new("brick".into(), png.to_string_lossy().to_string()));
        let mut o = Object::brush(Shape::Box, Vec3::new(4.0, 2.0, 1.0), Vec3::ZERO, [200; 3], Material::Brick);
        o.texture = Some(tid);
        m.add(o);
        (m, png.to_string_lossy().to_string(), bytes)
    }

    #[test]
    fn host_and_guests_edit_together() {
        let d = dir("session");
        let (mut hm, png, png_bytes) = host_map(&d);
        let hs = Session::host(&mut hm, "Hosty", [255, 0, 0], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code.to_lowercase(), "Guesty", [0, 255, 0], d.join("guest_cache")).unwrap(), Map::default());

        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        assert_eq!(h.s.pending[0].name, "Guesty");
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        wait(&mut [&mut h, &mut g], "welcome", |p| p[1].s.welcomed && p[1].m.objects.len() == 1);
        assert!(g.got.iter().any(|r| r.map_replaced));
        assert!(g.got.iter().any(|r| r.block.contains(&png)), "host's file path is blocked for the guest's loaders");
        assert!(g.m.id_base >= 2 << 40 && h.m.id_base == 1 << 40);

        // the texture file comes over and lands in the sandbox folder
        wait(&mut [&mut h, &mut g], "texture download", |p| p[1].got.iter().any(|r| !r.assets.is_empty()));
        let (key, local) = g.got.iter().flat_map(|r| r.assets.clone()).next().unwrap();
        assert_eq!(key, png);
        assert!(local.starts_with(d.join("guest_cache")));
        assert_eq!(std::fs::read(&local).unwrap(), png_bytes);

        // guest moves the wall, host sees it
        let wall = g.m.objects[0].id;
        g.m.get_mut(wall).unwrap().pos = Vec3::new(5.0, 0.0, 0.0);
        wait(&mut [&mut h, &mut g], "guest move", |p| p[0].m.get(wall).is_some_and(|o| o.pos.x == 5.0));
        // guest adds an object in its own id range
        let new_id = g.m.add(Object::brush(Shape::Box, Vec3::ONE, Vec3::Y * 3.0, [1, 2, 3], Material::Plastic));
        assert!(new_id > g.m.id_base);
        wait(&mut [&mut h, &mut g], "guest add", |p| p[0].m.get(new_id).is_some());
        // host adds and renames the map, guest sees it
        let hid = h.m.add(Object::brush(Shape::Wedge, Vec3::ONE, Vec3::ZERO, [9, 9, 9], Material::Plastic));
        h.m.name = "Our level".into();
        wait(&mut [&mut h, &mut g], "host add", |p| p[1].m.get(hid).is_some() && p[1].m.name == "Our level");
        assert_ne!(hid, new_id);
        // guest deletes, host loses it
        g.m.objects.retain(|o| o.id != hid);
        wait(&mut [&mut h, &mut g], "guest delete", |p| p[0].m.get(hid).is_none());

        // selections show in the other person's color
        g.pr.sel = vec![wall];
        g.pr.cam = Vec3::new(1.0, 2.0, 3.0);
        wait(&mut [&mut h, &mut g], "presence", |p| p[0].s.remote_selection().get(&wall) == Some(&[0, 255, 0]));
        assert_eq!(h.s.peers.values().next().unwrap().presence.as_ref().unwrap().cam, Vec3::new(1.0, 2.0, 3.0));

        // a second guest sees both others, but never gets anyone's IP
        let mut g2 = P::new(Session::join(&addr, &code, "Third", [0, 0, 255], d.join("g2_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g, &mut g2], "second join", |p| p[0].s.pending.len() == 1);
        let c2 = h.s.pending[0].conn;
        h.s.approve(c2, &h.m);
        wait(&mut [&mut h, &mut g, &mut g2], "second welcome", |p| p[2].s.welcomed && p[1].s.peers.len() == 2 && p[2].s.peers.len() == 2);
        assert_eq!(g2.m.objects.len(), h.m.objects.len());
        assert!(g2.s.peers.values().all(|r| r.addr.is_none()), "guests don't learn addresses");
        assert!(g2.m.id_base != g.m.id_base);

        // edits flow guest -> host -> other guest
        g2.m.get_mut(wall).unwrap().color = [7, 7, 7];
        wait(&mut [&mut h, &mut g, &mut g2], "relay", |p| p[1].m.get(wall).is_some_and(|o| o.color == [7, 7, 7]));

        // two people edit the same thing at once: everyone ends up agreeing
        g.m.get_mut(wall).unwrap().pos.y = 11.0;
        g2.m.get_mut(wall).unwrap().pos.y = 22.0;
        wait(&mut [&mut h, &mut g, &mut g2], "convergence", |p| {
            let ys: Vec<f32> = p.iter().map(|x| x.m.get(wall).map(|o| o.pos.y).unwrap_or(-1.0)).collect();
            ys[0] == ys[1] && ys[1] == ys[2] && (ys[0] == 11.0 || ys[0] == 22.0)
        });

        // host stops: guests are told why
        h.s.leave("The host stopped the session.");
        wait(&mut [&mut g, &mut g2], "end", |p| p[0].s.ended.is_some() && p[1].s.ended.is_some());
        assert!(g.s.ended.as_ref().unwrap().contains("host stopped"), "{:?}", g.s.ended);
        let mut m = g.m.clone();
        end_ranges(&mut m);
        assert_eq!(m.id_base, 0);
        let fresh = m.add(Object::brush(Shape::Box, Vec3::ONE, Vec3::ZERO, [0; 3], Material::Plastic));
        assert!(g.m.objects.iter().all(|o| o.id != fresh), "ids after the session don't collide");
    }

    #[test]
    fn props_arrive_textured() {
        let d = dir("props");
        // a model pack: models/barrel.obj -> barrel.mtl -> ../textures/wood.png
        let pack = d.join("pack");
        std::fs::create_dir_all(pack.join("models")).unwrap();
        std::fs::create_dir_all(pack.join("textures")).unwrap();
        let wood = pack.join("textures").join("wood.png");
        image::RgbaImage::from_fn(8, 8, |x, y| image::Rgba([120 + x as u8, 70 + y as u8, 30, 255])).save(&wood).unwrap();
        // a file elsewhere the .mtl also points at: must never be sent
        let elsewhere = d.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let secret = elsewhere.join("secret.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])).save(&secret).unwrap();
        std::fs::write(pack.join("models").join("barrel.mtl"), format!("newmtl wood\nKd 1 1 1\nmap_Kd ../textures/wood.png\nmap_Bump {}\n", secret.display())).unwrap();
        std::fs::write(
            pack.join("models").join("barrel.obj"),
            "mtllib barrel.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl wood\nf 1/1 2/2 3/3\n",
        )
        .unwrap();
        let model = pack.join("models").join("barrel.obj").to_string_lossy().to_string();
        let mut hm = Map::default();
        hm.add(Object::new(Kind::Prop { model: model.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Wood));
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        wait(&mut [&mut h, &mut g], "model download", |p| p[1].got.iter().any(|r| r.assets.iter().any(|a| a.0 == model)));
        let local = g.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == model).unwrap().1;
        assert!(local.starts_with(d.join("guest_cache")), "{}", local.display());
        let m = crate::mesh::load_model(&local.to_string_lossy()).unwrap();
        let tex = m.materials.iter().find(|t| !t.is_empty()).expect("the prop arrives with its texture");
        assert!(std::path::Path::new(tex).starts_with(local.parent().unwrap()), "texture sits in the model's sandbox folder: {tex}");
        assert_eq!(std::fs::read(tex).unwrap(), std::fs::read(&wood).unwrap(), "same texture");
        assert_eq!(m.uv.len(), m.pos.len());
        let names: Vec<String> = std::fs::read_dir(local.parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(names.iter().all(|n| !n.contains("secret")), "files outside the model's folder area aren't sent: {names:?}");
    }

    #[test]
    fn guest_props_reach_the_host_textured() {
        let d = dir("guest_props");
        let pack = d.join("guestpack");
        std::fs::create_dir_all(&pack).unwrap();
        let tex = pack.join("crate_diffuse.png");
        image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba([x as u8 * 50, 90, 40, 255])).save(&tex).unwrap();
        std::fs::write(pack.join("crate.mtl"), "newmtl c\nmap_Kd crate_diffuse.png\n").unwrap();
        std::fs::write(pack.join("crate.obj"), "mtllib crate.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl c\nf 1/1 2/2 3/3\n").unwrap();
        let model = pack.join("crate.obj").to_string_lossy().to_string();
        let mut hm = Map::default();
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        wait(&mut [&mut h, &mut g], "welcome", |p| p[1].s.welcomed);
        // the guest places a prop from its own disk
        g.m.add(Object::new(Kind::Prop { model: model.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Wood));
        wait(&mut [&mut h, &mut g], "host gets the model", |p| p[0].got.iter().any(|r| r.assets.iter().any(|a| a.0 == model)));
        let local = h.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == model).unwrap().1;
        let m = crate::mesh::load_model(&local.to_string_lossy()).unwrap();
        let t = m.materials.iter().find(|t| !t.is_empty()).expect("textured on the host too");
        assert_eq!(std::fs::read(t).unwrap(), std::fs::read(&tex).unwrap());
    }

    /// A real Half-Life prop as Anvil imports it (textures two folders up,
    /// in the content folder). Point ANVIL_REAL_CONTENT at a content folder
    /// with models/props_interiors/computer_monitor_p1.obj to run it.
    #[test]
    fn real_imported_prop_arrives_textured() {
        let Ok(src) = std::env::var("ANVIL_REAL_CONTENT") else { return };
        let src = std::path::PathBuf::from(src);
        let rel_obj = "models/props_interiors/computer_monitor_p1.obj";
        if !src.join(rel_obj).is_file() {
            return;
        }
        // copy it into this program's content folder, same layout as an import
        let content = crate::mount::content_dir();
        for rel in [rel_obj, "models/props_interiors/computer_monitor_p1.mtl", "materials/models/props_interiors/computer.png", "materials/models/props_interiors/screen.png"] {
            let to = content.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(src.join(rel), &to).unwrap();
        }
        let model = content.join(rel_obj).to_string_lossy().to_string();
        let d = dir("real_prop");
        let mut hm = Map::default();
        hm.add(Object::new(Kind::Prop { model: model.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Plastic));
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        wait(&mut [&mut h, &mut g], "model", |p| p[1].got.iter().any(|r| r.assets.iter().any(|a| a.0 == model)));
        let local = g.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == model).unwrap().1;
        let m = crate::mesh::load_model(&local.to_string_lossy()).unwrap();
        let texs: Vec<&String> = m.materials.iter().filter(|t| !t.is_empty()).collect();
        println!("REAL prop: {} triangles, textures {:?}", m.tris.len(), texs.iter().map(|t| std::path::Path::new(t).file_name().unwrap().to_string_lossy()).collect::<Vec<_>>());
        assert_eq!(texs.len(), 3, "all three materials textured");
        assert!(texs.iter().all(|t| std::path::Path::new(t).is_file()));
        let monitor_png = std::fs::read(content.join("materials/models/props_interiors/computer.png")).unwrap();
        assert!(texs.iter().any(|t| std::fs::read(t).unwrap() == monitor_png));
    }

    #[test]
    fn hostile_model_bundles_are_refused() {
        let png = {
            let mut c = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255])).write_to(&mut c, image::ImageFormat::Png).unwrap();
            b64_encode(c.get_ref())
        };
        assert!(check_bundle("m.obj", &[("ab12_wood.png".into(), png.clone()), ("m.mtl".into(), b64_encode(b"newmtl x"))]).is_ok());
        for bad in ["../evil.png", ".hidden.png", "sub/x.png", r"sub\x.png", "x.exe", "C:x.png", ""] {
            assert!(check_bundle("m.obj", &[(bad.to_string(), png.clone())]).is_err(), "{bad}");
            assert!(check_bundle("m.glb", &[(bad.to_string(), png.clone())]).is_err(), "{bad}");
        }
        assert!(check_bundle("m.obj", &[("fake.png".into(), b64_encode(b"MZ not a picture"))]).is_err(), "not really an image");
        let many: Vec<(String, String)> = (0..70).map(|i| (format!("t{i}.png"), png.clone())).collect();
        assert!(check_bundle("m.obj", &many).is_err(), "too many files");
        assert!(check_bundle("m.obj", &[("a.png".into(), png.clone()), ("A.PNG".into(), png.clone())]).is_err(), "same name twice");
        // glTF models bring a .bin and textures, nothing else
        assert!(check_bundle("m.glb", &[("x_mesh.bin".into(), b64_encode(&[1, 2, 3])), ("x_tex0.png".into(), png.clone())]).is_ok());
        assert!(check_bundle("m.glb", &[("x.mtl".into(), b64_encode(b"newmtl x"))]).is_err());
        assert!(check_bundle("m.obj", &[("x.bin".into(), b64_encode(&[1]))]).is_err());
        // and a received .gltf may only point at those files
        let files = vec![("x_mesh.bin".to_string(), vec![0u8; 4])];
        assert!(check_gltf(br#"{"buffers":[{"uri":"x_mesh.bin","byteLength":4}]}"#, &files).is_ok());
        for bad in [r#"{"buffers":[{"uri":"../../secret.bin"}]}"#, r#"{"images":[{"uri":"C:/Users/me/a.png"}]}"#, r#"{"images":[{"uri":"other.png"}]}"#, r#"{"buffers":[{"uri":"\\\\pc\\share\\a.bin"}]}"#, "MZ"] {
            assert!(check_gltf(bad.as_bytes(), &files).is_err(), "{bad}");
        }
    }

    /// A Sketchfab-style .gltf (separate .bin, textures in a subfolder) and a
    /// .glb with an embedded texture both arrive textured.
    #[test]
    fn sketchfab_models_arrive_textured() {
        let d = dir("gltf_props");
        let src = d.join("downloads").join("abc_crate").join("source");
        std::fs::create_dir_all(src.join("textures")).unwrap();
        let tex = src.join("textures").join("crate_baseColor.png");
        image::RgbaImage::from_fn(16, 16, |x, y| image::Rgba([x as u8 * 15, y as u8 * 15, 90, 255])).save(&tex).unwrap();
        let mut bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        bin.extend([0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()));
        std::fs::write(src.join("scene.bin"), &bin).unwrap();
        let gltf = serde_json::json!({
            "asset": { "version": "2.0" }, "scene": 0, "scenes": [{ "nodes": [0] }], "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "TEXCOORD_0": 1 }, "material": 0 }] }],
            "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }],
            "textures": [{ "source": 0 }], "images": [{ "uri": "textures/crate_baseColor.png" }],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" }
            ],
            "bufferViews": [{ "buffer": 0, "byteLength": 36 }, { "buffer": 0, "byteOffset": 36, "byteLength": 24 }],
            "buffers": [{ "uri": "scene.bin", "byteLength": 60 }]
        });
        std::fs::write(src.join("scene.gltf"), gltf.to_string()).unwrap();
        let gltf_model = src.join("scene.gltf").to_string_lossy().to_string();
        // a .glb with its texture inside (what Sketchfab's "glb" download is)
        let png = d.join("t.png");
        image::RgbaImage::from_fn(8, 8, |x, _| image::Rgba([x as u8 * 30, 200, 10, 255])).save(&png).unwrap();
        let mut scene = Map::default();
        let t = scene.add_texture(TextureDef::new("t".into(), png.to_string_lossy().to_string()));
        let mut b = Object::brush(Shape::Box, Vec3::splat(2.0), Vec3::ZERO, [255; 3], Material::Plastic);
        b.texture = Some(t);
        scene.add(b);
        let glb_path = d.join("downloads").join("def_barrel.glb");
        std::fs::write(&glb_path, crate::blender_export::scene_glb(&scene, &mut crate::mesh::ModelCache::default(), 1.0)).unwrap();
        let glb_model = glb_path.to_string_lossy().to_string();

        let mut hm = Map::default();
        for m in [&gltf_model, &glb_model] {
            hm.add(Object::new(Kind::Prop { model: m.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Plastic));
        }
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        let got = |p: &P, m: &str| p.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == m).map(|a| a.1);
        wait(&mut [&mut h, &mut g], "both models", |p| got(p[1], &gltf_model).is_some() && got(p[1], &glb_model).is_some());
        for m in [&gltf_model, &glb_model] {
            let local = got(&g, m).unwrap();
            assert!(local.starts_with(d.join("guest_cache")), "{}", local.display());
            let theirs = crate::mesh::load_model(m).unwrap();
            let mine = crate::mesh::load_model(&local.to_string_lossy()).unwrap_or_else(|e| panic!("{}: {e}", local.display()));
            assert_eq!(mine.tris.len(), theirs.tris.len(), "same shape");
            assert_eq!(mine.uv.len(), mine.pos.len());
            let t = mine.materials.iter().find(|t| !t.is_empty()).unwrap_or_else(|| panic!("{m} arrives textured: {:?}", mine.materials));
            assert!(std::path::Path::new(t).is_file(), "{t}");
            let a = image::open(t).unwrap().to_rgba8();
            let b = image::open(theirs.materials.iter().find(|t| !t.is_empty()).unwrap()).unwrap().to_rgba8();
            assert_eq!(a.as_raw(), b.as_raw(), "same texture pixels for {m}");
        }
        // only the model's own files went: one .gltf/.glb, one .bin, its textures
        let names: Vec<String> = std::fs::read_dir(got(&g, &gltf_model).unwrap().parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(h.s.log.iter().all(|l| !l.contains("can't be sent")), "{:?}", h.s.log);
    }

    /// Real Sketchfab downloads: set ANVIL_REAL_GLB to model paths split by ';'.
    #[test]
    fn real_sketchfab_models_arrive_textured() {
        let Ok(list) = std::env::var("ANVIL_REAL_GLB") else { return };
        let models: Vec<String> = list.split(';').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        let d = dir("real_glb");
        let mut hm = Map::default();
        for m in &models {
            hm.add(Object::new(Kind::Prop { model: m.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Plastic));
        }
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        let got = |p: &P, m: &str| p.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == m).map(|a| a.1);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(60) && !models.iter().all(|m| got(&g, m).is_some()) {
            for p in [&mut h, &mut g] {
                let r = p.s.pump(&mut p.m, &p.pr);
                p.got.push(r);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        println!("after {:?}: host log {:?}\nguest log {:?}", t.elapsed(), h.s.log, g.s.log);
        for m in &models {
            let local = got(&g, m).unwrap_or_else(|| panic!("{m} never arrived"));
            let theirs = crate::mesh::load_model(m).unwrap();
            let mine = crate::mesh::load_model(&local.to_string_lossy()).unwrap();
            let tex: Vec<&String> = mine.materials.iter().filter(|t| !t.is_empty()).collect();
            let size: u64 = std::fs::read_dir(local.parent().unwrap()).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum();
            println!("REAL {}: {} tris (host {}), {} textured materials (host {}), {} KB sent (file {} KB)", m.rsplit(['/', '\\']).next().unwrap(), mine.tris.len(), theirs.tris.len(), tex.len(), theirs.materials.iter().filter(|t| !t.is_empty()).count(), size / 1024, std::fs::metadata(m).unwrap().len() / 1024);
            assert_eq!(mine.tris.len(), theirs.tris.len());
            assert_eq!(tex.len(), theirs.materials.iter().filter(|t| !t.is_empty()).count());
            assert!(tex.iter().all(|t| image::image_dimensions(t).is_ok()));
        }
        println!("host log: {:?}", h.s.log);
    }

    /// A model with huge textures still gets through: they're shrunk to fit.
    #[test]
    fn big_sketchfab_textures_are_shrunk_to_fit() {
        let d = dir("gltf_big");
        let src = d.join("big");
        std::fs::create_dir_all(&src).unwrap();
        // noise doesn't compress: ~40 MB of png, over the 32 MB transfer limit
        let mut seed = 12345u32;
        let img = image::RgbImage::from_fn(3700, 3700, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
        });
        img.save(src.join("huge.png")).unwrap();
        assert!(std::fs::metadata(src.join("huge.png")).unwrap().len() > 33 * 1024 * 1024);
        let bin: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        std::fs::write(src.join("m.bin"), &bin).unwrap();
        let gltf = serde_json::json!({
            "asset": { "version": "2.0" }, "scenes": [{ "nodes": [0] }], "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "TEXCOORD_0": 1 }, "material": 0 }] }],
            "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }],
            "textures": [{ "source": 0 }], "images": [{ "uri": "huge.png" }],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" }
            ],
            "bufferViews": [{ "buffer": 0, "byteLength": 36 }, { "buffer": 0, "byteOffset": 36, "byteLength": 24 }],
            "buffers": [{ "uri": "m.bin", "byteLength": 60 }]
        });
        std::fs::write(src.join("m.gltf"), gltf.to_string()).unwrap();
        let model = src.join("m.gltf").to_string_lossy().to_string();
        let mut hm = Map::default();
        hm.add(Object::new(Kind::Prop { model: model.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [255; 3], Material::Plastic));
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("host_cache")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, &code, "G", [2, 2, 2], d.join("guest_cache")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "join request", |p| p[0].s.pending.len() == 1);
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        wait(&mut [&mut h, &mut g], "model download", |p| p[1].got.iter().any(|r| r.assets.iter().any(|a| a.0 == model)));
        let local = g.got.iter().flat_map(|r| r.assets.clone()).find(|a| a.0 == model).unwrap().1;
        let m = crate::mesh::load_model(&local.to_string_lossy()).unwrap();
        let t = m.materials.iter().find(|t| !t.is_empty()).expect("still textured");
        let (w, hh) = image::image_dimensions(t).unwrap();
        assert!(w <= 2048 && hh <= 2048 && w >= 1024, "{w}x{hh}");
        assert!(h.s.log.iter().any(|l| l.contains("shrunk to 2048")), "{:?}", h.s.log);
    }

    #[test]
    fn wrong_code_and_versions_are_refused() {
        let d = dir("refuse");
        let mut hm = Map::default();
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("c")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let mut h = P::new(hs, hm);
        let mut g = P::new(Session::join(&addr, "WRONG123", "G", [2, 2, 2], d.join("g")).unwrap(), Map::default());
        wait(&mut [&mut h, &mut g], "refusal", |p| p[1].s.ended.is_some());
        assert!(g.s.ended.as_ref().unwrap().contains("Wrong invite code"), "{:?}", g.s.ended);
        assert!(h.s.pending.is_empty());

        // an older Anvil gets a clear version message
        let mut raw = TcpStream::connect(&addr).unwrap();
        raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let hello = serde_json::to_string(&Msg::Hello { proto: PROTOCOL, app: "0.0.1".into(), name: "Old".into(), color: [0; 3], code: h.s.code.clone() }).unwrap();
        raw.write_all(format!("{hello}\n").as_bytes()).unwrap();
        let mut l = String::new();
        BufReader::new(raw).read_line(&mut l).unwrap();
        assert!(l.contains("Version mismatch") && l.contains("0.0.1") && l.contains(APP_VERSION), "{l}");
    }

    /// A hand-written client that sends whatever it likes.
    struct Raw {
        w: TcpStream,
        rx: mpsc::Receiver<String>,
    }

    impl Raw {
        fn connect(addr: &str, code: &str) -> Raw {
            let w = TcpStream::connect(addr).unwrap();
            let r = BufReader::new(w.try_clone().unwrap());
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                for l in r.lines() {
                    let Ok(l) = l else { break };
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                let _ = tx.send("<closed>".into());
            });
            let mut me = Raw { w, rx };
            me.send(&Msg::Hello { proto: PROTOCOL, app: APP_VERSION.into(), name: "Mallory".into(), color: [0; 3], code: code.into() });
            me
        }

        fn send(&mut self, m: &Msg) {
            let s = serde_json::to_string(m).unwrap();
            let _ = self.w.write_all(format!("{s}\n").as_bytes());
        }

        /// Pumps the host until a line containing `needle` arrives.
        fn expect(&mut self, h: &mut P, needle: &str) -> String {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(8) {
                let r = h.s.pump(&mut h.m, &h.pr);
                h.got.push(r);
                while let Ok(l) = self.rx.try_recv() {
                    if needle.split('|').any(|n| l.contains(n)) {
                        return l;
                    }
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            panic!("never got {needle}");
        }
    }

    #[test]
    fn peers_cant_read_files_or_send_bad_data() {
        let d = dir("evil");
        let (mut hm, png, _) = host_map(&d);
        let secret = d.join("secret.png");
        std::fs::write(&secret, b"\x89PNG not for you").unwrap();
        let secret = secret.to_string_lossy().to_string();
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("c")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let code = hs.code.clone();
        let mut h = P::new(hs, hm);
        let mut raw = Raw::connect(&addr, &code);
        let t = Instant::now();
        while h.s.pending.is_empty() && t.elapsed() < Duration::from_secs(5) {
            h.s.pump(&mut h.m, &h.pr);
            std::thread::sleep(Duration::from_millis(10));
        }
        let c = h.s.pending[0].conn;
        h.s.approve(c, &h.m);
        raw.expect(&mut h, "Welcome");

        // a file the map doesn't use is never sent
        let other = d.join("unrelated.png");
        std::fs::copy(&png, &other).unwrap();
        raw.send(&Msg::AssetReq { path: other.to_string_lossy().to_string() });
        let l = raw.expect(&mut h, "AssetMissing|AssetData");
        assert!(l.contains("AssetMissing"), "{l}");

        // ...and adding a file to the map yourself doesn't make the host send it
        let mut textures = h.m.textures.clone();
        let mut t = TextureDef::new("x".into(), secret.clone());
        t.id = 999;
        textures.push(t);
        let mut t2 = TextureDef::new("y".into(), r"\\evil-server\share\steal.png".into());
        t2.id = 1000;
        textures.push(t2);
        raw.send(&Msg::Meta { from: 0, seq: 1, name: "m".into(), folders: vec![], textures });
        raw.expect(&mut h, "Ack");
        assert!(h.m.textures.iter().any(|t| t.image == secret));
        let blocked: Vec<String> = h.got.iter().flat_map(|r| r.block.clone()).collect();
        assert!(blocked.contains(&secret) && blocked.iter().any(|p| p.contains("evil-server")), "peer paths are blocked for the host's loaders");
        raw.send(&Msg::AssetReq { path: secret.clone() });
        let l = raw.expect(&mut h, "AssetMissing|AssetData");
        assert!(l.contains("AssetMissing"), "secret leaked: {l}");
        // the host's own texture is fine to share
        raw.send(&Msg::AssetReq { path: png.clone() });
        let l = raw.expect(&mut h, "AssetMissing|AssetData");
        assert!(l.contains("AssetData"), "{l}");

        // broken geometry gets the sender removed and changes nothing
        let before = h.m.objects.clone();
        let mut bad = Object::new(Kind::Mesh { verts: vec![Vec3::ZERO; 3], faces: vec![vec![0, 1, 99]] }, Vec3::ZERO, [0; 3], Material::Plastic);
        bad.id = 5;
        raw.send(&Msg::Upsert { from: 0, seq: 2, objects: vec![bad] });
        raw.expect(&mut h, "removed you");
        assert_eq!(h.m.objects, before);
        assert!(h.s.peers.is_empty());
    }

    #[test]
    fn oversized_hello_is_dropped() {
        let d = dir("big");
        let mut hm = Map::default();
        let hs = Session::host(&mut hm, "H", [1, 1, 1], 0, false, d.join("c")).unwrap();
        let addr = format!("127.0.0.1:{}", hs.port);
        let mut h = P::new(hs, hm);
        let mut raw = TcpStream::connect(&addr).unwrap();
        raw.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let junk = vec![b'a'; 200 * 1024];
        let _ = raw.write_all(&junk);
        let t = Instant::now();
        let mut buf = [0u8; 16];
        loop {
            h.s.pump(&mut h.m, &h.pr);
            match raw.read(&mut buf) {
                Ok(0) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
                Ok(_) => {}
            }
            assert!(t.elapsed() < Duration::from_secs(6), "host kept the connection open");
        }
        assert!(h.s.pending.is_empty());
    }

    #[test]
    fn helpers_are_safe() {
        use std::net::IpAddr;
        for (ip, ok) in [
            ("192.168.1.5", true),
            ("10.0.0.2", true),
            ("172.20.1.1", true),
            ("127.0.0.1", true),
            ("100.101.102.103", true),
            ("8.8.8.8", false),
            ("1.1.1.1", false),
            ("::1", true),
            ("fd7a:115c:a1e0::1", true),
            ("2001:4860::8888", false),
        ] {
            assert_eq!(is_private_ip(ip.parse::<IpAddr>().unwrap()), ok, "{ip}");
        }
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 % 256) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert!(b64_decode("@@@@").is_none());
        let n = sandbox_name(r"..\..\Windows\System32\evil.png").unwrap();
        assert!(!n.contains('\\') && !n.contains('/') && !n.contains("..") && n.ends_with("_evil.png"), "{n}");
        assert!(sandbox_name("C:/x/run.exe").is_none());
        assert!(sandbox_name("C:/x/model.OBJ").unwrap().ends_with(".obj"));
        let a = new_invite_code();
        assert_eq!(a.len(), 8);
        assert_ne!(a, new_invite_code());
        assert!(crate::texture::is_network_path(r"\\server\share\a.png") && crate::texture::is_network_path("//server/a.png"));
        assert!(!crate::texture::is_network_path(r"C:\a.png"));
        assert!(crate::texture::open_image(r"\\127.0.0.1\c$\nothing.png").is_none());
        let mut o = Object::brush(Shape::Box, Vec3::ONE, Vec3::ZERO, [0; 3], Material::Plastic);
        o.id = 3;
        assert!(validate_object(&o).is_ok());
        o.pos.x = f32::NAN;
        assert!(validate_object(&o).is_err());
    }
}

#[test]
fn model_thumbnails_render_framed() {
    use crate::thumbs::*;
    let dir = out_dir("thumbs");
    // a tall thin box: the frame must fit it whatever its size
    let obj = dir.join("pillar.obj");
    let mut s = String::new();
    for (x, y, z) in [(-1.0, 0.0, -1.0), (1.0, 0.0, -1.0), (1.0, 0.0, 1.0), (-1.0, 0.0, 1.0), (-1.0, 30.0, -1.0), (1.0, 30.0, -1.0), (1.0, 30.0, 1.0), (-1.0, 30.0, 1.0)] {
        s += &format!("v {x} {y} {z}\n");
    }
    for f in ["1 2 3 4", "8 7 6 5", "1 5 6 2", "2 6 7 3", "3 7 8 4", "4 8 5 1"] {
        s += &format!("f {f}\n");
    }
    std::fs::write(&obj, s).unwrap();
    let mut map = Map::default();
    map.add(Object::new(Kind::Prop { model: obj.to_string_lossy().to_string(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [220, 60, 60], Material::Plastic));
    let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
    let ctx = eframe::egui::Context::default();
    let mut r = crate::view3d::Renderer::default();
    let px = render_scene(&ctx, &mut r, &scene, &mut crate::texture::TexCache::default()).unwrap();
    assert_eq!(px.len(), THUMB * THUMB * 4);
    let at = |x: usize, y: usize| &px[(y * THUMB + x) * 4..(y * THUMB + x) * 4 + 3];
    assert_eq!(at(1, 1), &[38, 42, 50], "corner is background");
    let c = at(THUMB / 2, THUMB / 2);
    assert!(c[0] > c[1] + 30, "center shows the red model: {c:?}");
    // the whole pillar fits: rows near the top and bottom both have model pixels
    let row_has = |y: usize| (0..THUMB).any(|x| at(x, y) != [38, 42, 50]);
    let first = (0..THUMB).find(|y| row_has(*y)).unwrap();
    let last = (0..THUMB).rev().find(|y| row_has(*y)).unwrap();
    assert!(first > 0 && last < THUMB - 1 && last - first > THUMB / 2, "{first}..{last}");
    image::RgbaImage::from_raw(THUMB as u32, THUMB as u32, px).unwrap().save(dir.join("thumb.png")).unwrap();
    // disk cache name changes when the file changes
    let k1 = cache_key(&obj.to_string_lossy());
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&obj, std::fs::read_to_string(&obj).unwrap() + "\n").unwrap();
    assert_ne!(k1, cache_key(&obj.to_string_lossy()));
}

#[test]
fn preview_shapes_and_extrude_height() {
    use crate::previews::*;
    let c: [Vec3; 8] = std::array::from_fn(|i| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32));
    let e = box_edges(c);
    assert_eq!(e.len(), 12);
    assert!(e.iter().all(|(a, b)| ((*a - *b).length() - 1.0).abs() < 1e-6), "every edge is one unit long");
    let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 4.0), Vec3::new(0.0, 0.0, 4.0)];
    assert_eq!(prism_edges(&pts, 2.0, 3.0).len(), 12);
    // camera in front of the outline, looking at it: the height is where the
    // mouse ray crosses the vertical plane through the middle
    let cam = Vec3::new(2.0, 5.0, -20.0);
    let target = Vec3::new(2.0, 2.0 + 7.5, 2.0);
    let d = (target - cam).normalize();
    let h = extrude_height_3d(cam, d, &pts, 2.0, Vec3::Z).unwrap();
    assert!((h - 7.5).abs() < 1e-3, "{h}");
    // looking straight down there's no side plane to use
    assert!(extrude_height_3d(cam, Vec3::NEG_Y, &pts, 2.0, Vec3::NEG_Y).is_none());
}

#[test]
fn shift_constrain_shapes() {
    use crate::previews::{outline_rect, square_end};
    use glam::Vec2;
    let r = outline_rect(Vec3::new(1.0, 2.0, 1.0), Vec3::new(5.0, 9.0, -2.0), false);
    assert_eq!(r.len(), 4);
    assert!(r.iter().all(|p| p.y == 2.0), "outline stays on the start height");
    assert_eq!((r[2].x, r[2].z), (5.0, -2.0));
    let sq = outline_rect(Vec3::ZERO, Vec3::new(3.0, 0.0, -7.0), true);
    assert_eq!((sq[2].x, sq[2].z), (7.0, -7.0), "square keeps the drag direction");
    assert!(outline_rect(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0), false).is_empty());
    assert_eq!(square_end(Vec2::new(1.0, 1.0), Vec2::new(-3.0, 2.0)), Vec2::new(-3.0, 5.0));
}

#[test]
fn randomize_only_touches_ticked_axes() {
    use crate::scatter::{randomize, ScatterSpec};
    let base: Vec<Object> = (0..50).map(|i| Object::brush(Shape::Box, Vec3::ONE, Vec3::new(i as f32, 0.0, 0.0), [0; 3], Material::Wood)).collect();
    let spec = ScatterSpec { pos_axes: [true, false, true], pos_min: -0.5, pos_max: 0.5, rot_axes: [false, true, false], rot_min: -20.0, rot_max: 20.0 };
    let mut a = base.clone();
    randomize(&mut a, 42, &spec);
    let mut b = base.clone();
    randomize(&mut b, 42, &spec);
    assert_eq!(a, b, "same seed, same result");
    let mut moved_any = false;
    for (o, r) in base.iter().zip(&a) {
        let d = r.pos - o.pos;
        assert!(d.y == 0.0 && r.rot.x == 0.0 && r.rot.z == 0.0, "untouched axes stay put");
        assert!(d.x.abs() <= 0.5 && d.z.abs() <= 0.5 && r.rot.y.abs() <= 20.0);
        moved_any |= d.x != 0.0;
    }
    assert!(moved_any);
    let mut c = base.clone();
    randomize(&mut c, 43, &spec);
    assert_ne!(a, c, "another seed, another layout");
}

#[test]
fn spawns_are_flat_and_invisible_in_roblox() {
    assert!(Settings::default().spawn_size.y <= 0.2 + 1e-6);
    let mut map = Map::default();
    map.add(Object::new(Kind::Spawn { size: Vec3::new(6.0, 0.2, 6.0) }, Vec3::ZERO, [90, 200, 110], Material::SmoothPlastic));
    let nodes = export_roblox::build_nodes(&map, &mut ModelCache::default(), &Settings::default());
    let xml = export_roblox::to_rbxmx(&nodes, "m");
    let at = xml.find("\"SpawnLocation\"").unwrap();
    let item = &xml[at..at + xml[at..].find("</Item>").unwrap()];
    assert!(item.contains("<float name=\"Transparency\">1</float>"), "{item}");
    assert!(!item.contains("\"Decal\""));
}

#[test]
fn far_grid_fades_out_instead_of_shimmering() {
    use crate::view3d::{Camera, RenderOpts, Renderer};
    let ctx = eframe::egui::Context::default();
    let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(640.0, 360.0));
    let cam = Camera::looking_at(Vec3::new(0.0, 6.0, 0.0), Vec3::new(0.0, 4.0, -60.0));
    let bg = [58u8, 66, 80];
    let opts = RenderOpts { fov: 70.0, grid: 1.0, plane_y: 0.0, show_grid: true, outlines: true, bg, scale: 1.0 / ctx.pixels_per_point(), filter: true, lighting: Default::default() };
    let mut r = Renderer::default();
    r.render(&ctx, rect, &cam, &opts, &[], &[], &HashMap::new(), &mut crate::texture::TexCache::default());
    let (w, h, px) = r.pixels();
    // how far each row is from the background, on average
    let row_diff = |y: usize| -> f32 {
        (0..w).map(|x| (0..3).map(|c| (px[(y * w + x) * 4 + c] as f32 - bg[c] as f32).abs()).sum::<f32>()).sum::<f32>() / w as f32
    };
    image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(out_dir("grid").join("grid.png")).unwrap();
    let horizon = (0..h).rev().find(|y| row_diff(*y) < 0.5).unwrap_or(0);
    let near = row_diff(h - 5);
    let far = row_diff(horizon + 3);
    assert!(near > 1.5, "grid visible close up ({near})");
    assert!(far < near * 0.5, "grid fades toward the horizon (near {near}, far {far})");
}

#[test]
fn lighting_shadows_ao_fog_and_lamps() {
    use crate::lighting::*;
    use crate::view3d::{Camera, RenderOpts, Renderer};
    use std::time::Instant;
    let mut map = Map::default();
    map.add(Object::brush(Shape::Box, Vec3::new(120.0, 1.0, 120.0), Vec3::new(0.0, -0.5, 0.0), [200, 200, 200], Material::Concrete));
    map.add(Object::brush(Shape::Box, Vec3::new(40.0, 14.0, 1.0), Vec3::new(0.0, 7.0, -12.0), [190, 180, 170], Material::Brick));
    map.add(Object::brush(Shape::Box, Vec3::new(1.0, 14.0, 30.0), Vec3::new(-20.0, 7.0, 2.5), [190, 180, 170], Material::Brick));
    map.add(Object::brush(Shape::Box, Vec3::new(4.0, 4.0, 4.0), Vec3::new(4.0, 2.0, 2.0), [170, 120, 80], Material::Wood));
    map.add(Object::brush(Shape::Cylinder, Vec3::new(2.0, 10.0, 2.0), Vec3::new(-8.0, 5.0, 4.0), [220, 220, 225], Material::Marble));
    let scene = crate::mesh::build_scene(&map, &mut ModelCache::default());
    let ctx = eframe::egui::Context::default();
    let rect = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(800.0, 450.0));
    let cam = Camera::looking_at(Vec3::new(18.0, 14.0, 30.0), Vec3::new(-4.0, 2.0, -4.0));
    let dir = out_dir("lighting");
    let sun = sun_dir(-55.0, 45.0);
    let lamp = PointLight { pos: Vec3::new(-14.0, 3.0, -6.0), range: 14.0, brightness: 1.2, color: [1.0, 0.55, 0.2] };
    let mut shots = Vec::new();
    for q in 0..5u8 {
        let mut l = preset(q, sun, true, 400.0, true);
        if l.on {
            l.lights = vec![lamp];
        }
        let opts = RenderOpts { fov: 60.0, grid: 1.0, plane_y: 0.0, show_grid: false, outlines: false, bg: [58, 66, 80], scale: 1.0 / ctx.pixels_per_point(), filter: true, lighting: l };
        let mut r = Renderer::default();
        r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut crate::texture::TexCache::default());
        let t = Instant::now();
        for _ in 0..3 {
            r.render(&ctx, rect, &cam, &opts, &scene, &[], &HashMap::new(), &mut crate::texture::TexCache::default());
        }
        let ms = t.elapsed().as_secs_f32() * 1000.0 / 3.0;
        let (w, h, px) = r.pixels();
        image::RgbaImage::from_raw(w as u32, h as u32, px.to_vec()).unwrap().save(dir.join(format!("q{q}.png"))).unwrap();
        println!("LIGHT quality {} ({}): {ms:.1} ms", q, QUALITY_NAMES[q as usize]);
        shots.push((w, h, px.to_vec()));
    }
    // helpers: brightness of a world point in a shot
    let (w, h, _) = shots[0];
    let k = cam.px_scale(h as f32, 60.0);
    let at = |shot: &Vec<u8>, p: Vec3| -> f32 {
        let v = cam.to_view(p, &cam.basis());
        let (x, y) = ((w as f32 * 0.5 + v.x / v.z * k) as usize, (h as f32 * 0.5 - v.y / v.z * k) as usize);
        let i = (y * w + x) * 4;
        (shot[i] as f32 + shot[i + 1] as f32 + shot[i + 2] as f32) / 3.0
    };
    // the crate's shadow falls away from the sun on the floor
    let shade_spot = Vec3::new(4.0, 0.01, 2.0) - Vec3::new(sun.x, 0.0, sun.z).normalize() * 4.5;
    let open_floor = Vec3::new(12.0, 0.01, 12.0);
    let (lit, shaded) = (at(&shots[2].2, open_floor), at(&shots[2].2, shade_spot));
    assert!(shaded < lit * 0.8, "shadow behind the crate (lit {lit}, shaded {shaded})");
    let (lit0, shaded0) = (at(&shots[0].2, open_floor), at(&shots[0].2, shade_spot));
    assert!((lit0 - shaded0).abs() < 2.0, "no shadows when lighting is off");
    // the lamp warms the floor near it (red > blue there)
    let near_lamp = Vec3::new(-14.0, 0.01, -5.0);
    let v = cam.to_view(near_lamp, &cam.basis());
    let (x, y) = ((w as f32 * 0.5 + v.x / v.z * k) as usize, (h as f32 * 0.5 - v.y / v.z * k) as usize);
    let i = (y * w + x) * 4;
    let s1 = &shots[1].2;
    assert!(s1[i] as f32 > s1[i + 2] as f32 + 25.0, "lamp light is orange: {:?}", &s1[i..i + 3]);
    // the sky is a gradient: the top row is bluer than the horizon
    let top = &shots[1].2[(2 * w + w / 2) * 4..][..3];
    assert!(top[2] > top[0] + 40, "sky at the top is blue: {top:?}");
}

#[test]
fn building_trim_runs_inside_and_doors_cut_it() {
    use crate::building::{house_trim, inset_outline, trim_hole};
    use crate::map::Hole;
    let sq = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 10.0)];
    for pts in [sq.to_vec(), sq.iter().rev().copied().collect::<Vec<_>>()] {
        let inner = inset_outline(&pts, 1.0);
        let (mn, mx) = inner.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        assert!((mn.x - 1.0).abs() < 1e-4 && (mx.z - 9.0).abs() < 1e-4, "inset goes inward either winding: {mn} {mx}");
    }
    let trim = house_trim(&sq, 1.0, 12.0, 0.0, 0.8, 0.25, true, true, [255; 3]);
    let base: Vec<&Object> = trim.iter().filter(|o| o.name.starts_with("Baseboard")).collect();
    let crown: Vec<&Object> = trim.iter().filter(|o| o.name.starts_with("Crown")).collect();
    assert_eq!((base.len(), crown.len()), (4, 4));
    for o in &base {
        let Kind::Brush { size, .. } = &o.kind else { panic!() };
        assert!((o.pos.y - 0.4).abs() < 1e-4 && (size.y - 0.8).abs() < 1e-4);
        // sits against the inside face of the 1-thick walls (inner face at 0.5 / 9.5)
        assert!(o.pos.x > 0.5 && o.pos.x < 9.5 && o.pos.z > 0.5 && o.pos.z < 9.5, "{}", o.pos);
    }
    assert!(crown.iter().all(|o| (o.pos.y - (12.0 - 0.4)).abs() < 1e-4));
    // a door in the front wall (z = 0) cuts the baseboard in front of it, not the crown
    let mut wall = Object::brush(Shape::Box, Vec3::new(10.0, 12.0, 1.0), Vec3::new(5.0, 6.0, 0.0), [255; 3], Material::Brick);
    wall.id = 1;
    // exactly the hole the Door tool cuts (it only spans the wall's thickness)
    let door = Hole { min: Vec3::new(-1.5, -6.1, -0.6), max: Vec3::new(1.5, 1.0, 0.6) };
    let corners2 = crate::building::opening_reach(&wall, &door, 2.0);
    let front = base.iter().find(|o| o.pos.z < 1.0).unwrap();
    let cut2 = trim_hole(front, &corners2).expect("the door cuts the baseboard in front of it");
    let wide = (0..3).map(|k| cut2.max[k] - cut2.min[k]).fold(0.0f32, f32::max);
    assert!(wide >= 3.0 - 1e-3, "cut is at least door-wide: {cut2:?}");
    let back = base.iter().find(|o| o.pos.z > 9.0).unwrap();
    assert!(trim_hole(back, &corners2).is_none(), "the far wall's baseboard is untouched");
    assert!(crown.iter().all(|c| trim_hole(c, &corners2).is_none()), "the crown is above the door");
}

#[test]
fn themes_differ_and_use_the_accent() {
    use crate::themes::*;
    let accent = [200, 40, 160];
    let all: Vec<_> = (0..THEME_NAMES.len() as u8).map(|t| visuals(t, accent)).collect();
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            assert_ne!(all[i].panel_fill, all[j].panel_fill, "{} vs {}", THEME_NAMES[i], THEME_NAMES[j]);
        }
    }
    assert!(!all[1].dark_mode && all[0].dark_mode);
    assert_eq!(all[0].hyperlink_color, eframe::egui::Color32::from_rgb(200, 40, 160));
}

mod collisions {
    use crate::collide::{Mover, World};
    use crate::map::{Map, Material, Object, Shape};
    use crate::mesh::{build_scene, ModelCache};
    use glam::{Quat, Vec3};

    fn world_with(statics: Vec<Object>, mover: &mut Object) -> (Vec<std::sync::Arc<crate::mesh::WorldMesh>>, u64) {
        let mut m = Map::default();
        for s in statics {
            m.add(s);
        }
        let id = m.add(mover.clone());
        mover.id = id;
        let scene = build_scene(&m, &mut ModelCache::default());
        (scene, id)
    }

    fn bx(size: Vec3, pos: Vec3) -> Object {
        Object::brush(Shape::Box, size, pos, [200; 3], Material::Concrete)
    }

    #[test]
    fn drops_onto_the_floor_flush() {
        let mut crate_ = bx(Vec3::splat(2.0), Vec3::new(0.0, 3.0, 0.0));
        let (scene, _) = world_with(vec![bx(Vec3::new(40.0, 1.0, 40.0), Vec3::new(0.0, -0.5, 0.0))], &mut crate_);
        let w = World::new(&scene, vec![Mover::new(&crate_, &mut ModelCache::default())]);
        let (d, hit) = w.resolve_translation(Vec3::new(0.0, -10.0, 0.0));
        let y = 3.0 + d.y;
        assert!(hit.is_some() && (y - 1.0).abs() < 0.05, "rests on the floor at y={y}");
        // resting flush, it can still slide sideways
        let mut on_floor = bx(Vec3::splat(2.0), Vec3::new(0.0, 1.0, 0.0));
        let (scene, _) = world_with(vec![bx(Vec3::new(40.0, 1.0, 40.0), Vec3::new(0.0, -0.5, 0.0))], &mut on_floor);
        let w = World::new(&scene, vec![Mover::new(&on_floor, &mut ModelCache::default())]);
        let (d, hit) = w.resolve_translation(Vec3::new(6.0, 0.0, -3.0));
        assert!(hit.is_none() && d == Vec3::new(6.0, 0.0, -3.0), "{d}");
    }

    #[test]
    fn stops_at_walls_and_slides_along_them() {
        let mut c = bx(Vec3::splat(2.0), Vec3::new(0.0, 1.0, 0.0));
        let wall = bx(Vec3::new(1.0, 6.0, 20.0), Vec3::new(5.0, 3.0, 0.0));
        let (scene, _) = world_with(vec![wall], &mut c);
        let w = World::new(&scene, vec![Mover::new(&c, &mut ModelCache::default())]);
        let (d, _) = w.resolve_translation(Vec3::new(10.0, 0.0, 3.0));
        assert!((d.x - 3.5).abs() < 0.05, "stops flush at the wall: {d}");
        assert!((d.z - 3.0).abs() < 1e-4, "still slides along it: {d}");
        // a thin wall isn't skipped by one big jump
        let mut c2 = bx(Vec3::splat(2.0), Vec3::new(0.0, 1.0, 0.0));
        let thin = bx(Vec3::new(0.2, 6.0, 20.0), Vec3::new(5.0, 3.0, 0.0));
        let (scene, _) = world_with(vec![thin], &mut c2);
        let w = World::new(&scene, vec![Mover::new(&c2, &mut ModelCache::default())]);
        let (d, _) = w.resolve_translation(Vec3::new(30.0, 0.0, 0.0));
        assert!(d.x < 4.0, "didn't tunnel through: {d}");
    }

    #[test]
    fn rotation_stops_at_contact() {
        // a plank pointing along X next to a wall at z = 3: turning 90° would swing it into the wall
        let mut plank = bx(Vec3::new(8.0, 0.5, 0.5), Vec3::new(0.0, 1.0, 0.0));
        let wall = bx(Vec3::new(20.0, 6.0, 1.0), Vec3::new(0.0, 3.0, 3.0));
        let (scene, _) = world_with(vec![wall], &mut plank);
        let w = World::new(&scene, vec![Mover::new(&plank, &mut ModelCache::default())]);
        let target = (plank.pos, Quat::from_rotation_y(90f32.to_radians()));
        let (s, hit) = w.resolve_fraction(&[target]);
        assert!(hit.is_some() && s > 0.2 && s < 0.9, "stopped part way: {s}");
        // the stopped pose really doesn't overlap, and a bit further does
        let q = Quat::IDENTITY.slerp(target.1, s);
        assert!(w.overlap(&[(plank.pos, q)]).is_none());
        assert!(w.overlap(&[(plank.pos, Quat::IDENTITY.slerp(target.1, (s + 0.08).min(1.0)))]).is_some());
    }
}

#[test]
fn plane_handles_move_in_one_plane() {
    use crate::gizmo::{self, GDrag, Xf};
    use crate::plugins::ViewInfo;
    use crate::view3d::Camera;
    use eframe::egui::{pos2, vec2, Rect};
    let cam = Camera::looking_at(Vec3::new(10.0, 12.0, 14.0), Vec3::ZERO);
    let view = ViewInfo::Persp { cam, rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)), fov: 60.0 };
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let c = Vec3::ZERO;
    let s = gizmo::handle_len(&view, c);
    // the XZ square (normal Y = handle 4) is visible from above and picks
    let q = gizmo::plane_quad(c, axes, 4, s);
    let mid = view.project((q[0] + q[2]) * 0.5).unwrap();
    assert_eq!(gizmo::hit(&view, gizmo::MOVE, c, axes, mid), Some(4));
    let g = GDrag::begin(&view, gizmo::MOVE, 4, c, axes, mid);
    match g.xf(&view, mid + vec2(60.0, 25.0), 1.0, true) {
        Xf::Move(d) => {
            assert!(d.y == 0.0 && (d.x != 0.0 || d.z != 0.0), "slides in XZ only: {d}");
            assert!(d.x.fract() == 0.0 && d.z.fract() == 0.0, "snapped: {d}");
        }
        x => panic!("{x:?}"),
    }
}

#[test]
fn prompts_and_teleports_export_to_roblox() {
    use crate::map::PromptSpec;
    let mut map = Map::default();
    let mut pr = PromptSpec { action_text: "Open \"door\"".into(), key: "F".into(), hold: 0.5, ..Default::default() };
    map.add(Object::new(Kind::Prompt { prompt: pr.clone() }, Vec3::new(1.0, 3.0, 2.0), [255; 3], Material::Neon));
    let target = map.add(Object::new(Kind::Target {}, Vec3::new(40.0, 1.0, -10.0), [255; 3], Material::Neon));
    let mut tp = Object::new(
        Kind::Teleport { size: Vec3::new(5.0, 0.4, 5.0), touch: true, dest: Some(target), cooldown: 2.0, exit_offset: Vec3::ZERO, prompt: PromptSpec::default() },
        Vec3::ZERO,
        [150, 90, 235],
        Material::Neon,
    );
    tp.name = "PadA".into();
    map.add(tp.clone());
    // a prompt teleport with a hostile key name: must not end up in the script
    pr.key = "E; game:Destroy() --".into();
    tp.name = "PadB".into();
    tp.kind = Kind::Teleport { size: Vec3::splat(2.0), touch: false, dest: Some(target), cooldown: 1.0, exit_offset: Vec3::new(0.0, 2.0, 0.0), prompt: pr };
    map.add(tp);
    let mut settings = Settings::default();
    settings.roblox_scale = 1.0;
    let nodes = export_roblox::build_nodes(&map, &mut ModelCache::default(), &settings);
    let xml = export_roblox::to_rbxmx(&nodes, "m");
    assert_eq!(xml.matches("class=\"ProximityPrompt\"").count(), 2);
    assert!(xml.contains("<token name=\"KeyboardKeyCode\">102</token>"), "F = 102");
    assert!(xml.contains("<token name=\"GamepadKeyCode\">1000</token>"));
    assert!(xml.contains("<float name=\"HoldDuration\">0.5</float>"));
    assert!(xml.contains("Open &quot;door&quot;"));
    assert_eq!(xml.matches("class=\"Script\"").count(), 2);
    assert!(xml.contains("<ProtectedString name=\"Source\"><![CDATA[-- Teleport made by Anvil"));
    assert!(xml.contains("CFrame.new(40, 1, -10"), "touch pad goes to the target");
    assert!(xml.contains("CFrame.new(40, 3, -10"), "exit offset is added");
    assert!(xml.contains("Touched:Connect") && xml.contains("Triggered:Connect"));
    assert!(!xml.contains("Destroy()"), "key names can't inject code");
    let lua = export_roblox::to_luau(&nodes, "m");
    assert!(lua.contains("pp.KeyboardKeyCode = Enum.KeyCode.F") && lua.contains("Enum.KeyCode.E\n"));
    assert!(!lua.contains("game:Destroy"));
    assert!(lua.contains("sc.Source = \"-- Teleport made by Anvil"));
    let dir = out_dir("roblox_entities");
    std::fs::write(dir.join("entities.lua"), &lua).unwrap();
    std::fs::write(dir.join("teleport_script.lua"), export_roblox::teleport_script("CFrame.new(1, 2, 3)", 2.0, true, "Spot")).unwrap();
    // round-trips through the map file
    let back: Map = serde_json::from_str(&serde_json::to_string(&map).unwrap()).unwrap();
    assert_eq!(back.objects, map.objects);
}

#[test]
fn live_sync_server_is_local_and_token_protected() {
    use crate::livesync::*;
    use std::io::{Read, Write};
    let token = new_token();
    let mut srv = LiveServer::start(0, token.clone()).unwrap();
    let port = srv.port;
    let get = |method: &str, path: &str, host: &str| -> (String, String) {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
        (head.lines().next().unwrap_or("").to_string(), body.to_string())
    };
    let host = format!("127.0.0.1:{port}");
    let mut map = Map::default();
    let wall = map.add(Object::brush(Shape::Box, Vec3::new(8.0, 4.0, 1.0), Vec3::new(1.0, 2.0, 3.0), [10, 20, 30], Material::Brick));
    let mut models = ModelCache::default();
    let settings = Settings::default();
    srv.update(&map, &mut models, &settings);

    let (st, body) = get("GET", &format!("/roblox/state?token={token}"), &host);
    assert!(st.contains("200"), "{st}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["objects"][0]["id"].as_u64(), Some(wall));
    let h1 = v["objects"][0]["hash"].as_str().unwrap().to_string();

    assert!(get("GET", "/roblox/state?token=nope", &host).0.contains("403"), "wrong token refused");
    assert!(get("GET", &format!("/roblox/state?token={token}"), "evil.example:80").0.contains("403"), "foreign Host refused (DNS rebinding)");
    assert!(get("POST", &format!("/roblox/state?token={token}"), &host).0.contains("405"));
    let (_, head_body) = get("GET", &format!("/roblox/state?token={token}"), &host);
    assert!(!head_body.contains("Access-Control"));

    let (_, body) = get("GET", &format!("/roblox/objects?ids={wall}&token={token}"), &host);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let part = &v["objects"][0]["nodes"][0];
    assert_eq!(part["t"], "Part");
    assert_eq!(part["cf"][0].as_f64(), Some(1.0 * settings.roblox_scale as f64));
    let (_, body) = get("GET", &format!("/blender/objects?ids={wall}&token={token}"), &host);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["objects"][0]["pos"], serde_json::json!([1.0, -3.0, 2.0]), "Blender is Z-up");
    assert_eq!(v["objects"][0]["faces"].as_array().unwrap().len(), 12);

    // an edit changes the hash; a delete removes it
    std::thread::sleep(std::time::Duration::from_millis(220));
    map.get_mut(wall).unwrap().pos.x = 9.0;
    srv.update(&map, &mut models, &settings);
    let (_, body) = get("GET", &format!("/roblox/state?token={token}"), &host);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_ne!(v["objects"][0]["hash"].as_str().unwrap(), h1);
    std::thread::sleep(std::time::Duration::from_millis(220));
    map.objects.clear();
    srv.update(&map, &mut models, &settings);
    let (_, body) = get("GET", &format!("/roblox/state?token={token}"), &host);
    assert!(body.contains("\"objects\":[]"), "{body}");

    let dir = out_dir("livesync");
    std::fs::write(dir.join("AnvilLiveSync.lua"), studio_plugin(port, &token)).unwrap();
    std::fs::write(dir.join("anvil_live_sync.py"), blender_addon(port, &token)).unwrap();
}

#[test]
fn blender_scene_export() {
    use crate::blender_export::*;
    let dir = out_dir("blender");
    let png = dir.join("bricks.png");
    image::RgbaImage::from_fn(16, 16, |x, y| image::Rgba([(x * 15) as u8, 80, (y * 15) as u8, 255])).save(&png).unwrap();
    let mut map = Map::default();
    map.name = "BlendTest".into();
    let tex = map.add_texture(TextureDef::new("bricks".into(), png.to_string_lossy().to_string()));
    let house = map.add_folder("House", None);
    let up = map.add_folder("Upstairs", Some(house));
    let mut wall = Object::brush(Shape::Box, Vec3::new(8.0, 4.0, 1.0), Vec3::new(2.0, 2.0, 0.0), [200, 100, 50], Material::Brick);
    wall.texture = Some(tex);
    wall.folder = Some(house);
    wall.name = "WallA".into();
    map.add(wall);
    let mut c = Object::brush(Shape::Cylinder, Vec3::new(2.0, 6.0, 2.0), Vec3::new(-4.0, 3.0, 2.0), [90, 90, 255], Material::Plastic);
    c.folder = Some(up);
    c.rot.y = 30.0;
    c.name = "Pillar".into();
    map.add(c);
    map.add(Object::new(Kind::Light { range: 20.0, brightness: 2.0 }, Vec3::new(0.0, 8.0, 0.0), [255, 220, 180], Material::Neon));
    map.add(Object::new(Kind::Spawn { size: Vec3::new(6.0, 0.2, 6.0) }, Vec3::ZERO, [0, 255, 0], Material::Plastic));
    let glb = scene_glb(&map, &mut ModelCache::default(), 0.28);
    assert_eq!(&glb[0..4], b"glTF");
    let jl = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + jl]).unwrap();
    let nodes = doc["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4 + 2, "4 objects + 2 folders");
    assert_eq!(doc["images"].as_array().unwrap().len(), 1, "texture embedded once");
    assert!(doc["extensions"]["KHR_lights_punctual"]["lights"][0]["type"] == "point");
    let wall_node = nodes.iter().find(|n| n["name"] == "WallA").unwrap();
    assert!((wall_node["translation"][0].as_f64().unwrap() - 0.56).abs() < 1e-5, "meters: 2 studs * 0.28");
    let house_node = nodes.iter().find(|n| n["name"] == "House").unwrap();
    assert_eq!(house_node["children"].as_array().unwrap().len(), 2, "wall + Upstairs");
    let path = dir.join("scene.glb");
    std::fs::write(&path, &glb).unwrap();
    if let Some(b) = find_blender("") {
        let out = dir.join("scene.blend");
        let _ = std::fs::remove_file(&out);
        make_blend(&b, &path, &out).unwrap();
        assert!(out.is_file());
        println!("BLEND made with {}", b.display());
    } else {
        println!("BLEND skipped: no Blender");
    }
}

mod security_and_updates {
    use crate::secure::{hex, protect, sha256, unprotect};
    use crate::updater::*;
    use std::io::Write;

    #[test]
    fn sha256_known_answers() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1_000_000];
        assert_eq!(hex(&sha256(&long)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn api_key_is_encrypted_for_this_user() {
        let enc = protect("my-secret-roblox-key").expect("DPAPI works on Windows");
        assert!(!enc.contains("my-secret"));
        assert_eq!(unprotect(&enc).as_deref(), Some("my-secret-roblox-key"));
        let mut bad = crate::team::b64_decode(&enc).unwrap();
        let n = bad.len();
        bad[n - 5] ^= 0xff;
        assert!(unprotect(&crate::team::b64_encode(&bad)).is_none(), "tampered data is refused");
    }

    #[test]
    fn update_checks() {
        assert!(is_newer("v0.3.0", "0.2.0") && is_newer("1.0", "0.9.9") && !is_newer("0.2.0", "0.2.0") && !is_newer("0.1.9", "0.2.0"));
        assert!(allowed_url("https://github.com/a/b/releases/download/v1/Anvil.zip"));
        assert!(!allowed_url("http://github.com/x") && !allowed_url("https://evil.com/github.com/") && !allowed_url("https://github.com.evil.io/"));
        assert!(valid_repo("someone/Anvil") && !valid_repo("a/b/c") && !valid_repo("../x") && !valid_repo("a b/c"));
        let sha = "ab".repeat(32);
        let v = serde_json::json!({
            "tag_name": "v0.3.0", "html_url": "https://github.com/o/r/releases/tag/v0.3.0",
            "body": format!("Changes...\n\nSHA-256 Anvil-0.3.0.zip: {sha}\n"),
            "assets": [{"name": "Anvil-0.3.0.zip", "browser_download_url": "https://github.com/o/r/releases/download/v0.3.0/Anvil-0.3.0.zip"}]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_deref()), ("0.3.0", Some(sha.as_str())));
        let mut evil = v.clone();
        evil["assets"][0]["browser_download_url"] = serde_json::json!("https://evil.example/Anvil.zip");
        assert!(parse_release(&evil).is_none(), "assets from outside GitHub are ignored");
    }

    fn zip_with(name: &str, data: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn update_zip_must_match_its_hash() {
        let zip = zip_with("Anvil/anvil.exe", b"MZ fake program");
        let good = hex(&sha256(&zip));
        assert_eq!(verify_and_extract(&zip, &good).unwrap(), b"MZ fake program");
        assert!(verify_and_extract(&zip, &"00".repeat(32)).is_err(), "wrong hash refused");
        let not_exe = zip_with("Anvil/anvil.exe", b"#!/bin/sh");
        assert!(verify_and_extract(&not_exe, &hex(&sha256(&not_exe))).is_err());
    }

    #[test]
    fn packages_refuse_too_many_files() {
        let dir = super::out_dir("pak_bomb");
        let p = dir.join("bomb.anvilpak");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut z = zip::ZipWriter::new(f);
            for i in 0..crate::package::MAX_FILES + 1 {
                z.start_file(format!("f{i}"), zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            }
            z.finish().unwrap();
        }
        let e = crate::package::open_package(&p).err().expect("refused");
        assert!(e.contains("limited"), "{e}");
    }

    #[test]
    fn new_plugins_wait_for_approval() {
        let mut host = crate::plugins::PluginHost::new();
        let file = "zz_untrusted_test.rhai";
        let path = host.dir.join(file);
        let src = "fn info() { #{ name: \"Untrusted test\", kind: \"command\", description: \"x\" } }\nfn run() { status(\"hi\"); }\n";
        std::fs::write(&path, src).unwrap();
        host.reload();
        assert!(host.plugins.iter().all(|p| p.file != file), "not loaded before approval");
        let pending = host.pending.iter().find(|p| p.file == file).cloned().expect("asks first");
        host.approve(&pending.key);
        host.reload();
        assert!(host.plugins.iter().any(|p| p.file == file), "loads once allowed");
        // changing the file asks again
        std::fs::write(&path, format!("{src}// edited\n")).unwrap();
        host.reload();
        assert!(host.plugins.iter().all(|p| p.file != file) && host.pending.iter().any(|p| p.file == file));
        let _ = std::fs::remove_file(&path);
        // the bundled ones are always trusted
        assert!(host.plugins.iter().any(|p| p.file == "transform_gizmo.rhai"));
    }
}

#[test]
fn mesh_booleans_have_the_right_volume() {
    use crate::mesh_csg::*;
    let vol = |polys: &[Vec<Vec3>]| -> f32 {
        polys.iter().map(|p| (1..p.len() - 1).map(|i| p[0].dot(p[i].cross(p[i + 1])) / 6.0).sum::<f32>()).sum()
    };
    let mut models = ModelCache::default();
    let a = Object::brush(Shape::Box, Vec3::splat(2.0), Vec3::ZERO, [0; 3], Material::Plastic);
    let b = Object::brush(Shape::Box, Vec3::splat(2.0), Vec3::new(1.0, 0.5, 0.0), [0; 3], Material::Plastic);
    let (pa, pb) = (object_polys(&a, &mut models), object_polys(&b, &mut models));
    assert!((vol(&pa) - 8.0).abs() < 1e-3);
    // overlap is 1 x 1.5 x 2 = 3
    let u = vol(&csg(pa.clone(), pb.clone(), Op::Union));
    let s = vol(&csg(pa.clone(), pb.clone(), Op::Subtract));
    let i = vol(&csg(pa.clone(), pb.clone(), Op::Intersect));
    assert!((u - 13.0).abs() < 1e-2, "union {u}");
    assert!((s - 5.0).abs() < 1e-2, "subtract {s}");
    assert!((i - 3.0).abs() < 1e-2, "intersect {i}");
    // a sphere bitten out of a wall with a door hole (several boxes inside)
    let mut wall = Object::brush(Shape::Box, Vec3::new(10.0, 6.0, 2.0), Vec3::ZERO, [0; 3], Material::Plastic);
    if let Kind::Brush { holes, .. } = &mut wall.kind {
        holes.push(Hole { min: Vec3::new(-1.0, -3.0, -1.0), max: Vec3::new(1.0, 1.0, 1.0) });
    }
    let ball = Object::brush(Shape::Sphere, Vec3::splat(3.0), Vec3::new(4.0, 2.0, 0.0), [0; 3], Material::Plastic);
    let pw = object_polys(&wall, &mut models);
    assert!((vol(&pw) - (120.0 - 16.0)).abs() < 0.05, "wall with door {}", vol(&pw));
    let bitten = csg(pw.clone(), object_polys(&ball, &mut models), Op::Subtract);
    let v = vol(&bitten);
    assert!(v < 104.0 - 1.0 && v > 104.0 - 14.2, "a chunk of the ball came out: {v}");
    let (verts, faces, c) = to_mesh(&bitten).unwrap();
    assert!(!verts.is_empty() && faces.iter().all(|f| f.len() >= 3));
    assert!(c.length() < 6.0);
}

#[test]
fn gltf_models_load_with_transforms_and_textures() {
    // our own scene exporter writes a textured, multi-node .glb: read it back
    let dir = out_dir("gltf_in");
    let png = dir.join("t.png");
    image::RgbaImage::from_fn(8, 8, |x, _| image::Rgba([x as u8 * 30, 10, 200, 255])).save(&png).unwrap();
    let mut map = Map::default();
    let tex = map.add_texture(TextureDef::new("t".into(), png.to_string_lossy().to_string()));
    let mut a = Object::brush(Shape::Box, Vec3::new(2.0, 2.0, 2.0), Vec3::new(10.0, 1.0, 0.0), [255; 3], Material::Plastic);
    a.texture = Some(tex);
    let mut b = Object::brush(Shape::Box, Vec3::new(4.0, 1.0, 1.0), Vec3::new(-3.0, 0.5, 2.0), [255, 0, 0], Material::Plastic);
    b.rot.y = 90.0;
    map.add(a);
    map.add(b);
    let glb = crate::blender_export::scene_glb(&map, &mut ModelCache::default(), 1.0);
    let path = dir.join("scene.glb");
    std::fs::write(&path, glb).unwrap();
    let m = crate::mesh::load_model(&path.to_string_lossy()).unwrap();
    assert_eq!(m.tris.len(), 24, "two boxes");
    let (mn, mx) = m.pos.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    // b is turned 90°: its 4-long side now runs along Z
    assert!((mn.x - (-3.5)).abs() < 1e-3 && (mx.x - 11.0).abs() < 1e-3, "{mn} {mx}");
    assert!((mn.z - (-1.0)).abs() < 1e-3 && (mx.z - 4.0).abs() < 1e-3, "{mn} {mx}");
    assert!(m.materials.iter().any(|p| p.ends_with(".png") && std::path::Path::new(p).is_file()), "embedded texture saved: {:?}", m.materials);
    assert_eq!(m.uv.len(), m.pos.len());
    // a .gltf can't reach outside its folder
    let bad = dir.join("bad.gltf");
    std::fs::write(&bad, r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../../secret.bin","byteLength":4}]}"#).unwrap();
    assert!(crate::mesh::load_model(&bad.to_string_lossy()).is_err());
}

mod plugin_web_api {
    use crate::plugin_web::*;

    /// The Sketchfab tests share one saved token, so they take turns.
    static SKETCHFAB: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A plugin host with the bundled Sketchfab plugin freshly installed
    /// (ignores whatever copy an earlier test run left in the folder).
    fn sketchfab_host() -> (crate::plugins::PluginHost, usize) {
        let mut host = crate::plugins::PluginHost::new();
        let _ = std::fs::remove_file(host.dir.join("sketchfab.rhai"));
        host.install_builtins();
        host.reload();
        let idx = host.plugins.iter().position(|p| p.file == "sketchfab.rhai").expect("bundled Sketchfab plugin loads");
        (host, idx)
    }

    #[test]
    fn hosts_are_fenced() {
        let hosts = vec!["api.sketchfab.com".to_string()];
        assert!(url_allowed("https://api.sketchfab.com/v3/search?q=x", &hosts));
        assert!(url_allowed("https://API.sketchfab.com:443/x", &hosts));
        assert!(!url_allowed("http://api.sketchfab.com/x", &hosts), "https only");
        assert!(!url_allowed("https://evil.com/api.sketchfab.com", &hosts));
        assert!(!url_allowed("https://api.sketchfab.com@evil.com/", &hosts), "no user@host tricks");
        assert!(!url_allowed("https://api.sketchfab.com.evil.com/", &hosts));
        assert_eq!(url_encode("a b&c/ä"), "a%20b%26c%2F%C3%A4");
        assert!(!safe_name("../../evil.exe").contains('/') && !safe_name(r"..\x").contains('\\') && !safe_name(".hidden").starts_with('.'));
    }

    #[test]
    fn files_stay_in_the_plugin_folder() {
        let root = plugin_dir("zz_test_plugin.rhai");
        std::fs::create_dir_all(&root).unwrap();
        let outside = super::out_dir("plugin_outside").join("x.zip");
        std::fs::write(&outside, b"PK").unwrap();
        assert!(unzip_in(&outside, &root).is_err(), "can't unzip outside its folder");
        assert!(find_files(&super::out_dir("plugin_outside"), "zip", &root).is_empty());
        // a real zip inside works
        let z = root.join("model.zip");
        {
            use std::io::Write;
            let f = std::fs::File::create(&z).unwrap();
            let mut w = zip::ZipWriter::new(f);
            w.start_file("sub/scene.gltf", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"{}").unwrap();
            w.finish().unwrap();
        }
        let dir = unzip_in(&z, &root).unwrap();
        let found = find_files(std::path::Path::new(&dir), "gltf", &root);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("scene.gltf"));
    }

    /// A download that stops halfway must fail, not leave half a model
    /// that looks finished (a 48 MB Sketchfab chair arrived as 16 MB).
    #[test]
    fn cut_off_downloads_fail() {
        let dir = crate::tests::out_dir("cut_off");
        let p = dir.join("chair.glb");
        let _ = std::fs::remove_file(&p);
        let data = vec![7u8; 3_000_000];
        // the server said 3 MB but only 1 MB came
        let r = save_body(&data[..1_000_000], 3_000_000, &p, &|_| {});
        assert!(r.as_ref().is_err_and(|e| e.contains("cut off")), "{r:?}");
        assert!(!p.exists(), "no half file left behind");
        // the connection breaks partway
        struct Breaks(usize);
        impl std::io::Read for Breaks {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"));
                }
                self.0 -= 1;
                b[0] = 1;
                Ok(1)
            }
        }
        assert!(save_body(Breaks(500), 0, &p, &|_| {}).is_err());
        assert!(!p.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // a whole download, also when replacing an older copy
        std::fs::write(&p, b"old").unwrap();
        let last = std::cell::Cell::new(0);
        save_body(&data[..], 3_000_000, &p, &|g| last.set(g)).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), data);
        assert_eq!(last.get(), 3_000_000, "progress reaches the end");
        // size unknown (or compressed): whatever arrives is kept
        save_body(&data[..10], 0, &p, &|_| {}).unwrap();
        assert_eq!(std::fs::read(&p).unwrap().len(), 10);
    }

    #[test]
    fn panel_plugin_loads_and_draws() {
        let _turn = SKETCHFAB.lock().unwrap_or_else(|e| e.into_inner());
        let (mut host, idx) = sketchfab_host();
        assert_eq!(host.plugins[idx].kind, crate::plugins::PluginKind::Panel);
        assert!(host.plugins[idx].hosts.contains(&"api.sketchfab.com".to_string()));
        let (w, _) = host.panel(idx, &crate::map::Map::default(), &crate::mesh::ModelCache::default(), 1.0);
        assert!(w.iter().any(|x| matches!(x, Widget::Row(items) if items.iter().any(|i| matches!(i, Widget::Button { id, .. } if id == "search")))), "{w:?}");
        let out = host.panel_event(idx, &crate::map::Map::default(), &crate::mesh::ModelCache::default(), 1.0, "q", "barrel");
        assert!(out.status.is_none(), "{:?} {:?}", out.status, host.log);
    }

    #[test]
    fn sketchfab_token_saves_and_forgets() {
        let _turn = SKETCHFAB.lock().unwrap_or_else(|e| e.into_inner());
        let (mut host, idx) = sketchfab_host();
        let (map, models) = (crate::map::Map::default(), crate::mesh::ModelCache::default());
        let ev = |host: &mut crate::plugins::PluginHost, id: &str, v: &str| host.panel_event(idx, &map, &models, 1.0, id, v);
        let out = ev(&mut host, "token_in", "  test-token-123  ");
        assert!(out.status.is_none(), "{:?}", out.status);
        let out = ev(&mut host, "save_token", "");
        assert!(out.status.is_none(), "saving the token failed: {:?}", out.status);
        assert_eq!(secret_get("sketchfab.rhai", "token"), "test-token-123", "saved, trimmed and encrypted");
        let (w, _) = host.panel(idx, &map, &models, 1.0);
        assert!(w.iter().any(|x| matches!(x, Widget::Row(items) if items.iter().any(|i| matches!(i, Widget::Button { id, .. } if id == "forget_token")))));
        let out = ev(&mut host, "forget_token", "");
        assert!(out.status.is_none(), "{:?}", out.status);
        assert_eq!(secret_get("sketchfab.rhai", "token"), "");
    }

    /// The whole Sketchfab plugin, the way clicking through it runs: every
    /// line of the script executes (web replies are canned in test builds).
    #[test]
    fn sketchfab_end_to_end() {
        let _turn = SKETCHFAB.lock().unwrap_or_else(|e| e.into_inner());
        use crate::map::{Map, Material, Object, Shape};
        use glam::Vec3;
        use std::io::Write;
        let (mut host, idx) = sketchfab_host();
        let (map, models) = (Map::default(), crate::mesh::ModelCache::default());
        let mut statuses: Vec<String> = Vec::new();
        let mut ev = |host: &mut crate::plugins::PluginHost, id: &str, v: &str| {
            let out = host.panel_event(idx, &map, &models, 1.0, id, v);
            if let Some(s) = out.status {
                statuses.push(format!("{id}: {s}"));
            }
        };
        let log_start = host.log.len();

        // 1. token
        ev(&mut host, "token_in", "  tok-abc  ");
        ev(&mut host, "save_token", "");
        assert_eq!(secret_get("sketchfab.rhai", "token"), "tok-abc");

        // 2. search, with one complete result and one with almost nothing in it
        let long = format!("Old Barrel: rusty / dented (game-ready) {}", "x".repeat(90));
        mock(
            "https://api.sketchfab.com/v3/search?type=models&downloadable=true&count=24&q=old%20barrel",
            serde_json::json!({ "results": [
                { "uid": "aaa111", "name": long, "user": { "displayName": "Someone" }, "license": { "label": "CC Attribution" },
                  "viewerUrl": "https://sketchfab.com/3d-models/x-aaa111",
                  "thumbnails": { "images": [ { "url": "https://media.sketchfab.com/t_small.jpeg", "width": 100, "height": 100 },
                                              { "url": "https://media.sketchfab.com/t.jpeg", "width": 256, "height": 144 } ] } },
                { "uid": "bbb222", "name": "Crate" }
            ]}).to_string().as_bytes(),
        );
        ev(&mut host, "q", "old barrel");
        ev(&mut host, "search", "");
        let (w, _) = host.panel(idx, &map, &models, 1.0);
        let rows: Vec<&Vec<Widget>> = w.iter().filter_map(|x| if let Widget::Row(i) = x { Some(i) } else { None }).collect();
        let has = |id: &str| rows.iter().any(|r| r.iter().any(|i| matches!(i, Widget::Button { id: b, .. } if b == id)));
        assert!(has("dl:aaa111") && has("dl:bbb222"), "{w:?}");
        assert!(rows.iter().any(|r| r.iter().any(|i| matches!(i, Widget::Image { url, .. } if url == "https://media.sketchfab.com/t.jpeg"))), "picks the >=200px thumbnail");

        // 3. downloads: a .glb, and a zipped .gltf with a separate .bin
        let mut cube = Map::default();
        cube.add(Object::brush(Shape::Box, Vec3::splat(2.0), Vec3::ZERO, [200; 3], Material::Wood));
        let glb = crate::blender_export::scene_glb(&cube, &mut crate::mesh::ModelCache::default(), 1.0);
        let tri: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let gltf = serde_json::json!({
            "asset": { "version": "2.0" }, "scene": 0, "scenes": [{ "nodes": [0] }], "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
            "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] }],
            "bufferViews": [{ "buffer": 0, "byteLength": 36 }], "buffers": [{ "uri": "scene.bin", "byteLength": 36 }]
        });
        let mut zip = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut zip);
            z.start_file("source/scene.gltf", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(gltf.to_string().as_bytes()).unwrap();
            z.start_file("source/scene.bin", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(&tri).unwrap();
            z.finish().unwrap();
        }
        let s3 = "https://sketchfab-prod-media.s3.amazonaws.com/archives";
        mock("https://api.sketchfab.com/v3/models/aaa111/download", serde_json::json!({ "glb": { "url": format!("{s3}/aaa111/model.glb"), "size": glb.len() } }).to_string().as_bytes());
        mock(&format!("{s3}/aaa111/model.glb"), &glb);
        mock("https://api.sketchfab.com/v3/models/bbb222/download", serde_json::json!({ "gltf": { "url": format!("{s3}/bbb222/model.zip"), "size": 1 } }).to_string().as_bytes());
        mock(&format!("{s3}/bbb222/model.zip"), zip.get_ref());
        ev(&mut host, "dl:aaa111", "");
        ev(&mut host, "dl:bbb222", "");
        assert!(mock_headers("https://api.sketchfab.com/v3/models/aaa111/download").contains(&("Authorization".to_string(), "Token tok-abc".to_string())), "sends the token");

        // 4. the panel finishes the jobs and imports both into the library
        let mut imports = Vec::new();
        let t = std::time::Instant::now();
        while imports.len() < 2 && t.elapsed() < std::time::Duration::from_secs(10) {
            let (_, out) = host.panel(idx, &map, &models, 1.0);
            imports.extend(out.imports);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_eq!(imports.len(), 2, "both models imported; log {:?}", &host.log[log_start..]);
        let root = plugin_dir("sketchfab.rhai");
        for (path, name) in &imports {
            assert!(inside(std::path::Path::new(path), &root), "{path}");
            crate::mesh::load_model(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert!(imports.iter().any(|(p, n)| p.ends_with(".glb") && n.starts_with("Old Barrel")));
        assert!(imports.iter().any(|(p, n)| p.ends_with("scene.gltf") && n == "Crate"));

        ev(&mut host, "forget_token", "");

        // 5. nothing went wrong anywhere along the way
        let errors: Vec<&String> = host.log[log_start..].iter().filter(|l| !l.starts_with("Loaded ")).collect();
        assert!(statuses.is_empty() && errors.is_empty(), "statuses {statuses:?}\nlog {errors:?}");
    }

    /// An OAuth token ("Bearer") works too: the "Token" try is refused, the
    /// plugin retries with "Bearer" and the download goes through.
    #[test]
    fn sketchfab_bearer_token_fallback() {
        let _turn = SKETCHFAB.lock().unwrap_or_else(|e| e.into_inner());
        use crate::map::{Map, Material, Object, Shape};
        let (mut host, idx) = sketchfab_host();
        let (map, models) = (Map::default(), crate::mesh::ModelCache::default());
        host.panel_event(idx, &map, &models, 1.0, "token_in", "oauth-xyz");
        host.panel_event(idx, &map, &models, 1.0, "save_token", "");
        let mut cube = Map::default();
        cube.add(Object::brush(Shape::Box, glam::Vec3::ONE, glam::Vec3::ZERO, [200; 3], Material::Wood));
        let glb = crate::blender_export::scene_glb(&cube, &mut crate::mesh::ModelCache::default(), 1.0);
        let file = "https://sketchfab-prod-media.s3.amazonaws.com/archives/ddd444/m.glb";
        mock_auth("https://api.sketchfab.com/v3/models/ddd444/download", "Bearer oauth-xyz", serde_json::json!({ "glb": { "url": file } }).to_string().as_bytes());
        mock(file, &glb);
        let out = host.panel_event(idx, &map, &models, 1.0, "dl:ddd444", "");
        assert!(out.status.is_none(), "{:?}", out.status);
        let mut imports = Vec::new();
        let t = std::time::Instant::now();
        while imports.is_empty() && t.elapsed() < std::time::Duration::from_secs(10) {
            imports.extend(host.panel(idx, &map, &models, 1.0).1.imports);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_eq!(imports.len(), 1, "log {:?}", host.log);
        crate::mesh::load_model(&imports[0].0).unwrap();
        host.panel_event(idx, &map, &models, 1.0, "forget_token", "");
    }

    /// Your case: the plugins folder still has a released (broken) copy. It's
    /// replaced with the fixed one on start, and loads without asking.
    #[test]
    fn broken_released_copy_is_replaced() {
        let _turn = SKETCHFAB.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(old) = std::process::Command::new("git").args(["show", "v0.2.1:plugins/sketchfab.rhai"]).output() else { return };
        if !old.status.success() {
            return;
        }
        let mut host = crate::plugins::PluginHost::new();
        let path = host.dir.join("sketchfab.rhai");
        std::fs::write(&path, &old.stdout).unwrap();
        host.install_builtins();
        host.reload();
        let now = std::fs::read(&path).unwrap();
        let bundled = crate::plugins::BUILTIN.iter().find(|(n, _)| *n == "sketchfab.rhai").unwrap().1;
        assert_eq!(now, bundled.as_bytes(), "the old copy was replaced");
        assert!(host.plugins.iter().any(|p| p.file == "sketchfab.rhai"), "and it loads");
        assert!(host.pending.iter().all(|p| p.file != "sketchfab.rhai"), "without an approval prompt");
    }

    /// Every bundled plugin from every released version (git tags) is either
    /// the current one or on the upgrade list, so nobody is left with an old
    /// broken copy. Skips when git or the tags aren't available.
    #[test]
    fn released_plugins_all_upgrade() {
        let Ok(out) = std::process::Command::new("git").args(["tag", "-l", "v*"]).output() else { return };
        let tags: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let mut checked = 0;
        for tag in &tags {
            for (name, src) in crate::plugins::BUILTIN {
                let Ok(o) = std::process::Command::new("git").args(["show", &format!("{tag}:plugins/{name}")]).output() else { continue };
                if !o.status.success() {
                    continue;
                }
                let h = crate::plugins::fnv(&o.stdout);
                let current = o.stdout == src.as_bytes();
                let listed = crate::plugins::OLD_BUILTINS.iter().any(|(n, x)| *n == name && *x == h);
                assert!(current || listed, "{tag} shipped a different {name} (fnv {h:#018x}); add it to OLD_BUILTINS");
                checked += 1;
            }
        }
        println!("checked {checked} released plugin files from {} tag(s)", tags.len());
    }

    #[test]
    #[ignore] // talks to sketchfab.com
    fn sketchfab_search_live() {
        let (mut host, idx) = sketchfab_host();
        let (map, models) = (crate::map::Map::default(), crate::mesh::ModelCache::default());
        host.panel_event(idx, &map, &models, 1.0, "q", "barrel");
        host.panel_event(idx, &map, &models, 1.0, "search", "");
        let (w, _) = host.panel(idx, &map, &models, 1.0);
        let rows = w.iter().filter(|x| matches!(x, Widget::Row(items) if items.iter().any(|i| matches!(i, Widget::Button { id, .. } if id.starts_with("dl:"))))).count();
        let imgs = w.iter().filter(|x| matches!(x, Widget::Row(items) if items.iter().any(|i| matches!(i, Widget::Image { url, .. } if url.starts_with("https://media.sketchfab.com"))))).count();
        println!("LIVE results {rows}, with thumbnails {imgs}; log {:?}", host.log.iter().rev().take(3).collect::<Vec<_>>());
        assert!(rows > 0);
    }
}

#[test]
fn doors_cut_through_mesh_walls() {
    use crate::mesh_csg::*;
    use crate::mesh_openings::*;
    let vol = |polys: &[Vec<Vec3>]| -> f32 { polys.iter().map(|p| (1..p.len() - 1).map(|i| p[0].dot(p[i].cross(p[i + 1])) / 6.0).sum::<f32>()).sum() };
    let mut models = ModelCache::default();
    // a mesh wall 10 long, 8 high, 1 thick, turned 30 degrees, standing on y = 0
    let (v, f) = polymesh::box_poly(Vec3::new(10.0, 8.0, 1.0));
    let mut wall = Object::new(Kind::Mesh { verts: v, faces: f }, Vec3::new(3.0, 4.0, -2.0), [200; 3], Material::Concrete);
    wall.rot.y = 30.0;
    let wm = crate::mesh::world_mesh(&wall, &[], &mut models);
    let s = Settings::default();
    // aim at the wall's face from in front of it
    let n = wall.rotation() * Vec3::Z;
    let target = wall.pos + n * 0.5 + wall.rotation() * Vec3::new(1.0, -1.0, 0.0);
    let o = target + n * 20.0;
    let cut = plan_mesh_cut(&wm, o, -n, false, &s, 0.0).unwrap();
    assert!((cut.size.z - 1.4).abs() < 0.05, "measured the wall's thickness: {}", cut.size.z);
    assert!((cut.center.y - (s.door_height.min(8.1) / 2.0 - 0.05)).abs() < 0.1, "door stands on the floor: {}", cut.center.y);
    let along = cut.rot * Vec3::X;
    assert!(along.dot(wall.rotation() * Vec3::X).abs() > 0.999, "lined up with the wall");
    let before = vol(&object_polys(&wall, &mut models));
    let after = vol(&csg(object_polys(&wall, &mut models), cutter_polys(cut.center, cut.size, cut.rot), Op::Subtract));
    let door = s.door_width * (s.door_height.min(8.0)) * 1.0;
    assert!((before - after - door).abs() < 0.1, "removed exactly a door's worth: {} vs {door}", before - after);
    // windows get glass and sit at the sill
    let w = plan_mesh_cut(&wm, o, -n, true, &s, 0.0).unwrap();
    assert!(w.glass.is_some() == s.window_glass);
    assert!((w.center.y - w.size.y / 2.0 - s.window_sill).abs() < 0.05);
}

#[test]
fn pbr_maps_reach_the_material_variant() {
    let mut map = Map::default();
    let mut t = TextureDef::new("metal".into(), "C:/x/metal.png".into());
    t.roughness_map = "C:/x/metal_rough.png".into();
    t.roblox_image_id = "111".into();
    t.roblox_roughness_id = "222".into();
    t.roblox_metalness_id = "333".into();
    t.metalness_map = "C:/x/metal_metal.png".into();
    let tid = map.add_texture(t);
    let mut o = Object::brush(Shape::Box, Vec3::ONE, Vec3::ZERO, [255; 3], Material::Metal);
    o.texture = Some(tid);
    map.add(o);
    let nodes = export_roblox::build_nodes(&map, &mut ModelCache::default(), &Settings::default());
    let xml = export_roblox::to_rbxmx(&nodes, "m");
    assert!(xml.contains("<Content name=\"RoughnessMap\"><url>rbxassetid://222</url></Content>"));
    assert!(xml.contains("<Content name=\"MetalnessMap\"><url>rbxassetid://333</url></Content>"));
    let lua = export_roblox::to_luau(&nodes, "m");
    assert!(lua.contains("mv.RoughnessMap = \"rbxassetid://222\"") && lua.contains("mv.MetalnessMap = \"rbxassetid://333\""));
}

#[test]
#[ignore] // talks to GitHub: checks the real published release
fn updater_reads_the_real_release() {
    use crate::updater::*;
    // same version as this build: nothing to offer, but it must parse cleanly
    assert_eq!(check(DEFAULT_REPO), Ok(None));
    let body = std::process::Command::new("gh").args(["api", &format!("repos/{DEFAULT_REPO}/releases/latest")]).output().unwrap().stdout;
    let rel = parse_release(&serde_json::from_slice(&body).unwrap()).expect("release parses");
    let sha = rel.sha256.clone().expect("SHA-256 found in the notes");
    println!("LIVE {} {} sha {}", rel.version, rel.zip_url, sha);
    if let Ok(p) = std::env::var("ANVIL_RELEASE_ZIP") {
        let zip = std::fs::read(p).unwrap();
        let exe = verify_and_extract(&zip, &sha).expect("the published zip passes the updater's checks");
        println!("LIVE exe {} bytes", exe.len());
    }
}

mod stair_maker {
    use crate::map::{Kind, Map, Material, Object, Shape};
    use crate::mesh::{build_scene, ModelCache};
    use crate::stairs::*;
    use glam::Vec3;

    fn spec() -> StairSpec {
        StairSpec { width: 4.0, step_height: 1.0, rise: 12.0, rail: RAIL_LEFT | RAIL_RIGHT, rail_height: 3.0, stringers: true, color: [200; 3], material: Material::Concrete, trim_color: [90, 70, 50] }
    }

    fn bounds(objs: &[Object], pick: impl Fn(&Object) -> bool) -> (Vec3, Vec3) {
        let mut m = Map::default();
        for o in objs.iter().filter(|o| pick(o)) {
            m.add(o.clone());
        }
        let s = build_scene(&m, &mut ModelCache::default());
        s.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), w| (a.min(w.min), b.max(w.max)))
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-3
    }

    #[test]
    fn stairs_fill_the_space_between_the_clicks() {
        let s = spec();
        // floor at y=0, landing 6 up and 8 further along +Z
        let lay = stair_layout(Vec3::ZERO, Vec3::new(0.0, 6.0, 8.0), &s).unwrap();
        assert_eq!(lay.n, 6);
        let objs = build_stairs(&lay, &s, &[], &[], &mut ModelCache::default());
        let steps = |o: &Object| o.name.starts_with("Step");
        assert_eq!(objs.iter().filter(|o| steps(o)).count(), 6);
        let (mn, mx) = bounds(&objs, steps);
        assert!(close(mn, Vec3::new(-2.0, 0.0, 0.0)) && close(mx, Vec3::new(2.0, 6.0, 8.0)), "{mn} {mx}");
        assert_eq!(objs.iter().filter(|o| o.name.starts_with("Trim")).count(), 2);
        assert_eq!(objs.iter().filter(|o| o.name.starts_with("Handrail")).count(), 2);
        assert!(objs.iter().filter(|o| o.name.starts_with("Post")).count() >= 4);
        // clicking the top first gives the same stairs
        let lay2 = stair_layout(Vec3::new(0.0, 6.0, 8.0), Vec3::ZERO, &s).unwrap();
        assert!(close(lay2.bottom, lay.bottom) && (lay2.yaw - lay.yaw).abs() < 1e-5);
    }

    #[test]
    fn stairs_turn_to_face_the_second_click() {
        let s = spec();
        let lay = stair_layout(Vec3::new(10.0, 2.0, 5.0), Vec3::new(18.0, 6.0, 5.0), &s).unwrap();
        let objs = build_stairs(&lay, &s, &[], &[], &mut ModelCache::default());
        let (mn, mx) = bounds(&objs, |o| o.name.starts_with("Step"));
        assert!(close(mn, Vec3::new(10.0, 2.0, 3.0)) && close(mx, Vec3::new(18.0, 6.0, 7.0)), "{mn} {mx}");
        // the rail climbs toward +X
        let rail = objs.iter().find(|o| o.name == "Handrail left").unwrap();
        let along = rail.rotation() * Vec3::Z;
        assert!(along.x > 0.8 && along.y > 0.3 && along.z.abs() < 1e-4, "{along}");
        // walking up toward +X, your left hand is on the -Z side
        let right = objs.iter().find(|o| o.name == "Handrail right").unwrap();
        assert!(rail.pos.z < 5.0 && right.pos.z > 5.0, "{} {}", rail.pos, right.pos);
        // posts stand on the steps
        for p in objs.iter().filter(|o| o.name.starts_with("Post")) {
            let Kind::Brush { size, .. } = &p.kind else { panic!() };
            let foot = p.pos.y - size.y * 0.5;
            let rel = foot - 2.0;
            assert!(rel > 0.9 && rel <= 4.0 + 1e-3 && (rel - rel.round()).abs() < 1e-3, "post foot {foot}");
        }
    }

    #[test]
    fn flat_clicks_use_the_default_height_and_rails_are_optional() {
        let mut s = spec();
        s.rail = 0;
        s.stringers = false;
        let lay = stair_layout(Vec3::ZERO, Vec3::new(-12.0, 0.0, 0.0), &s).unwrap();
        assert!((lay.height - 12.0).abs() < 1e-4 && lay.n == 12);
        let objs = build_stairs(&lay, &s, &[], &[], &mut ModelCache::default());
        assert_eq!(objs.len(), 12);
        assert!(stair_layout(Vec3::ZERO, Vec3::new(0.1, 3.0, 0.0), &s).is_none(), "no run, no stairs");
    }

    #[test]
    fn prefabs_are_stretched_to_each_step() {
        let s = spec();
        let lay = stair_layout(Vec3::ZERO, Vec3::new(0.0, 3.0, 6.0), &s).unwrap();
        // a two-part step: a slab with a nosing strip
        let mut slab = Object::brush(Shape::Box, Vec3::new(2.0, 0.5, 2.0), Vec3::new(100.0, 0.25, 0.0), [1; 3], Material::Wood);
        slab.name = "slab".into();
        let nose = Object::brush(Shape::Cylinder, Vec3::new(2.0, 0.2, 0.2), Vec3::new(100.0, 0.4, -1.0), [2; 3], Material::Metal);
        let objs = build_stairs(&lay, &s, &[slab, nose], &[], &mut ModelCache::default());
        let steps: Vec<&Object> = objs.iter().filter(|o| o.name.starts_with("Step")).collect();
        assert_eq!(steps.len(), 6);
        let (mn, mx) = bounds(&objs, |o| o.name.starts_with("Step 2 "));
        assert!(close(mn, Vec3::new(-2.0, 1.0, 2.0)) && close(mx, Vec3::new(2.0, 2.0, 4.0)), "{mn} {mx}");
        assert!(steps.iter().any(|o| matches!(o.kind, Kind::Brush { shape: Shape::Cylinder, .. })));
    }
}

#[test]
fn title_font_and_logo_load() {
    use eframe::egui;
    let ctx = egui::Context::default();
    let brand = crate::branding::Brand::new(&ctx);
    assert!(brand.logo.is_some(), "logo decodes");
    let icon = crate::branding::window_icon().unwrap();
    assert_eq!((icon.width, icon.height), (256, 256));
    let have_font = std::path::Path::new(r"C:\Windows\Fonts\OCRAEXT.TTF").exists();
    assert_eq!(brand.title_font, have_font);
    let mut width = 0.0;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            width = ui.label(brand.title(17.0)).rect.width();
        });
    });
    assert!(width > 20.0, "title laid out ({width})");
    println!("OCR A Extended found: {have_font}, title width {width}");
}

mod uv_editing {
    use crate::map::{Material, Object, Shape, TexParams, TextureDef};
    use crate::mesh::{box_uv, ModelCache};
    use crate::uv_editor::*;
    use glam::{Vec2, Vec3};

    fn setup() -> (Object, TextureDef, TexParams) {
        let mut o = Object::brush(Shape::Box, Vec3::new(8.0, 4.0, 1.0), Vec3::new(3.0, 2.0, 0.0), [255; 3], Material::Plastic);
        o.rot = Vec3::new(0.0, 25.0, 0.0);
        let mut t = TextureDef::new("t".into(), String::new());
        t.scale = Vec2::new(4.0, 4.0);
        t.offset = Vec2::new(0.3, -0.2);
        (o, t, TexParams { scale: Vec2::new(1.5, 0.75), offset: Vec2::new(0.4, 1.1), rotation: 20.0, tint: [255; 3] })
    }

    fn bbox(uvs: &[[Vec2; 3]]) -> (Vec2, Vec2) {
        uvs.iter().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)))
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        (a - b).abs().max_element() < 1e-3
    }

    #[test]
    fn fit_to_face_covers_it_exactly_once() {
        let (o, t, p) = setup();
        let mut m = ModelCache::default();
        let fp = fit_params(&o, Some(4), &t, &p, &mut m).unwrap();
        let (a, b) = bbox(&target_uvs(&o, Some(4), &t, &fp, &mut m));
        assert!(near(a, Vec2::ZERO) && near(b, Vec2::ONE), "{a} {b}");
        // whole object: fits the biggest face (the 8x4 front/back)
        let big = largest_face(&o, &mut m).unwrap();
        assert!(big == 4 || big == 5, "{big}");
    }

    #[test]
    fn move_scale_rotate_do_what_the_handles_show() {
        let (o, t, p) = setup();
        let mut m = ModelCache::default();
        let before = target_uvs(&o, Some(4), &t, &p, &mut m);
        let (a0, b0) = bbox(&before);
        let c0 = (a0 + b0) * 0.5;

        // move: every UV shifts by the drag
        let d = Vec2::new(0.25, -0.5);
        let after = target_uvs(&o, Some(4), &t, &moved(&t, &p, d), &mut m);
        for (x, y) in before.iter().flatten().zip(after.iter().flatten()) {
            assert!(near(*x + d, *y), "{x} {y}");
        }

        // scale around the center: center stays, size grows
        let after = target_uvs(&o, Some(4), &t, &scaled(&t, &p, c0, Vec2::splat(2.0)), &mut m);
        for (x, y) in before.iter().flatten().zip(after.iter().flatten()) {
            assert!(near(c0 + (*x - c0) * 2.0, *y), "{x} {y}");
        }

        // rotate around the center: each UV turns around it
        let rp = rotated(&t, &p, c0, 30.0);
        assert!((rp.rotation - 50.0).abs() < 1e-4);
        let after = target_uvs(&o, Some(4), &t, &rp, &mut m);
        for (x, y) in before.iter().flatten().zip(after.iter().flatten()) {
            assert!(near(c0 + rot2(*x - c0, 30.0), *y), "{x} {y}");
        }
    }

    #[test]
    fn snapping_steps() {
        let (_, t, mut p) = setup();
        p.rotation = 22.0;
        p.scale = Vec2::new(1.3, 0.05);
        assert_eq!(snapped(&t, p, 'r').rotation, 15.0);
        let s = snapped(&t, p, 's').scale;
        assert!((s.x - 1.25).abs() < 1e-5 && (s.y - 0.05).abs() < 1e-6, "tiny scales are left alone: {s}");
        let o = snapped(&t, p, 'm').offset;
        let step = tile(&t, &p) / 8.0;
        assert!(((o.x / step.x) - (o.x / step.x).round()).abs() < 1e-4);
    }

    #[test]
    fn viewport_drag_keeps_the_texture_under_the_mouse() {
        let (_, t, p) = setup();
        // a wall facing +Z; grab a point and drag it along the wall
        let tri = [Vec3::new(-4.0, -2.0, 0.5), Vec3::new(4.0, -2.0, 0.5), Vec3::new(4.0, 2.0, 0.5)];
        let (_, u, v) = box_uv(tri, Vec2::ONE, Vec2::ZERO);
        let grab = Vec3::new(1.0, 0.5, 0.5);
        let dl = Vec3::new(1.75, -0.6, 0.0);
        let uv_at = |q: Vec3, p: &TexParams| {
            let (uv, _, _) = box_uv([q, q + Vec3::X, q + Vec3::Y], t.scale * p.scale, t.offset + p.offset);
            rot2(uv[0], p.rotation)
        };
        let mut np = p;
        np.offset = p.offset - Vec2::new(dl.dot(u), -dl.dot(v));
        assert!(near(uv_at(grab, &p), uv_at(grab + dl, &np)));
    }
}
