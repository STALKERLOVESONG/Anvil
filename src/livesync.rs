//! Experimental live sync: Roblox Studio and Blender mirror the open map as
//! you edit it (one way, Anvil -> them).
//!
//! Anvil serves read-only JSON over HTTP on 127.0.0.1 only. Every request
//! needs the secret token, the Host header must be localhost (so a web page
//! can't reach it through DNS rebinding), and no CORS headers are sent, so
//! browsers won't hand the data to websites. The Studio plugin and Blender
//! add-on are generated with the port and token baked in.

use crate::app::AnvilApp;
use crate::map::{Kind, Map, Object};
use crate::mesh::{world_mesh, ModelCache};
use crate::settings::Settings;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 47700;
const MAX_IDS: usize = 200;

#[derive(Default)]
struct Entry {
    hash: u64,
    json: String,
}

/// What the server hands out (updated by the editor a few times a second).
#[derive(Default)]
pub struct LiveData {
    pub version: u64,
    roblox: BTreeMap<u64, Entry>,
    blender: BTreeMap<u64, Entry>,
}

pub struct LiveServer {
    pub port: u16,
    pub data: Arc<Mutex<LiveData>>,
    pub requests: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    /// object id -> hash of what was last published
    seen: BTreeMap<u64, u64>,
    last: Option<Instant>,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100_0000_01b3))
}

/// A fresh random token (16 characters).
pub fn new_token() -> String {
    let a = crate::team::new_invite_code();
    let b = crate::team::new_invite_code();
    format!("{a}{b}").to_lowercase()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn respond(mut s: &TcpStream, code: &str, body: &str) {
    let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = s.flush();
}

/// Handles one request. Only GET, only with the token, only for localhost.
pub fn handle(stream: TcpStream, data: &Mutex<LiveData>, token: &str, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let mut s = &stream;
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.len() > 16 * 1024 {
            respond(&stream, "431 Request Header Fields Too Large", "{}");
            return;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" {
        respond(&stream, "405 Method Not Allowed", "{}");
        return;
    }
    let host_ok = lines.filter_map(|l| l.split_once(':')).filter(|(k, _)| k.trim().eq_ignore_ascii_case("host")).any(|(_, v)| {
        let v = v.trim();
        v == format!("127.0.0.1:{port}") || v == format!("localhost:{port}") || v == "127.0.0.1" || v == "localhost"
    });
    if !host_ok {
        respond(&stream, "403 Forbidden", "{\"error\":\"bad host\"}");
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q: BTreeMap<&str, &str> = query.split('&').filter_map(|kv| kv.split_once('=')).collect();
    if !q.get("token").is_some_and(|t| same(t, token)) {
        respond(&stream, "403 Forbidden", "{\"error\":\"wrong token\"}");
        return;
    }
    let d = match data.lock() {
        Ok(d) => d,
        Err(_) => return,
    };
    let (kind, what) = path.trim_start_matches('/').split_once('/').unwrap_or(("", ""));
    let table = match kind {
        "roblox" => &d.roblox,
        "blender" => &d.blender,
        _ => {
            respond(&stream, "404 Not Found", "{}");
            return;
        }
    };
    match what {
        "state" => {
            let objs: Vec<String> = table.iter().map(|(id, e)| format!("{{\"id\":{id},\"hash\":\"{:016x}\"}}", e.hash)).collect();
            respond(&stream, "200 OK", &format!("{{\"version\":{},\"objects\":[{}]}}", d.version, objs.join(",")));
        }
        "objects" => {
            let ids: Vec<u64> = q.get("ids").unwrap_or(&"").split(',').filter_map(|x| x.parse().ok()).take(MAX_IDS).collect();
            let objs: Vec<&str> = ids.iter().filter_map(|id| table.get(id).map(|e| e.json.as_str())).collect();
            respond(&stream, "200 OK", &format!("{{\"objects\":[{}]}}", objs.join(",")));
        }
        _ => respond(&stream, "404 Not Found", "{}"),
    }
}

impl LiveServer {
    pub fn start(port: u16, token: String) -> Result<LiveServer, String> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("Couldn't use port {port}: {e}"))?;
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let _ = listener.set_nonblocking(true);
        let data = Arc::new(Mutex::new(LiveData::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicU64::new(0));
        let (d, st, rq) = (data.clone(), stop.clone(), requests.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, addr)) => {
                        if !addr.ip().is_loopback() {
                            continue;
                        }
                        let _ = s.set_nonblocking(false);
                        rq.fetch_add(1, Ordering::Relaxed);
                        handle(s, &d, &token, port);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(30)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        Ok(LiveServer { port, data, requests, stop, seen: BTreeMap::new(), last: None })
    }

    /// Publishes changed objects (call every frame; does work ~5x a second).
    pub fn update(&mut self, map: &Map, models: &mut ModelCache, settings: &Settings) {
        if self.last.is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
            return;
        }
        self.last = Some(Instant::now());
        let base = fnv(&serde_json::to_vec(&(&map.textures, &map.folders, settings.roblox_scale)).unwrap_or_default());
        let mut changed: Vec<(u64, u64, String, String)> = Vec::new();
        let mut alive = std::collections::BTreeSet::new();
        for o in map.objects.iter().filter(|o| !o.hidden) {
            alive.insert(o.id);
            let h = fnv(&serde_json::to_vec(o).unwrap_or_default()) ^ base;
            if self.seen.get(&o.id) == Some(&h) {
                continue;
            }
            self.seen.insert(o.id, h);
            changed.push((o.id, h, roblox_json(map, o, models, settings), blender_json(map, o, models)));
        }
        let gone: Vec<u64> = self.seen.keys().filter(|id| !alive.contains(id)).copied().collect();
        if changed.is_empty() && gone.is_empty() {
            return;
        }
        let Ok(mut d) = self.data.lock() else { return };
        for id in &gone {
            self.seen.remove(id);
            d.roblox.remove(id);
            d.blender.remove(id);
        }
        for (id, h, r, b) in changed {
            d.roblox.insert(id, Entry { hash: h, json: r });
            d.blender.insert(id, Entry { hash: h, json: b });
        }
        d.version += 1;
    }
}

