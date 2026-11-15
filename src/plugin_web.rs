//! What plugins may do beyond editing the map, all fenced in:
//! * web: HTTPS only, and only to the hosts the plugin lists in `info()`
//!   (which you see before allowing it);
//! * files: only inside the plugin's own folder, `content/plugins/<name>/`;
//! * secrets (API tokens): encrypted with Windows DPAPI.

use rhai::{Array, Dynamic, Map as RMap, INT};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

const MAX_TEXT: u64 = 8 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;

/// A plugin's private folder for downloads.
pub fn plugin_dir(plugin_file: &str) -> PathBuf {
    let stem = Path::new(plugin_file).file_stem().and_then(|s| s.to_str()).unwrap_or("plugin");
    let safe: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    crate::mount::content_dir().join("plugins").join(safe)
}

/// The host of an https:// URL (no user@, no other schemes).
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let auth = rest.split(['/', '?', '#']).next()?;
    if auth.contains('@') || auth.is_empty() {
        return None;
    }
    let host = auth.rsplit_once(':').map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { auth }).unwrap_or(auth);
    Some(host.to_ascii_lowercase())
}

pub fn url_allowed(url: &str, hosts: &[String]) -> bool {
    https_host(url).is_some_and(|h| hosts.iter().any(|a| a.eq_ignore_ascii_case(&h)))
}

/// Test builds only: canned responses per URL (plus the headers each URL got).
#[cfg(test)]
static MOCK: Mutex<Option<HashMap<String, Vec<(Option<String>, Vec<u8>)>>>> = Mutex::new(None);
#[cfg(test)]
static MOCK_SEEN: Mutex<Option<HashMap<String, Vec<(String, String)>>>> = Mutex::new(None);

#[cfg(test)]
pub fn mock(url: &str, body: &[u8]) {
    MOCK.lock().unwrap().get_or_insert_with(HashMap::new).entry(url.to_string()).or_default().push((None, body.to_vec()));
}

/// Answers only requests carrying this exact Authorization header.
#[cfg(test)]
pub fn mock_auth(url: &str, auth: &str, body: &[u8]) {
    MOCK.lock().unwrap().get_or_insert_with(HashMap::new).entry(url.to_string()).or_default().push((Some(auth.to_string()), body.to_vec()));
}

#[cfg(test)]
pub fn mock_headers(url: &str) -> Vec<(String, String)> {
    MOCK_SEEN.lock().unwrap().as_ref().and_then(|m| m.get(url).cloned()).unwrap_or_default()
}

/// None = not mocked; Some(Err) = mocked but refused (wrong Authorization).
#[cfg(test)]
fn mocked(url: &str, headers: &[(String, String)]) -> Option<Result<Vec<u8>, String>> {
    let entries = MOCK.lock().ok()?.as_ref()?.get(url).cloned()?;
    MOCK_SEEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(url.to_string(), headers.to_vec());
    let auth = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.clone());
    let hit = entries.iter().find(|(a, _)| a.is_none() || *a == auth).map(|(_, b)| b.clone());
    Some(hit.ok_or_else(|| "status code 401".to_string()))
}

/// `download`: a big file. Instead of 30 s for the whole request it may take
/// as long as it needs, as long as data keeps coming (gives up after a
/// minute with nothing).
fn request(url: &str, headers: &[(String, String)], hosts: &[String], download: bool) -> Result<ureq::Response, String> {
    if !url_allowed(url, hosts) {
        return Err(format!("Not allowed: {url} isn't https to one of this plugin's hosts"));
    }
    let s = std::time::Duration::from_secs;
    let agent = if download { ureq::AgentBuilder::new().timeout_connect(s(20)).timeout_read(s(60)).build() } else { ureq::AgentBuilder::new().timeout(s(30)).build() };
    let mut r = agent.get(url).set("User-Agent", "Anvil-plugin");
    for (k, v) in headers {
        r = r.set(k, v);
    }
    let resp = r.call().map_err(|e| e.to_string())?;
    if !url_allowed(resp.get_url(), hosts) {
        return Err("Redirected to a host this plugin didn't list".into());
    }
    Ok(resp)
}

