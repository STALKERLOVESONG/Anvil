//! Source engine maps (.bsp from Garry's Mod, Half-Life 2 and other Source
//! games) turned into ordinary, editable Anvil objects:
//!
//! * every brush becomes its own polygon mesh, with each face keeping its
//!   Hammer texture alignment (`FaceTex::uv`),
//! * displacements (terrain) become meshes,
//! * static props and prop_* entities become textured props,
//! * lights become lights and player starts become spawns,
//! * clip / invisible brushes go into a hidden folder.
//!
//! Map files come from the internet, so everything here is bounds-checked:
//! a broken or hostile file gives an error, never a crash or a huge
//! allocation, and packed file names can't escape the content folder.

use crate::map::{FaceTex, Kind, Map, Material, Object, TexParams, TextureDef, UvAxes};
use crate::mount::{self, Content, Layered, Mounts};
use crate::source::norm;
use glam::{DVec3, Mat3, Quat, Vec2, Vec3};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 2 << 30;
const MAX_PAK_FILE: u64 = 256 << 20;
const MAX_PAK_TOTAL: u64 = 1536 << 20;

// contents flags
const CONTENTS_SOLID: i32 = 0x1;
const CONTENTS_WINDOW: i32 = 0x2;
const CONTENTS_GRATE: i32 = 0x8;
const CONTENTS_SLIME: i32 = 0x10;
const CONTENTS_WATER: i32 = 0x20;
const CONTENTS_PLAYERCLIP: i32 = 0x10000;
const CONTENTS_MONSTERCLIP: i32 = 0x20000;
const CONTENTS_ORIGIN: i32 = 0x1000000;
const CONTENTS_LADDER: i32 = 0x20000000;
// surface flags that are never drawn
const SURF_SKY2D: i32 = 0x2;
const SURF_SKY: i32 = 0x4;
const SURF_NODRAW: i32 = 0x80;
const SURF_HINT: i32 = 0x100;
const SURF_SKIP: i32 = 0x200;

// ---------------------------------------------------------------------------
// little-endian reads (0 when out of range; callers check sizes first)
// ---------------------------------------------------------------------------

