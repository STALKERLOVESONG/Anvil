//! Team Create: edit one map together over the network.
//!
//! * Star layout: one host, guests connect only to the host. Guests never
//!   learn each other's IP addresses.
//! * Joining needs the invite code AND the host clicking Allow. The host
//!   accepts LAN / VPN (Tailscale, ZeroTier) addresses only, unless they turn
//!   on internet joins.
//! * The wire format is newline-delimited JSON with a fixed set of message
//!   types. Nothing received is ever run as code. Every object and name is
//!   checked (finite numbers, sane sizes, valid indices) before it's used.
//! * Files (textures, models) are only sent if the map uses them AND they
//!   were added on this PC. A path a peer put in the map is never read from
//!   disk, never served and never opened directly. It is downloaded into a
//!   sandbox folder under a cleaned-up file name instead.
//! * Every message has a size cap, and every connection has a rate cap.

use crate::map::{Folder, Kind, Map, Object, TextureDef};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PROTOCOL: u32 = 1;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PORT: u16 = 47600;
pub const HOST_ID: u32 = 1;
pub const MAX_PEERS: usize = 16;
const HELLO_LIMIT: usize = 64 * 1024;
const MSG_LIMIT: usize = 48 * 1024 * 1024;
const WELCOME_LIMIT: usize = 512 * 1024 * 1024;
pub const ASSET_LIMIT: u64 = 24 * 1024 * 1024;
pub const ASSET_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "bmp", "tga", "obj", "stl", "glb", "gltf"];
const TICK: Duration = Duration::from_millis(100);
const MAX_ASKS: usize = 4;
const MAX_HANDSHAKES: usize = 32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: u32,
    pub name: String,
    pub color: [u8; 3],
}

/// Where someone is looking and what they have selected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub cam: Vec3,
    pub look: Vec3,
    pub sel: Vec<u64>,
    pub tool: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Msg {
    Hello { proto: u32, app: String, name: String, color: [u8; 3], code: String },
    Refuse { reason: String },
    Welcome { you: PeerInfo, id_base: u64, map: Map, peers: Vec<PeerInfo> },
    Joined { peer: PeerInfo },
    Left { peer: u32 },
    Presence { peer: u32, p: Presence },
    Upsert { from: u32, seq: u64, objects: Vec<Object> },
    Remove { from: u32, seq: u64, ids: Vec<u64> },
    Meta { from: u32, seq: u64, name: String, folders: Vec<Folder>, textures: Vec<TextureDef> },
    Ack { seq: u64 },
    AssetReq { path: String },
    /// A file the map uses. For an .obj, `extra` carries its .mtl files and
    /// textures as (flat file name, base64) so props arrive textured.
    AssetData {
        path: String,
        data: String,
        #[serde(default)]
        extra: Vec<(String, String)>,
    },
    AssetMissing { path: String },
    Bye { reason: String },
}