fn folder_names(map: &Map, o: &Object) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = o.folder;
    while let Some(f) = cur.and_then(|id| map.folder(id)) {
        out.push(f.name.clone());
        cur = f.parent;
        if out.len() > 32 {
            break;
        }
    }
    out.reverse();
    out
}

pub fn roblox_json(map: &Map, o: &Object, models: &mut ModelCache, settings: &Settings) -> String {
    let nodes: Vec<serde_json::Value> = crate::export_roblox::object_nodes_for(map, o, models, settings).iter().map(crate::export_roblox::node_json).filter(|v| !v.is_null()).collect();
    serde_json::json!({"id": o.id, "path": folder_names(map, o), "nodes": nodes}).to_string()
}

/// World-space mesh in Blender's axes (Z up): (x, y, z) -> (x, -z, y).
pub fn blender_json(map: &Map, o: &Object, models: &mut ModelCache) -> String {
    let wm = world_mesh(o, &map.textures, models);
    let verts: Vec<[f32; 3]> = wm.pos.iter().map(|p| [p.x, -p.z, p.y]).collect();
    let faces: Vec<[u32; 3]> = if matches!(o.kind, Kind::Light { .. } | Kind::Prompt { .. } | Kind::Target {}) { Vec::new() } else { wm.mesh.tris.clone() };
    let tex = map.texture(o.texture).map(|t| t.image.clone()).unwrap_or_default();
    serde_json::json!({
        "id": o.id, "name": o.name, "path": folder_names(map, o), "verts": verts, "faces": faces,
        "color": [o.color[0] as f32 / 255.0, o.color[1] as f32 / 255.0, o.color[2] as f32 / 255.0, 1.0 - o.transparency],
        "kind": o.type_label(), "pos": [o.pos.x, -o.pos.z, o.pos.y], "texture": tex
    })
    .to_string()
}

