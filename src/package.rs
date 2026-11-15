//! Shareable map packages (.anvilpak): a zip holding the map plus every
//! texture, normal map and prop model it uses (with their .mtl files and
//! textures), so the map opens on any PC.

use crate::map::{Kind, Map};
use crate::settings::{ModelEntry, Settings};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or("file").to_string()
}

/// Packages open into `packages/<name>/` next to anvil.exe.
pub fn packages_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("packages")
}

/// Path value of an .mtl / .obj line like `map_Kd -s 1 1 1 tex/a.png` (last token).
fn line_path(rest: &str) -> String {
    rest.split_whitespace().last().unwrap_or("").to_string()
}

/// A model with the files it needs, flattened into one folder: the model
/// itself (an OBJ is rewritten to point at the flat names), its .mtl files
/// and their textures. `allow` decides which referenced files may be read.
pub struct ModelBundle {
    pub model: Vec<u8>,
    /// (flat file name, contents)
    pub files: Vec<(String, Vec<u8>)>,
    pub missing: Vec<String>,
    /// File extension to save `model` with (a .glb comes out as a .gltf).
    pub ext: String,
}

/// `model`'s file name with the extension a bundle saves it under.
pub fn bundled_name(model: &str, ext: &str) -> String {
    let name = file_name(model);
    let stem = name.rsplit_once('.').map(|x| x.0).unwrap_or(&name);
    format!("{stem}.{ext}")
}

/// Flat, safe file name for a referenced file (hash of the full path + its name).
pub fn flat_name(full: &Path) -> String {
    let name = file_name(&full.to_string_lossy());
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{:08x}_{}", fnv(&full.to_string_lossy()) as u32, clean.trim_start_matches('.'))
}

pub fn model_bundle(model: &str, bytes: &[u8], allow: &dyn Fn(&Path) -> bool) -> ModelBundle {
    let lower = model.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|x| x.1.to_string()).unwrap_or_default();
    let mut out = ModelBundle { model: bytes.to_vec(), files: Vec::new(), missing: Vec::new(), ext };
    if lower.ends_with(".glb") || lower.ends_with(".gltf") {
        // a .gltf's .bin and textures (and a .glb's embedded ones) as flat files;
        // if it can't be read, the file goes as it is
        if let Ok((json, files)) = crate::gltf::bundle(model, allow, None) {
            out.model = json;
            out.files = files;
            out.ext = "gltf".into();
        } else if lower.ends_with(".gltf") {
            out.missing.push(format!("{model} (its .bin or textures)"));
        }
        return out;
    }
    if !lower.ends_with(".obj") {
        return out;
    }
    let src_dir = Path::new(model).parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let text = String::from_utf8_lossy(bytes);
    let mut new_obj = String::with_capacity(text.len());
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("mtllib ") else {
            new_obj.push_str(line);
            new_obj.push('\n');
            continue;
        };
        let mut names = Vec::new();
        for mtl in rest.split_whitespace() {
            let mtl_src = src_dir.join(mtl);
            let text = (allow(&mtl_src) && mtl.to_ascii_lowercase().ends_with(".mtl")).then(|| std::fs::read_to_string(&mtl_src).ok()).flatten();
            let Some(mtl_text) = text else {
                out.missing.push(mtl_src.to_string_lossy().to_string());
                continue;
            };
            let mtl_dir = mtl_src.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            let mut new_mtl = String::new();
            for l in mtl_text.lines() {
                let t = l.trim_start();
                let key = t.split_whitespace().next().unwrap_or("");
                if key.starts_with("map_") || key == "bump" || key == "norm" || key == "disp" || key == "decal" {
                    let p = line_path(&t[key.len()..]);
                    let full = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { mtl_dir.join(&p) };
                    let read = allow(&full).then(|| std::fs::read(&full).ok()).flatten();
                    match read {
                        Some(b) => {
                            let fname = flat_name(&full);
                            if !out.files.iter().any(|f| f.0 == fname) {
                                out.files.push((fname.clone(), b));
                            }
                            new_mtl.push_str(&format!("{key} {fname}\n"));
                        }
                        None => out.missing.push(full.to_string_lossy().to_string()),
                    }
                } else {
                    new_mtl.push_str(l);
                    new_mtl.push('\n');
                }
            }
            let mname = flat_name(&mtl_src);
            out.files.push((mname.clone(), new_mtl.into_bytes()));
            names.push(mname);
        }
        new_obj.push_str(&format!("mtllib {}\n", names.join(" ")));
    }
    out.model = new_obj.into_bytes();
    out
}