pub fn http_get(url: &str, headers: &[(String, String)], hosts: &[String]) -> Result<String, String> {
    #[cfg(test)]
    if url_allowed(url, hosts) {
        if let Some(r) = mocked(url, headers) {
            return r.map(|b| String::from_utf8_lossy(&b).into_owned());
        }
    }
    let resp = request(url, headers, hosts, false)?;
    let mut s = String::new();
    resp.into_reader().take(MAX_TEXT).read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

#[derive(Clone, Default)]
struct Job {
    state: String,
    path: String,
    got: u64,
    total: u64,
}

static JOBS: Mutex<Option<HashMap<i64, Job>>> = Mutex::new(None);
static NEXT_JOB: AtomicI64 = AtomicI64::new(1);

fn set_job(id: i64, f: impl FnOnce(&mut Job)) {
    if let Ok(mut j) = JOBS.lock() {
        f(j.get_or_insert_with(HashMap::new).entry(id).or_default());
    }
}

/// A safe file name: plain characters only, no leading dot, the stem cut to
/// 60 characters but the extension always kept.
pub fn safe_name(name: &str) -> String {
    let clean = |s: &str, max: usize| -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(max).collect() };
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()) => (s, Some(e)),
        _ => (name, None),
    };
    let mut stem = clean(stem, 60);
    if stem.trim_matches('_').is_empty() {
        stem = "download".into();
    }
    match ext {
        Some(e) => format!("{stem}.{}", e.to_ascii_lowercase()),
        None => stem,
    }
}

/// Starts a background download into `dir`. Returns a job id.
pub fn start_download(url: String, name: &str, headers: Vec<(String, String)>, hosts: Vec<String>, dir: PathBuf) -> i64 {
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(safe_name(name));
    set_job(id, |j| {
        j.state = "running".into();
        j.path = path.to_string_lossy().to_string();
    });
    std::thread::spawn(move || {
        let r = (|| -> Result<(), String> {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            #[cfg(test)]
            if url_allowed(&url, &hosts) {
                if let Some(r) = mocked(&url, &headers) {
                    return std::fs::write(&path, r?).map_err(|e| e.to_string());
                }
            }
            let resp = request(&url, &headers, &hosts, true)?;
            let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            set_job(id, |j| j.total = total);
            // a compressed body's length isn't the length that comes out
            let plain = resp.header("Content-Encoding").is_none_or(|e| e.eq_ignore_ascii_case("identity"));
            save_body(resp.into_reader(), if plain { total } else { 0 }, &path, &|got| set_job(id, |j| j.got = got))
        })();
        set_job(id, |j| {
            j.state = match r {
                Ok(()) => "done".into(),
                Err(e) => format!("error: {e}"),
            }
        });
    });
    id
}

/// Writes a download to `path` through a `.part` file, so a download that
/// stops halfway (connection dropped, Anvil closed) never looks finished.
/// `total` (Content-Length, 0 = unknown) must match what arrived.
pub fn save_body(rd: impl Read, total: u64, path: &Path, progress: &dyn Fn(u64)) -> Result<(), String> {
    let part = path.with_extension(format!("{}.part", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let r = (|| -> Result<(), String> {
        let mut rd = rd.take(MAX_DOWNLOAD + 1);
        let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        loop {
            let n = match rd.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("download stopped: {e}")),
            };
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > MAX_DOWNLOAD {
                return Err("download is over 1 GB".into());
            }
            std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
            progress(got);
        }
        if total > 0 && got != total {
            return Err(format!("download was cut off ({} of {} MB), try again", got / 1_000_000, total.div_ceil(1_000_000)));
        }
        drop(f);
        std::fs::rename(&part, path).map_err(|e| e.to_string())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

/// (state, file path, progress 0..1)
pub fn job(id: i64) -> (String, String, f64) {
    let j = JOBS.lock().ok().and_then(|j| j.as_ref().and_then(|m| m.get(&id).cloned())).unwrap_or_default();
    let p = if j.total > 0 { j.got as f64 / j.total as f64 } else { 0.0 };
    (if j.state.is_empty() { "unknown".into() } else { j.state }, j.path, p.min(1.0))
}

/// Is `path` inside `root` (after resolving . / .. / links)?
pub fn inside(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => p.starts_with(r),
        _ => false,
    }
}

/// Unzips a file from the plugin's folder into a folder next to it (with the
/// same limits as .anvilpak packages).
pub fn unzip_in(zip: &Path, root: &Path) -> Result<String, String> {
    if !inside(zip, root) {
        return Err("unzip only works on files in the plugin's own folder".into());
    }
    let out = zip.with_extension("");
    let f = std::fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut za = zip::ZipArchive::new(f).map_err(|e| format!("not a zip: {e}"))?;
    if za.len() > crate::package::MAX_FILES {
        return Err("too many files in the zip".into());
    }
    let mut total = 0u64;
    for i in 0..za.len() {
        let mut e = za.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else { continue };
        let dest = out.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        (&mut e).take(crate::package::MAX_FILE_BYTES + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
        total += buf.len() as u64;
        if buf.len() as u64 > crate::package::MAX_FILE_BYTES || total > crate::package::MAX_TOTAL_BYTES {
            return Err("the zip unpacks to too much data".into());
        }
        std::fs::write(&dest, buf).map_err(|e| e.to_string())?;
    }
    Ok(out.to_string_lossy().to_string())
}

/// Files ending in `.ext` under `dir` (inside the plugin's folder).
pub fn find_files(dir: &Path, ext: &str, root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if !inside(dir, root) {
        return out;
    }
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 8 {
                stack.push((p, depth + 1));
            } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(&ext)) {
                out.push(p.to_string_lossy().to_string());
            }
            if out.len() > 2000 {
                return out;
            }
        }
    }
    out.sort();
    out
}