enum Event {
    Hello { conn: u32, name: String, color: [u8; 3], addr: SocketAddr, stream: TcpStream },
    Msg { conn: u32, msg: Msg },
    Closed { conn: u32, reason: String },
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

fn obj_hash(o: &Object) -> u64 {
    fnv(&serde_json::to_vec(o).unwrap_or_default())
}

fn meta_hash(map: &Map) -> u64 {
    fnv(&serde_json::to_vec(&(&map.name, &map.folders, &map.textures)).unwrap_or_default())
}

/// OS-seeded randomness (std's hasher keys come from the OS RNG).
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// 8-character invite code (about 40 bits), no look-alike characters.
pub fn new_invite_code() -> String {
    const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let (mut a, mut b) = (random_u64(), random_u64());
    (0..8)
        .map(|i| {
            let r = if i < 4 { &mut a } else { &mut b };
            let c = A[(*r % 32) as usize] as char;
            *r /= 32;
            c
        })
        .collect()
}

fn same_code(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect::<String>();
    let (a, b) = (norm(a), norm(b));
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// LAN, loopback, link-local, CGNAT / Tailscale (100.64/10) and IPv6 private ranges.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private() || v.is_loopback() || v.is_link_local() || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let s = v.segments();
            v.is_loopback() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// This PC's LAN address (what friends on the same network type in). No
/// packet is sent: connecting a UDP socket only picks the route.
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.254.254.254:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

pub fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(32).collect();
    let t = t.trim();
    if t.is_empty() { "Player".into() } else { t.to_string() }
}

fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

pub fn version_refusal(host: &str, guest: &str) -> String {
    if host == guest {
        "This Team Create session uses a different protocol. Update Anvil on both PCs.".into()
    } else {
        format!("Version mismatch: the host is on Anvil {host}, you're on {guest}. Both of you need the same version to work together.")
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(B64[(n >> 18) as usize & 63] as char);
        s.push(B64[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    s
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut val = [255u8; 256];
    for (i, c) in B64.iter().enumerate() {
        val[*c as usize] = i as u8;
    }
    let b = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(b.len() * 3 / 4);
    for c in b.chunks(4) {
        let mut n = 0u32;
        for (i, x) in c.iter().enumerate() {
            let v = val[*x as usize];
            if v == 255 {
                return None;
            }
            n |= (v as u32) << (18 - 6 * i);
        }
        match c.len() {
            4 => out.extend([(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => out.extend([(n >> 16) as u8, (n >> 8) as u8]),
            2 => out.push((n >> 16) as u8),
            _ => return None,
        }
    }
    Some(out)
}

pub fn path_ext(p: &str) -> String {
    Path::new(p).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Sandbox file name for a downloaded asset: hash of the remote path + the
/// base name with anything unusual removed, so it can't escape the folder.
pub fn sandbox_name(remote: &str) -> Option<String> {
    let ext = path_ext(remote);
    if !ASSET_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let base = remote.rsplit(['/', '\\']).next().unwrap_or("file");
    let stem = base.rsplit_once('.').map(|x| x.0).unwrap_or(base);
    let stem: String = stem.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(40).collect();
    Some(format!("{:016x}_{}.{ext}", fnv(remote.as_bytes()), if stem.is_empty() { "file" } else { &stem }))
}

const BUNDLE_LIMIT: u64 = 32 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 64;
const BUNDLE_EXTS: [&str; 6] = ["mtl", "png", "jpg", "jpeg", "bmp", "tga"];
/// What a glTF bundle holds: one .bin and its textures (always png / jpg).
const GLTF_BUNDLE_EXTS: [&str; 3] = ["bin", "png", "jpg"];
/// Biggest .glb / .gltf (or .bin) read to send: it's repacked and its
/// textures shrunk until it fits the transfer limits.
const GLTF_SOURCE_LIMIT: u64 = 512 * 1024 * 1024;

fn is_gltf(p: &str) -> bool {
    matches!(path_ext(p).as_str(), "gltf" | "glb")
}

/// Files that may travel with this kind of model.
fn bundle_exts(model: &str) -> &'static [&'static str] {
    if is_gltf(model) {
        &GLTF_BUNDLE_EXTS
    } else {
        &BUNDLE_EXTS
    }
}

/// May this file go out with a shared model? Only .mtl and images (an .obj)
/// or .bin and images (a glTF), small enough, not on a network share, and
/// only from the model's own folder area (its folder's parent), Anvil's
/// content folder (Half-Life imports), the Team Create cache or opened
/// packages. Even an odd model can't be used to send anything else.
pub fn bundle_allowed(obj: &Path, dep: &Path, cache: &Path) -> bool {
    let ds = dep.to_string_lossy();
    let model = obj.to_string_lossy();
    let exts: &[&str] = if is_gltf(&model) { &["bin", "png", "jpg", "jpeg"] } else { &BUNDLE_EXTS };
    if !exts.contains(&path_ext(&ds).as_str()) || crate::texture::is_network_path(&ds) {
        return false;
    }
    let limit = if is_gltf(&model) { GLTF_SOURCE_LIMIT } else { ASSET_LIMIT };
    if !std::fs::metadata(dep).is_ok_and(|m| m.is_file() && m.len() <= limit) {
        return false;
    }
    let Ok(d) = dep.canonicalize() else { return false };
    let roots = [obj.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()), Some(crate::mount::content_dir()), Some(cache.to_path_buf()), Some(crate::package::packages_dir())];
    roots.into_iter().flatten().filter_map(|r| r.canonicalize().ok()).any(|r| d.starts_with(r))
}

/// Checks a received model bundle for `model`: plain file names only (no
/// folders, no dots in front), the types that model kind may bring, size
/// limits, images that really are images. Returns decoded files.
pub fn check_bundle(model: &str, extra: &[(String, String)]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let exts = bundle_exts(model);
    if extra.len() > MAX_BUNDLE_FILES {
        return Err("too many files with the model".into());
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for (name, b64) in extra {
        let ok_name = !name.is_empty() && name.len() <= 120 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
        if !ok_name || !exts.contains(&path_ext(name).as_str()) {
            return Err(format!("unsafe file name with the model: {name}"));
        }
        if out.iter().any(|(n, _): &(String, Vec<u8>)| n.eq_ignore_ascii_case(name)) {
            return Err(format!("{name} sent twice"));
        }
        let bytes = b64_decode(b64).ok_or("damaged data")?;
        total += bytes.len() as u64;
        if bytes.len() as u64 > ASSET_LIMIT || total > BUNDLE_LIMIT {
            return Err("the model's textures are too big".into());
        }
        if !matches!(path_ext(name).as_str(), "mtl" | "tga" | "bin") && image::guess_format(&bytes).is_err() {
            return Err(format!("{name} isn't really an image"));
        }
        out.push((name.clone(), bytes));
    }
    Ok(out)
}

/// A received .gltf may only point at files that came with it.
pub fn check_gltf(gltf: &[u8], files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let json: serde_json::Value = serde_json::from_slice(gltf).map_err(|_| "not a glTF model")?;
    for r in crate::gltf::referenced_files(&json) {
        if !files.iter().any(|(n, _)| *n == r) {
            return Err(format!("the model points at a file that wasn't sent with it: {r}"));
        }
    }
    Ok(())
}

/// Every file path the map points at.
pub fn map_paths(map: &Map) -> HashSet<String> {
    let mut s = HashSet::new();
    for t in &map.textures {
        for p in [&t.image, &t.normal_map, &t.roughness_map, &t.metalness_map] {
            if !p.trim().is_empty() {
                s.insert(p.clone());
            }
        }
    }
    for o in &map.objects {
        if let Kind::Prop { model, .. } = &o.kind {
            if !model.trim().is_empty() {
                s.insert(model.clone());
            }
        }
    }
    s
}

fn object_paths(o: &Object) -> Vec<String> {
    match &o.kind {
        Kind::Prop { model, .. } if !model.trim().is_empty() => vec![model.clone()],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Validation of everything that comes off the network
// ---------------------------------------------------------------------------

fn ok_f(x: f32) -> bool {
    x.is_finite() && x.abs() <= 1.0e7
}
fn ok_v3(v: Vec3) -> bool {
    ok_f(v.x) && ok_f(v.y) && ok_f(v.z)
}
fn ok_v2(v: Vec2) -> bool {
    ok_f(v.x) && ok_f(v.y)
}

fn ok_prompt(p: &crate::map::PromptSpec) -> bool {
    p.action_text.chars().count() <= 200 && p.object_text.chars().count() <= 200 && p.key.len() <= 32 && p.gamepad.len() <= 32 && ok_f(p.hold) && ok_f(p.distance) && ok_v2(p.ui_offset)
}

pub fn validate_object(o: &Object) -> Result<(), String> {
    let bad = |w: &str| Err(format!("object {}: bad {w}", o.id));
    if o.id == 0 {
        return bad("id");
    }
    if o.name.chars().count() > 256 {
        return bad("name");
    }
    if !ok_v3(o.pos) || !ok_v3(o.rot) || !o.transparency.is_finite() {
        return bad("transform");
    }
    let tp = &o.tex_params;
    if !ok_v2(tp.scale) || !ok_v2(tp.offset) || !ok_f(tp.rotation) {
        return bad("texture settings");
    }
    if o.face_tex.len() > 1_000_000 || o.face_tex.iter().flatten().any(|f| !ok_v2(f.params.scale) || !ok_v2(f.params.offset) || !ok_f(f.params.rotation)) {
        return bad("face textures");
    }
    if let Some(t) = &o.text {
        if t.text.chars().count() > 4096 || t.font.len() > 1024 || t.font_name.len() > 256 || !ok_f(t.height) || !ok_f(t.depth) {
            return bad("text");
        }
    }
    match &o.kind {
        Kind::Brush { size, steps, holes, .. } => {
            if !ok_v3(*size) || *steps > 512 || holes.len() > 512 || holes.iter().any(|h| !ok_v3(h.min) || !ok_v3(h.max)) {
                return bad("brush");
            }
        }
        Kind::Mesh { verts, faces } => {
            if verts.len() > 1_000_000 || faces.len() > 1_000_000 || verts.iter().any(|v| !ok_v3(*v)) {
                return bad("mesh");
            }
            let n = verts.len() as u32;
            let mut total = 0usize;
            for f in faces {
                total += f.len();
                if f.len() > 200_000 || total > 6_000_000 || f.iter().any(|i| *i >= n) {
                    return bad("mesh faces");
                }
            }
        }
        Kind::Prop { model, scale, roblox_mesh_id } => {
            if model.len() > 1024 || roblox_mesh_id.len() > 128 || !ok_v3(*scale) {
                return bad("prop");
            }
        }
        Kind::Decal { size } => {
            if !ok_v2(*size) {
                return bad("decal");
            }
        }
        Kind::Light { range, brightness } => {
            if !ok_f(*range) || !ok_f(*brightness) {
                return bad("light");
            }
        }
        Kind::Spawn { size } => {
            if !ok_v3(*size) {
                return bad("spawn");
            }
        }
        Kind::Prompt { prompt } => {
            if !ok_prompt(prompt) {
                return bad("prompt");
            }
        }
        Kind::Teleport { size, cooldown, exit_offset, prompt, .. } => {
            if !ok_v3(*size) || !ok_f(*cooldown) || !ok_v3(*exit_offset) || !ok_prompt(prompt) {
                return bad("teleport");
            }
        }
        Kind::Target {} => {}
    }
    Ok(())
}

pub fn validate_meta(name: &str, folders: &[Folder], textures: &[TextureDef]) -> Result<(), String> {
    if name.chars().count() > 256 || folders.len() > 20_000 || textures.len() > 20_000 {
        return Err("map info too large".into());
    }
    if folders.iter().any(|f| f.id == 0 || f.name.chars().count() > 256) {
        return Err("bad folder".into());
    }
    for t in textures {
        if t.id == 0 || t.name.chars().count() > 256 || t.image.len() > 1024 || t.normal_map.len() > 1024 || t.roughness_map.len() > 1024 || t.metalness_map.len() > 1024 || t.roblox_roughness_id.len() > 128 || t.roblox_metalness_id.len() > 128 || t.roblox_image_id.len() > 128 || t.roblox_normal_id.len() > 128 || !ok_v2(t.scale) || !ok_v2(t.offset) {
            return Err(format!("bad texture {}", t.id));
        }
    }
    Ok(())
}

pub fn validate_map(map: &Map) -> Result<(), String> {
    if map.objects.len() > 500_000 {
        return Err("map has too many objects".into());
    }
    validate_meta(&map.name, &map.folders, &map.textures)?;
    map.objects.iter().try_for_each(validate_object)
}

fn clean_presence(mut p: Presence) -> Option<Presence> {
    if !ok_v3(p.cam) || !ok_v3(p.look) {
        return None;
    }
    p.sel.truncate(20_000);
    p.tool = clean_text(&p.tool, 32);
    Some(p)
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

fn line(msg: &Msg) -> Arc<str> {
    let mut s = serde_json::to_string(msg).unwrap_or_default();
    s.push('\n');
    s.into()
}

struct Link {
    tx: SyncSender<Arc<str>>,
    stream: TcpStream,
}

impl Link {
    fn new(stream: TcpStream) -> std::io::Result<Link> {
        let mut w = stream.try_clone()?;
        let _ = w.set_write_timeout(Some(Duration::from_secs(20)));
        let (tx, rx) = mpsc::sync_channel::<Arc<str>>(4096);
        std::thread::spawn(move || {
            for l in rx {
                if w.write_all(l.as_bytes()).is_err() {
                    break;
                }
            }
            let _ = w.flush();
            let _ = w.shutdown(Shutdown::Both);
        });
        Ok(Link { tx, stream })
    }

    /// Queues a line. False if the peer is too far behind (or gone).
    fn send(&self, l: &Arc<str>) -> bool {
        self.tx.try_send(l.clone()).is_ok()
    }
}

fn read_msg(r: &mut BufReader<TcpStream>, limit: usize, buf: &mut Vec<u8>) -> Result<Msg, String> {
    buf.clear();
    let n = r.by_ref().take(limit as u64 + 1).read_until(b'\n', buf).map_err(|e| match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => "timed out".to_string(),
        _ => "connection lost".to_string(),
    })?;
    if n == 0 {
        return Err("connection closed".into());
    }
    if buf.last() != Some(&b'\n') {
        return Err(if n > limit { "sent a message that was too large".into() } else { "connection closed".into() });
    }
    serde_json::from_slice(buf).map_err(|_| "sent something that isn't a valid message".into())
}

fn reader_loop(conn: u32, mut r: BufReader<TcpStream>, limit: usize, tx: Sender<Event>) {
    let mut buf = Vec::new();
    let mut window = Instant::now();
    let (mut bytes, mut count) = (0usize, 0usize);
    loop {
        match read_msg(&mut r, limit, &mut buf) {
            Ok(msg) => {
                if window.elapsed() > Duration::from_secs(10) {
                    window = Instant::now();
                    bytes = 0;
                    count = 0;
                }
                bytes += buf.len();
                count += 1;
                if bytes > 400 * 1024 * 1024 || count > 4000 {
                    let _ = tx.send(Event::Closed { conn, reason: "was sending far too much data".into() });
                    break;
                }
                if tx.send(Event::Msg { conn, msg }).is_err() {
                    break;
                }
            }
            Err(e) => {
                let _ = tx.send(Event::Closed { conn, reason: e });
                break;
            }
        }
    }
    let _ = r.get_ref().shutdown(Shutdown::Both);
}

struct HostShared {
    code: String,
    allow_internet: bool,
    fails: Mutex<HashMap<IpAddr, u32>>,
    handshakes: AtomicUsize,
    next_conn: AtomicU32,
}

fn refuse(mut s: &TcpStream, reason: &str) {
    let _ = s.write_all(line(&Msg::Refuse { reason: reason.into() }).as_bytes());
    let _ = s.flush();
    let _ = s.shutdown(Shutdown::Both);
}

fn listen_loop(listener: TcpListener, shared: Arc<HostShared>, tx: Sender<Event>, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, addr)) => {
                let ip = addr.ip();
                if !shared.allow_internet && !is_private_ip(ip) {
                    let _ = s.shutdown(Shutdown::Both);
                    continue;
                }
                if shared.fails.lock().map(|f| f.get(&ip).copied().unwrap_or(0) >= 5).unwrap_or(true) {
                    let _ = s.shutdown(Shutdown::Both);
                    continue;
                }
                if shared.handshakes.load(Ordering::Relaxed) >= MAX_HANDSHAKES {
                    let _ = s.shutdown(Shutdown::Both);
                    continue;
                }
                let _ = s.set_nonblocking(false);
                let conn = shared.next_conn.fetch_add(1, Ordering::Relaxed);
                let (shared, tx) = (shared.clone(), tx.clone());
                shared.handshakes.fetch_add(1, Ordering::Relaxed);
                std::thread::spawn(move || {
                    handshake(conn, s, addr, &shared, tx);
                    shared.handshakes.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn handshake(conn: u32, s: TcpStream, addr: SocketAddr, shared: &HostShared, tx: Sender<Event>) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(rs) = s.try_clone() else { return };
    let mut r = BufReader::new(rs);
    let mut buf = Vec::new();
    let Ok(Msg::Hello { proto, app, name, color, code }) = read_msg(&mut r, HELLO_LIMIT, &mut buf) else {
        let _ = s.shutdown(Shutdown::Both);
        return;
    };
    if proto != PROTOCOL || app != APP_VERSION {
        refuse(&s, &version_refusal(APP_VERSION, &clean_text(&app, 32)));
        return;
    }
    if !same_code(&code, &shared.code) {
        if let Ok(mut f) = shared.fails.lock() {
            *f.entry(addr.ip()).or_insert(0) += 1;
        }
        std::thread::sleep(Duration::from_secs(1));
        refuse(&s, "Wrong invite code.");
        return;
    }
    let Ok(stream) = s.try_clone() else { return };
    let _ = s.set_read_timeout(Some(Duration::from_secs(90)));
    if tx.send(Event::Hello { conn, name: clean_name(&name), color, addr, stream }).is_err() {
        return;
    }
    reader_loop(conn, r, MSG_LIMIT, tx);
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

pub struct Remote {
    pub info: PeerInfo,
    pub presence: Option<Presence>,
    /// Host only: where they connected from.
    pub addr: Option<SocketAddr>,
}

pub struct Pending {
    pub conn: u32,
    pub name: String,
    pub color: [u8; 3],
    pub addr: SocketAddr,
    stream: TcpStream,
}

/// What a `pump` changed, for the app to react to.
#[derive(Default, Debug)]
pub struct Pumped {
    /// A guest just received the host's map (replaces whatever was open).
    pub map_replaced: bool,
    pub changed: Vec<u64>,
    pub removed: Vec<u64>,
    pub meta_changed: bool,
    /// Paths a peer put in the map: loaders must never open these directly.
    pub block: Vec<String>,
    /// Downloaded files: (path in the map, local sandbox copy).
    pub assets: Vec<(String, PathBuf)>,
}

pub struct Session {
    pub host: bool,
    pub me: PeerInfo,
    pub peers: BTreeMap<u32, Remote>,
    pub pending: Vec<Pending>,
    pub code: String,
    pub port: u16,
    pub log: Vec<String>,
    /// Set when the session is over (with the reason).
    pub ended: Option<String>,
    pub welcomed: bool,
    pub allow_internet: bool,
    /// Start of this person's id range (object ids never collide).
    pub id_base: u64,
    rx: Receiver<Event>,
    tx: Sender<Event>,
    links: HashMap<u32, Link>,
    stop: Arc<AtomicBool>,
    sent: HashMap<u64, u64>,
    meta_hash: u64,
    seq: u64,
    acked: u64,
    inflight: HashMap<u64, u64>,
    inflight_meta: u64,
    local_paths: HashSet<String>,
    remote_paths: HashMap<String, u32>,
    remap: HashMap<String, PathBuf>,
    asked: HashSet<String>,
    missing: HashSet<String>,
    waiting: HashMap<String, Vec<u32>>,
    cache_dir: PathBuf,
    last_tick: Option<Instant>,
    last_presence: (Option<Instant>, u64),
}

impl Session {
    fn base(host: bool, me: PeerInfo, cache_dir: PathBuf) -> Session {
        let (tx, rx) = mpsc::channel();
        Session {
            host,
            me,
            peers: BTreeMap::new(),
            pending: Vec::new(),
            code: String::new(),
            port: 0,
            log: Vec::new(),
            ended: None,
            welcomed: host,
            allow_internet: false,
            id_base: 0,
            rx,
            tx,
            links: HashMap::new(),
            stop: Arc::new(AtomicBool::new(false)),
            sent: HashMap::new(),
            meta_hash: 0,
            seq: 0,
            acked: 0,
            inflight: HashMap::new(),
            inflight_meta: 0,
            local_paths: HashSet::new(),
            remote_paths: HashMap::new(),
            remap: HashMap::new(),
            asked: HashSet::new(),
            missing: HashSet::new(),
            waiting: HashMap::new(),
            cache_dir,
            last_tick: None,
            last_presence: (None, 0),
        }
    }

    /// Starts hosting `map` on `port` (0 = pick a free one).
    pub fn host(map: &mut Map, name: &str, color: [u8; 3], port: u16, allow_internet: bool, cache_dir: PathBuf) -> Result<Session, String> {
        let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("Couldn't open port {port}: {e}"))?;
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let mut s = Session::base(true, PeerInfo { id: HOST_ID, name: clean_name(name), color }, cache_dir);
        s.code = new_invite_code();
        s.port = port;
        s.allow_internet = allow_internet;
        let shared = Arc::new(HostShared {
            code: s.code.clone(),
            allow_internet,
            fails: Mutex::new(HashMap::new()),
            handshakes: AtomicUsize::new(0),
            next_conn: AtomicU32::new(HOST_ID + 1),
        });
        let (tx, stop) = (s.tx.clone(), s.stop.clone());
        std::thread::spawn(move || listen_loop(listener, shared, tx, stop));
        s.id_base = (HOST_ID as u64) << 40;
        map.id_base = s.id_base;
        map.local_next = 0;
        s.local_paths = map_paths(map);
        s.mark_all_sent(map);
        s.log.push(format!("Hosting on port {port}"));
        Ok(s)
    }

    /// Connects to a host ("192.168.1.20" or "name:port").
    pub fn join(addr: &str, code: &str, name: &str, color: [u8; 3], cache_dir: PathBuf) -> Result<Session, String> {
        let addr = addr.trim();
        if addr.is_empty() {
            return Err("Type the host's address".into());
        }
        let with_port = if addr.contains(':') && !addr.starts_with('[') && addr.matches(':').count() > 1 {
            format!("[{addr}]:{DEFAULT_PORT}")
        } else if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
            addr.to_string()
        } else {
            format!("{addr}:{DEFAULT_PORT}")
        };
        let sa = with_port.to_socket_addrs().map_err(|e| format!("Can't find {addr}: {e}"))?.next().ok_or("Can't find that address")?;
        let s = TcpStream::connect_timeout(&sa, Duration::from_secs(6)).map_err(|e| format!("Couldn't reach {sa}: {e}"))?;
        let _ = s.set_nodelay(true);
        let _ = s.set_read_timeout(Some(Duration::from_secs(600)));
        let mut me = Session::base(false, PeerInfo { id: 0, name: clean_name(name), color }, cache_dir);
        let link = Link::new(s.try_clone().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        link.send(&line(&Msg::Hello { proto: PROTOCOL, app: APP_VERSION.into(), name: me.me.name.clone(), color, code: code.trim().into() }));
        let r = BufReader::new(s);
        let tx = me.tx.clone();
        std::thread::spawn(move || reader_loop(HOST_ID, r, WELCOME_LIMIT, tx));
        me.links.insert(HOST_ID, link);
        me.log.push("Waiting for the host to let you in…".into());
        Ok(me)
    }

    fn mark_all_sent(&mut self, map: &Map) {
        self.sent = map.objects.iter().map(|o| (o.id, obj_hash(o))).collect();
        self.meta_hash = meta_hash(map);
    }

    fn send_to(&mut self, conn: u32, l: &Arc<str>) {
        let ok = self.links.get(&conn).is_some_and(|k| k.send(l));
        if !ok && self.links.contains_key(&conn) {
            let _ = self.tx.send(Event::Closed { conn, reason: "couldn't keep up".into() });
        }
    }

    fn broadcast(&mut self, l: &Arc<str>, except: Option<u32>) {
        let ids: Vec<u32> = self.links.keys().copied().filter(|c| Some(*c) != except).collect();
        for c in ids {
            self.send_to(c, l);
        }
    }

    /// Host: let a waiting person in.
    pub fn approve(&mut self, conn: u32, map: &Map) {
        let Some(i) = self.pending.iter().position(|p| p.conn == conn) else { return };
        let p = self.pending.remove(i);
        if self.links.len() >= MAX_PEERS {
            refuse(&p.stream, "The session is full.");
            return;
        }
        let Ok(link) = Link::new(p.stream) else { return };
        let info = PeerInfo { id: conn, name: p.name, color: p.color };
        let mut peers: Vec<PeerInfo> = vec![self.me.clone()];
        peers.extend(self.peers.values().map(|r| r.info.clone()));
        self.links.insert(conn, link);
        let welcome = line(&Msg::Welcome { you: info.clone(), id_base: (conn as u64) << 40, map: map.clone(), peers });
        self.send_to(conn, &welcome);
        self.broadcast(&line(&Msg::Joined { peer: info.clone() }), Some(conn));
        self.log.push(format!("{} joined", info.name));
        self.peers.insert(conn, Remote { info, presence: None, addr: Some(p.addr) });
        self.last_presence.0 = None;
    }

    /// Host: turn a waiting person away.
    pub fn deny(&mut self, conn: u32) {
        if let Some(i) = self.pending.iter().position(|p| p.conn == conn) {
            let p = self.pending.remove(i);
            refuse(&p.stream, "The host didn't let you in.");
            self.log.push(format!("Turned away {}", p.name));
        }
    }

    /// Host: remove someone from the session.
    pub fn kick(&mut self, conn: u32) {
        if let Some(l) = self.links.get(&conn) {
            l.send(&line(&Msg::Bye { reason: "The host removed you from the session.".into() }));
        }
        self.drop_peer(conn, "was removed");
    }

    fn drop_peer(&mut self, conn: u32, why: &str) {
        if let Some(l) = self.links.remove(&conn) {
            let _ = l.stream.shutdown(Shutdown::Read);
        }
        if let Some(r) = self.peers.remove(&conn) {
            self.log.push(format!("{} {why}", r.info.name));
            if self.host {
                self.broadcast(&line(&Msg::Left { peer: conn }), None);
            }
        }
        self.waiting.values_mut().for_each(|w| w.retain(|c| *c != conn));
    }

    /// Ends the session (tells everyone).
    pub fn leave(&mut self, reason: &str) {
        if self.ended.is_some() {
            return;
        }
        let bye = line(&Msg::Bye { reason: reason.into() });
        self.broadcast(&bye, None);
        for p in self.pending.drain(..) {
            refuse(&p.stream, reason);
        }
        self.links.clear(); // writers flush the goodbye, then close
        self.stop.store(true, Ordering::Relaxed);
        self.ended = Some("You left the session.".into());
    }

    /// Processes network traffic and sends local changes (about 10x a second).
    pub fn pump(&mut self, map: &mut Map, presence: &Presence) -> Pumped {
        let mut out = Pumped::default();
        if self.welcomed && map.id_base != self.id_base {
            // an undo step from before the session brought back the old range
            map.id_base = self.id_base;
            map.local_next = 0;
        }
        while self.ended.is_none() {
            let Ok(ev) = self.rx.try_recv() else { break };
            self.handle(ev, map, &mut out);
        }
        if self.ended.is_some() || !self.welcomed {
            return out;
        }
        let now = Instant::now();
        if self.last_tick.is_some_and(|t| now - t < TICK) {
            return out;
        }
        self.last_tick = Some(now);
        self.sync_out(map);
        self.presence_out(presence, now);
        self.request_assets(map);
        out
    }

    fn handle(&mut self, ev: Event, map: &mut Map, out: &mut Pumped) {
        match ev {
            Event::Hello { conn, name, color, addr, stream } => {
                if !self.host {
                    return;
                }
                if self.links.len() + self.pending.len() >= MAX_PEERS {
                    refuse(&stream, "The session is full.");
                    return;
                }
                self.log.push(format!("{name} wants to join (from {})", addr.ip()));
                self.pending.push(Pending { conn, name, color, addr, stream });
            }
            Event::Closed { conn, reason } => {
                if self.host {
                    if let Some(i) = self.pending.iter().position(|p| p.conn == conn) {
                        self.pending.remove(i);
                    } else {
                        let why = if reason == "connection closed" { "left".to_string() } else { format!("disconnected ({reason})") };
                        self.drop_peer(conn, &why);
                    }
                } else if conn == HOST_ID {
                    self.links.clear();
                    self.stop.store(true, Ordering::Relaxed);
                    self.ended = Some(if self.welcomed { format!("Lost the connection to the host ({reason}).") } else { format!("Couldn't join: the host closed the connection ({reason}).") });
                }
            }
            Event::Msg { conn, msg } => {
                if self.host && !self.links.contains_key(&conn) {
                    return; // still waiting for approval
                }
                self.handle_msg(conn, msg, map, out);
            }
        }
    }

    fn note_remote_paths(&mut self, paths: impl IntoIterator<Item = String>, origin: u32, out: &mut Pumped) {
        for p in paths {
            if self.local_paths.contains(&p) || self.remote_paths.contains_key(&p) {
                continue;
            }
            self.remote_paths.insert(p.clone(), origin);
            out.block.push(p);
        }
    }

    fn apply_upsert(&mut self, objects: Vec<Object>, origin: u32, map: &mut Map, out: &mut Pumped) {
        let mut index: HashMap<u64, usize> = map.objects.iter().enumerate().map(|(i, o)| (o.id, i)).collect();
        for o in objects {
            self.note_remote_paths(object_paths(&o), origin, out);
            self.sent.insert(o.id, obj_hash(&o));
            out.changed.push(o.id);
            match index.get(&o.id) {
                Some(&i) => map.objects[i] = o,
                None => {
                    index.insert(o.id, map.objects.len());
                    map.objects.push(o);
                }
            }
        }
    }

    fn apply_meta(&mut self, name: String, folders: Vec<Folder>, textures: Vec<TextureDef>, origin: u32, map: &mut Map, out: &mut Pumped) {
        let paths: Vec<String> = textures.iter().flat_map(|t| [t.image.clone(), t.normal_map.clone(), t.roughness_map.clone(), t.metalness_map.clone()]).filter(|p| !p.trim().is_empty()).collect();
        self.note_remote_paths(paths, origin, out);
        map.name = clean_text(&name, 256);
        map.folders = folders;
        map.textures = textures;
        self.meta_hash = meta_hash(map);
        out.meta_changed = true;
    }

    fn handle_msg(&mut self, conn: u32, msg: Msg, map: &mut Map, out: &mut Pumped) {
        match msg {
            Msg::Hello { .. } => {}
            Msg::Refuse { reason } | Msg::Bye { reason } => {
                if !self.host && conn == HOST_ID {
                    self.links.clear();
                    self.stop.store(true, Ordering::Relaxed);
                    self.ended = Some(clean_text(&reason, 300));
                } else if self.host {
                    self.drop_peer(conn, "left");
                }
            }
            Msg::Welcome { you, id_base, map: m, peers } => {
                if self.host || self.welcomed {
                    return;
                }
                if let Err(e) = validate_map(&m) {
                    self.leave("bad map");
                    self.ended = Some(format!("The host sent a map that failed safety checks ({e})."));
                    return;
                }
                *map = m;
                if id_base < (2u64 << 40) || id_base > (1u64 << 62) {
                    self.leave("bad id range");
                    self.ended = Some("The host sent a bad id range.".into());
                    return;
                }
                self.id_base = id_base;
                map.id_base = id_base;
                map.local_next = 0;
                self.me.id = you.id;
                self.welcomed = true;
                self.mark_all_sent(map);
                let paths: Vec<String> = map_paths(map).into_iter().collect();
                self.note_remote_paths(paths, HOST_ID, out);
                for p in peers {
                    if p.id != self.me.id {
                        let info = PeerInfo { id: p.id, name: clean_name(&p.name), color: p.color };
                        self.peers.insert(p.id, Remote { info, presence: None, addr: None });
                    }
                }
                out.map_replaced = true;
                self.log.push("Joined! You're editing the host's map.".into());
            }
            Msg::Joined { peer } => {
                if !self.host && peer.id != self.me.id {
                    let info = PeerInfo { id: peer.id, name: clean_name(&peer.name), color: peer.color };
                    self.log.push(format!("{} joined", info.name));
                    self.peers.insert(peer.id, Remote { info, presence: None, addr: None });
                }
            }
            Msg::Left { peer } => {
                if !self.host {
                    if let Some(r) = self.peers.remove(&peer) {
                        self.log.push(format!("{} left", r.info.name));
                    }
                }
            }
            Msg::Presence { peer, p } => {
                let Some(p) = clean_presence(p) else { return };
                let who = if self.host { conn } else { peer };
                if self.host {
                    self.broadcast(&line(&Msg::Presence { peer: conn, p: p.clone() }), Some(conn));
                }
                if let Some(r) = self.peers.get_mut(&who) {
                    r.presence = Some(p);
                }
            }
            Msg::Upsert { seq, objects, .. } => {
                if let Some(e) = objects.iter().find_map(|o| validate_object(o).err()) {
                    self.reject(conn, &e);
                    return;
                }
                if self.host {
                    self.broadcast(&line(&Msg::Upsert { from: conn, seq: 0, objects: objects.clone() }), Some(conn));
                    self.apply_upsert(objects, conn, map, out);
                    self.send_to(conn, &line(&Msg::Ack { seq }));
                } else {
                    let acked = self.acked;
                    let objects: Vec<Object> = objects.into_iter().filter(|o| self.inflight.get(&o.id).is_none_or(|s| *s <= acked)).collect();
                    self.apply_upsert(objects, HOST_ID, map, out);
                }
            }
            Msg::Remove { seq, ids, .. } => {
                if ids.len() > 1_000_000 {
                    self.reject(conn, "too many ids");
                    return;
                }
                let ids: Vec<u64> = if self.host {
                    self.broadcast(&line(&Msg::Remove { from: conn, seq: 0, ids: ids.clone() }), Some(conn));
                    self.send_to(conn, &line(&Msg::Ack { seq }));
                    ids
                } else {
                    let acked = self.acked;
                    ids.into_iter().filter(|id| self.inflight.get(id).is_none_or(|s| *s <= acked)).collect()
                };
                let set: HashSet<u64> = ids.iter().copied().collect();
                map.objects.retain(|o| !set.contains(&o.id));
                for id in ids {
                    self.sent.remove(&id);
                    out.removed.push(id);
                }
            }
            Msg::Meta { seq, name, folders, textures, .. } => {
                if let Err(e) = validate_meta(&name, &folders, &textures) {
                    self.reject(conn, &e);
                    return;
                }
                if self.host {
                    self.broadcast(&line(&Msg::Meta { from: conn, seq: 0, name: name.clone(), folders: folders.clone(), textures: textures.clone() }), Some(conn));
                    self.apply_meta(name, folders, textures, conn, map, out);
                    self.send_to(conn, &line(&Msg::Ack { seq }));
                } else if self.inflight_meta <= self.acked {
                    self.apply_meta(name, folders, textures, HOST_ID, map, out);
                }
            }
            Msg::Ack { seq } => {
                if !self.host {
                    self.acked = self.acked.max(seq);
                    let acked = self.acked;
                    self.inflight.retain(|_, s| *s > acked);
                }
            }
            Msg::AssetReq { path } => self.serve(conn, path, map),
            Msg::AssetData { path, data, extra } => {
                if !self.asked.remove(&path) {
                    return; // never asked for it
                }
                match self.store_asset(&path, &data, &extra) {
                    Ok(local) => {
                        self.remap.insert(path.clone(), local.clone());
                        out.assets.push((path.clone(), local.clone()));
                        for w in self.waiting.remove(&path).unwrap_or_default() {
                            self.send_file(w, &path, &local);
                        }
                    }
                    Err(e) => {
                        self.log.push(format!("Skipped a file from the network: {e}"));
                        self.missing.insert(path.clone());
                        self.tell_waiters_missing(&path);
                    }
                }
            }
            Msg::AssetMissing { path } => {
                if self.asked.remove(&path) {
                    self.missing.insert(path.clone());
                    self.tell_waiters_missing(&path);
                }
            }
        }
    }

    fn reject(&mut self, conn: u32, why: &str) {
        self.log.push(format!("Ignored bad data ({why})"));
        if self.host {
            self.kick(conn);
        }
    }

    fn tell_waiters_missing(&mut self, path: &str) {
        for w in self.waiting.remove(path).unwrap_or_default() {
            self.send_to(w, &line(&Msg::AssetMissing { path: path.to_string() }));
        }
    }

    /// Someone asked for a file. Only files this map uses are ever sent, and
    /// only ones that were added on this PC (or that we downloaded).
    fn serve(&mut self, conn: u32, path: String, map: &Map) {
        let used = map_paths(map).contains(&path);
        if used {
            if let Some(local) = self.remap.get(&path).cloned() {
                self.send_file(conn, &path, &local);
                return;
            }
            if self.local_paths.contains(&path) && !self.remote_paths.contains_key(&path) {
                self.send_file(conn, &path, Path::new(&path));
                return;
            }
            if self.host {
                if let Some(&origin) = self.remote_paths.get(&path) {
                    if origin != conn && self.links.contains_key(&origin) && !self.missing.contains(&path) {
                        self.waiting.entry(path.clone()).or_default().push(conn);
                        if self.asked.insert(path.clone()) {
                            self.send_to(origin, &line(&Msg::AssetReq { path }));
                        }
                        return;
                    }
                }
            }
        }
        self.send_to(conn, &line(&Msg::AssetMissing { path }));
    }

    fn send_file(&mut self, conn: u32, key: &str, file: &Path) {
        let ok_ext = ASSET_EXTS.contains(&path_ext(key).as_str()) && ASSET_EXTS.contains(&path_ext(&file.to_string_lossy()).as_str());
        if ok_ext && is_gltf(key) && is_gltf(&file.to_string_lossy()) {
            let msg = self.gltf_msg(key, file);
            self.send_to(conn, &line(&msg));
            return;
        }
        let data = (ok_ext && !crate::texture::is_network_path(&file.to_string_lossy()))
            .then(|| std::fs::metadata(file).ok())
            .flatten()
            .filter(|m| m.is_file() && m.len() <= ASSET_LIMIT)
            .and_then(|_| std::fs::read(file).ok());
        let msg = match data {
            Some(d) if path_ext(key) == "obj" => {
                let cache = self.cache_dir.clone();
                let b = crate::package::model_bundle(&file.to_string_lossy(), &d, &|dep| bundle_allowed(file, dep, &cache));
                let total: u64 = b.files.iter().map(|f| f.1.len() as u64).sum::<u64>() + b.model.len() as u64;
                if total <= BUNDLE_LIMIT && b.files.len() <= MAX_BUNDLE_FILES {
                    Msg::AssetData { path: key.to_string(), data: b64_encode(&b.model), extra: b.files.iter().map(|(n, d)| (n.clone(), b64_encode(d))).collect() }
                } else {
                    self.log.push(format!("{}: its textures are too big to send, sending the shape only", key.rsplit(['/', '\\']).next().unwrap_or(key)));
                    Msg::AssetData { path: key.to_string(), data: b64_encode(&d), extra: Vec::new() }
                }
            }
            Some(d) => Msg::AssetData { path: key.to_string(), data: b64_encode(&d), extra: Vec::new() },
            None => Msg::AssetMissing { path: key.to_string() },
        };
        self.send_to(conn, &line(&msg));
    }

    /// A .glb / .gltf (e.g. from Sketchfab) goes as a .gltf + .bin + textures.
    /// Big textures are shrunk step by step until it fits the transfer limit.
    fn gltf_msg(&mut self, key: &str, file: &Path) -> Msg {
        let short = key.rsplit(['/', '\\']).next().unwrap_or(key).to_string();
        let fs = file.to_string_lossy().to_string();
        let ok = !crate::texture::is_network_path(&fs) && std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.len() <= GLTF_SOURCE_LIMIT);
        if !ok {
            return Msg::AssetMissing { path: key.to_string() };
        }
        let cache = self.cache_dir.clone();
        let allow = |dep: &Path| bundle_allowed(file, dep, &cache);
        for max in [None, Some(2048), Some(1024), Some(512), Some(256)] {
            let (json, files) = match crate::gltf::bundle(&fs, &allow, max) {
                Ok(b) => b,
                Err(e) => {
                    self.log.push(format!("{short} can't be sent: {e}"));
                    return Msg::AssetMissing { path: key.to_string() };
                }
            };
            let total: u64 = files.iter().map(|f| f.1.len() as u64).sum::<u64>() + json.len() as u64;
            let fits = json.len() as u64 <= ASSET_LIMIT && files.iter().all(|f| f.1.len() as u64 <= ASSET_LIMIT) && total <= BUNDLE_LIMIT && files.len() <= MAX_BUNDLE_FILES;
            if fits {
                if let Some(m) = max {
                    self.log.push(format!("{short}: textures shrunk to {m} px so it fits through Team Create"));
                }
                return Msg::AssetData { path: key.to_string(), data: b64_encode(&json), extra: files.iter().map(|(n, d)| (n.clone(), b64_encode(d))).collect() };
            }
        }
        self.log.push(format!("{short} is too big to send (its mesh alone is over {} MB)", BUNDLE_LIMIT / 1024 / 1024));
        Msg::AssetMissing { path: key.to_string() }
    }

    fn store_asset(&self, path: &str, data: &str, extra: &[(String, String)]) -> Result<PathBuf, String> {
        if data.len() as u64 > ASSET_LIMIT * 4 / 3 + 8 {
            return Err("too large".into());
        }
        let name = sandbox_name(path).ok_or("file type not allowed")?;
        if is_gltf(path) {
            return self.store_gltf(path, &name, data, extra);
        }
        if !extra.is_empty() {
            if path_ext(path) != "obj" {
                return Err("only .obj and glTF models come with extra files".into());
            }
            let files = check_bundle(path, extra)?;
            // each model gets its own folder, so its .mtl / texture names can't clash
            let dir = self.cache_dir.join(format!("model_{:016x}", fnv(path.as_bytes())));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for (n, bytes) in files {
                std::fs::write(dir.join(n), bytes).map_err(|e| e.to_string())?;
            }
            let bytes = b64_decode(data).ok_or("damaged data")?;
            if bytes.len() as u64 > ASSET_LIMIT {
                return Err("too large".into());
            }
            let out = dir.join(name);
            std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
            return Ok(out);
        }
        let bytes = b64_decode(data).ok_or("damaged data")?;
        if bytes.len() as u64 > ASSET_LIMIT {
            return Err("too large".into());
        }
        if matches!(path_ext(path).as_str(), "png" | "jpg" | "jpeg" | "bmp") && image::guess_format(&bytes).is_err() {
            return Err("not really an image".into());
        }
        std::fs::create_dir_all(&self.cache_dir).map_err(|e| e.to_string())?;
        let out = self.cache_dir.join(name);
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// A received glTF: either a plain .glb (older Anvil) or a .gltf whose
    /// every outside file is one of the checked files sent with it.
    fn store_gltf(&self, path: &str, name: &str, data: &str, extra: &[(String, String)]) -> Result<PathBuf, String> {
        let bytes = b64_decode(data).ok_or("damaged data")?;
        if bytes.len() as u64 > ASSET_LIMIT {
            return Err("too large".into());
        }
        let dir = self.cache_dir.join(format!("model_{:016x}", fnv(path.as_bytes())));
        let stem = name.rsplit_once('.').map(|x| x.0).unwrap_or(name);
        let (out_name, files) = if bytes.starts_with(b"glTF") {
            if !extra.is_empty() {
                return Err("a .glb doesn't come with extra files".into());
            }
            (format!("{stem}.glb"), Vec::new())
        } else {
            let files = check_bundle(path, extra)?;
            check_gltf(&bytes, &files)?;
            (format!("{stem}.gltf"), files)
        };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (n, b) in files {
            std::fs::write(dir.join(n), b).map_err(|e| e.to_string())?;
        }
        let out = dir.join(out_name);
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
        Ok(out)
    }

    fn request_assets(&mut self, map: &Map) {
        if self.asked.len() >= MAX_ASKS {
            return;
        }
        let mut want: Vec<(String, u32)> = map_paths(map)
            .into_iter()
            .filter(|p| !self.remap.contains_key(p) && !self.asked.contains(p) && !self.missing.contains(p))
            .filter_map(|p| self.remote_paths.get(&p).map(|o| (p, *o)))
            .collect();
        want.sort();
        for (p, origin) in want {
            if self.asked.len() >= MAX_ASKS {
                break;
            }
            let target = if self.host { origin } else { HOST_ID };
            if !self.links.contains_key(&target) {
                continue;
            }
            self.asked.insert(p.clone());
            self.send_to(target, &line(&Msg::AssetReq { path: p }));
        }
    }

    fn sync_out(&mut self, map: &Map) {
        for p in map_paths(map) {
            if !self.remote_paths.contains_key(&p) {
                self.local_paths.insert(p);
            }
        }
        let mut ups = Vec::new();
        let mut seen = HashSet::with_capacity(map.objects.len());
        for o in &map.objects {
            seen.insert(o.id);
            let h = obj_hash(o);
            if self.sent.get(&o.id) != Some(&h) {
                self.sent.insert(o.id, h);
                ups.push(o.clone());
            }
        }
        let gone: Vec<u64> = self.sent.keys().filter(|id| !seen.contains(id)).copied().collect();
        for id in &gone {
            self.sent.remove(id);
        }
        let mh = meta_hash(map);
        if mh != self.meta_hash {
            self.meta_hash = mh;
            self.seq += 1;
            self.inflight_meta = self.seq;
            let msg = Msg::Meta { from: self.me.id, seq: self.seq, name: map.name.clone(), folders: map.folders.clone(), textures: map.textures.clone() };
            self.send_out(&line(&msg));
        }
        for chunk in ups.chunks(256) {
            self.seq += 1;
            for o in chunk {
                self.inflight.insert(o.id, self.seq);
            }
            let msg = Msg::Upsert { from: self.me.id, seq: self.seq, objects: chunk.to_vec() };
            self.send_out(&line(&msg));
        }
        if !gone.is_empty() {
            self.seq += 1;
            for id in &gone {
                self.inflight.insert(*id, self.seq);
            }
            let msg = Msg::Remove { from: self.me.id, seq: self.seq, ids: gone };
            self.send_out(&line(&msg));
        }
        if self.host {
            self.inflight.clear();
        }
    }

    fn send_out(&mut self, l: &Arc<str>) {
        if self.host {
            self.broadcast(l, None);
        } else {
            self.send_to(HOST_ID, l);
        }
    }

    fn presence_out(&mut self, p: &Presence, now: Instant) {
        let h = fnv(&serde_json::to_vec(p).unwrap_or_default());
        let stale = self.last_presence.0.is_none_or(|t| now - t > Duration::from_secs(2));
        if h != self.last_presence.1 || stale {
            self.last_presence = (Some(now), h);
            self.send_out(&line(&Msg::Presence { peer: self.me.id, p: p.clone() }));
        }
    }

    /// Everyone's selections in their color (for the renderer).
    pub fn remote_selection(&self) -> HashMap<u64, [u8; 3]> {
        let mut m = HashMap::new();
        for r in self.peers.values() {
            if let Some(p) = &r.presence {
                for id in &p.sel {
                    m.insert(*id, r.info.color);
                }
            }
        }
        m
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave("The session ended.");
    }
}

/// Where downloaded Team Create files go (next to anvil.exe).
pub fn default_cache_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("team_files")
}

/// After leaving a session: plain ids again, continuing after the highest one.
pub fn end_ranges(map: &mut Map) {
    let max = map.objects.iter().map(|o| o.id).chain(map.folders.iter().map(|f| f.id)).chain(map.textures.iter().map(|t| t.id)).max().unwrap_or(0);
    map.next_id = map.next_id.max(max);
    map.id_base = 0;
    map.local_next = 0;
}
