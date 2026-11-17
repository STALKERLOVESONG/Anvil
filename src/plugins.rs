//! Plugin system. Plugins are Rhai scripts (https://rhai.rs) in the
//! `plugins` folder next to the executable. Each one defines `fn info()`
//! returning `#{ name, description, kind }` where kind is:
//!
//! * `"command"` – `fn run()` runs once from the Plugins window
//! * `"tool"`    – `fn on_click(hit)` is called for every click in a viewport
//! * `"move"`    – `fn adjust_move(ids, box, delta)` can change the drag
//!   delta while the selection is being moved (return the new delta)
//! * `"gizmo"`   – draws handles on the selection (`fn draw()`) and owns the
//!   drags that start on them: `fn pointer_down(ev) -> bool` (return true to
//!   take the drag), `fn pointer_drag(ev)`, `fn pointer_up(ev)`; the editor's
//!   move/rotate/scale mode and local/global space come from
//!   `gizmo_mode()` / `gizmo_local()`
//!
//! Scripts edit the map through the functions registered below; every
//! change a plugin makes is a single undo step.

use crate::map::{Kind, Map, Material, Object, Shape};
use crate::mesh::{world_mesh, ModelCache};
use crate::view2d::{View2D, ViewKind};
use crate::view3d::Camera;
use eframe::egui::{Pos2, Rect};
use glam::{Mat3, Quat, Vec3};
use rhai::{Array, Dynamic, Engine, Map as RMap, Scope, AST, FLOAT, INT};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const BUILTIN: [(&str, &str); 5] = [
    ("sketchfab.rhai", include_str!("../plugins/sketchfab.rhai")),
    ("transform_gizmo.rhai", include_str!("../plugins/transform_gizmo.rhai")),
    ("align_sizes.rhai", include_str!("../plugins/align_sizes.rhai")),
    ("snap_to_part.rhai", include_str!("../plugins/snap_to_part.rhai")),
    ("random_colors.rhai", include_str!("../plugins/random_colors.rhai")),
];

pub const TEMPLATE: &str = include_str!("../plugins/template.rhai");

/// FNV-1a hashes of older bundled plugin versions. An installed file that
/// still matches one of these was never edited, so it's safe to upgrade.
/// Earlier bundled versions (FNV-1a of the exact file). A plugins-folder copy
/// that matches one of these is replaced with the new version on start.
/// Every released version must be listed (a test checks the git tags).
pub(crate) const OLD_BUILTINS: [(&str, u64); 4] = [
    ("transform_gizmo.rhai", 0x732b_1b31_58d1_2ad5),
    ("transform_gizmo.rhai", 0x718d_635e_b156_c31e),
    // 0.2.0: saving the token failed (Rhai's trim() returns nothing)
    ("sketchfab.rhai", 0xf97e_6b8e_ce81_0d54),
    // 0.2.1: downloading failed (Rhai has no char is_alphanumeric)
    ("sketchfab.rhai", 0x9c62_f9af_0608_4fe4),
];

pub(crate) fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100_0000_01b3))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PluginKind {
    Command,
    Tool,
    Move,
    Gizmo,
    /// Has its own window (`panel()` / `panel_event(id, value)`).
    Panel,
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Command => "command",
            PluginKind::Tool => "tool",
            PluginKind::Move => "move hook",
            PluginKind::Gizmo => "gizmo",
            PluginKind::Panel => "panel",
        }
    }
}

/// The viewport a gizmo plugin is drawing in / being clicked in.
#[derive(Clone, Copy, Default)]
pub enum ViewInfo {
    #[default]
    None,
    Ortho { view: View2D, rect: Rect },
    Persp { cam: Camera, rect: Rect, fov: f32 },
}

impl ViewInfo {
    pub fn project(&self, p: Vec3) -> Option<Pos2> {
        match self {
            ViewInfo::None => None,
            ViewInfo::Ortho { view, rect } => Some(view.world_to_screen(*rect, p)),
            ViewInfo::Persp { cam, rect, fov } => cam.project(*rect, *fov, p),
        }
    }

    /// World units covered by one screen point at `p` (for constant-size handles).
    pub fn units_per_pixel(&self, p: Vec3) -> f32 {
        match self {
            ViewInfo::None => 0.1,
            ViewInfo::Ortho { view, .. } => 1.0 / view.zoom.max(1e-4),
            ViewInfo::Persp { cam, rect, fov } => cam.units_per_pixel(*rect, *fov, p),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            ViewInfo::None => "",
            ViewInfo::Ortho { view, .. } => match view.kind {
                ViewKind::Top => "top",
                ViewKind::Front => "front",
                ViewKind::Side => "side",
            },
            ViewInfo::Persp { .. } => "3d",
        }
    }

    /// Mouse ray: origin and direction into the scene.
    pub fn ray(&self, s: Pos2) -> (Vec3, Vec3) {
        match self {
            ViewInfo::None => (Vec3::ZERO, Vec3::NEG_Z),
            ViewInfo::Ortho { view, rect } => {
                let dir = match view.kind {
                    ViewKind::Top => Vec3::NEG_Y,
                    ViewKind::Front => Vec3::NEG_Z,
                    ViewKind::Side => Vec3::X,
                };
                let p = view.uv_to_world(view.screen_to_uv(*rect, s), 0.0);
                (p - dir * 10_000.0, dir)
            }
            ViewInfo::Persp { cam, rect, fov } => cam.ray(*rect, *fov, s),
        }
    }
}

/// World-space overlay drawn by gizmo plugins (always on top).
#[derive(Clone, Copy, Debug)]
pub enum DrawCmd {
    Line { a: Vec3, b: Vec3, color: [u8; 3], width: f32 },
    Point { p: Vec3, color: [u8; 3], size: f32, square: bool },
}