pub fn save_package(map: &Map, settings: &Settings, path: &Path) -> Result<String, String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut done: HashMap<String, String> = HashMap::new();
    let mut missing = Vec::new();
    let mut out_map = map.clone();

    // textures and normal maps
    let mut add_texture = |src: &str, files: &mut Vec<(String, Vec<u8>)>, missing: &mut Vec<String>| -> Option<String> {
        if src.trim().is_empty() {
            return None;
        }
        if let Some(d) = done.get(src) {
            return Some(d.clone());
        }
        match std::fs::read(src) {
            Ok(b) => {
                let dst = format!("assets/textures/{:08x}_{}", fnv(src) as u32, file_name(src));
                files.push((dst.clone(), b));
                done.insert(src.to_string(), dst.clone());
                Some(dst)
            }
            Err(_) => {
                missing.push(src.to_string());
                None
            }
        }
    };
    for t in &mut out_map.textures {
        if let Some(d) = add_texture(&t.image, &mut files, &mut missing) {
            t.image = d;
        }
        if let Some(d) = add_texture(&t.normal_map, &mut files, &mut missing) {
            t.normal_map = d;
        }
        if let Some(d) = add_texture(&t.roughness_map, &mut files, &mut missing) {
            t.roughness_map = d;
        }
        if let Some(d) = add_texture(&t.metalness_map, &mut files, &mut missing) {
            t.metalness_map = d;
        }
    }

    // prop models (+ .mtl + their textures)
    let mut model_map: HashMap<String, String> = HashMap::new();
    for o in &map.objects {
        let Kind::Prop { model, .. } = &o.kind else { continue };
        if model_map.contains_key(model) {
            continue;
        }
        let Ok(bytes) = std::fs::read(model) else {
            missing.push(model.clone());
            continue;
        };
        let folder = format!("assets/models/{:08x}", fnv(model) as u32);
        let b = model_bundle(model, &bytes, &|_| true);
        missing.extend(b.missing);
        for (name, data) in b.files {
            files.push((format!("{folder}/{name}"), data));
        }
        let name = bundled_name(model, &b.ext);
        files.push((format!("{folder}/{name}"), b.model));
        model_map.insert(model.clone(), format!("{folder}/{name}"));
    }
    for o in &mut out_map.objects {
        if let Kind::Prop { model, .. } = &mut o.kind {
            if let Some(d) = model_map.get(model.as_str()) {
                *model = d.clone();
            }
        }
    }
    let library: Vec<ModelEntry> = settings
        .model_library
        .iter()
        .filter_map(|e| model_map.get(&e.path).map(|d| ModelEntry { path: d.clone(), ..e.clone() }))
        .collect();

    let file = std::fs::File::create(path).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(bytes).map_err(|e| e.to_string())
    };
    put("map.anvil", serde_json::to_string_pretty(&out_map).map_err(|e| e.to_string())?.as_bytes())?;
    put("library.json", serde_json::to_string_pretty(&library).map_err(|e| e.to_string())?.as_bytes())?;
    let mut seen = std::collections::HashSet::new();
    for (name, bytes) in &files {
        if seen.insert(name.clone()) {
            put(name, bytes)?;
        }
    }
    zw.finish().map_err(|e| e.to_string())?;
    let mut msg = format!("Packaged {} with {} file(s) into {}", map.name, seen.len(), path.display());
    if !missing.is_empty() {
        msg.push_str(&format!(" ({} missing file(s) skipped, e.g. {})", missing.len(), missing[0]));
    }
    Ok(msg)
}

pub struct Opened {
    pub map: Map,
    pub library: Vec<ModelEntry>,
    pub dir: PathBuf,
}

/// Limits for opening packages (against "zip bombs").
pub const MAX_FILES: usize = 20_000;
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 3 * 1024 * 1024 * 1024;

pub fn open_package(path: &Path) -> Result<Opened, String> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("package").to_string();
    let safe: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let dir = packages_dir().join(safe);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Could not clear {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| format!("Not a valid .anvilpak: {e}"))?;
    if za.len() > MAX_FILES {
        return Err(format!("This package has {} files; packages are limited to {MAX_FILES}", za.len()));
    }
    let mut total: u64 = 0;
    for i in 0..za.len() {
        let mut f = za.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue }; // refuses ../ paths
        // never trust the sizes a zip claims: count while reading
        if f.size() > MAX_FILE_BYTES {
            return Err(format!("{} is too big ({} MB) for a package", rel.display(), f.size() / 1_000_000));
        }
        let out = dir.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        (&mut f).take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
        if buf.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("{} unpacks to more than {} MB", rel.display(), MAX_FILE_BYTES / 1_000_000));
        }
        total += buf.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(format!("This package unpacks to more than {} GB, which looks like a zip bomb", MAX_TOTAL_BYTES / 1_000_000_000));
        }
        std::fs::write(&out, buf).map_err(|e| e.to_string())?;
    }
    let mut map = Map::load(&dir.join("map.anvil"))?;
    let abs = |p: &str| -> String {
        if p.starts_with("assets/") {
            dir.join(p).to_string_lossy().to_string()
        } else {
            p.to_string()
        }
    };
    for t in &mut map.textures {
        t.image = abs(&t.image);
        t.normal_map = abs(&t.normal_map);
        t.roughness_map = abs(&t.roughness_map);
        t.metalness_map = abs(&t.metalness_map);
    }
    for o in &mut map.objects {
        if let Kind::Prop { model, .. } = &mut o.kind {
            *model = abs(model);
        }
    }
    let library: Vec<ModelEntry> = std::fs::read_to_string(dir.join("library.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<ModelEntry>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|mut e| {
            e.path = abs(&e.path);
            e
        })
        .collect();
    Ok(Opened { map, library, dir })
}