/// The Roblox Studio plugin (a single .lua file for the Plugins folder).
pub fn studio_plugin(port: u16, token: &str) -> String {
    STUDIO_PLUGIN.replace("__PORT__", &port.to_string()).replace("__TOKEN__", token)
}

/// The Blender add-on (Edit > Preferences > Add-ons > Install..., pick this .py).
pub fn blender_addon(port: u16, token: &str) -> String {
    BLENDER_ADDON.replace("__PORT__", &port.to_string()).replace("__TOKEN__", token)
}

const STUDIO_PLUGIN: &str = r#"-- Anvil Live Sync (Roblox Studio plugin, made by Anvil)
-- Mirrors the map open in Anvil into workspace.AnvilLive while it's running.
-- Click the "Live sync" button in the Plugins tab to start / stop.
-- Studio asks once whether this plugin may talk to 127.0.0.1: allow it.

local HttpService = game:GetService("HttpService")
local BASE = "http://127.0.0.1:__PORT__/roblox/"
local TOKEN = "__TOKEN__"

local toolbar = plugin:CreateToolbar("Anvil")
local button = toolbar:CreateButton("Live sync", "Mirror the map open in Anvil into this place", "rbxasset://textures/StudioToolbox/AssetPreview/play.png")
button.ClickableWhenViewportHidden = true

local running = false
local known = {} -- id -> { hash = string, insts = { Instance } }

local function get(what)
    local sep = string.find(what, "?", 1, true) and "&" or "?"
    local ok, res = pcall(function()
        return HttpService:GetAsync(BASE .. what .. sep .. "token=" .. TOKEN, true)
    end)
    if not ok then return nil, res end
    local ok2, data = pcall(function() return HttpService:JSONDecode(res) end)
    if not ok2 then return nil, data end
    return data
end

local function root()
    local r = workspace:FindFirstChild("AnvilLive")
    if not r then
        r = Instance.new("Model")
        r.Name = "AnvilLive"
        r.Parent = workspace
    end
    return r
end

local function folder(path)
    local cur = root()
    for _, name in ipairs(path) do
        local nxt = cur:FindFirstChild(name)
        if not nxt then
            nxt = Instance.new("Model")
            nxt.Name = name
            nxt.Parent = cur
        end
        cur = nxt
    end
    return cur
end

local function v3(t) return Vector3.new(t[1], t[2], t[3]) end

local function child(c, parent)
    local i
    if c.t == "SpecialMesh" then
        i = Instance.new("SpecialMesh")
        i.MeshType = Enum.MeshType[c.type] or Enum.MeshType.Sphere
        if c.id then i.MeshId = c.id end
        i.Scale = v3(c.scale)
    elseif c.t == "PointLight" then
        i = Instance.new("PointLight")
        i.Range = c.range
        i.Brightness = c.brightness
        i.Color = Color3.fromRGB(c.color[1], c.color[2], c.color[3])
        i.Shadows = true
    elseif c.t == "StringValue" then
        i = Instance.new("StringValue")
        i.Name = c.name
        i.Value = c.value
    elseif c.t == "Texture" then
        i = Instance.new("Texture")
        i.Face = Enum.NormalId[c.face]
        i.Texture = c.id
        i.StudsPerTileU = c.su
        i.StudsPerTileV = c.sv
        i.OffsetStudsU = c.ou
        i.OffsetStudsV = c.ov
        i.Color3 = Color3.fromRGB(c.color[1], c.color[2], c.color[3])
    elseif c.t == "Decal" then
        i = Instance.new("Decal")
        i.Face = Enum.NormalId[c.face]
        i.Texture = c.id
        i.Transparency = c.transparency
    elseif c.t == "ProximityPrompt" then
        i = Instance.new("ProximityPrompt")
        i.ActionText = c.action
        i.ObjectText = c.object
        pcall(function() i.KeyboardKeyCode = Enum.KeyCode[c.key] end)
        pcall(function() i.GamepadKeyCode = Enum.KeyCode[c.pad] end)
        i.HoldDuration = c.hold
        i.MaxActivationDistance = c.distance
        i.RequiresLineOfSight = c.los
        i.Enabled = c.enabled
        i.Style = c.custom and Enum.ProximityPromptStyle.Custom or Enum.ProximityPromptStyle.Default
        i.UIOffset = Vector2.new(c.offset[1], c.offset[2])
    elseif c.t == "Script" then
        i = Instance.new("Script")
        i.Name = c.name
        i.Source = c.source
    end
    if i then i.Parent = parent end