fn i32_(b: &[u8], o: usize) -> i32 {
    b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn u16_(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn i16_(b: &[u8], o: usize) -> i16 {
    u16_(b, o) as i16
}
fn f32_(b: &[u8], o: usize) -> f32 {
    let v = b.get(o..o + 4).map(|s| f32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0.0);
    if v.is_finite() {
        v
    } else {
        0.0
    }
}
fn vec_(b: &[u8], o: usize) -> DVec3 {
    DVec3::new(f32_(b, o) as f64, f32_(b, o + 4) as f64, f32_(b, o + 8) as f64)
}

// ---------------------------------------------------------------------------
// the file
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct TexInfo {
    pub s: [f32; 4],
    pub t: [f32; 4],
    pub flags: i32,
    pub texdata: i32,
}

#[derive(Clone, Debug)]
pub struct TexData {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub plane: u16,
    pub side: u8,
    pub first_edge: i32,
    pub num_edges: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub first_side: i32,
    pub num_sides: i32,
    pub contents: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct BrushSide {
    pub plane: u16,
    pub texinfo: i16,
    #[allow(dead_code)]
    pub dispinfo: i16,
    pub bevel: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Model {
    #[allow(dead_code)]
    pub origin: DVec3,
    pub headnode: i32,
    pub first_face: i32,
    pub num_faces: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct DispInfo {
    pub start: DVec3,
    pub first_vert: i32,
    pub power: i32,
    pub face: u16,
}

#[derive(Clone, Debug)]
pub struct StaticProp {
    pub model: String,
    pub origin: DVec3,
    pub angles: DVec3,
    pub solid: bool,
}

/// A parsed .bsp.
pub struct Bsp {
    #[allow(dead_code)]
    pub version: i32,
    pub planes: Vec<(DVec3, f64)>,
    pub texinfo: Vec<TexInfo>,
    pub texdata: Vec<TexData>,
    pub verts: Vec<DVec3>,
    pub edges: Vec<[u16; 2]>,
    pub surfedges: Vec<i32>,
    pub faces: Vec<Face>,
    pub brushes: Vec<Brush>,
    pub sides: Vec<BrushSide>,
    pub models: Vec<Model>,
    /// (plane, [front, back]); a negative child is leaf `-1 - child`.
    pub nodes: Vec<(i32, [i32; 2])>,
    /// (area, first leafbrush, leafbrush count)
    pub leafs: Vec<(u16, u16, u16)>,
    pub leafbrushes: Vec<u16>,
    pub dispinfo: Vec<DispInfo>,
    pub dispverts: Vec<(DVec3, f32)>,
    pub entities: Vec<HashMap<String, String>>,
    pub static_props: Vec<StaticProp>,
    /// Files packed into the map (materials/…, models/…), by normalized path.
    pub pak: HashMap<String, Vec<u8>>,
    pub warnings: Vec<String>,
}

fn lump_slice<'a>(b: &'a [u8], lumps: &[(usize, usize, i32)], i: usize) -> Result<&'a [u8], String> {
    let (ofs, len, _) = lumps[i];
    let s = b.get(ofs..ofs.checked_add(len).ok_or("bad lump")?).ok_or_else(|| format!("lump {i} is outside the file"))?;
    if s.starts_with(b"LZMA") {
        return Err("this map's data is compressed (a newer CS:GO / TF2 style map); only Garry's Mod / Half-Life 2 style maps are supported".into());
    }
    Ok(s)
}

fn records<T>(s: &[u8], size: usize, f: impl Fn(&[u8]) -> T) -> Vec<T> {
    s.chunks_exact(size).map(f).collect()
}

/// Splits entity text into key/value blocks.
pub fn parse_entities(text: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: Option<HashMap<String, String>> = None;
    let mut key: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                cur = Some(HashMap::new());
                key = None;
            }
            '}' => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
            }
            '"' => {
                let mut s = String::new();
                for ch in chars.by_ref() {
                    if ch == '"' {
                        break;
                    }
                    s.push(ch);
                }
                if let Some(e) = cur.as_mut() {
                    match key.take() {
                        None => key = Some(s.to_ascii_lowercase()),
                        // first value wins (outputs repeat keys like "OnTrigger")
                        Some(k) => {
                            e.entry(k).or_insert(s);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

impl Bsp {
    pub fn parse(b: &[u8]) -> Result<Bsp, String> {
        if b.len() < 8 + 64 * 16 || &b[0..4] != b"VBSP" {
            return Err("not a Source engine map (.bsp)".into());
        }
        let version = i32_(b, 4);
        if !(17..=29).contains(&version) {
            return Err(format!("unsupported map version {version}"));
        }
        let mut lumps = Vec::with_capacity(64);
        for i in 0..64 {
            let o = 8 + i * 16;
            let (a, c, d) = (i32_(b, o), i32_(b, o + 4), i32_(b, o + 8));
            // Left 4 Dead 2 stores (version, offset, length)
            let ok = |ofs: i32, len: i32| ofs >= 0 && len >= 0 && (ofs as u64 + len as u64) <= b.len() as u64;
            let (ofs, len, ver) = if ok(a, c) { (a, c, d) } else if ok(c, d) { (c, d, a) } else { (0, 0, 0) };
            lumps.push((ofs as usize, len as usize, ver));
        }
        let l = |i: usize| lump_slice(b, &lumps, i);
        let mut warnings = Vec::new();

        let planes = records(l(1)?, 20, |r| (vec_(r, 0), f32_(r, 12) as f64));
        let texinfo = records(l(6)?, 72, |r| TexInfo {
            s: [f32_(r, 0), f32_(r, 4), f32_(r, 8), f32_(r, 12)],
            t: [f32_(r, 16), f32_(r, 20), f32_(r, 24), f32_(r, 28)],
            flags: i32_(r, 64),
            texdata: i32_(r, 68),
        });
        let strdata = l(43)?;
        let strtab: Vec<i32> = records(l(44)?, 4, |r| i32_(r, 0));
        let texdata = records(l(2)?, 32, |r| {
            let name = strtab
                .get(i32_(r, 12).max(0) as usize)
                .and_then(|o| strdata.get((*o).max(0) as usize..))
                .map(|s| String::from_utf8_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len()).min(256)]).to_string())
                .unwrap_or_default();
            TexData { name, width: i32_(r, 16).clamp(1, 65536) as u32, height: i32_(r, 20).clamp(1, 65536) as u32 }
        });
        let verts = records(l(3)?, 12, |r| vec_(r, 0));
        let edges = records(l(12)?, 4, |r| [u16_(r, 0), u16_(r, 2)]);
        let surfedges = records(l(13)?, 4, |r| i32_(r, 0));
        let faces = records(l(7)?, 56, |r| Face {
            plane: u16_(r, 0),
            side: r[2],
            first_edge: i32_(r, 4),
            num_edges: i16_(r, 8),
            texinfo: i16_(r, 10),
            dispinfo: i16_(r, 12),
        });
        let brushes = records(l(18)?, 12, |r| Brush { first_side: i32_(r, 0), num_sides: i32_(r, 4), contents: i32_(r, 8) });
        let sides = records(l(19)?, 8, |r| BrushSide { plane: u16_(r, 0), texinfo: i16_(r, 2), dispinfo: i16_(r, 4), bevel: r[6] != 0 });
        let models = records(l(14)?, 48, |r| Model { origin: vec_(r, 24), headnode: i32_(r, 36), first_face: i32_(r, 40), num_faces: i32_(r, 44) });
        let nodes = records(l(5)?, 32, |r| (i32_(r, 0), [i32_(r, 4), i32_(r, 8)]));
        let leaf_size = if lumps[10].2 == 0 { 56 } else { 32 };
        let leafs = records(l(10)?, leaf_size, |r| ((u16_(r, 6) & 0x1ff), u16_(r, 24), u16_(r, 26)));
        let leafbrushes = records(l(17)?, 2, |r| u16_(r, 0));
        let dispinfo = records(l(26)?, 176, |r| DispInfo { start: vec_(r, 0), first_vert: i32_(r, 12), power: i32_(r, 20), face: u16_(r, 36) });
        let dispverts = records(l(33)?, 20, |r| (vec_(r, 0), f32_(r, 12)));
        let ents = l(0)?;
        let entities = parse_entities(&String::from_utf8_lossy(&ents[..ents.iter().position(|c| *c == 0).unwrap_or(ents.len())]));

        // static props live in the "game lump"
        let mut static_props = Vec::new();
        if let Ok(g) = l(35) {
            let n = i32_(g, 0).clamp(0, 64) as usize;
            for k in 0..n {
                let o = 4 + k * 16;
                if g.len() < o + 16 {
                    break;
                }
                let (id, flags, ver, ofs, len) = (&g[o..o + 4], u16_(g, o + 4), u16_(g, o + 6), i32_(g, o + 8), i32_(g, o + 12));
                if id != b"prps" {
                    continue; // "sprp" stored little-endian
                }
                if flags & 1 != 0 {
                    warnings.push("static props are compressed in this map, so they were skipped".into());
                    continue;
                }
                let Some(s) = b.get(ofs.max(0) as usize..(ofs.max(0) as usize).saturating_add(len.max(0) as usize)) else {
                    warnings.push("static prop data is outside the file".into());
                    continue;
                };
                match read_static_props(s, ver) {
                    Ok(p) => static_props = p,
                    Err(e) => warnings.push(format!("static props: {e}")),
                }
            }
        }

        let mut pak = HashMap::new();
        if let Ok(p) = l(40) {
            if p.len() > 22 {
                match read_pak(p) {
                    Ok(m) => pak = m,
                    Err(e) => warnings.push(format!("packed files: {e}")),
                }
            }
        }

        Ok(Bsp {
            version,
            planes,
            texinfo,
            texdata,
            verts,
            edges,
            surfedges,
            faces,
            brushes,
            sides,
            models,
            nodes,
            leafs,
            leafbrushes,
            dispinfo,
            dispverts,
            entities,
            static_props,
            pak,
            warnings,
        })
    }

    pub fn open(path: &Path) -> Result<Bsp, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_FILE {
            return Err("that file is too big to be a map".into());
        }
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Bsp::parse(&b)
    }

    /// The polygon of a face (from its edges), in Source units.
    pub fn face_poly(&self, f: &Face) -> Vec<DVec3> {
        let mut out = Vec::new();
        for e in 0..f.num_edges.max(0) as i64 {
            let Some(&se) = self.surfedges.get((f.first_edge as i64 + e).max(0) as usize) else { return Vec::new() };
            let Some(ed) = self.edges.get(se.unsigned_abs() as usize) else { return Vec::new() };
            let vi = if se >= 0 { ed[0] } else { ed[1] };
            let Some(v) = self.verts.get(vi as usize) else { return Vec::new() };
            out.push(*v);
        }
        out
    }

    /// Every leaf under a node (iterative; cycles in a broken file are ignored).
    fn leaves_under(&self, head: i32) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![head];
        while let Some(n) = stack.pop() {
            if n < 0 {
                let leaf = (-1 - n) as usize;
                if leaf < self.leafs.len() {
                    out.push(leaf);
                }
                continue;
            }
            let ni = n as usize;
            if ni >= self.nodes.len() || seen[ni] {
                continue;
            }
            seen[ni] = true;
            stack.extend(self.nodes[ni].1);
        }
        out
    }

    /// Brushes inside a model's part of the tree.
    pub fn model_brushes(&self, model: usize) -> Vec<usize> {
        let Some(m) = self.models.get(model) else { return Vec::new() };
        let mut set = HashSet::new();
        let mut out = Vec::new();
        for leaf in self.leaves_under(m.headnode) {
            let (_, first, n) = self.leafs[leaf];
            for k in first as usize..first as usize + n as usize {
                if let Some(&bi) = self.leafbrushes.get(k) {
                    if (bi as usize) < self.brushes.len() && set.insert(bi) {
                        out.push(bi as usize);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The area a point is in (world tree).
    pub fn area_at(&self, p: DVec3) -> Option<u16> {
        let mut n = self.models.first()?.headnode;
        for _ in 0..self.nodes.len() + 1 {
            if n < 0 {
                return self.leafs.get((-1 - n) as usize).map(|l| l.0);
            }
            let (plane, kids) = *self.nodes.get(n as usize)?;
            let (normal, dist) = *self.planes.get(plane.max(0) as usize)?;
            n = if normal.dot(p) - dist >= 0.0 { kids[0] } else { kids[1] };
        }
        None
    }

    fn material(&self, texinfo: i16) -> Option<(&TexInfo, &TexData)> {
        let ti = self.texinfo.get(usize::try_from(texinfo).ok()?)?;
        let td = self.texdata.get(usize::try_from(ti.texdata).ok()?)?;
        Some((ti, td))
    }

    /// Whether a face with this texinfo is drawn in game.
    fn visible(&self, texinfo: i16) -> bool {
        let Some((ti, td)) = self.material(texinfo) else { return false };
        if ti.flags & (SURF_NODRAW | SURF_SKY | SURF_SKY2D | SURF_HINT | SURF_SKIP) != 0 {
            return false;
        }
        let n = norm(&td.name);
        !(n.starts_with("tools/") && n != "tools/toolsblack")
    }
}

fn read_static_props(s: &[u8], ver: u16) -> Result<Vec<StaticProp>, String> {
    let mut o = 0usize;
    let dict = i32_(s, o).clamp(0, 65536) as usize;
    o += 4;
    let mut names = Vec::with_capacity(dict);
    for _ in 0..dict {
        let r = s.get(o..o + 128).ok_or("cut off")?;
        let end = r.iter().position(|c| *c == 0).unwrap_or(128);
        names.push(String::from_utf8_lossy(&r[..end]).to_string());
        o += 128;
    }
    let leaves = i32_(s, o).clamp(0, 1 << 20) as usize;
    o += 4 + leaves * 2;
    let count = i32_(s, o).clamp(0, 1 << 20) as usize;
    o += 4;
    if count == 0 {
        return Ok(Vec::new());
    }
    let rest = s.len().saturating_sub(o);
    let size = rest / count;
    if size < 32 {
        return Err(format!("unknown static prop layout (version {ver})"));
    }
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let r = &s[o + k * size..o + (k + 1) * size];
        let Some(model) = names.get(u16_(r, 24) as usize) else { continue };
        out.push(StaticProp { model: model.clone(), origin: vec_(r, 0), angles: vec_(r, 12), solid: r[30] != 0 });
    }
    Ok(out)
}

fn read_pak(p: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(p)).map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    let mut total = 0u64;
    for i in 0..z.len().min(100_000) {
        let Ok(mut f) = z.by_index(i) else { continue };
        if f.is_dir() {
            continue;
        }
        let name = norm(f.name());
        if !(name.starts_with("materials/") || name.starts_with("models/")) {
            continue;
        }
        let mut buf = Vec::new();
        if (&mut f).take(MAX_PAK_FILE + 1).read_to_end(&mut buf).is_err() || buf.len() as u64 > MAX_PAK_FILE {
            continue;
        }
        total += buf.len() as u64;
        if total > MAX_PAK_TOTAL {
            return Err("too much packed data; the rest was skipped".into());
        }
        out.insert(name, buf);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------

/// A big square on a plane, counter-clockwise seen from the front.
fn base_winding(n: DVec3, d: f64) -> Vec<DVec3> {
    let up = if n.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let u = up.cross(n).normalize();
    let v = n.cross(u);
    let c = n * d;
    let r = 131072.0;
    vec![c - u * r - v * r, c + u * r - v * r, c + u * r + v * r, c - u * r + v * r]
}

/// Keeps the part of a polygon behind a plane (n . p <= d).
fn clip(poly: &[DVec3], n: DVec3, d: f64) -> Vec<DVec3> {
    const EPS: f64 = 0.01;
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let da = n.dot(a) - d;
        let db = n.dot(b) - d;
        if da <= EPS {
            out.push(a);
        }
        if (da < -EPS && db > EPS) || (da > EPS && db < -EPS) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// The polygon of every side of a convex brush (None where a side ends up
/// with no area), in Source units.
pub fn brush_polys(planes: &[(DVec3, f64)]) -> Vec<Option<Vec<DVec3>>> {
    (0..planes.len())
        .map(|i| {
            let (n, d) = planes[i];
            if n.length_squared() < 0.5 {
                return None;
            }
            let mut w = base_winding(n, d);
            for (j, &(nj, dj)) in planes.iter().enumerate() {
                if j == i {
                    continue;
                }
                // the same plane twice: keep only the first
                if nj.dot(n) > 0.9999 && (dj - d).abs() < 0.01 {
                    if j < i {
                        return None;
                    }
                    continue;
                }
                w = clip(&w, nj, dj);
                if w.len() < 3 {
                    return None;
                }
            }
            Some(w)
        })
        .collect()
}

/// Source (Z up, X forward, Y left) -> Anvil (Y up) in studs.
fn to_anvil(p: DVec3, k: f64) -> DVec3 {
    DVec3::new(p.x, p.z, -p.y) * k
}

/// A Source rotation (pitch, yaw, roll in degrees) as an Anvil rotation.
pub fn source_rotation(angles: DVec3) -> Quat {
    let (p, y, r) = (angles.x.to_radians() as f32, angles.y.to_radians() as f32, angles.z.to_radians() as f32);
    let src = Mat3::from_rotation_z(y) * Mat3::from_rotation_y(p) * Mat3::from_rotation_x(r);
    let c = Mat3::from_cols(Vec3::X, Vec3::NEG_Z, Vec3::Y);
    Quat::from_mat3(&(c * src * c.transpose())).normalize()
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    World,
    Terrain,
    BrushEntities,
    Props,
    Lights,
    Spawns,
    Hidden,
    Skybox,
}

impl Group {
    fn name(self) -> &'static str {
        match self {
            Group::World => "World",
            Group::Terrain => "Terrain (displacements)",
            Group::BrushEntities => "Brush entities",
            Group::Props => "Props",
            Group::Lights => "Lights",
            Group::Spawns => "Spawns",
            Group::Hidden => "Clips and invisible walls (hidden)",
            Group::Skybox => "3D skybox (hidden)",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub brushes: usize,
    pub faces: usize,
    pub displacements: usize,
    pub brush_entities: usize,
    pub props: usize,
    pub lights: usize,
    pub spawns: usize,
    pub hidden: usize,
    pub skybox: usize,
    pub textures: usize,
    pub missing_materials: Vec<String>,
    pub missing_models: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} brushes ({} faces), {} terrain pieces, {} brush entities, {} props, {} lights, {} spawns, {} textures",
            self.brushes, self.faces, self.displacements, self.brush_entities, self.props, self.lights, self.spawns, self.textures
        );
        if self.hidden + self.skybox > 0 {
            s += &format!("; {} hidden (clips / 3D skybox)", self.hidden + self.skybox);
        }
        if !self.missing_materials.is_empty() {
            s += &format!("; {} materials not found", self.missing_materials.len());
        }
        if !self.missing_models.is_empty() {
            s += &format!("; {} models not found", self.missing_models.len());
        }
        s
    }
}

/// An imported map, ready to be added to an Anvil map with [`apply`].
/// Texture ids in objects are indexes into `textures` until then.
pub struct Imported {
    pub name: String,
    pub objects: Vec<(Object, Group)>,
    pub textures: Vec<TextureDef>,
    pub report: Report,
}

pub struct Options {
    /// Studs per Source unit (Settings > Source scale).
    pub scale: f32,
    pub spawn_size: Vec3,
    pub props: bool,
}

struct Ctx<'a> {
    bsp: &'a Bsp,
    content: &'a dyn Content,
    k: f64,
    /// material name -> texture index (None = not found)
    mats: HashMap<String, Option<usize>>,
    textures: Vec<TextureDef>,
    report: Report,
    progress: &'a dyn Fn(String),
    /// (plane normal, plane distance, centre) of every displacement face
    disp_faces: Vec<(DVec3, f64, DVec3)>,
}

impl Ctx<'_> {
    fn texture_for(&mut self, texinfo: i16) -> Option<usize> {
        let (_, td) = self.bsp.material(texinfo)?;
        let name = norm(&td.name);
        if let Some(r) = self.mats.get(&name) {
            return *r;
        }
        let r = match mount::import_material(self.content, &name) {
            Ok(f) => {
                let mut t = TextureDef::new(f.name, f.image);
                t.normal_map = f.normal;
                t.alpha = f.alpha;
                let s = 0.25 * self.k as f32;
                t.scale = Vec2::new(td.width as f32 * s, td.height as f32 * s);
                self.textures.push(t);
                if self.textures.len() % 20 == 0 {
                    (self.progress)(format!("Textures: {}", self.textures.len()));
                }
                Some(self.textures.len() - 1)
            }
            Err(_) => {
                self.report.missing_materials.push(name.clone());
                None
            }
        };
        self.mats.insert(name, r);
        r
    }

    /// The face texture of a Source face whose object sits at `pos` (studs).
    fn face_tex(&mut self, texinfo: i16, pos: DVec3) -> Option<FaceTex> {
        let tex = self.texture_for(texinfo)?;
        let (ti, _) = self.bsp.material(texinfo)?;
        let k = self.k;
        let axis = |a: [f32; 4]| {
            let s = DVec3::new(a[0] as f64, a[2] as f64, -(a[1] as f64)); // Source -> Anvil axes
            // u_texels = p_src . s + w  ->  in tile units of 0.25 texels per Source unit
            ((s * 0.25).as_vec3(), ((pos.dot(s) / k + a[3] as f64) * 0.25 * k) as f32)
        };
        let (u, ou) = axis(ti.s);
        let (v, ov) = axis(ti.t);
        Some(FaceTex {
            texture: Some(tex as u64),
            params: TexParams { offset: Vec2::new(ou, ov), ..TexParams::default() },
            uv: Some(UvAxes { u, v }),
        })
    }

    /// A mesh object from polygons in Source units (each with its texinfo).
    fn mesh_object(&mut self, polys: &[(Vec<DVec3>, i16)], offset: DVec3) -> Option<Object> {
        let k = self.k;
        let mut mn = DVec3::splat(f64::MAX);
        let mut mx = DVec3::splat(f64::MIN);
        for (p, _) in polys {
            for v in p {
                let a = to_anvil(*v + offset, k);
                mn = mn.min(a);
                mx = mx.max(a);
            }
        }
        if mn.x > mx.x {
            return None;
        }
        let center = (mn + mx) * 0.5;
        let mut verts: Vec<Vec3> = Vec::new();
        let mut index: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut faces = Vec::new();
        let mut face_tex = Vec::new();
        for (p, texinfo) in polys {
            let mut f: Vec<u32> = Vec::with_capacity(p.len());
            for v in p {
                let s = *v + offset;
                let key = ((s.x * 16.0).round() as i64, (s.y * 16.0).round() as i64, (s.z * 16.0).round() as i64);
                let i = *index.entry(key).or_insert_with(|| {
                    verts.push((to_anvil(s, k) - center).as_vec3());
                    verts.len() as u32 - 1
                });
                if f.last() != Some(&i) && f.first() != Some(&i) {
                    f.push(i);
                }
            }
            if f.len() < 3 {
                continue;
            }
            faces.push(f);
            face_tex.push(self.face_tex(*texinfo, center));
            self.report.faces += 1;
        }
        if faces.is_empty() {
            return None;
        }
        let mut o = Object::new(Kind::Mesh { verts, faces }, center.as_vec3(), [200, 200, 200], Material::Plastic);
        o.face_tex = face_tex;
        Some(o)
    }

    fn brush_object(&mut self, bi: usize, offset: DVec3) -> Option<(Object, bool)> {
        let bsp = self.bsp;
        let b = bsp.brushes[bi];
        if b.contents & CONTENTS_ORIGIN != 0 {
            return None;
        }
        let first = b.first_side.max(0) as usize;
        let sides: Vec<BrushSide> = (first..first + b.num_sides.clamp(0, 4096) as usize).filter_map(|i| bsp.sides.get(i).copied()).filter(|s| !s.bevel).collect();
        let planes: Vec<(DVec3, f64)> = sides.iter().map(|s| bsp.planes.get(s.plane as usize).copied().unwrap_or((DVec3::ZERO, 0.0))).collect();
        // a displacement's brush: the terrain mesh replaces it. (The side's
        // own dispinfo field isn't reliable: compilers leave 0 in it.)
        let is_disp = self.disp_faces.iter().any(|(n, d, c)| {
            planes.iter().any(|(pn, pd)| pn.dot(*n) > 0.999 && (pd - d).abs() < 0.5) && planes.iter().all(|(pn, pd)| pn.dot(*c) - pd <= 0.5)
        });
        if is_disp {
            return None;
        }
        let polys = brush_polys(&planes);
        let mut shown = Vec::new();
        let mut all = Vec::new();
        let mut all_sky = true;
        for (s, p) in sides.iter().zip(polys) {
            let Some(p) = p else { continue };
            if !bsp.material(s.texinfo).is_some_and(|(ti, _)| ti.flags & (SURF_SKY | SURF_SKY2D) != 0) {
                all_sky = false;
            }
            if bsp.visible(s.texinfo) {
                shown.push((p.clone(), s.texinfo));
            }
            all.push((p, -1i16));
        }
        if !shown.is_empty() {
            let mut o = self.mesh_object(&shown, offset)?;
            if b.contents & (CONTENTS_WATER | CONTENTS_SLIME) != 0 {
                o.collide = false;
                o.transparency = 0.35;
                o.color = [70, 120, 160];
            }
            self.report.brushes += 1;
            return Some((o, false));
        }
        // nothing drawn: a clip / invisible wall keeps its collision, hidden
        let solid = b.contents & (CONTENTS_SOLID | CONTENTS_WINDOW | CONTENTS_GRATE | CONTENTS_PLAYERCLIP | CONTENTS_MONSTERCLIP | CONTENTS_LADDER) != 0;
        if !solid || all_sky {
            return None;
        }
        let faces_before = self.report.faces;
        let mut o = self.mesh_object(&all, offset)?;
        self.report.faces = faces_before;
        o.hidden = true;
        o.color = [220, 120, 60];
        o.transparency = 0.6;
        Some((o, true))
    }

    /// The grid mesh of displacement `di`.
    fn displacement(&mut self, di: usize) -> Option<Object> {
        let bsp = self.bsp;
        let d = bsp.dispinfo[di];
        let face = *bsp.faces.get(d.face as usize)?;
        let corners = bsp.face_poly(&face);
        if corners.len() != 4 || !(1..=4).contains(&d.power) {
            return None;
        }
        let start = (0..4).min_by(|&a, &b| (corners[a] - d.start).length_squared().total_cmp(&(corners[b] - d.start).length_squared()))?;
        let c: Vec<DVec3> = (0..4).map(|i| corners[(start + i) % 4]).collect();
        let n = (1usize << d.power) + 1;
        let first = usize::try_from(d.first_vert).ok()?;
        if first + n * n > bsp.dispverts.len() {
            return None;
        }
        let mut pts = Vec::with_capacity(n * n);
        for y in 0..n {
            let ty = y as f64 / (n - 1) as f64;
            let a = c[0].lerp(c[1], ty);
            let b = c[3].lerp(c[2], ty);
            for x in 0..n {
                let base = a.lerp(b, x as f64 / (n - 1) as f64);
                let (dir, dist) = bsp.dispverts[first + y * n + x];
                pts.push(base + dir * dist as f64);
            }
        }
        // winding: the grid's quads must face the way the face does
        let (pn, _) = *bsp.planes.get(face.plane as usize)?;
        let pn = if face.side != 0 { -pn } else { pn };
        let flip = (c[3] - c[0]).cross(c[1] - c[0]).dot(pn) < 0.0;
        let mut polys = Vec::with_capacity((n - 1) * (n - 1));
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let mut q = vec![pts[y * n + x], pts[y * n + x + 1], pts[(y + 1) * n + x + 1], pts[(y + 1) * n + x]];
                if flip {
                    q.reverse();
                }
                polys.push((q, face.texinfo));
            }
        }
        let mut o = self.mesh_object(&polys, DVec3::ZERO)?;
        o.name = format!("terrain {di}");
        self.report.displacements += 1;
        Some(o)
    }
}

fn num3(s: Option<&String>) -> Option<DVec3> {
    let v: Vec<f64> = s?.split_whitespace().filter_map(|x| x.parse().ok()).filter(|x: &f64| x.is_finite()).collect();
    (v.len() >= 3).then(|| DVec3::new(v[0], v[1], v[2]))
}

fn short_name(material: &str) -> String {
    material.rsplit('/').next().unwrap_or(material).to_string()
}

/// Reads a map and turns it into Anvil objects. `mounts` supplies the
/// game content (textures, models); the map's own packed files come first.
pub fn import(path: &Path, mounts: Option<&Mounts>, opts: &Options, progress: &dyn Fn(String)) -> Result<Imported, String> {
    progress("Reading the map…".into());
    let bsp = Bsp::open(path)?;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("map").to_string();
    // an addon folder layout (addon/maps/x.bsp) has the map's content next to it
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(parent) = path.parent() {
        for d in [parent.to_path_buf(), parent.parent().map(|p| p.to_path_buf()).unwrap_or_default()] {
            if d.join("materials").is_dir() || d.join("models").is_dir() {
                dirs.push(d);
            }
        }
    }
    let content = Layered { files: &bsp.pak, dirs, base: mounts };
    import_parsed(&bsp, &name, &content, opts, progress)
}

pub fn import_parsed(bsp: &Bsp, name: &str, content: &dyn Content, opts: &Options, progress: &dyn Fn(String)) -> Result<Imported, String> {
    let k = opts.scale.max(1e-4) as f64;
    let disp_faces = bsp
        .dispinfo
        .iter()
        .filter_map(|d| {
            let f = bsp.faces.get(d.face as usize)?;
            let (n, dist) = *bsp.planes.get(f.plane as usize)?;
            let poly = bsp.face_poly(f);
            (!poly.is_empty()).then(|| (n, dist, poly.iter().copied().sum::<DVec3>() / poly.len() as f64))
        })
        .collect();
    let mut cx = Ctx { bsp, content, k, mats: HashMap::new(), textures: Vec::new(), report: Report::default(), progress, disp_faces };
    cx.report.warnings = bsp.warnings.clone();
    let mut objects: Vec<(Object, Group)> = Vec::new();

    // the 3D skybox: everything in the sky_camera's area is hidden
    let sky_area = bsp
        .entities
        .iter()
        .find(|e| e.get("classname").map(|c| c.as_str()) == Some("sky_camera"))
        .and_then(|e| num3(e.get("origin")))
        .and_then(|p| bsp.area_at(p));
    let in_sky = |p: DVec3| sky_area.is_some() && bsp.area_at(p) == sky_area;

    // world brushes
    progress("Building brushes…".into());
    let mut used = vec![false; bsp.brushes.len()];
    for bi in bsp.model_brushes(0) {
        used[bi] = true;
        let Some((mut o, hidden)) = cx.brush_object(bi, DVec3::ZERO) else { continue };
        let first_tex = o.face_tex.iter().flatten().filter_map(|f| f.texture).next();
        let label = first_tex.and_then(|t| cx.textures.get(t as usize)).map(|t| short_name(&t.name)).unwrap_or_else(|| "brush".into());
        o.name = format!("{label} {bi}");
        // a point just above the brush tells which area it's in
        let top = match &o.kind {
            Kind::Mesh { verts, .. } => verts.iter().map(|v| v.y).fold(f32::MIN, f32::max),
            _ => 0.0,
        };
        let probe = DVec3::new(o.pos.x as f64, -(o.pos.z as f64), (o.pos.y + top) as f64) / k;
        let group = if hidden {
            cx.report.hidden += 1;
            Group::Hidden
        } else if sky_area.is_some() && in_sky(probe + DVec3::Z * 2.0) {
            o.hidden = true;
            cx.report.brushes -= 1;
            cx.report.skybox += 1;
            Group::Skybox
        } else {
            Group::World
        };
        objects.push((o, group));
    }

    // terrain
    if !bsp.dispinfo.is_empty() {
        progress("Building terrain…".into());
    }
    for di in 0..bsp.dispinfo.len() {
        if let Some(o) = cx.displacement(di) {
            objects.push((o, Group::Terrain));
        }
    }

    // entities
    progress("Placing entities…".into());
    let mut models: HashMap<String, Result<String, String>> = HashMap::new();
    let mut prop = |cx: &mut Ctx, mdl: &str, origin: DVec3, angles: DVec3, scale: f32, solid: bool, label: &str| -> Option<Object> {
        let key = norm(mdl);
        if !key.ends_with(".mdl") {
            return None;
        }
        let r = models.entry(key.clone()).or_insert_with(|| {
            let r = mount::import_model(cx.content, &key, k as f32).map(|p| p.to_string_lossy().to_string());
            if let Err(e) = &r {
                cx.report.missing_models.push(if e.ends_with("not found") { key.clone() } else { format!("{key} ({e})") });
            }
            r
        });
        let Ok(path) = r.clone() else { return None };
        let mut o = Object::new(Kind::Prop { model: path, scale: Vec3::splat(scale), roblox_mesh_id: String::new() }, to_anvil(origin, k).as_vec3(), [255, 255, 255], Material::Plastic);
        o.set_rotation(source_rotation(angles));
        o.collide = solid;
        o.name = format!("{label} {}", short_name(&key).trim_end_matches(".mdl"));
        cx.report.props += 1;
        Some(o)
    };
    if opts.props {
        for (i, sp) in bsp.static_props.iter().enumerate() {
            if i % 50 == 0 {
                progress(format!("Props: {i} of {}", bsp.static_props.len()));
            }
            if let Some(mut o) = prop(&mut cx, &sp.model, sp.origin, sp.angles, 1.0, sp.solid, "static") {
                let g = if in_sky(sp.origin + DVec3::Z * 2.0) {
                    o.hidden = true;
                    cx.report.props -= 1;
                    cx.report.skybox += 1;
                    Group::Skybox
                } else {
                    Group::Props
                };
                objects.push((o, g));
            }
        }
    }
    for (ei, e) in bsp.entities.iter().enumerate() {
        let class = e.get("classname").map(|s| s.as_str()).unwrap_or("");
        let origin = num3(e.get("origin")).unwrap_or(DVec3::ZERO);
        let angles = num3(e.get("angles")).unwrap_or(DVec3::ZERO);
        let label = e.get("targetname").filter(|t| !t.is_empty()).cloned().unwrap_or_else(|| class.to_string());
        let sky = in_sky(origin + DVec3::Z * 2.0);
        // brush entities (doors, breakables, func_detail-like brushes)
        if let Some(mi) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse::<usize>().ok()) {
            if mi == 0 || mi >= bsp.models.len() || class.starts_with("trigger_") {
                continue;
            }
            let offset = origin;
            let brushes: Vec<usize> = bsp.model_brushes(mi).into_iter().filter(|b| !used[*b]).collect();
            let mut made = false;
            for bi in &brushes {
                used[*bi] = true;
                let Some((mut o, hidden)) = cx.brush_object(*bi, offset) else { continue };
                o.name = format!("{label} {bi}");
                if class == "func_illusionary" {
                    o.collide = false;
                }
                let g = if hidden {
                    cx.report.hidden += 1;
                    Group::Hidden
                } else {
                    cx.report.brushes -= 1;
                    made = true;
                    if sky {
                        o.hidden = true;
                        cx.report.skybox += 1;
                        Group::Skybox
                    } else {
                        Group::BrushEntities
                    }
                };
                objects.push((o, g));
            }
            if brushes.is_empty() {
                // no brushes kept for it: use its drawn faces
                let m = bsp.models[mi];
                let polys: Vec<(Vec<DVec3>, i16)> = (m.first_face.max(0) as usize..(m.first_face.max(0) + m.num_faces.clamp(0, 65536)) as usize)
                    .filter_map(|fi| bsp.faces.get(fi))
                    .filter(|f| bsp.visible(f.texinfo) && f.dispinfo < 0)
                    .map(|f| (bsp.face_poly(f), f.texinfo))
                    .filter(|(p, _)| p.len() >= 3)
                    .collect();
                if let Some(mut o) = cx.mesh_object(&polys, offset) {
                    o.name = label.clone();
                    if class == "func_illusionary" {
                        o.collide = false;
                    }
                    made = true;
                    objects.push((o, Group::BrushEntities));
                }
            }
            if made {
                cx.report.brush_entities += 1;
            }
            continue;
        }
        match class {
            "light" | "light_spot" | "light_dynamic" => {
                let v: Vec<f64> = e.get("_light").map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
                let rgb = [v.first(), v.get(1), v.get(2)].map(|c| c.copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8);
                let b = v.get(3).copied().filter(|b| *b > 0.0).unwrap_or(200.0);
                let num = |key: &str| e.get(key).and_then(|s| s.trim().parse::<f64>().ok()).filter(|x| x.is_finite() && *x > 0.0);
                let reach = num("_zero_percent_distance").or_else(|| num("_fifty_percent_distance").map(|x| x * 2.0)).or_else(|| num("distance")).unwrap_or(b.sqrt() * 28.0);
                let mut o = Object::new(Kind::Light { range: (reach * k).clamp(1.0, 4000.0) as f32, brightness: (2.0 * (b / 200.0).sqrt()).clamp(0.3, 6.0) as f32 }, to_anvil(origin, k).as_vec3(), rgb, Material::Neon);
                o.collide = false;
                o.name = label;
                cx.report.lights += 1;
                if sky {
                    o.hidden = true;
                    objects.push((o, Group::Skybox));
                } else {
                    objects.push((o, Group::Lights));
                }
            }
            "info_player_start" | "info_player_deathmatch" | "info_player_combine" | "info_player_rebel" | "info_player_terrorist" | "info_player_counterterrorist" | "info_player_teamspawn" | "info_player_allies" | "info_player_axis" | "gmod_player_start" => {
                let mut pos = to_anvil(origin, k).as_vec3();
                pos.y += opts.spawn_size.y * 0.5;
                let mut o = Object::new(Kind::Spawn { size: opts.spawn_size }, pos, [90, 200, 110], Material::SmoothPlastic);
                o.rot.y = source_rotation(DVec3::new(0.0, angles.y, 0.0)).to_euler(glam::EulerRot::YXZ).0.to_degrees();
                o.name = format!("{label} {ei}");
                cx.report.spawns += 1;
                objects.push((o, Group::Spawns));
            }
            c if opts.props && c.starts_with("prop_") => {
                let Some(mdl) = e.get("model") else { continue };
                let scale = e.get("modelscale").and_then(|s| s.trim().parse::<f32>().ok()).filter(|s| s.is_finite() && *s > 0.01).unwrap_or(1.0).min(100.0);
                if let Some(mut o) = prop(&mut cx, mdl, origin, angles, scale, true, c) {
                    o.name = format!("{label} {}", o.name.split_once(' ').map(|x| x.1).unwrap_or(""));
                    if sky {
                        o.hidden = true;
                        objects.push((o, Group::Skybox));
                    } else {
                        objects.push((o, Group::Props));
                    }
                }
            }
            _ => {}
        }
    }
    cx.report.textures = cx.textures.len();
    cx.report.missing_materials.sort();
    cx.report.missing_models.sort();
    progress(format!("Done: {}", cx.report.summary()));
    Ok(Imported { name: name.to_string(), objects, textures: cx.textures, report: cx.report })
}

