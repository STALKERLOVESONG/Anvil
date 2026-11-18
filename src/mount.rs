//! Mounted game content (Half-Life 2, Garry's Mod and other Source games):
//! loose folders, VPK archives and GMA addons, plus importing models and
//! materials into ordinary OBJ / PNG files that the rest of Anvil uses.

use crate::source::{self, decode_vtf, load_mdl, norm, parse_vmt, Gma, Vpk};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

enum Archive {
    Vpk(Vpk),
    Gma(Gma),
}

/// Everything mounted, searched in order.
pub struct Mounts {
    dirs: Vec<PathBuf>,
    archives: Vec<Archive>,
    pub summary: Vec<String>,
    pub models: Vec<String>,
    pub materials: Vec<String>,
}

const SKIP_DIRS: [&str; 18] = [
    "models", "materials", "sound", "maps", "bin", "platform", "cfg", "data", "lua", "gamemodes", "resource", "screenshots", "saves", "demos",
    "dupes", "media", "particles", "scripts",
];

fn is_content_root(d: &Path) -> bool {
    d.join("models").is_dir() || d.join("materials").is_dir()
}

fn name_of(d: &Path) -> String {
    d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase()
}

impl Mounts {
    /// Mounts every path: a game/addon folder, a `*_dir.vpk`, a `.gma`, or a
    /// folder of those (searched a few levels deep, e.g. a whole Steam game).
    pub fn build(paths: &[String]) -> Mounts {
        let mut m = Mounts { dirs: Vec::new(), archives: Vec::new(), summary: Vec::new(), models: Vec::new(), materials: Vec::new() };
        for p in paths {
            let p = PathBuf::from(p);
            m.expand(&p, 0);
        }
        m.index();
        m
    }

    fn add_file(&mut self, f: &Path) {
        let n = name_of(f);
        if n.ends_with("_dir.vpk") {
            match Vpk::open(f) {
                Ok(v) => {
                    self.summary.push(format!("VPK  {}", f.display()));
                    self.archives.push(Archive::Vpk(v));
                }
                Err(e) => self.summary.push(format!("(!) {}: {e}", f.display())),
            }
        } else if n.ends_with(".gma") {
            match Gma::open(f) {
                Ok(g) => {
                    self.summary.push(format!("GMA  {}  ({})", g.title, f.display()));
                    self.archives.push(Archive::Gma(g));
                }
                Err(e) => self.summary.push(format!("(!) {}: {e}", f.display())),
            }
        }
    }

    fn expand(&mut self, p: &Path, depth: usize) {
        if p.is_file() {
            self.add_file(p);
            return;
        }
        if !p.is_dir() || depth > 5 {
            return;
        }
        let n = name_of(p);
        if depth == 0 && (n == "models" || n == "materials") {
            self.summary.push(format!("DIR  {}", p.display()));
            self.dirs.push(p.to_path_buf());
            return;
        }
        if is_content_root(p) || (depth > 0 && (n == "models" || n == "materials")) {
            if !self.dirs.contains(&p.to_path_buf()) {
                self.summary.push(format!("DIR  {}", p.display()));
                self.dirs.push(p.to_path_buf());
            }
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for e in entries {
            if e.is_file() {
                self.add_file(&e);
            } else if e.is_dir() {
                let en = name_of(&e);
                if SKIP_DIRS.contains(&en.as_str()) {
                    // an addon folder literally named "models"/"materials" (loose content dumps)
                    if (en == "models" || en == "materials") && !is_content_root(p) {
                        self.expand(&e, depth + 1);
                    }
                    continue;
                }
                self.expand(&e, depth + 1);
            }
        }
    }

    /// Candidate filesystem paths for a game-relative path in one folder mount.
    fn dir_candidates(root: &Path, rel: &str) -> Vec<PathBuf> {
        let mut v = vec![root.join(rel)];
        let n = name_of(root);
        if let Some((first, rest)) = rel.split_once('/') {
            if first == n {
                v.push(root.join(rest));
            }
        }
        v
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let rel = norm(rel);
        for d in &self.dirs {
            for c in Self::dir_candidates(d, &rel) {
                if c.is_file() {
                    if let Ok(b) = std::fs::read(&c) {
                        return Some(b);
                    }
                }
            }
        }
        for a in &self.archives {
            let r = match a {
                Archive::Vpk(v) => v.read(&rel),
                Archive::Gma(g) => g.read(&rel),
            };
            if r.is_some() {
                return r;
            }
        }
        None
    }

    fn index(&mut self) {
        let mut models = BTreeSet::new();
        let mut mats = BTreeSet::new();
        let mut add = |rel: &str| {
            if rel.starts_with("models/") && rel.ends_with(".mdl") {
                models.insert(rel.to_string());
            } else if rel.starts_with("materials/") && rel.ends_with(".vmt") {
                mats.insert(rel.trim_start_matches("materials/").trim_end_matches(".vmt").to_string());
            }
        };
        for a in &self.archives {
            match a {
                Archive::Vpk(v) => v.names().for_each(|n| add(n)),
                Archive::Gma(g) => g.names().for_each(|n| add(n)),
            }
        }
        for d in &self.dirs {
            let n = name_of(d);
            let tops: Vec<(PathBuf, &str)> = if n == "models" || n == "materials" {
                vec![(d.clone(), if n == "models" { "models" } else { "materials" })]
            } else {
                vec![(d.join("models"), "models"), (d.join("materials"), "materials")]
            };
            for (top, prefix) in tops {
                let mut stack = vec![top.clone()];
                while let Some(dir) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else if let Ok(r) = p.strip_prefix(&top) {
                            add(&norm(&format!("{prefix}/{}", r.to_string_lossy())));
                        }
                    }
                }
            }
        }
        self.models = models.into_iter().collect();
        self.materials = mats.into_iter().collect();
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.archives.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Steam auto-detection
// ---------------------------------------------------------------------------

/// Source games whose content is worth mounting, by Steam folder name.
const SOURCE_GAMES: [&str; 12] = [
    "GarrysMod",
    "Half-Life 2",
    "Half-Life 2 Deathmatch",
    "Half-Life 2 Lost Coast",
    "Counter-Strike Source",
    "Day of Defeat Source",
    "Portal",
    "Portal 2",
    "Team Fortress 2",
    "Left 4 Dead 2",
    "Black Mesa",
    "Source SDK Base 2013 Singleplayer",
];

pub(crate) fn steam_libraries() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(out) = std::process::Command::new("reg").args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().find(|l| l.contains("SteamPath")) {
            if let Some(p) = line.split("REG_SZ").nth(1) {
                roots.push(PathBuf::from(p.trim()));
            }
        }
    }
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    let mut libs = Vec::new();
    for r in roots {
        let vdf = r.join("steamapps").join("libraryfolders.vdf");
        if let Ok(t) = std::fs::read_to_string(&vdf) {
            for line in t.lines() {
                let l = line.trim();
                if l.starts_with("\"path\"") {
                    if let Some(v) = l.split('"').nth(3) {
                        libs.push(PathBuf::from(v.replace("\\\\", "\\")));
                    }
                }
            }
        }
        libs.push(r);
    }
    let mut seen = BTreeSet::new();
    libs.retain(|p| seen.insert(p.to_string_lossy().to_ascii_lowercase()));
    libs
}