pub struct Plugin {
    pub file: String,
    pub name: String,
    pub description: String,
    pub kind: PluginKind,
    /// Websites (https hosts) this plugin may talk to, from its info().
    pub hosts: Vec<String>,
    /// Its info() says `programs: true`: it may ask you to pick folders and
    /// start programs inside them (asking you first, each new program).
    pub programs: bool,
    ast: AST,
}

#[derive(Default)]
struct Ctx {
    map: Map,
    models: ModelCache,
    bounds: HashMap<u64, (Vec3, Vec3)>,
    selection: Vec<u64>,
    grid: f32,
    changed: bool,
    sel_changed: bool,
    status: Option<String>,
    log: Vec<String>,
    state: HashMap<String, Dynamic>,
    plugin: String,
    view: ViewInfo,
    draw: Vec<DrawCmd>,
    touched: std::collections::HashSet<u64>,
    removed: Vec<u64>,
    gizmo_mode: String,
    gizmo_local: bool,
    hosts: Vec<String>,
    imports: Vec<(String, String)>,
    programs: bool,
    plugin_name: String,
}

/// What a plugin call changed.
#[derive(Default)]
pub struct Outcome {
    /// Objects the plugin added or changed (full new state).
    pub changed: Vec<Object>,
    /// Objects the plugin deleted.
    pub removed: Vec<u64>,
    pub next_id: u64,
    pub selection: Option<Vec<u64>>,
    pub status: Option<String>,
    /// Models the plugin wants in the library: (file, display name).
    pub imports: Vec<(String, String)>,
}

impl Outcome {
    pub fn has_changes(&self) -> bool {
        !self.changed.is_empty() || !self.removed.is_empty()
    }

    /// Applies the plugin's edits to a map.
    pub fn apply_to(&self, map: &mut Map) {
        for o in &self.changed {
            match map.get_mut(o.id) {
                Some(x) => *x = o.clone(),
                None => map.objects.push(o.clone()),
            }
        }
        if !self.removed.is_empty() {
            map.remove(&self.removed);
        }
        map.next_id = map.next_id.max(self.next_id);
    }
}

/// Information about a viewport click handed to tool plugins.
pub struct Hit {
    pub id: u64,
    pub point: Vec3,
    pub normal: Vec3,
    pub edge: (Vec3, Vec3),
}

pub struct PluginHost {
    engine: Engine,
    ctx: Rc<RefCell<Ctx>>,
    pub plugins: Vec<Plugin>,
    pub log: Vec<String>,
    pub dir: PathBuf,
    /// Bundled plugins written to disk during this launch (first run / new version).
    pub newly_installed: Vec<String>,
    /// "file:hash" of plugins the user allowed to run.
    trusted: std::collections::HashSet<String>,
    /// New or changed plugins waiting for the user's OK (not loaded).
    pub pending: Vec<PendingPlugin>,
}

/// A plugin file nobody has approved yet.
#[derive(Clone, Debug)]
pub struct PendingPlugin {
    pub file: String,
    pub key: String,
    pub preview: String,
}

/// The allow-list key for a plugin file's exact contents.
pub fn trust_key(file: &str, bytes: &[u8]) -> String {
    format!("{file}:{:016x}", fnv(bytes))
}

// ----- conversions -----------------------------------------------------------

fn num(d: &Dynamic) -> f32 {
    d.as_float().map(|f| f as f32).or_else(|_| d.as_int().map(|i| i as f32)).unwrap_or(0.0)
}

fn v3(a: &Array) -> Vec3 {
    Vec3::new(a.first().map(num).unwrap_or(0.0), a.get(1).map(num).unwrap_or(0.0), a.get(2).map(num).unwrap_or(0.0))
}

fn arr(v: Vec3) -> Array {
    vec![Dynamic::from(v.x as FLOAT), Dynamic::from(v.y as FLOAT), Dynamic::from(v.z as FLOAT)]
}

fn col3(a: &Array) -> [u8; 3] {
    let v = v3(a);
    [v.x.clamp(0.0, 255.0) as u8, v.y.clamp(0.0, 255.0) as u8, v.z.clamp(0.0, 255.0) as u8]
}

fn p2(a: &Array) -> Pos2 {
    Pos2::new(a.first().map(num).unwrap_or(0.0), a.get(1).map(num).unwrap_or(0.0))
}

fn arr2(p: Pos2) -> Array {
    vec![Dynamic::from(p.x as FLOAT), Dynamic::from(p.y as FLOAT)]
}

pub fn event_map(view: &ViewInfo, mouse: Pos2, shift: bool, ctrl: bool, alt: bool) -> RMap {
    let (o, d) = view.ray(mouse);
    let mut m = RMap::new();
    m.insert("view".into(), Dynamic::from(view.name().to_string()));
    m.insert("mouse".into(), Dynamic::from(arr2(mouse)));
    m.insert("origin".into(), Dynamic::from(arr(o)));
    m.insert("dir".into(), Dynamic::from(arr(d)));
    m.insert("shift".into(), Dynamic::from(shift));
    m.insert("ctrl".into(), Dynamic::from(ctrl));
    m.insert("alt".into(), Dynamic::from(alt));
    m
}

fn dyn_v3(d: &Dynamic) -> Vec3 {
    d.clone().into_array().map(|a| v3(&a)).unwrap_or(Vec3::ZERO)
}

fn bounds_map(mn: Vec3, mx: Vec3) -> RMap {
    let mut m = RMap::new();
    m.insert("min".into(), Dynamic::from(arr(mn)));
    m.insert("max".into(), Dynamic::from(arr(mx)));
    m
}

/// Local-space size of a mesh's vertices.
fn mesh_extent(verts: &[Vec3]) -> Vec3 {
    if verts.is_empty() {
        return Vec3::ZERO;
    }
    let (mn, mx) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    mx - mn
}