fn secrets_file() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from(".")).join("plugin_secrets.json")
}

pub fn secret_get(plugin: &str, name: &str) -> String {
    let Some(v) = std::fs::read_to_string(secrets_file()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return String::new() };
    v.get(format!("{plugin}:{name}")).and_then(|x| x.as_str()).and_then(crate::secure::unprotect).unwrap_or_default()
}

pub fn secret_set(plugin: &str, name: &str, value: &str) -> bool {
    let mut v = std::fs::read_to_string(secrets_file()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    let key = format!("{plugin}:{name}");
    if value.is_empty() {
        if let Some(m) = v.as_object_mut() {
            m.remove(&key);
        }
    } else {
        let Some(enc) = crate::secure::protect(value) else { return false };
        v[key] = serde_json::Value::String(enc);
    }
    std::fs::write(secrets_file(), v.to_string()).is_ok()
}

pub fn json_to_dynamic(v: &serde_json::Value) -> Dynamic {
    match v {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Dynamic::from(i as INT),
            None => Dynamic::from(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Array(a) => Dynamic::from(a.iter().map(json_to_dynamic).collect::<Array>()),
        serde_json::Value::Object(o) => {
            let mut m = RMap::new();
            for (k, x) in o {
                m.insert(k.as_str().into(), json_to_dynamic(x));
            }
            Dynamic::from(m)
        }
    }
}

pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn headers_from(m: &RMap) -> Vec<(String, String)> {
    m.iter().map(|(k, v)| (k.to_string(), v.to_string())).filter(|(k, v)| !k.contains(['\r', '\n']) && !v.contains(['\r', '\n'])).collect()
}

/// One panel element a plugin asked for.
#[derive(Clone, Debug)]
pub enum Widget {
    Label { text: String, strong: bool, small: bool },
    Input { id: String, value: String, hint: String, password: bool },
    Button { id: String, text: String },
    Image { url: String, w: f32, h: f32, id: Option<String> },
    Link { text: String, url: String },
    Progress(f32),
    Separator,
    Row(Vec<Widget>),
}

pub fn widgets(d: &Dynamic) -> Vec<Widget> {
    let Some(arr) = d.clone().try_cast::<Array>() else { return Vec::new() };
    arr.iter().filter_map(widget).take(500).collect()
}

fn widget(d: &Dynamic) -> Option<Widget> {
    let m = d.clone().try_cast::<RMap>()?;
    let s = |k: &str| m.get(k).map(|v| v.to_string()).filter(|v| v != "()").unwrap_or_default();
    let f = |k: &str, def: f32| m.get(k).and_then(|v| v.as_float().ok().map(|x| x as f32).or_else(|| v.as_int().ok().map(|x| x as f32))).unwrap_or(def);
    let b = |k: &str| m.get(k).and_then(|v| v.as_bool().ok()).unwrap_or(false);
    Some(match s("t").as_str() {
        "label" => Widget::Label { text: s("text"), strong: b("strong"), small: b("small") },
        "input" => Widget::Input { id: s("id"), value: s("value"), hint: s("hint"), password: b("password") },
        "button" => Widget::Button { id: s("id"), text: s("text") },
        "image" => Widget::Image { url: s("url"), w: f("w", 96.0).clamp(8.0, 512.0), h: f("h", 96.0).clamp(8.0, 512.0), id: m.get("id").map(|v| v.to_string()) },
        "link" => Widget::Link { text: s("text"), url: s("url") },
        "progress" => Widget::Progress(f("value", 0.0).clamp(0.0, 1.0)),
        "separator" => Widget::Separator,
        "row" => Widget::Row(m.get("items").map(widgets).unwrap_or_default()),
        _ => return None,
    })
}
