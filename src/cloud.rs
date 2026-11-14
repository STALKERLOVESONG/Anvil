//! Roblox Open Cloud asset uploads (https://apis.roblox.com/assets/v1).
//!
//! Needs an API key with the **assets** API system and Read + Write
//! permissions. Uploads run on a background thread and report back over a
//! channel so the editor never freezes.

use crate::export_roblox;
use crate::glb;
use crate::map::Map;
use crate::mesh::ModelCache;
use crate::settings::Settings;
use crate::texture::content_type;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

const ASSETS_URL: &str = "https://apis.roblox.com/assets/v1/assets";
const OPERATIONS_URL: &str = "https://apis.roblox.com/assets/v1/operations";

#[derive(Clone, Debug)]
pub struct Creator {
    pub group: bool,
    pub id: String,
}

pub enum CloudMsg {
    Log(String),
    /// slot: 0 color, 1 normal, 2 roughness, 3 metalness
    TextureId { tex: u64, slot: u8, id: String },
    ModelId { path: String, id: String },
    MapId(String),
    Finished,
}

pub struct Job {
    pub map: Map,
    pub settings: Settings,
    pub models: ModelCache,
    pub key: String,
    pub creator: Creator,
    pub textures: bool,
    pub props: bool,
    pub upload_map: bool,
    /// Re-upload even when an id is already stored.
    pub force: bool,
}

// ----- API key storage (kept out of maps and settings) ----------------------

fn secrets_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("anvil_secrets.json")
}

/// The saved Open Cloud key. It's stored encrypted with Windows DPAPI (only
/// your Windows account can read it); an old plain-text file is upgraded.
pub fn load_key() -> String {
    if let Ok(k) = std::env::var("ANVIL_ROBLOX_API_KEY") {
        return k;
    }
    let Some(v) = std::fs::read_to_string(secrets_path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return String::new();
    };
    if let Some(enc) = v.get("roblox_api_key_dpapi").and_then(|k| k.as_str()) {
        return crate::secure::unprotect(enc).unwrap_or_default();
    }
    let plain = v.get("roblox_api_key").and_then(|k| k.as_str()).unwrap_or("").to_string();
    if !plain.is_empty() {
        let _ = save_key(&plain); // re-save encrypted, dropping the plain copy
    }
    plain
}

pub fn save_key(key: &str) -> Result<(), String> {
    let v = match crate::secure::protect(key) {
        Some(enc) => serde_json::json!({ "roblox_api_key_dpapi": enc }),
        None => serde_json::json!({ "roblox_api_key": key }),
    };
    std::fs::write(secrets_path(), v.to_string()).map_err(|e| e.to_string())
}

pub fn forget_key() {
    let _ = std::fs::remove_file(secrets_path());
}

// ----- HTTP -----------------------------------------------------------------

fn trim_to(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Builds the multipart body for POST /assets/v1/assets.
pub fn multipart_body(
    boundary: &str,
    asset_type: &str,
    display_name: &str,
    description: &str,
    creator: &Creator,
    file_name: &str,
    file_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let creator_json = if creator.group {
        serde_json::json!({ "groupId": creator.id.trim() })
    } else {
        serde_json::json!({ "userId": creator.id.trim() })
    };
    let request = serde_json::json!({
        "assetType": asset_type,
        "displayName": trim_to(display_name, 50),
        "description": trim_to(description, 1000),
        "creationContext": { "creator": creator_json },
    })
    .to_string();
    let mut body = Vec::with_capacity(bytes.len() + 1024);
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n{request}\r\n").as_bytes());
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"fileContent\"; filename=\"{}\"\r\nContent-Type: {file_type}\r\n\r\n",
            file_name.replace('"', "")
        )
        .as_bytes(),
    );
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn read_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            format!("HTTP {code}: {}", trim_to(&text, 400))
        }
        other => other.to_string(),
    }
}

fn asset_id_of(v: &serde_json::Value) -> Option<String> {
    let id = v.get("response")?.get("assetId")?;
    match id {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Uploads one asset and waits for the operation to finish. Returns the asset id.
#[allow(clippy::too_many_arguments)]
pub fn upload_asset(
    key: &str,
    creator: &Creator,
    asset_type: &str,
    display_name: &str,
    description: &str,
    file_name: &str,
    file_type: &str,
    bytes: &[u8],
    log: &dyn Fn(String),
) -> Result<String, String> {
    if key.trim().is_empty() {
        return Err("No API key set".into());
    }
    if creator.id.trim().is_empty() {
        return Err("No creator (user or group) id set".into());
    }
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(format!("{file_name} is over the 20 MB upload limit"));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let boundary = format!("----AnvilBoundary{nanos:x}");
    let body = multipart_body(&boundary, asset_type, display_name, description, creator, file_name, file_type, bytes);
    let resp = ureq::post(ASSETS_URL)
        .set("x-api-key", key.trim())
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .timeout(Duration::from_secs(120))
        .send_bytes(&body)
        .map_err(read_error)?;
    let text = resp.into_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| format!("Unexpected reply: {}", trim_to(&text, 300)))?;
    if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
        if let Some(id) = asset_id_of(&v) {
            return Ok(id);
        }
    }
    let op = v
        .get("operationId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .or_else(|| v.get("path").and_then(|x| x.as_str()).map(|p| p.trim_start_matches("operations/").to_string()))
        .ok_or_else(|| format!("No operation id in reply: {}", trim_to(&text, 300)))?;
    log(format!("  {display_name}: processing (operation {op})"));
    let mut wait = 1.0f32;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_secs_f32(wait));
        wait = (wait * 1.5).min(8.0);
        let r = ureq::get(&format!("{OPERATIONS_URL}/{op}"))
            .set("x-api-key", key.trim())
            .timeout(Duration::from_secs(60))
            .call()
            .map_err(read_error)?;
        let t = r.into_string().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&t).map_err(|_| format!("Unexpected reply: {}", trim_to(&t, 300)))?;
        if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
            if let Some(id) = asset_id_of(&v) {
                return Ok(id);
            }
            let msg = v
                .get("error")
                .or_else(|| v.get("status"))
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("upload failed");
            return Err(msg.to_string());
        }
    }
    Err(format!("Timed out waiting for operation {op}"))
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or("file").to_string()
}