fn object_bounds(c: &mut Ctx, id: u64) -> Option<(Vec3, Vec3)> {
    if let Some(b) = c.bounds.get(&id) {
        return Some(*b);
    }
    let Ctx { map, models, bounds, .. } = c;
    let o = map.get(id)?;
    let wm = world_mesh(o, &map.textures, models);
    bounds.insert(id, (wm.min, wm.max));
    Some((wm.min, wm.max))
}

fn modify(c: &mut Ctx, id: u64, f: impl FnOnce(&mut Object)) {
    if let Some(o) = c.map.get_mut(id) {
        f(o);
        c.changed = true;
        c.touched.insert(id);
        c.bounds.remove(&id);
    }
}

// ----- engine ----------------------------------------------------------------

fn build_engine(ctx: &Rc<RefCell<Ctx>>) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(5_000_000);
    e.set_max_call_levels(64);

    let c = ctx.clone();
    e.on_print(move |s| c.borrow_mut().log.push(s.to_string()));
    let c = ctx.clone();
    e.on_debug(move |s, _, _| c.borrow_mut().log.push(format!("[debug] {s}")));

    // queries
    let c = ctx.clone();
    e.register_fn("objects", move || -> Array { c.borrow().map.objects.iter().filter(|o| !o.hidden).map(|o| Dynamic::from(o.id as INT)).collect() });
    let c = ctx.clone();
    e.register_fn("parts", move || -> Array {
        c.borrow()
            .map
            .objects
            .iter()
            .filter(|o| !o.hidden && o.collide && !matches!(o.kind, Kind::Light { .. } | Kind::Decal { .. }))
            .map(|o| Dynamic::from(o.id as INT))
            .collect()
    });
    let c = ctx.clone();
    e.register_fn("selection", move || -> Array { c.borrow().selection.iter().map(|id| Dynamic::from(*id as INT)).collect() });
    let c = ctx.clone();
    e.register_fn("set_selection", move |ids: Array| {
        let mut x = c.borrow_mut();
        x.selection = ids.iter().filter_map(|d| d.as_int().ok()).map(|i| i as u64).collect();
        x.sel_changed = true;
    });
    let c = ctx.clone();
    e.register_fn("exists", move |id: INT| c.borrow().map.get(id as u64).is_some());
    let c = ctx.clone();
    e.register_fn("get_name", move |id: INT| -> String { c.borrow().map.get(id as u64).map(|o| o.name.clone()).unwrap_or_default() });
    let c = ctx.clone();
    e.register_fn("get_type", move |id: INT| -> String { c.borrow().map.get(id as u64).map(|o| o.type_label().to_string()).unwrap_or_default() });
    let c = ctx.clone();
    e.register_fn("get_pos", move |id: INT| -> Array { arr(c.borrow().map.get(id as u64).map(|o| o.pos).unwrap_or_default()) });
    let c = ctx.clone();
    e.register_fn("get_rot", move |id: INT| -> Array { arr(c.borrow().map.get(id as u64).map(|o| o.rot).unwrap_or_default()) });
    let c = ctx.clone();
    e.register_fn("get_size", move |id: INT| -> Array {
        let mut x = c.borrow_mut();
        let size = match x.map.get(id as u64).map(|o| o.kind.clone()) {
            Some(Kind::Brush { size, .. }) | Some(Kind::Spawn { size }) => size,
            Some(Kind::Decal { size }) => Vec3::new(size.x, size.y, 0.0),
            Some(Kind::Prop { scale, .. }) => scale,
            Some(Kind::Mesh { verts, .. }) => mesh_extent(&verts),
            Some(_) => object_bounds(&mut x, id as u64).map(|(a, b)| b - a).unwrap_or_default(),
            None => Vec3::ZERO,
        };
        arr(size)
    });
    let c = ctx.clone();
    e.register_fn("get_color", move |id: INT| -> Array {
        let col = c.borrow().map.get(id as u64).map(|o| o.color).unwrap_or_default();
        col.iter().map(|v| Dynamic::from(*v as INT)).collect()
    });
    let c = ctx.clone();
    e.register_fn("bounds", move |id: INT| -> RMap {
        let mut x = c.borrow_mut();
        let (mn, mx) = object_bounds(&mut x, id as u64).unwrap_or_default();
        bounds_map(mn, mx)
    });
    let c = ctx.clone();
    e.register_fn("to_local", move |id: INT, p: Array| -> Array {
        arr(c.borrow().map.get(id as u64).map(|o| o.to_local(v3(&p))).unwrap_or_default())
    });
    let c = ctx.clone();
    e.register_fn("to_world", move |id: INT, p: Array| -> Array {
        arr(c.borrow().map.get(id as u64).map(|o| o.to_world(v3(&p))).unwrap_or_default())
    });
    let c = ctx.clone();
    e.register_fn("axis", move |id: INT, k: INT| -> Array {
        let r = c.borrow().map.get(id as u64).map(|o| Mat3::from_quat(o.rotation())).unwrap_or(Mat3::IDENTITY);
        arr(r.col((k.clamp(0, 2)) as usize))
    });
    let c = ctx.clone();
    e.register_fn("grid", move || -> FLOAT { c.borrow().grid as FLOAT });

    // edits
    let c = ctx.clone();
    e.register_fn("set_pos", move |id: INT, p: Array| modify(&mut c.borrow_mut(), id as u64, |o| o.pos = v3(&p)));
    let c = ctx.clone();
    e.register_fn("set_rot", move |id: INT, p: Array| modify(&mut c.borrow_mut(), id as u64, |o| o.rot = v3(&p)));
    let c = ctx.clone();
    e.register_fn("set_size", move |id: INT, p: Array| {
        let s = v3(&p).max(Vec3::splat(0.001));
        modify(&mut c.borrow_mut(), id as u64, |o| match &mut o.kind {
            Kind::Brush { size, .. } | Kind::Spawn { size } => *size = s,
            Kind::Decal { size } => *size = s.truncate(),
            Kind::Prop { scale, .. } => *scale = s,
            Kind::Mesh { verts, .. } => {
                // scale the vertices about the object's origin, per local axis
                let cur = mesh_extent(verts);
                let f = Vec3::new(
                    if cur.x > 1e-5 { s.x / cur.x } else { 1.0 },
                    if cur.y > 1e-5 { s.y / cur.y } else { 1.0 },
                    if cur.z > 1e-5 { s.z / cur.z } else { 1.0 },
                );
                for v in verts.iter_mut() {
                    *v *= f;
                }
            }
            _ => {}
        })
    });
    let c = ctx.clone();
    e.register_fn("set_name", move |id: INT, n: &str| {
        let n = n.to_string();
        modify(&mut c.borrow_mut(), id as u64, |o| o.name = n)
    });
    let c = ctx.clone();
    e.register_fn("set_color", move |id: INT, col: Array| {
        let v = v3(&col);
        modify(&mut c.borrow_mut(), id as u64, |o| o.color = [v.x.clamp(0.0, 255.0) as u8, v.y.clamp(0.0, 255.0) as u8, v.z.clamp(0.0, 255.0) as u8])
    });
    let c = ctx.clone();
    e.register_fn("set_material", move |id: INT, m: &str| {
        if let Some(mat) = Material::from_label(m) {
            modify(&mut c.borrow_mut(), id as u64, |o| o.material = mat);
        }
    });
    let c = ctx.clone();
    e.register_fn("add_box", move |p: Array, s: Array| -> INT {
        let mut x = c.borrow_mut();
        x.changed = true;
        let id = x.map.add(Object::brush(Shape::Box, v3(&s).max(Vec3::splat(0.01)), v3(&p), [163, 162, 165], Material::Plastic));
        x.touched.insert(id);
        id as INT
    });
    let c = ctx.clone();
    e.register_fn("delete", move |id: INT| {
        let mut x = c.borrow_mut();
        x.map.remove(&[id as u64]);
        x.selection.retain(|s| *s != id as u64);
        x.touched.remove(&(id as u64));
        x.removed.push(id as u64);
        x.changed = true;
    });
    let c = ctx.clone();
    e.register_fn("status", move |s: &str| c.borrow_mut().status = Some(s.to_string()));

    // web, files and secrets (fenced in: see plugin_web.rs)
    {
        use crate::plugin_web as w;
        let c = ctx.clone();
        e.register_fn("http_get", move |url: &str| -> String {
            let (hosts, _) = { let x = c.borrow(); (x.hosts.clone(), ()) };
            match w::http_get(url, &[], &hosts) {
                Ok(s) => s,
                Err(err) => {
                    c.borrow_mut().log.push(format!("http_get: {err}"));
                    String::new()
                }
            }
        });
        let c = ctx.clone();
        e.register_fn("http_get", move |url: &str, headers: RMap| -> String {
            let hosts = c.borrow().hosts.clone();
            match w::http_get(url, &w::headers_from(&headers), &hosts) {
                Ok(s) => s,
                Err(err) => {
                    c.borrow_mut().log.push(format!("http_get: {err}"));
                    String::new()
                }
            }
        });
        e.register_fn("json_parse", |s: &str| -> Dynamic { serde_json::from_str::<serde_json::Value>(s).map(|v| w::json_to_dynamic(&v)).unwrap_or(Dynamic::UNIT) });
        e.register_fn("url_encode", |s: &str| -> String { w::url_encode(s) });
        let c = ctx.clone();
        e.register_fn("plugin_folder", move || -> String { w::plugin_dir(&c.borrow().plugin).to_string_lossy().to_string() });
        let c = ctx.clone();
        e.register_fn("download", move |url: &str, name: &str| -> INT {
            let x = c.borrow();
            w::start_download(url.to_string(), name, Vec::new(), x.hosts.clone(), w::plugin_dir(&x.plugin)) as INT
        });
        let c = ctx.clone();
        e.register_fn("download", move |url: &str, name: &str, headers: RMap| -> INT {
            let x = c.borrow();
            w::start_download(url.to_string(), name, w::headers_from(&headers), x.hosts.clone(), w::plugin_dir(&x.plugin)) as INT
        });
        e.register_fn("job_status", |id: INT| -> String { w::job(id).0 });
        e.register_fn("job_file", |id: INT| -> String { w::job(id).1 });
        e.register_fn("job_progress", |id: INT| -> f64 { w::job(id).2 });
        let c = ctx.clone();
        e.register_fn("unzip", move |path: &str| -> String {
            let root = w::plugin_dir(&c.borrow().plugin);
            match w::unzip_in(std::path::Path::new(path), &root) {
                Ok(d) => d,
                Err(err) => {
                    c.borrow_mut().log.push(format!("unzip: {err}"));
                    String::new()
                }
            }
        });
        let c = ctx.clone();
        e.register_fn("find_files", move |dir: &str, ext: &str| -> Array {
            let root = w::plugin_dir(&c.borrow().plugin);
            w::find_files(std::path::Path::new(dir), ext, &root).into_iter().map(Dynamic::from).collect()
        });
        let c = ctx.clone();
        e.register_fn("import_model", move |path: &str, name: &str| -> bool {
            let root = w::plugin_dir(&c.borrow().plugin);
            let p = std::path::Path::new(path);
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if !w::inside(p, &root) || !["obj", "stl", "glb", "gltf"].contains(&ext.as_str()) {
                c.borrow_mut().log.push(format!("import_model: {path} isn't a model in this plugin's folder"));
                return false;
            }
            c.borrow_mut().imports.push((path.to_string(), name.chars().take(80).collect()));
            true
        });
        // folders you pick and programs you allow (only `programs: true` plugins)
        fn refuse(c: &std::rc::Rc<std::cell::RefCell<Ctx>>, what: &str) -> bool {
            let mut x = c.borrow_mut();
            if x.programs {
                return false;
            }
            x.log.push(format!("{what}: this plugin's info() doesn't say programs: true"));
            true
        }
        let c = ctx.clone();
        e.register_fn("pick_folder", move |title: &str| -> String {
            if refuse(&c, "pick_folder") {
                return String::new();
            }
            let plugin = c.borrow().plugin.clone();
            w::pick_folder(&plugin, title)
        });
        let c = ctx.clone();
        e.register_fn("granted_folders", move || -> Array { w::granted_folders(&c.borrow().plugin).into_iter().map(Dynamic::from).collect() });
        let c = ctx.clone();
        e.register_fn("exists", move |path: &str| -> bool { w::exists(&c.borrow().plugin, path) });
        fn run(c: &std::rc::Rc<std::cell::RefCell<Ctx>>, exe: &str, args: Array, env: RMap, cwd: &str) -> bool {
            if refuse(c, "run_program") {
                return false;
            }
            let (plugin, name) = {
                let x = c.borrow();
                (x.plugin.clone(), x.plugin_name.clone())
            };
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            match crate::plugin_web::run_program(&plugin, &name, exe, &args, &env, cwd) {
                Ok(()) => true,
                Err(err) => {
                    c.borrow_mut().log.push(format!("run_program: {err}"));
                    false
                }
            }
        }
        let c = ctx.clone();
        e.register_fn("run_program", move |exe: &str, args: Array| -> bool { run(&c, exe, args, RMap::new(), "") });
        let c = ctx.clone();
        e.register_fn("run_program", move |exe: &str, args: Array, env: RMap| -> bool { run(&c, exe, args, env, "") });
        let c = ctx.clone();
        e.register_fn("run_program", move |exe: &str, args: Array, env: RMap, cwd: &str| -> bool { run(&c, exe, args, env, cwd) });
        // the map written into the plugin's own folder, for an outside playtest
        let c = ctx.clone();
        e.register_fn("export_scene", move |name: &str, meters: f64| -> String {
            let mut guard = c.borrow_mut();
            let x = &mut *guard;
            let dir = w::plugin_dir(&x.plugin);
            let path = dir.join(w::safe_name(name));
            let bytes = crate::blender_export::scene_glb_with(&x.map, &mut x.models, meters as f32, true);
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, bytes)) {
                Ok(()) => path.to_string_lossy().to_string(),
                Err(err) => {
                    x.log.push(format!("export_scene: {err}"));
                    String::new()
                }
            }
        });
        let c = ctx.clone();
        e.register_fn("export_boxes", move |name: &str, meters: f64, props: bool| -> String {
            let mut guard = c.borrow_mut();
            let x = &mut *guard;
            let dir = w::plugin_dir(&x.plugin);
            let path = dir.join(w::safe_name(name));
            let bytes = crate::play_export::collision_gltf(&x.map, &mut x.models, meters as f32, props);
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, bytes)) {
                Ok(()) => path.to_string_lossy().to_string(),
                Err(err) => {
                    x.log.push(format!("export_boxes: {err}"));
                    String::new()
                }
            }
        });
        let c = ctx.clone();
        e.register_fn("secret_get", move |name: &str| -> String { w::secret_get(&c.borrow().plugin, name) });
        let c = ctx.clone();
        e.register_fn("secret_set", move |name: &str, value: &str| -> bool { w::secret_set(&c.borrow().plugin, name, value) });
    }

    // per-plugin persistent state
    let c = ctx.clone();
    e.register_fn("state_get", move |k: &str| -> Dynamic {
        let x = c.borrow();
        x.state.get(&format!("{}::{k}", x.plugin)).cloned().unwrap_or(Dynamic::UNIT)
    });
    let c = ctx.clone();
    e.register_fn("state_set", move |k: &str, v: Dynamic| {
        let mut x = c.borrow_mut();
        let key = format!("{}::{k}", x.plugin);
        x.state.insert(key, v);
    });

    // selection & transforms
    let c = ctx.clone();
    e.register_fn("selection_bounds", move || -> RMap {
        let mut x = c.borrow_mut();
        let ids = x.selection.clone();
        let mut out: Option<(Vec3, Vec3)> = None;
        for id in ids {
            if let Some((a, b)) = object_bounds(&mut x, id) {
                out = Some(match out {
                    None => (a, b),
                    Some((p, q)) => (p.min(a), q.max(b)),
                });
            }
        }
        let (mn, mx) = out.unwrap_or_default();
        bounds_map(mn, mx)
    });
    let c = ctx.clone();
    e.register_fn("rotate_world", move |id: INT, axis: Array, deg: FLOAT, pivot: Array| {
        let a = v3(&axis).normalize_or_zero();
        if a == Vec3::ZERO {
            return;
        }
        let q = Quat::from_axis_angle(a, (deg as f32).to_radians());
        let pv = v3(&pivot);
        modify(&mut c.borrow_mut(), id as u64, |o| {
            let r = q * o.rotation();
            o.set_rotation(r);
            o.pos = pv + q * (o.pos - pv);
        })
    });

    // transform gizmo state (owned by the app: toolbar + W/E/R/X keys)
    let c = ctx.clone();
    e.register_fn("gizmo_mode", move || -> String {
        let m = c.borrow().gizmo_mode.clone();
        if m.is_empty() {
            "move".into()
        } else {
            m
        }
    });
    let c = ctx.clone();
    e.register_fn("gizmo_local", move || -> bool { c.borrow().gizmo_local });

    // viewport (gizmo plugins)
    let c = ctx.clone();
    e.register_fn("view_name", move || -> String { c.borrow().view.name().to_string() });
    let c = ctx.clone();
    e.register_fn("project", move |p: Array| -> Dynamic {
        match c.borrow().view.project(v3(&p)) {
            Some(s) => Dynamic::from(arr2(s)),
            None => Dynamic::UNIT,
        }
    });
    let c = ctx.clone();
    e.register_fn("units_per_pixel", move |p: Array| -> FLOAT { c.borrow().view.units_per_pixel(v3(&p)) as FLOAT });
    let c = ctx.clone();
    e.register_fn("draw_line", move |a: Array, b: Array, col: Array, w: FLOAT| {
        c.borrow_mut().draw.push(DrawCmd::Line { a: v3(&a), b: v3(&b), color: col3(&col), width: w as f32 })
    });
    let c = ctx.clone();
    e.register_fn("draw_line", move |a: Array, b: Array, col: Array| {
        c.borrow_mut().draw.push(DrawCmd::Line { a: v3(&a), b: v3(&b), color: col3(&col), width: 2.0 })
    });
    let c = ctx.clone();
    e.register_fn("draw_point", move |p: Array, col: Array, size: FLOAT| {
        c.borrow_mut().draw.push(DrawCmd::Point { p: v3(&p), color: col3(&col), size: size as f32, square: false })
    });
    let c = ctx.clone();
    e.register_fn("draw_box", move |p: Array, col: Array, size: FLOAT| {
        c.borrow_mut().draw.push(DrawCmd::Point { p: v3(&p), color: col3(&col), size: size as f32, square: true })
    });
    e.register_fn("dist2d", |a: Array, b: Array| -> FLOAT { (p2(&a) - p2(&b)).length() as FLOAT });
    e.register_fn("seg_dist", |p: Array, a: Array, b: Array| -> FLOAT {
        crate::view2d::dist_to_segment(p2(&p), p2(&a), p2(&b)) as FLOAT
    });
    e.register_fn("atan2", |y: FLOAT, x: FLOAT| -> FLOAT { y.atan2(x) });
    e.register_fn("rad2deg", |r: FLOAT| -> FLOAT { r.to_degrees() });
    e.register_fn("deg2rad", |d: FLOAT| -> FLOAT { d.to_radians() });

    // vector helpers
    e.register_fn("vadd", |a: Array, b: Array| -> Array { arr(v3(&a) + v3(&b)) });
    e.register_fn("vsub", |a: Array, b: Array| -> Array { arr(v3(&a) - v3(&b)) });
    e.register_fn("vscale", |a: Array, s: FLOAT| -> Array { arr(v3(&a) * s as f32) });
    e.register_fn("vdot", |a: Array, b: Array| -> FLOAT { v3(&a).dot(v3(&b)) as FLOAT });
    e.register_fn("vcross", |a: Array, b: Array| -> Array { arr(v3(&a).cross(v3(&b))) });
    e.register_fn("vlen", |a: Array| -> FLOAT { v3(&a).length() as FLOAT });
    e.register_fn("vnorm", |a: Array| -> Array { arr(v3(&a).normalize_or_zero()) });
    e
}

