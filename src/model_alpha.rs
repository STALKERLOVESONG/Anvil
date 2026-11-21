//! Transparency of a whole OBJ model (all its textured materials): read and
//! set through its .mtl `map_d` / `d` lines. Used by the Inspector so props
//! imported without it (or before Anvil knew about it) can be fixed:
//! tree leaves and fences "Cut out", glass "See-through".

use std::path::{Path, PathBuf};

/// The .mtl an OBJ uses, if it sits next to the OBJ (only those are edited).
fn mtl_of(obj: &Path) -> Option<PathBuf> {
    if !obj.extension().is_some_and(|e| e.eq_ignore_ascii_case("obj")) {
        return None;
    }
    let text = std::fs::read_to_string(obj).ok()?;
    let name = text.lines().take(200).find_map(|l| l.trim().strip_prefix("mtllib ").map(|s| s.trim().to_string()))?;
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
        return None;
    }
    let p = obj.parent()?.join(name);
    p.is_file().then_some(p)
}

/// One material block of an .mtl: its lines, and whether it has a picture.
fn blocks(text: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![Vec::new()];
    for l in text.lines() {
        if l.trim_start().starts_with("newmtl ") {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(l.to_string());
    }
    out
}

fn key(l: &str) -> &str {
    l.split_whitespace().next().unwrap_or("")
}

fn textured(b: &[String]) -> Option<String> {
    b.iter().find(|l| key(l) == "map_Kd").map(|l| l.trim_start()["map_Kd".len()..].trim().to_string())
}

/// 0 solid, 1 cut out, 2 see-through: the strongest any textured material
/// of the model uses. None when it isn't an OBJ with textured materials.
pub fn model_alpha(obj: &str) -> Option<u8> {
    let mtl = mtl_of(Path::new(obj))?;
    let text = std::fs::read_to_string(mtl).ok()?;
    let mut any = false;
    let mut mode = 0;
    for b in blocks(&text) {
        if textured(&b).is_none() {
            continue;
        }
        any = true;
        let map_d = b.iter().any(|l| key(l) == "map_d");
        let see = b.iter().any(|l| key(l) == "d" && l.split_whitespace().nth(1).and_then(|v| v.parse::<f32>().ok()).is_some_and(|d| d < 0.9999));
        mode = mode.max(match (map_d, see) {
            (_, true) => 2,
            (true, false) => 1,
            _ => 0,
        });
    }
    any.then_some(mode)
}

/// Gives every textured material of the model this transparency, using the
/// picture's own alpha. Returns how many materials changed.
pub fn set_model_alpha(obj: &str, mode: u8) -> Result<usize, String> {
    let mtl = mtl_of(Path::new(obj)).ok_or("this model has no material file next to it to change")?;
    let text = std::fs::read_to_string(&mtl).map_err(|e| e.to_string())?;
    let mut out = String::with_capacity(text.len() + 256);
    let mut n = 0;
    for b in blocks(&text) {
        let Some(pic) = textured(&b) else {
            for l in &b {
                out.push_str(l);
                out.push('\n');
            }
            continue;
        };
        n += 1;
        for l in b.iter().filter(|l| !matches!(key(l), "map_d" | "d" | "Tr")) {
            out.push_str(l);
            out.push('\n');
            if key(l) == "map_Kd" {
                match mode {
                    1 => out.push_str(&format!("map_d {pic}\n")),
                    2 => out.push_str(&format!("map_d {pic}\nd 0.999\n")),
                    _ => {}
                }
            }
        }
    }
    std::fs::write(&mtl, out).map_err(|e| e.to_string())?;
    Ok(n)
}
