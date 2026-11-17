//! Copy and paste of objects (Ctrl+C / Ctrl+X / Ctrl+V). What's copied goes
//! on the system clipboard as text (so it pastes into another Anvil window
//! too), with the textures the objects use, so they keep their look.

use crate::map::{Map, Object, TextureDef};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Biggest clipboard text taken (a hostile paste can't eat all memory).
const MAX_TEXT: usize = 64 * 1024 * 1024;
const MAX_OBJECTS: usize = 50_000;

#[derive(Serialize, Deserialize)]
pub struct Clip {
    /// Marks Anvil's own clipboard text.
    pub anvil_clipboard: u32,
    pub objects: Vec<Object>,
    #[serde(default)]
    pub textures: Vec<TextureDef>,
}

/// The objects `ids` (and their textures) as clipboard text.
pub fn copy(map: &Map, ids: &[u64]) -> Option<String> {
    let objects: Vec<Object> = ids.iter().filter_map(|id| map.get(*id).cloned()).collect();
    if objects.is_empty() {
        return None;
    }
    let mut used: Vec<u64> = Vec::new();
    for o in &objects {
        used.extend(o.texture);
        used.extend(o.face_tex.iter().flatten().filter_map(|f| f.texture));
    }
    let textures = map.textures.iter().filter(|t| used.contains(&t.id)).cloned().collect();
    serde_json::to_string(&Clip { anvil_clipboard: 1, objects, textures }).ok()
}

/// Anvil objects from clipboard text (None if it's something else).
pub fn parse(text: &str) -> Option<Clip> {
    let t = text.trim_start();
    if t.len() > MAX_TEXT || !t.starts_with('{') || !t.contains("\"anvil_clipboard\"") {
        return None;
    }
    let clip: Clip = serde_json::from_str(t).ok()?;
    (clip.anvil_clipboard == 1 && !clip.objects.is_empty() && clip.objects.len() <= MAX_OBJECTS).then_some(clip)
}

/// Adds the clip's objects to `map`, moved by `delta`, as new objects with
/// new ids. Their textures are matched to the map's (same picture) or
/// added; a folder the map doesn't have is dropped. Returns the new ids.
pub fn paste(map: &mut Map, clip: &Clip, delta: Vec3) -> Vec<u64> {
    // the clip's texture ids -> this map's
    let mut tex_map: Vec<(u64, u64)> = Vec::new();
    for t in &clip.textures {
        let id = match map.textures.iter().find(|m| m.image == t.image && m.normal_map == t.normal_map) {
            Some(m) => m.id,
            None => map.add_texture(TextureDef { id: 0, ..t.clone() }),
        };
        tex_map.push((t.id, id));
    }
    let remap = |id: Option<u64>| id.and_then(|i| tex_map.iter().find(|(a, _)| *a == i).map(|(_, b)| *b));
    let mut ids = Vec::new();
    for o in &clip.objects {
        let mut o = o.clone();
        o.pos += delta;
        o.name = String::new();
        o.texture = remap(o.texture);
        for f in o.face_tex.iter_mut().flatten() {
            f.texture = remap(f.texture);
        }
        if o.folder.is_some_and(|f| map.folder(f).is_none()) {
            o.folder = None;
        }
        ids.push(map.add(o));
    }
    ids
}