impl PluginHost {
    pub fn new() -> Self {
        let ctx = Rc::new(RefCell::new(Ctx::default()));
        let engine = build_engine(&ctx);
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("plugins");
        let mut host = PluginHost { engine, ctx, plugins: Vec::new(), log: Vec::new(), dir, newly_installed: Vec::new(), trusted: Default::default(), pending: Vec::new() };
        host.install_builtins();
        host.reload();
        host
    }

    /// Like `new`, with the user's list of approved plugins.
    pub fn new_trusted(trusted: &[String]) -> Self {
        let mut h = Self::new();
        h.trusted = trusted.iter().cloned().collect();
        h.reload();
        h
    }

    /// The user said this plugin may run. Returns the key to remember.
    pub fn approve(&mut self, key: &str) -> String {
        self.trusted.insert(key.to_string());
        self.pending.retain(|p| p.key != key);
        key.to_string()
    }

    /// The user said no: forget it until the next reload.
    pub fn dismiss(&mut self, key: &str) {
        self.pending.retain(|p| p.key != key);
    }

    /// Bundled plugins and the template are always allowed.
    fn is_trusted(&self, file: &str, bytes: &[u8]) -> bool {
        BUILTIN.iter().any(|(n, src)| *n == file && src.as_bytes() == bytes) || bytes == TEMPLATE.as_bytes() || self.trusted.contains(&trust_key(file, bytes))
    }