/// Adds an import to a map under one folder named after it. Returns the new
/// object ids.
pub fn apply(map: &mut Map, imp: Imported) -> Vec<u64> {
    let root = map.add_folder(&imp.name, None);
    let mut tex_ids = Vec::with_capacity(imp.textures.len());
    for t in imp.textures {
        let same = map.textures.iter().find(|x| x.image == t.image && x.scale == t.scale && x.alpha == t.alpha && x.normal_map == t.normal_map).map(|x| x.id);
        tex_ids.push(match same {
            Some(id) => id,
            None => map.add_texture(t),
        });
    }
    let order = [Group::World, Group::Terrain, Group::BrushEntities, Group::Props, Group::Lights, Group::Spawns, Group::Hidden, Group::Skybox];
    let mut folders: HashMap<Group, u64> = HashMap::new();
    for g in order {
        if imp.objects.iter().any(|(_, og)| *og == g) {
            folders.insert(g, map.add_folder(g.name(), Some(root)));
        }
    }
    let mut ids = Vec::with_capacity(imp.objects.len());
    for (mut o, g) in imp.objects {
        o.folder = folders.get(&g).copied();
        for ft in o.face_tex.iter_mut().flatten() {
            ft.texture = ft.texture.and_then(|i| tex_ids.get(i as usize).copied());
        }
        ids.push(map.add(o));
    }
    ids
}