/// Runs an upload job on a background thread.
pub fn spawn(job: Job) -> Receiver<CloudMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || run(job, tx));
    rx
}

/// The Roblox asset id field for a texture map slot.
pub fn texture_slot_id(t: &mut crate::map::TextureDef, slot: u8) -> &mut String {
    match slot {
        0 => &mut t.roblox_image_id,
        1 => &mut t.roblox_normal_id,
        2 => &mut t.roblox_roughness_id,
        _ => &mut t.roblox_metalness_id,
    }
}

fn run(mut job: Job, tx: Sender<CloudMsg>) {
    let log = |s: String| {
        let _ = tx.send(CloudMsg::Log(s));
    };
    let mut ok = 0;
    let mut failed = 0;

    if job.textures {
        log("Uploading textures as Decal assets…".into());
        let defs = job.map.textures.clone();
        for t in defs {
            for slot in 0u8..4 {
                let (path, existing, label) = match slot {
                    0 => (&t.image, &t.roblox_image_id, ""),
                    1 => (&t.normal_map, &t.roblox_normal_id, " normal"),
                    2 => (&t.roughness_map, &t.roblox_roughness_id, " roughness"),
                    _ => (&t.metalness_map, &t.roblox_metalness_id, " metalness"),
                };
                if path.trim().is_empty() || (!existing.trim().is_empty() && !job.force) {
                    continue;
                }
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        log(format!("  {}: could not read {path}: {e}", t.name));
                        failed += 1;
                        continue;
                    }
                };
                let name = format!("{}{label}", t.name);
                match upload_asset(&job.key, &job.creator, "Decal", &name, "Uploaded by Anvil", &file_name(path), content_type(path), &bytes, &log) {
                    Ok(id) => {
                        log(format!("  {name}: rbxassetid://{id}"));
                        if let Some(tt) = job.map.textures.iter_mut().find(|x| x.id == t.id) {
                            *texture_slot_id(tt, slot) = id.clone();
                        }
                        let _ = tx.send(CloudMsg::TextureId { tex: t.id, slot, id });
                        ok += 1;
                    }
                    Err(e) => {
                        log(format!("  {name}: FAILED: {e}"));
                        failed += 1;
                    }
                }
            }
        }
    }

    if job.props {
        log("Uploading prop models as Model assets (.glb)…".into());
        let lib = job.settings.model_library.clone();
        for m in lib {
            if !m.roblox_model_id.trim().is_empty() && !job.force {
                continue;
            }
            if !job.map.objects.iter().any(|o| matches!(&o.kind, crate::map::Kind::Prop { model, .. } if *model == m.path)) {
                continue; // only upload models this map uses
            }
            let mesh = match job.models.get(&m.path) {
                Ok(mesh) => mesh,
                Err(e) => {
                    log(format!("  {}: {e}", file_name(&m.path)));
                    failed += 1;
                    continue;
                }
            };
            let stem = Path::new(&m.path).file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
            let bytes = glb::mesh_to_glb(&mesh, job.settings.roblox_scale, &stem);
            match upload_asset(&job.key, &job.creator, "Model", &stem, "Uploaded by Anvil", &format!("{stem}.glb"), "model/gltf-binary", &bytes, &log) {
                Ok(id) => {
                    log(format!("  {stem}: model asset {id}"));
                    if let Some(e) = job.settings.model_library.iter_mut().find(|x| x.path == m.path) {
                        e.roblox_model_id = id.clone();
                    }
                    let _ = tx.send(CloudMsg::ModelId { path: m.path.clone(), id });
                    ok += 1;
                }
                Err(e) => {
                    log(format!("  {stem}: FAILED: {e}"));
                    failed += 1;
                }
            }
        }
    }

    if job.upload_map {
        log("Uploading the map as a Model asset (.rbxmx)…".into());
        let nodes = export_roblox::build_nodes(&job.map, &mut job.models, &job.settings);
        let xml = export_roblox::to_rbxmx(&nodes, &job.map.name);
        let name = job.map.name.clone();
        match upload_asset(&job.key, &job.creator, "Model", &name, "Map built with Anvil", &format!("{name}.rbxmx"), "model/x-rbxm", xml.as_bytes(), &log) {
            Ok(id) => {
                log(format!("  Map uploaded: asset {id}. Insert it from Toolbox > My Models, or with the snippet below."));
                let _ = tx.send(CloudMsg::MapId(id));
                ok += 1;
            }
            Err(e) => {
                log(format!("  Map: FAILED: {e}"));
                failed += 1;
            }
        }
    }

    log(format!("Done: {ok} uploaded, {failed} failed."));
    let _ = tx.send(CloudMsg::Finished);
}