    /// Writes the bundled plugins into the plugins folder if they're missing.
    pub fn install_builtins(&mut self) {
        let _ = std::fs::create_dir_all(&self.dir);
        for (name, src) in BUILTIN {
            let p = self.dir.join(name);
            if !p.exists() {
                if std::fs::write(&p, src).is_ok() {
                    self.newly_installed.push(name.to_string());
                }
            } else if let Ok(cur) = std::fs::read(&p) {
                let h = fnv(&cur);
                if cur != src.as_bytes() && OLD_BUILTINS.iter().any(|(n, old)| *n == name && *old == h) {
                    if std::fs::write(&p, src).is_ok() {
                        self.log.push(format!("Updated the bundled plugin {name} to the new version"));
                    }
                }
            }
        }
    }

    pub fn reload(&mut self) {
        self.plugins.clear();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "rhai")).collect())
            .unwrap_or_default();
        files.sort();
        self.pending.clear();
        for f in files {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let bytes = std::fs::read(&f).unwrap_or_default();
            if !self.is_trusted(&name, &bytes) {
                let preview: String = String::from_utf8_lossy(&bytes).lines().take(40).collect::<Vec<_>>().join("\n");
                self.pending.push(PendingPlugin { key: trust_key(&name, &bytes), file: name, preview });
                continue;
            }
            match self.load(&f) {
                Ok(p) => self.plugins.push(p),
                Err(e) => self.log.push(format!("{}: {e}", f.display())),
            }
        }
        self.log.push(format!("Loaded {} plugin(s) from {}", self.plugins.len(), self.dir.display()));
    }

    fn load(&mut self, path: &Path) -> Result<Plugin, String> {
        let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("plugin").to_string();
        let ast = self.engine.compile_file(path.to_path_buf()).map_err(|e| e.to_string())?;
        let mut scope = Scope::new();
        let info: RMap = self
            .engine
            .call_fn::<RMap>(&mut scope, &ast, "info", ())
            .map_err(|e| format!("info() failed: {e}"))?;
        let get = |k: &str| info.get(k).and_then(|v| v.clone().into_string().ok()).unwrap_or_default();
        let kind = match get("kind").to_ascii_lowercase().as_str() {
            "tool" => PluginKind::Tool,
            "move" => PluginKind::Move,
            "gizmo" => PluginKind::Gizmo,
            "panel" => PluginKind::Panel,
            _ => PluginKind::Command,
        };
        let hosts: Vec<String> = info
            .get("hosts")
            .and_then(|h| h.clone().try_cast::<Array>())
            .map(|a| a.iter().map(|x| x.to_string().to_ascii_lowercase()).filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')).take(16).collect())
            .unwrap_or_default();
        let name = if get("name").is_empty() { file.clone() } else { get("name") };
        let programs = info.get("programs").and_then(|v| v.as_bool().ok()).unwrap_or(false);
        Ok(Plugin { file, name, description: get("description"), kind, hosts, programs, ast })
    }

    pub fn new_plugin_file(&mut self) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let mut n = 1;
        loop {
            let p = self.dir.join(format!("my_plugin_{n}.rhai"));
            if !p.exists() {
                std::fs::write(&p, TEMPLATE).map_err(|e| e.to_string())?;
                return Ok(p);
            }
            n += 1;
        }
    }

    pub fn has_fn(&self, idx: usize, name: &str) -> bool {
        self.plugins.get(idx).is_some_and(|p| p.ast.iter_functions().any(|f| f.name == name))
    }

    /// Sets the transform gizmo state the app owns (W/E/R + local/global).
    pub fn set_gizmo(&mut self, mode: &str, local: bool) {
        let mut c = self.ctx.borrow_mut();
        c.gizmo_mode = mode.to_string();
        c.gizmo_local = local;
    }

    fn prepare(&self, idx: usize, map: &Map, models: &ModelCache, selection: &[u64], grid: f32, bounds: HashMap<u64, (Vec3, Vec3)>) {
        self.prepare_with(idx, map, models, selection, grid, bounds, false)
    }

    /// `selection_only`: give the script a map holding just the selected
    /// objects (gizmo calls run several times a frame; copying the whole
    /// map each time was a big source of lag).
    #[allow(clippy::too_many_arguments)]
    fn prepare_with(&self, idx: usize, map: &Map, models: &ModelCache, selection: &[u64], grid: f32, bounds: HashMap<u64, (Vec3, Vec3)>, selection_only: bool) {
        let mut c = self.ctx.borrow_mut();
        c.draw.clear();
        c.touched.clear();
        c.removed.clear();
        c.map = if selection_only {
            Map {
                objects: map.objects.iter().filter(|o| selection.contains(&o.id)).cloned().collect(),
                next_id: map.next_id,
                id_base: map.id_base,
                local_next: map.local_next,
                ..Map::default()
            }
        } else {
            map.clone()
        };
        c.models = models.clone();
        c.bounds = bounds;
        c.selection = selection.to_vec();
        c.grid = grid;
        c.changed = false;
        c.sel_changed = false;
        c.status = None;
        c.plugin = self.plugins.get(idx).map(|p| p.file.clone()).unwrap_or_default();
        c.hosts = self.plugins.get(idx).map(|p| p.hosts.clone()).unwrap_or_default();
        c.programs = self.plugins.get(idx).is_some_and(|p| p.programs);
        c.plugin_name = self.plugins.get(idx).map(|p| p.name.clone()).unwrap_or_default();
        c.imports.clear();
    }

    /// A panel plugin's window contents.
    pub fn panel(&mut self, idx: usize, map: &Map, models: &ModelCache, grid: f32) -> (Vec<crate::plugin_web::Widget>, Outcome) {
        self.prepare_with(idx, map, models, &[], grid, HashMap::new(), true);
        let r = self.call(idx, "panel", ());
        let out = self.finish();
        match r {
            Ok(d) => (crate::plugin_web::widgets(&d), out),
            Err(e) => {
                self.log.push(e.clone());
                (vec![crate::plugin_web::Widget::Label { text: e, strong: false, small: true }], out)
            }
        }
    }

    /// A button / input in a panel plugin's window was used.
    pub fn panel_event(&mut self, idx: usize, map: &Map, models: &ModelCache, grid: f32, id: &str, value: &str) -> Outcome {
        // the whole map: a button may export it (the per-frame panel() stays light)
        self.prepare_with(idx, map, models, &[], grid, HashMap::new(), false);
        let r = self.call(idx, "panel_event", (id.to_string(), value.to_string()));
        let mut out = self.finish();
        if let Err(e) = r {
            self.log.push(e.clone());
            out.status = Some(e);
        }
        out
    }

    fn finish(&mut self) -> Outcome {
        let mut c = self.ctx.borrow_mut();
        self.log.append(&mut c.log);
        let changed = c.touched.iter().filter_map(|id| c.map.get(*id).cloned()).collect();
        Outcome {
            changed,
            removed: std::mem::take(&mut c.removed),
            next_id: c.map.next_id,
            selection: (c.sel_changed || c.changed).then(|| c.selection.clone()),
            status: c.status.take(),
            imports: std::mem::take(&mut c.imports),
        }
    }

    fn call(&mut self, idx: usize, func: &str, args: impl rhai::FuncArgs) -> Result<Dynamic, String> {
        let p = self.plugins.get(idx).ok_or("no such plugin")?;
        let mut scope = Scope::new();
        let r = self.engine.call_fn::<Dynamic>(&mut scope, &p.ast, func, args);
        r.map_err(|e| format!("{} {func}(): {e}", p.name))
    }

    /// The first loaded plugin that runs playtests (`fn playtest()`).
    pub fn playtest_plugin(&self) -> Option<usize> {
        (0..self.plugins.len()).find(|i| self.has_fn(*i, "playtest"))
    }

    /// Runs a plugin's `playtest()` with the whole map.
    pub fn playtest(&mut self, idx: usize, map: &Map, models: &ModelCache, grid: f32) -> Outcome {
        self.prepare_with(idx, map, models, &[], grid, HashMap::new(), false);
        let r = self.call(idx, "playtest", ());
        let mut out = self.finish();
        if let Err(e) = r {
            self.log.push(e.clone());
            out.status = Some(e);
        }
        out
    }

    pub fn run_command(&mut self, idx: usize, map: &Map, models: &ModelCache, sel: &[u64], grid: f32, bounds: HashMap<u64, (Vec3, Vec3)>) -> Outcome {
        self.prepare(idx, map, models, sel, grid, bounds);
        let r = self.call(idx, "run", ());
        let mut out = self.finish();
        if let Err(e) = r {
            self.log.push(e.clone());
            out.status = Some(e);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tool_click(&mut self, idx: usize, map: &Map, models: &ModelCache, sel: &[u64], grid: f32, bounds: HashMap<u64, (Vec3, Vec3)>, hit: &Hit) -> Outcome {
        self.prepare(idx, map, models, sel, grid, bounds);
        let mut h = RMap::new();
        h.insert("id".into(), Dynamic::from(hit.id as INT));
        h.insert("point".into(), Dynamic::from(arr(hit.point)));
        h.insert("normal".into(), Dynamic::from(arr(hit.normal)));
        h.insert("edge".into(), Dynamic::from(vec![Dynamic::from(arr(hit.edge.0)), Dynamic::from(arr(hit.edge.1))]));
        let r = self.call(idx, "on_click", (h,));
        let mut out = self.finish();
        if let Err(e) = r {
            self.log.push(e.clone());
            out.status = Some(e);
        }
        out
    }

    /// Lets a move plugin adjust a drag delta. Map edits made here are ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn adjust_move(
        &mut self,
        idx: usize,
        map: &Map,
        models: &ModelCache,
        ids: &[u64],
        grid: f32,
        bounds: HashMap<u64, (Vec3, Vec3)>,
        bmin: Vec3,
        bmax: Vec3,
        delta: Vec3,
    ) -> Vec3 {
        self.prepare(idx, map, models, ids, grid, bounds);
        let id_arr: Array = ids.iter().map(|i| Dynamic::from(*i as INT)).collect();
        let r = self.call(idx, "adjust_move", (id_arr, bounds_map(bmin, bmax), arr(delta)));
        let _ = self.finish();
        match r {
            Ok(d) if d.is_array() => {
                let v = dyn_v3(&d);
                if v.is_finite() {
                    v
                } else {
                    delta
                }
            }
            Ok(_) => delta,
            Err(e) => {
                if self.log.last() != Some(&e) {
                    self.log.push(e);
                }
                delta
            }
        }
    }

    fn log_once(&mut self, e: String) {
        if !self.log.iter().rev().take(3).any(|l| *l == e) {
            self.log.push(e);
        }
    }

    /// Collects a gizmo plugin's overlay for one viewport.
    #[allow(clippy::too_many_arguments)]
    pub fn gizmo_draw(&mut self, idx: usize, map: &Map, models: &ModelCache, sel: &[u64], grid: f32, bounds: HashMap<u64, (Vec3, Vec3)>, view: ViewInfo) -> Vec<DrawCmd> {
        if !self.has_fn(idx, "draw") {
            return Vec::new();
        }
        self.prepare_with(idx, map, models, sel, grid, bounds, true);
        self.ctx.borrow_mut().view = view;
        let r = self.call(idx, "draw", ());
        let cmds = std::mem::take(&mut self.ctx.borrow_mut().draw);
        let _ = self.finish();
        if let Err(e) = r {
            self.log_once(e);
        }
        cmds
    }

    /// Sends a pointer event (`pointer_down` / `pointer_drag` / `pointer_up`).
    /// Returns what changed and whether the plugin took the event.
    #[allow(clippy::too_many_arguments)]
    pub fn gizmo_event(
        &mut self,
        idx: usize,
        func: &str,
        map: &Map,
        models: &ModelCache,
        sel: &[u64],
        grid: f32,
        bounds: HashMap<u64, (Vec3, Vec3)>,
        view: ViewInfo,
        ev: RMap,
    ) -> (Outcome, bool) {
        if !self.has_fn(idx, func) {
            return (Outcome::default(), false);
        }
        self.prepare_with(idx, map, models, sel, grid, bounds, true);
        self.ctx.borrow_mut().view = view;
        let r = self.call(idx, func, (ev,));
        let mut out = self.finish();
        let taken = match r {
            Ok(d) => d.as_bool().unwrap_or(false),
            Err(e) => {
                out.status = Some(e.clone());
                self.log_once(e);
                false
            }
        };
        (out, taken)
    }


    pub fn clear_state_for(&mut self, file: &str) {
        let prefix = format!("{file}::");
        self.ctx.borrow_mut().state.retain(|k, _| !k.starts_with(&prefix));
    }

    #[allow(dead_code)]
    pub fn clear_state(&mut self) {
        self.ctx.borrow_mut().state.clear();
    }
}