end

local function build(n, parent, out)
    if n.t == "Model" then
        local m = Instance.new("Model")
        m.Name = n.name
        for _, c in ipairs(n.children) do build(c, m, nil) end
        m.Parent = parent
        if out then table.insert(out, m) end
    elseif n.t == "Part" then
        local ok, p = pcall(function() return Instance.new(n.class) end)
        if not ok then p = Instance.new("Part") end
        p.Name = n.name
        if n.shape and p:IsA("Part") then p.Shape = Enum.PartType[n.shape] end
        p.Anchored = n.anchored
        p.CanCollide = n.collide
        p.Size = v3(n.size)
        local c = n.cf
        p.CFrame = CFrame.new(c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9], c[10], c[11], c[12])
        p.Color = Color3.fromRGB(n.color[1], n.color[2], n.color[3])
        pcall(function() p.Material = Enum.Material[n.material] end)
        if n.variant then p.MaterialVariant = n.variant end
        p.Transparency = n.transparency
        p.TopSurface = Enum.SurfaceType.Smooth
        p.BottomSurface = Enum.SurfaceType.Smooth
        for _, ch in ipairs(n.children) do child(ch, p) end
        p.Parent = parent
        if out then table.insert(out, p) end
    end
end

local function clear(id)
    local k = known[id]
    if k then
        for _, i in ipairs(k.insts) do i:Destroy() end
        known[id] = nil
    end
end