/// Installed Source games (folders to mount).
pub fn detect_steam() -> Vec<String> {
    let mut out = Vec::new();
    for lib in steam_libraries() {
        let common = lib.join("steamapps").join("common");
        for g in SOURCE_GAMES {
            let d = common.join(g);
            if d.is_dir() {
                out.push(d.to_string_lossy().to_string());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// importing into plain files
// ---------------------------------------------------------------------------

/// Where imported content is written: `content/` next to anvil.exe.
pub fn content_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("content")
}

pub struct MaterialFiles {
    pub name: String,
    pub image: String,
    pub normal: String,
    pub translucent: bool,
    /// 0 solid, 1 cut-out ($alphatest), 2 see-through ($translucent).
    pub alpha: u8,
}

/// Follows a material (path under `materials/`, no extension) to its VTFs.
fn resolve_material(m: &Mounts, mat: &str) -> Option<(String, Option<String>, u8)> {
    let mat = norm(mat);
    let mut params = std::collections::HashMap::new();
    let mut shader = String::new();
    if let Some(bytes) = m.read(&format!("materials/{mat}.vmt")) {
        let v = parse_vmt(&String::from_utf8_lossy(&bytes));
        shader = v.shader.clone();
        params = v.params;
        if shader == "patch" {
            if let Some(inc) = params.get("include").cloned() {
                if let Some(b2) = m.read(&inc) {
                    let base = parse_vmt(&String::from_utf8_lossy(&b2));
                    let mut merged = base.params;
                    for (k, v) in params.iter().filter(|(k, _)| k.starts_with('$')) {
                        merged.insert(k.clone(), v.clone());
                    }
                    params = merged;
                    shader = base.shader;
                }
            }
        }
    }
    let base = params.get("$basetexture").cloned().unwrap_or_else(|| mat.clone());
    let base_vtf = format!("materials/{}.vtf", norm(&base));
    m.read(&base_vtf)?;
    let bump = params.get("$bumpmap").or_else(|| params.get("$normalmap")).map(|b| format!("materials/{}.vtf", norm(b)));
    let on = |k: &str| params.get(k).is_some_and(|v| v.trim() != "0");
    let alpha = if on("$alphatest") {
        1
    } else if on("$translucent") || shader.contains("decal") {
        2
    } else {
        0
    };
    Some((base_vtf, bump, alpha))
}

fn save_vtf_png(m: &Mounts, vtf_rel: &str, out: &Path) -> Result<(), String> {
    if out.is_file() {
        return Ok(()); // already imported
    }
    let bytes = m.read(vtf_rel).ok_or_else(|| format!("{vtf_rel} not found"))?;
    let img = decode_vtf(&bytes, 1024)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    image::RgbaImage::from_raw(img.w, img.h, img.data)
        .ok_or("bad image size")?
        .save(out)
        .map_err(|e| e.to_string())
}

/// Decodes a material's base texture for previews.
pub fn preview_material(m: &Mounts, mat: &str, max_dim: u32) -> Result<source::Rgba, String> {
    let (vtf, _, _) = resolve_material(m, mat).ok_or_else(|| format!("{mat}: no base texture found"))?;
    decode_vtf(&m.read(&vtf).ok_or("texture missing")?, max_dim)
}

/// Converts a material into PNG files under `content/materials/`.
pub fn import_material(m: &Mounts, mat: &str) -> Result<MaterialFiles, String> {
    let mat = norm(mat);
    let (base, bump, alpha) = resolve_material(m, &mat).ok_or_else(|| format!("{mat}: no base texture found"))?;
    let translucent = alpha > 0;
    let dir = content_dir();
    let image = dir.join(format!("{}.png", base.trim_end_matches(".vtf")));
    save_vtf_png(m, &base, &image)?;
    let mut normal = String::new();
    if let Some(b) = bump {
        let p = dir.join(format!("{}.png", b.trim_end_matches(".vtf")));
        if save_vtf_png(m, &b, &p).is_ok() {
            normal = p.to_string_lossy().to_string();
        }
    }
    let name = mat.rsplit('/').next().unwrap_or(&mat).to_string();
    Ok(MaterialFiles { name, image: image.to_string_lossy().to_string(), normal, translucent, alpha })
}

/// Converts a model (and its textures) into an OBJ + MTL + PNGs under
/// `content/models/`. `scale` converts Source units to editor units.
pub fn import_model(m: &Mounts, mdl: &str, scale: f32) -> Result<PathBuf, String> {
    let rel = norm(mdl);
    let model = load_mdl(&|r| m.read(r), &rel)?;
    let dir = content_dir();
    let obj_path = dir.join(rel.trim_end_matches(".mdl").to_string() + ".obj");
    let obj_dir = obj_path.parent().ok_or("bad path")?.to_path_buf();
    std::fs::create_dir_all(&obj_dir).map_err(|e| e.to_string())?;
    let stem = obj_path.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();

    let mut mtl = String::from("# Imported by Anvil\n");
    let mut mat_names = Vec::new();
    for (i, mat) in model.materials.iter().enumerate() {
        let mname = format!("m{i}_{}", mat.rsplit('/').next().unwrap_or("mat").replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_"));
        let _ = writeln!(mtl, "\nnewmtl {mname}\nKd 1 1 1");
        if let Ok(files) = import_material(m, mat) {
            if let Some(rel) = pathdiff(Path::new(&files.image), &obj_dir) {
                let _ = writeln!(mtl, "map_Kd {rel}");
                // the picture's alpha: holes (cut-out) or see-through
                match files.alpha {
                    1 => {
                        let _ = writeln!(mtl, "map_d {rel}");
                    }
                    2 => {
                        let _ = writeln!(mtl, "map_d {rel}\nd 0.999");
                    }
                    _ => {}
                }
            }
            if !files.normal.is_empty() {
                if let Some(rel) = pathdiff(Path::new(&files.normal), &obj_dir) {
                    let _ = writeln!(mtl, "map_Bump {rel}");
                }
            }
        }
        mat_names.push(mname);
    }
    let mut obj = format!("# Imported by Anvil from {rel}\nmtllib {stem}.mtl\no {stem}\n");
    for p in &model.pos {
        let q = *p * scale;
        let _ = writeln!(obj, "v {} {} {}", q.x, q.y, q.z);
    }
    for t in &model.uv {
        let _ = writeln!(obj, "vt {} {}", t.x, 1.0 - t.y);
    }
    for n in &model.normal {
        let _ = writeln!(obj, "vn {} {} {}", n.x, n.y, n.z);
    }
    let mut order: Vec<usize> = (0..model.tris.len()).collect();
    order.sort_by_key(|&i| model.tri_mat[i]);
    let mut cur = u16::MAX;
    for i in order {
        let mi = model.tri_mat[i];
        if mi != cur {
            cur = mi;
            let name = mat_names.get(mi as usize).cloned().unwrap_or_else(|| "default".into());
            let _ = writeln!(obj, "usemtl {name}");
        }
        let t = model.tris[i];
        let _ = writeln!(obj, "f {0}/{0}/{0} {1}/{1}/{1} {2}/{2}/{2}", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    std::fs::write(&obj_path, obj).map_err(|e| e.to_string())?;
    std::fs::write(obj_dir.join(format!("{stem}.mtl")), mtl).map_err(|e| e.to_string())?;
    Ok(obj_path)
}

/// `target` relative to `base` with forward slashes (both absolute).
pub fn pathdiff(target: &Path, base: &Path) -> Option<String> {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..b.len() {
        parts.push("..".into());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    Some(parts.join("/"))
}