local function sync()
    local st, err = get("state")
    if not st then
        warn("Anvil live sync: can't reach Anvil (" .. tostring(err) .. ")")
        return false
    end
    local want, need = {}, {}
    for _, o in ipairs(st.objects) do
        local id = tostring(o.id)
        want[id] = o.hash
        if not known[id] or known[id].hash ~= o.hash then table.insert(need, id) end
    end
    for id in pairs(known) do
        if not want[id] then clear(id) end
    end
    local i = 1
    while i <= #need do
        local batch = {}
        for j = i, math.min(i + 49, #need) do table.insert(batch, need[j]) end
        i = i + 50
        local data = get("objects?ids=" .. table.concat(batch, ","))
        if data then
            for _, o in ipairs(data.objects) do
                local id = tostring(o.id)
                clear(id)
                local insts = {}
                local parent = folder(o.path)
                for _, n in ipairs(o.nodes) do build(n, parent, insts) end
                known[id] = { hash = want[id], insts = insts }
            end
        end
    end
    return true
end

button.Click:Connect(function()
    running = not running
    button:SetActive(running)
    if running then
        print("Anvil live sync: started (127.0.0.1:__PORT__)")
        task.spawn(function()
            while running do
                if not sync() then task.wait(2) end
                task.wait(0.3)
            end
        end)
    else
        print("Anvil live sync: stopped")
    end
end)
"#;

const BLENDER_ADDON: &str = r#"# Anvil Live Sync (Blender add-on, made by Anvil)
# Install: Edit > Preferences > Add-ons > Install..., pick this file, tick it.
# Then in the 3D view sidebar (N) > Anvil > Start live sync.
# Mirrors the map open in Anvil into the "Anvil Live" collection.

bl_info = {
    "name": "Anvil Live Sync",
    "author": "Anvil",
    "version": (1, 0),
    "blender": (3, 0, 0),
    "location": "View3D > Sidebar > Anvil",
    "description": "Mirror the map open in the Anvil level editor (one way)",
    "category": "Import-Export",
}

import json
import urllib.request

import bpy

BASE = "http://127.0.0.1:__PORT__/blender/"
TOKEN = "__TOKEN__"
_state = {"running": False, "known": {}}


def _get(what):
    sep = "&" if "?" in what else "?"
    with urllib.request.urlopen(BASE + what + sep + "token=" + TOKEN, timeout=2) as r:
        return json.loads(r.read().decode("utf-8"))


def _collection(path):
    root = bpy.data.collections.get("Anvil Live")
    if root is None:
        root = bpy.data.collections.new("Anvil Live")
        bpy.context.scene.collection.children.link(root)
    cur = root
    for name in path:
        key = "Anvil " + name
        nxt = cur.children.get(key)
        if nxt is None:
            nxt = bpy.data.collections.new(key)
            cur.children.link(nxt)
        cur = nxt
    return cur


def _material(rgba):
    name = "Anvil_%02x%02x%02x" % tuple(int(max(0.0, min(1.0, c)) * 255) for c in rgba[:3])
    m = bpy.data.materials.get(name)
    if m is None:
        m = bpy.data.materials.new(name)
        m.diffuse_color = rgba
        m.use_nodes = True
        bsdf = m.node_tree.nodes.get("Principled BSDF")
        if bsdf:
            bsdf.inputs["Base Color"].default_value = (rgba[0], rgba[1], rgba[2], 1.0)
    return m


def _remove(oid):
    ob = bpy.data.objects.get("anvil_%s" % oid)
    if ob is not None:
        me = ob.data
        bpy.data.objects.remove(ob, do_unlink=True)
        if me is not None and me.users == 0:
            bpy.data.meshes.remove(me)
    _state["known"].pop(oid, None)


def _apply(o):
    oid = str(o["id"])
    name = "anvil_%s" % oid
    me = bpy.data.meshes.get(name) or bpy.data.meshes.new(name)
    me.clear_geometry()
    me.from_pydata(o["verts"], [], o["faces"])
    me.update()
    me.materials.clear()
    me.materials.append(_material(o["color"]))
    ob = bpy.data.objects.get(name)
    if ob is None:
        ob = bpy.data.objects.new(name, me)
    ob["anvil_name"] = o["name"]
    coll = _collection(o["path"])
    for c in list(ob.users_collection):
        if c != coll:
            c.objects.unlink(ob)
    if coll not in ob.users_collection:
        coll.objects.link(ob)


def _sync():
    st = _get("state")
    want = {str(o["id"]): o["hash"] for o in st["objects"]}
    for oid in list(_state["known"].keys()):
        if oid not in want:
            _remove(oid)
    need = [oid for oid, h in want.items() if _state["known"].get(oid) != h]
    for i in range(0, len(need), 50):
        data = _get("objects?ids=" + ",".join(need[i:i + 50]))
        for o in data["objects"]:
            _apply(o)
            _state["known"][str(o["id"])] = want.get(str(o["id"]))


def _tick():
    if not _state["running"]:
        return None
    try:
        _sync()
    except Exception as e:  # Anvil closed, etc.
        print("Anvil live sync:", e)
        return 2.0
    return 0.4


class ANVIL_OT_live(bpy.types.Operator):
    bl_idname = "anvil.live_sync"
    bl_label = "Start / stop Anvil live sync"

    def execute(self, context):
        _state["running"] = not _state["running"]
        if _state["running"]:
            bpy.app.timers.register(_tick)
        return {"FINISHED"}


class ANVIL_PT_panel(bpy.types.Panel):
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Anvil"
    bl_label = "Anvil live sync"

    def draw(self, context):
        on = _state["running"]
        self.layout.operator("anvil.live_sync", text="Stop live sync" if on else "Start live sync", icon="PAUSE" if on else "PLAY")
        self.layout.label(text="127.0.0.1:__PORT__")


def register():
    bpy.utils.register_class(ANVIL_OT_live)
    bpy.utils.register_class(ANVIL_PT_panel)


def unregister():
    _state["running"] = False
    bpy.utils.unregister_class(ANVIL_PT_panel)
    bpy.utils.unregister_class(ANVIL_OT_live)
"#;

impl AnvilApp {
    pub(crate) fn poll_live(&mut self) {
        if !self.settings.live_sync {
            self.live = None;
            return;
        }
        if self.settings.live_token.len() < 16 {
            self.settings.live_token = new_token();
            let _ = self.settings.save();
        }
        if self.live.as_ref().is_none_or(|l| l.port != self.settings.live_port) && self.live_error.is_empty() {
            match LiveServer::start(self.settings.live_port, self.settings.live_token.clone()) {
                Ok(s) => self.live = Some(s),
                Err(e) => self.live_error = e,
            }
        }
        if let Some(l) = &mut self.live {
            l.update(&self.map, &mut self.models, &self.settings);
        }
    }

    pub(crate) fn live_settings_ui(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{self, RichText};
        ui.label(RichText::new("Experimental: Roblox Studio and Blender mirror the map open in Anvil as you edit it (one way). Only programs on this PC can connect, and only with the secret token built into the plugin / add-on.").small().weak());
        let mut ch = false;
        ch |= ui.checkbox(&mut self.settings.live_sync, "Live sync on").changed();
        ui.horizontal(|ui| {
            ui.label("Port");
            ch |= ui.add(egui::DragValue::new(&mut self.settings.live_port).range(1024..=65535)).changed();
            if ui.small_button("New token").on_hover_text("Makes the old plugin / add-on stop working; install them again").clicked() {
                self.settings.live_token = new_token();
                self.live = None;
                ch = true;
            }
        });
        if ch {
            self.live_error.clear();
            let _ = self.settings.save();
        }
        if !self.live_error.is_empty() {
            ui.label(RichText::new(&self.live_error).color(egui::Color32::from_rgb(255, 170, 60)));
        }
        if let Some(l) = &self.live {
            ui.label(format!("Serving on 127.0.0.1:{} · {} requests so far", l.port, l.requests.load(Ordering::Relaxed)));
        }
        ui.horizontal(|ui| {
            if ui.button("Install Roblox Studio plugin").on_hover_text("Writes AnvilLiveSync.lua into your Roblox Plugins folder").clicked() {
                self.status = match install_studio_plugin(self.settings.live_port, &self.settings.live_token) {
                    Ok(p) => format!("Installed the Studio plugin to {p}. Restart Studio, then click Plugins > Live sync."),
                    Err(e) => e,
                };
            }
            if ui.button("Save Blender add-on…").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_file_name("anvil_live_sync.py").add_filter("Python", &["py"]).save_file() {
                    self.status = match std::fs::write(&p, blender_addon(self.settings.live_port, &self.settings.live_token)) {
                        Ok(()) => format!("Saved {}. In Blender: Edit > Preferences > Add-ons > Install…", p.display()),
                        Err(e) => format!("Couldn't save: {e}"),
                    };
                }
            }
        });
    }
}

fn install_studio_plugin(port: u16, token: &str) -> Result<String, String> {
    let base = std::env::var("LOCALAPPDATA").map_err(|_| "Can't find your AppData folder".to_string())?;
    let dir = std::path::Path::new(&base).join("Roblox").join("Plugins");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let p = dir.join("AnvilLiveSync.lua");
    std::fs::write(&p, studio_plugin(port, token)).map_err(|e| format!("Couldn't write {}: {e}", p.display()))?;
    Ok(p.display().to_string())
}
