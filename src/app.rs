//! Editor core: state, menus, library, inspector, outliner, the four
//! viewports and tool interaction. Building tools live in `building.rs`,
//! mesh editing in `editmode.rs` and the tool windows in `windows.rs`.

use crate::cloud::{self, CloudMsg};
use crate::editmode::EditState;
use crate::export_roblox;
use crate::export_unity;
use crate::map::{Kind, Map, Material, Object, Shape};
use crate::mesh::{build_scene, world_mesh, ModelCache, SceneCache, WorldMesh};
use crate::playtest::{Play, PlayInput};
use crate::plugins::{event_map, DrawCmd, Hit, Outcome, PluginHost, PluginKind, ViewInfo};
use crate::settings::{ModelEntry, Preset, Settings};
use crate::texture::TexCache;
use crate::view2d::{self, View2D, ViewKind};
use crate::view3d::{pick_ray, ray_plane, Camera, RenderOpts, Renderer};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Key, PointerButton, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind,
};
use glam::{Mat3, Quat, Vec2, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Select,
    Block,
    Wall,
    Layout,
    Polygon,
    Door,
    Window,
    Trim,
    Prop,
    Decal,
    Light,
    Spawn,
    Text,
    Face,
    Knife,
    Stairs,
    Prompt,
    Teleport,
    Plugin(usize),
}

impl Tool {
    pub(crate) const BASIC: [Tool; 18] = [
        Tool::Select,
        Tool::Block,
        Tool::Wall,
        Tool::Layout,
        Tool::Polygon,
        Tool::Door,
        Tool::Window,
        Tool::Trim,
        Tool::Prop,
        Tool::Decal,
        Tool::Light,
        Tool::Spawn,
        Tool::Text,
        Tool::Face,
        Tool::Knife,
        Tool::Stairs,
        Tool::Prompt,
        Tool::Teleport,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Block => "Block",
            Tool::Wall => "Wall",
            Tool::Layout => "Building",
            Tool::Polygon => "Polygon",
            Tool::Door => "Door",
            Tool::Window => "Window",
            Tool::Trim => "Trim",
            Tool::Prop => "Prop",
            Tool::Decal => "Decal",
            Tool::Light => "Light",
            Tool::Spawn => "Spawn",
            Tool::Text => "Text",
            Tool::Face => "Faces",
            Tool::Knife => "Knife",
            Tool::Stairs => "Stairs",
            Tool::Prompt => "Prompt",
            Tool::Teleport => "Teleport",
            Tool::Plugin(_) => "Plugin",
        }
    }

    fn key(self) -> Option<(Key, &'static str)> {
        Some(match self {
            Tool::Select => (Key::Num1, "1"),
            Tool::Block => (Key::Num2, "2"),
            Tool::Wall => (Key::Num3, "3"),
            Tool::Layout => (Key::Num4, "4"),
            Tool::Polygon => (Key::Num5, "5"),
            Tool::Door => (Key::Num6, "6"),
            Tool::Window => (Key::Num7, "7"),
            Tool::Prop => (Key::Num8, "8"),
            Tool::Decal => (Key::Num9, "9"),
            Tool::Light => (Key::Num0, "0"),
            _ => return None,
        })
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click to select (Shift/Ctrl+click adds), drag to move, drag the white handles to resize. Shift+drag clones.",
            Tool::Block => "Drag a rectangle in a 2D view to draw a brush.",
            Tool::Wall => "Drag a line in the Top or 3D view to draw a wall. It stays straight (every 45°); hold Ctrl for any angle.",
            Tool::Layout => "Drag a box in the Top or 3D view for a rectangular building (Alt = square). Or click corners one by one: lines stay straight (every 45°, Ctrl = any angle). Click the first point, double-click or press Enter to build. Backspace removes a point.",
            Tool::Polygon => "Click points of a closed shape, then close it (first point / double-click / Enter) to make an extruded mesh you can edit.",
            Tool::Door => "Click a wall to cut a door opening (a boolean hole). Click a floor to cut a hatch.",
            Tool::Window => "Click a wall to cut a window opening (adds a glass pane if enabled in Settings).",
            Tool::Trim => "Click a wall in the 3D view to run a trim strip along it at that height. On a building it wraps all the way around (outside or inside, whichever side you click). Ctrl = no height snapping.",
            Tool::Prop => "Click to place the selected library model (it drops the moment you press). R / Shift+R turns it 15° (Alt+R: 90°), or Alt+scroll.",
            Tool::Decal => "Click a wall or floor to stick a decal on it. Pick its image in the Textures window.",
            Tool::Light => "Click to place a point light.",
            Tool::Spawn => "Click to place a spawn point (playtest starts here).",
            Tool::Face => "Click faces of walls, floors or meshes (Shift/Ctrl+click adds), then give just those faces a texture in the Inspector.",
            Tool::Prompt => "Click to place a Roblox ProximityPrompt (\"Press E to …\"). Set its text, key, hold time and range in the Inspector.",
            Tool::Teleport => "Click to place a teleport pad, then click where it sends players. Two-way makes a pad back. Exports to Roblox with a working Script.",
            Tool::Stairs => "Click where the stairs start (bottom), then where they end (top: a floor above, the top of a wall or platform). Options and prefabs below.",
            Tool::Knife => "Drag a line across walls or meshes to cut them. Cuts the selection, or everything under the line if nothing is selected. Hold Ctrl in 2D views to skip grid snapping.",
            Tool::Text => "Type your text and pick a font below, then click a wall or the floor to place 3D text. It's saved as a mesh, so everyone sees it even without the font.",
            Tool::Plugin(_) => "Plugin tool: click in a viewport.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    Quad,
    /// 0 = 3D, 1 = Top, 2 = Front, 3 = Side
    Single(usize),
}

pub(crate) enum Drag {
    None,
    Move2D { view: usize, start_uv: Vec2, bmin_uv: Vec2, bmin: Vec3, bmax: Vec3, origs: Vec<(u64, Vec3)> },
    Resize2D { view: usize, handle: (i32, i32), uv_min: Vec2, uv_max: Vec2, wmin: Vec3, wmax: Vec3, origs: Vec<Object> },
    Marquee { view: usize, start: Pos2 },
    Create2D { view: usize, start_uv: Vec2 },
    Wall2D { view: usize, start: Vec3 },
    Move3D { plane_y: f32, start: Vec3, bmin: Vec3, bmax: Vec3, origs: Vec<(u64, Vec3)> },
    Wall3D { start: Vec3 },
    EditMove2D { view: usize, start_uv: Vec2, origs: Vec<(u32, Vec3)> },
    EditMove3D { plane_y: f32, start: Vec3, origs: Vec<(u32, Vec3)> },
    /// view: Some(2D view index) or None for the 3D view
    EditMarquee { view: Option<usize>, start: Pos2 },
    /// A gizmo plugin owns this drag (slot: 0 = 3D, 1-3 = 2D views)
    Gizmo { slot: usize },
    /// The vertex / face gizmo in mesh edit mode.
    EditGizmo { slot: usize, g: crate::gizmo::GDrag, starts: Vec<(u32, Vec3)> },
    /// Knife line (slot: 0 = 3D, 1-3 = 2D views), start in screen space.
    Knife { slot: usize, start: Pos2 },
    /// Door / window tool held down: preview, cut on release.
    Opening { slot: usize },
    /// Building / Polygon with Shift: drag out an exact rectangle (Alt = square).
    Outline2D { view: usize, start: Vec3, square: bool },
    Outline3D { start: Vec3, square: bool },
}

pub(crate) struct BrushTemplate {
    pub shape: Shape,
    pub size: Vec3,
    pub color: [u8; 3],
    pub material: Material,
    pub steps: u32,
    pub transparency: f32,
}

pub struct AnvilApp {
    pub(crate) map: Map,
    pub(crate) path: Option<PathBuf>,
    pub(crate) dirty: bool,
    pub(crate) settings: Settings,
    pub(crate) models: ModelCache,
    pub(crate) textures: TexCache,
    pub(crate) tex_previews: HashMap<String, egui::TextureHandle>,
    pub(crate) selection: Vec<u64>,
    pub(crate) tool: Tool,
    pub(crate) brush_shape: Shape,
    pub(crate) active_preset: Option<usize>,
    pub(crate) active_model: Option<String>,
    pub(crate) active_texture: Option<u64>,
    pub(crate) work_y: f32,
    pub(crate) views: [View2D; 3],
    pub(crate) cam: Camera,
    pub(crate) renderer: Renderer,
    pub(crate) layout: Layout,
    pub(crate) drag: Drag,
    pub(crate) drag_cur: Pos2,
    pub(crate) undo: Vec<Map>,
    pub(crate) redo: Vec<Map>,
    pub(crate) team: Option<crate::team::Session>,
    pub(crate) brand: crate::branding::Brand,
    pub(crate) uv: crate::uv_editor::UvEditor,
    pub(crate) thumbs: crate::thumbs::Thumbs,
    pub(crate) ghost: crate::previews::Ghost,
    pub(crate) scatter: crate::scatter::ScatterUi,
    /// Textures for new walls / floors from the Wall and Building tools.
    pub(crate) build_wall_texture: Option<u64>,
    pub(crate) build_floor_texture: Option<u64>,
    pub(crate) theme_applied: Option<(u8, [u8; 3])>,
    /// The selection as it was when a gizmo drag started (for collisions).
    pub(crate) gizmo_origs: Vec<Object>,
    /// Teleport tool: the pad waiting for its destination click.
    pub(crate) tp_pending: Option<u64>,
    pub(crate) live: Option<crate::livesync::LiveServer>,
    pub(crate) live_error: String,
    pub(crate) blend_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    pub(crate) update: crate::updater::UpdState,
    pub(crate) update_checked: bool,
    pub(crate) update_hidden: bool,
    /// Panel plugins with their window open (file names).
    pub(crate) open_panels: Vec<String>,
    pub(crate) panel_state: crate::plugin_panels::PanelState,
    pub(crate) poly_extrude: Option<crate::previews::PolyExtrude>,
    /// Stair maker: the first click, waiting for the second.
    pub(crate) stair_start: Option<Vec3>,
    pub(crate) team_ui: crate::team_ui::TeamUi,
    pub(crate) inspector_editing: bool,
    pub(crate) show_settings: bool,
    pub(crate) show_help: bool,
    pub(crate) show_textures: bool,
    pub(crate) show_plugins: bool,
    pub(crate) show_export: bool,
    pub(crate) export_tab_roblox: bool,
    pub(crate) status: String,
    pub(crate) cursor_text: String,
    pub(crate) hovered_slot: Option<usize>,
    pub(crate) title: String,
    pub(crate) layout_pts: Vec<Vec3>,
    /// Last object clicked in the object list (where a Ctrl+click range starts).
    pub(crate) outliner_anchor: Option<u64>,
    /// Turn (degrees about up) for props, spawns and prompts being placed:
    /// R / Shift+R or Alt+scroll while placing.
    pub(crate) place_yaw: f32,
    /// The point under the mouse in the view it's over (where Ctrl+V pastes).
    pub(crate) cursor_world: Option<Vec3>,
    /// The last copy, for Edit > Paste (Ctrl+V reads the system clipboard).
    pub(crate) clip_text: Option<String>,
    /// When this Anvil started (names this session's untitled autosave).
    pub(crate) session_id: u64,
    /// Unsaved work found at start (newest autosave), offered to reopen.
    pub(crate) restore_offer: Option<(PathBuf, std::time::SystemTime)>,
    /// Camera as last drawn, and when it last moved (for drawing faster while moving).
    pub(crate) last_cam: Option<crate::view3d::Camera>,
    pub(crate) cam_moved_at: f64,
    pub(crate) edit: Option<EditState>,
    pub(crate) play: Option<(Play, Layout)>,
    pub(crate) plugins: PluginHost,
    pub(crate) cloud_rx: Option<Receiver<CloudMsg>>,
    pub(crate) cloud_log: Vec<String>,
    pub(crate) cloud_key: String,
    pub(crate) remember_key: bool,
    pub(crate) cloud_textures: bool,
    pub(crate) cloud_props: bool,
    pub(crate) cloud_map: bool,
    pub(crate) cloud_force: bool,
    pub(crate) last_map_asset: Option<String>,
    pub(crate) rename_folder: Option<(u64, String)>,
    // mounted Half-Life / Source content
    pub(crate) show_content: bool,
    pub(crate) mounts: Option<Arc<crate::mount::Mounts>>,
    pub(crate) mounts_rx: Option<Receiver<crate::mount::Mounts>>,
    pub(crate) content_search: String,
    pub(crate) content_tab_models: bool,
    pub(crate) content_filtered: Vec<String>,
    pub(crate) content_key: (String, bool, usize),
    pub(crate) content_sel: Option<String>,
    pub(crate) content_preview: Option<(String, egui::TextureHandle)>,
    pub(crate) last_autosave: f64,
    /// Rebuilds only objects that changed since the last frame.
    pub(crate) scene_cache: SceneCache,
    // 3D text tool
    pub(crate) fonts: Option<Vec<crate::text::FontInfo>>,
    pub(crate) fonts_rx: Option<Receiver<Vec<crate::text::FontInfo>>>,
    pub(crate) font_search: String,
    /// Transform gizmo: 0 move, 1 rotate, 2 scale; and local/global axes.
    pub(crate) gizmo_mode: u8,
    pub(crate) gizmo_local: bool,
    /// Perspective camera to return to after a 2D (ortho) view.
    pub(crate) persp_cam: Option<Camera>,
    /// 3D view: 0 perspective, 1 top, 2 bottom, 3 front, 4 back, 5 left, 6 right.
    pub(crate) view_preset: u8,
    /// Faces tool selection: (object id, face key).
    pub(crate) face_sel: Vec<(u64, usize)>,
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

pub(crate) fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn lighten(c: [u8; 3]) -> Color32 {
    let f = |v: u8| (v as f32 * 0.6 + 255.0 * 0.4) as u8;
    Color32::from_rgb(f(c[0]), f(c[1]), f(c[2]))
}

pub(crate) fn snap(v: f32, g: f32) -> f32 {
    if g > 0.0 {
        (v / g).round() * g
    } else {
        v
    }
}

pub(crate) fn snap2(v: Vec2, g: f32) -> Vec2 {
    Vec2::new(snap(v.x, g), snap(v.y, g))
}

pub(crate) fn fmt_num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

pub(crate) fn vec3_edit(ui: &mut egui::Ui, v: &mut Vec3, speed: f64, min: Option<f32>) -> bool {
    let mut ch = false;
    ui.horizontal(|ui| {
        for (i, l) in ["x", "y", "z"].iter().enumerate() {
            let mut dv = egui::DragValue::new(&mut v[i]).speed(speed).prefix(format!("{l} ")).max_decimals(3);
            if let Some(m) = min {
                dv = dv.range(m..=1.0e6);
            }
            ch |= ui.add(dv).changed();
        }
    });
    ch
}

pub(crate) fn vec2_edit(ui: &mut egui::Ui, v: &mut Vec2, speed: f64, min: Option<f32>, labels: [&str; 2]) -> bool {
    let mut ch = false;
    ui.horizontal(|ui| {
        for (i, l) in labels.iter().enumerate() {
            let mut dv = egui::DragValue::new(&mut v[i]).speed(speed).prefix(format!("{l} ")).max_decimals(3);
            if let Some(m) = min {
                dv = dv.range(m..=1.0e6);
            }
            ch |= ui.add(dv).changed();
        }
    });
    ch
}

pub(crate) fn material_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, m: &mut Material) -> bool {
    let mut ch = false;
    egui::ComboBox::from_id_salt(id).selected_text(m.label()).width(140.0).show_ui(ui, |ui| {
        for mm in Material::ALL {
            ch |= ui.selectable_value(m, mm, mm.label()).changed();
        }
    });
    ch
}

pub(crate) fn shape_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, s: &mut Shape) -> bool {
    let mut ch = false;
    egui::ComboBox::from_id_salt(id).selected_text(s.label()).width(140.0).show_ui(ui, |ui| {
        for ss in Shape::ALL {
            ch |= ui.selectable_value(s, ss, ss.label()).changed();
        }
    });
    ch
}

pub(crate) fn sel_bounds(scene: &[Arc<WorldMesh>], sel: &[u64]) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    for wm in scene.iter().filter(|w| sel.contains(&w.id)) {
        out = Some(match out {
            None => (wm.min, wm.max),
            Some((a, b)) => (a.min(wm.min), b.max(wm.max)),
        });
    }
    out
}

/// Rescales objects from an old world AABB to a new one along the given axes.
pub(crate) fn apply_scale(map: &mut Map, origs: &[Object], omin: Vec3, omax: Vec3, nmin: Vec3, nmax: Vec3, axes: &[usize]) {
    for o in origs {
        let Some(obj) = map.get_mut(o.id) else { continue };
        *obj = o.clone();
        let rm = Mat3::from_quat(o.rotation());
        for &a in axes {
            let oe = omax[a] - omin[a];
            if oe.abs() < 1e-6 {
                continue;
            }
            let f = (nmax[a] - nmin[a]) / oe;
            obj.pos[a] = nmin[a] + (o.pos[a] - omin[a]) * f;
            let k = (0..3)
                .max_by(|&i, &j| rm.col(i)[a].abs().partial_cmp(&rm.col(j)[a].abs()).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(a);
            match &mut obj.kind {
                Kind::Brush { size, holes, .. } => {
                    size[k] *= f;
                    for h in holes {
                        h.min[k] *= f;
                        h.max[k] *= f;
                    }
                }
                Kind::Spawn { size } => size[k] *= f,
                Kind::Prop { scale, .. } => scale[k] *= f,
                Kind::Decal { size } => {
                    if k < 2 {
                        size[k] *= f
                    }
                }
                Kind::Mesh { verts, .. } => {
                    for v in verts {
                        v[k] *= f;
                    }
                }
                Kind::Teleport { size, .. } => size[k] *= f,
                Kind::Light { .. } | Kind::Prompt { .. } | Kind::Target {} => {}
            }
        }
    }
}

pub(crate) fn file_name(p: &str) -> String {
    Path::new(p).file_name().and_then(|f| f.to_str()).unwrap_or(p).to_string()
}

/// Draws gizmo / plugin overlay commands on top of a viewport.
pub(crate) fn paint_cmds(painter: &egui::Painter, view: ViewInfo, cmds: &[DrawCmd]) {
    for c in cmds {
        match *c {
            DrawCmd::Line { a, b, color, width } => {
                let seg = match view {
                    ViewInfo::Persp { cam, rect, fov } => cam.project_segment(rect, fov, a, b),
                    _ => view.project(a).zip(view.project(b)),
                };
                if let Some((p, q)) = seg {
                    painter.line_segment([p, q], Stroke::new(width, rgb(color)));
                }
            }
            DrawCmd::Point { p, color, size, square } => {
                if let Some(s) = view.project(p) {
                    if square {
                        let r = Rect::from_center_size(s, vec2(size, size));
                        painter.rect_filled(r, CornerRadius::ZERO, rgb(color));
                        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::BLACK), StrokeKind::Inside);
                    } else {
                        painter.circle_filled(s, size * 0.5, rgb(color));
                    }
                }
            }
        }
    }
}

fn rect_center(resp: &egui::Response) -> Pos2 {
    resp.rect.center()
}

fn dist_point_segment(p: Vec3, a: Vec3, b: Vec3) -> (f32, Vec3) {
    let ab = b - a;
    let t = if ab.length_squared() < 1e-12 { 0.0 } else { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
    let q = a + ab * t;
    ((p - q).length(), q)
}

impl AnvilApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let restore_offer = crate::autosave::unsaved_work(&crate::autosave::dir(), &settings.recent_files);
        let key = cloud::load_key();
        let trusted_plugins = settings.trusted_plugins.clone();
        let mut app = AnvilApp {
            map: Map::default(),
            path: None,
            dirty: false,
            brush_shape: settings.default_shape,
            settings,
            models: ModelCache::default(),
            textures: TexCache::default(),
            tex_previews: HashMap::new(),
            selection: Vec::new(),
            tool: Tool::Select,
            active_preset: None,
            active_model: None,
            active_texture: None,
            work_y: 0.0,
            views: [View2D::new(ViewKind::Top), View2D::new(ViewKind::Front), View2D::new(ViewKind::Side)],
            cam: Camera::looking_at(Vec3::new(-34.0, 26.0, 34.0), Vec3::new(0.0, 2.0, 0.0)),
            renderer: Renderer::default(),
            layout: Layout::Quad,
            drag: Drag::None,
            drag_cur: Pos2::ZERO,
            undo: Vec::new(),
            redo: Vec::new(),
            team: None,
            brand: crate::branding::Brand::new(&cc.egui_ctx),
            stair_start: None,
            uv: Default::default(),
            thumbs: Default::default(),
            ghost: Default::default(),
            scatter: Default::default(),
            build_wall_texture: None,
            build_floor_texture: None,
            theme_applied: None,
            gizmo_origs: Vec::new(),
            tp_pending: None,
            live: None,
            live_error: String::new(),
            blend_rx: None,
            update: Default::default(),
            update_checked: false,
            update_hidden: false,
            open_panels: Vec::new(),
            panel_state: Default::default(),
            poly_extrude: None,
            team_ui: Default::default(),
            inspector_editing: false,
            show_settings: false,
            show_help: false,
            show_textures: false,
            show_plugins: false,
            show_export: false,
            export_tab_roblox: true,
            status: "Welcome to Anvil. Press F1 for controls.".into(),
            cursor_text: String::new(),
            hovered_slot: None,
            title: String::new(),
            layout_pts: Vec::new(),
            outliner_anchor: None,
            place_yaw: 0.0,
            cursor_world: None,
            clip_text: None,
            session_id: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            restore_offer,
            last_cam: None,
            cam_moved_at: 0.0,
            edit: None,
            play: None,
            plugins: PluginHost::new_trusted(&trusted_plugins),
            cloud_rx: None,
            cloud_log: Vec::new(),
            remember_key: !key.is_empty(),
            cloud_key: key,
            cloud_textures: true,
            cloud_props: true,
            cloud_map: true,
            cloud_force: false,
            last_map_asset: None,
            rename_folder: None,
            show_content: false,
            mounts: None,
            mounts_rx: None,
            content_search: String::new(),
            content_tab_models: true,
            content_filtered: Vec::new(),
            content_key: (String::from("\u{0}"), true, 0),
            content_sel: None,
            content_preview: None,
            last_autosave: 0.0,
            scene_cache: SceneCache::default(),
            fonts: None,
            fonts_rx: None,
            font_search: String::new(),
            gizmo_mode: 0,
            gizmo_local: false,
            persp_cam: None,
            view_preset: 0,
            face_sel: Vec::new(),
        };
        app.starter_map();
        // switch on bundled gizmo plugins the first time they are installed
        for f in app.plugins.newly_installed.clone() {
            let is_gizmo = app.plugins.plugins.iter().any(|p| p.file == f && p.kind == PluginKind::Gizmo);
            if is_gizmo && !app.settings.enabled_plugins.contains(&f) {
                app.settings.enabled_plugins.push(f);
                let _ = app.settings.save();
            }
        }
        if let Some(arg) = std::env::args().nth(1) {
            app.open_path(PathBuf::from(arg));
        }
        app
    }

    fn starter_map(&mut self) {
        self.map = Map::default();
        let mut floor = Object::brush(Shape::Box, Vec3::new(64.0, 1.0, 64.0), Vec3::new(0.0, -0.5, 0.0), [110, 112, 118], Material::Slate);
        floor.name = "Baseplate".into();
        self.map.add(floor);
        self.dirty = false;
    }

    // ----- undo / selection helpers --------------------------------------

    pub(crate) fn push_undo(&mut self) {
        self.undo.push(self.map.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    /// Pushes an undo step for an edit already made to `originals`' objects:
    /// the snapshot is the current map with those objects put back. This
    /// avoids copying the whole map every frame "just in case".
    pub(crate) fn push_undo_restoring(&mut self, originals: Vec<Object>) {
        let mut before = self.map.clone();
        for o in originals {
            if let Some(x) = before.get_mut(o.id) {
                *x = o;
            }
        }
        self.undo.push(before);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(m) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.map, m));
            self.prune_selection();
            self.dirty = true;
            self.status = "Undo".into();
        }
    }

    fn redo(&mut self) {
        if let Some(m) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.map, m));
            self.prune_selection();
            self.dirty = true;
            self.status = "Redo".into();
        }
    }

    pub(crate) fn prune_selection(&mut self) {
        let map = &self.map;
        self.selection.retain(|id| map.get(*id).is_some());
        if let Some(e) = &mut self.edit {
            match map.get(e.obj).map(|o| &o.kind) {
                Some(Kind::Mesh { verts, faces }) => {
                    e.verts.retain(|v| (*v as usize) < verts.len());
                    e.faces.retain(|f| *f < faces.len());
                }
                _ => self.edit = None,
            }
        }
    }

    pub(crate) fn snap_step(&self) -> f32 {
        if self.settings.snap {
            self.settings.grid
        } else {
            0.0
        }
    }

    pub(crate) fn click_select(&mut self, hit: Option<u64>, add: bool) {
        match hit {
            Some(id) => {
                if add {
                    if let Some(i) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(i);
                    } else {
                        self.selection.push(id);
                    }
                } else {
                    self.selection = vec![id];
                }
            }
            None => {
                if !add {
                    self.selection.clear();
                }
            }
        }
    }

    fn sel_positions(&self) -> Vec<(u64, Vec3)> {
        self.selection.iter().filter_map(|id| self.map.get(*id).map(|o| (o.id, o.pos))).collect()
    }

    pub(crate) fn selected_objects(&self) -> Vec<Object> {
        self.selection.iter().filter_map(|id| self.map.get(*id).cloned()).collect()
    }

    /// Ctrl+D: copies of the selection exactly where it is (selected, ready to move).
    pub(crate) fn duplicate_in_place(&mut self) {
        self.push_undo();
        self.duplicate(Vec3::ZERO);
        self.status = format!("Duplicated {} object(s) in place: drag them or use the arrow keys to move them", self.selection.len());
    }

    /// Ctrl+C (Ctrl+X with `cut`): the selection onto the clipboard.
    pub(crate) fn copy_selection(&mut self, ctx: &egui::Context, cut: bool) {
        let Some(text) = crate::clipboard::copy(&self.map, &self.selection) else {
            self.status = "Select something to copy".into();
            return;
        };
        ctx.copy_text(text.clone());
        self.clip_text = Some(text);
        let n = self.selection.len();
        if cut {
            self.delete_selection();
            self.status = format!("Cut {n} object(s): Ctrl+V pastes them where your mouse is");
        } else {
            self.status = format!("Copied {n} object(s): Ctrl+V pastes them where your mouse is");
        }
    }

    /// Pastes clipboard objects: under the mouse when it's over a view
    /// (`at_mouse`), else one grid step over from where they were copied.
    pub(crate) fn paste_text(&mut self, text: &str, at_mouse: bool) {
        let Some(clip) = crate::clipboard::parse(text) else { return };
        let g = self.snap_step();
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for o in &clip.objects {
            let wm = world_mesh(o, &clip.textures, &mut self.models);
            if !wm.pos.is_empty() {
                mn = mn.min(wm.min);
                mx = mx.max(wm.max);
            }
        }
        if mn.x > mx.x {
            let c = clip.objects.iter().fold(Vec3::ZERO, |a, o| a + o.pos) / clip.objects.len() as f32;
            (mn, mx) = (c, c);
        }
        let target = if at_mouse && self.hovered_slot.is_some() { self.cursor_world } else { None };
        let delta = match target {
            // the group's bottom middle lands on the point under the mouse
            Some(p) => {
                let d = p - Vec3::new((mn.x + mx.x) * 0.5, mn.y, (mn.z + mx.z) * 0.5);
                Vec3::new(snap(d.x, g), d.y, snap(d.z, g))
            }
            None => {
                let s = self.settings.grid.max(1.0);
                Vec3::new(s, 0.0, s)
            }
        };
        self.push_undo();
        self.selection = crate::clipboard::paste(&mut self.map, &clip, delta);
        self.status = format!("Pasted {} object(s)", self.selection.len());
    }

    /// Duplicates the selection (offset by `offset`) and selects the copies.
    pub(crate) fn duplicate(&mut self, offset: Vec3) {
        let objs = self.selected_objects();
        let mut ids = Vec::new();
        for mut o in objs {
            o.pos += offset;
            o.name = String::new();
            ids.push(self.map.add(o));
        }
        self.selection = ids;
    }

    pub(crate) fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        let ids = std::mem::take(&mut self.selection);
        self.map.remove(&ids);
        self.status = format!("Deleted {} object(s)", ids.len());
    }

    pub(crate) fn hide_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        for id in self.selection.clone() {
            if let Some(o) = self.map.get_mut(id) {
                o.hidden = true;
            }
        }
        self.selection.clear();
    }

    pub(crate) fn unhide_all(&mut self) {
        self.push_undo();
        for o in &mut self.map.objects {
            o.hidden = false;
        }
    }

    fn selection_bounds(&mut self) -> Option<(Vec3, Vec3)> {
        let mut out: Option<(Vec3, Vec3)> = None;
        let use_all = self.selection.is_empty();
        for o in &self.map.objects {
            if o.hidden || !(use_all || self.selection.contains(&o.id)) {
                continue;
            }
            let wm = world_mesh(o, &self.map.textures, &mut self.models);
            out = Some(match out {
                None => (wm.min, wm.max),
                Some((a, b)) => (a.min(wm.min), b.max(wm.max)),
            });
        }
        out
    }

    /// Lighting for the 3D view from the settings and the map's Light objects.
    pub(crate) fn lighting(&self) -> crate::lighting::Lighting {
        let st = &self.settings;
        let sun = crate::lighting::sun_dir(st.sun_azimuth, st.sun_elevation);
        let mut l = crate::lighting::preset(st.light_quality, sun, st.fog_enabled, st.fog_distance, st.sky_enabled);
        if l.on {
            let cam = self.cam.pos;
            let mut lights: Vec<crate::lighting::PointLight> = self
                .map
                .objects
                .iter()
                .filter(|o| !o.hidden)
                .filter_map(|o| match o.kind {
                    Kind::Light { range, brightness } => Some(crate::lighting::PointLight {
                        pos: o.pos,
                        range: range.max(0.1),
                        brightness: (brightness * 0.45).clamp(0.0, 4.0),
                        color: [o.color[0] as f32 / 255.0, o.color[1] as f32 / 255.0, o.color[2] as f32 / 255.0],
                    }),
                    _ => None,
                })
                .collect();
            lights.sort_by(|a, b| (a.pos - cam).length_squared().total_cmp(&(b.pos - cam).length_squared()));
            lights.truncate(64);
            l.lights = lights;
        }
        l
    }

    pub(crate) fn focus(&mut self) {
        let Some((mn, mx)) = self.selection_bounds() else { return };
        let c = (mn + mx) * 0.5;
        let r = ((mx - mn).length() * 0.5).max(1.0);
        self.cam.pos = c - self.cam.forward() * (r * 1.9 + 3.0);
        for v in &mut self.views {
            v.center = v.to_uv(c);
            v.zoom = (180.0 / r).clamp(0.05, 200.0);
        }
    }

    // ----- folders --------------------------------------------------------

    fn group_selection(&mut self) {
        if self.selection.is_empty() {
            self.status = "Select objects to group (Shift+click to pick several)".into();
            return;
        }
        let folders: Vec<Option<u64>> = self.selection.iter().filter_map(|id| self.map.get(*id)).map(|o| self.map.folder_of(o)).collect();
        let parent = if folders.windows(2).all(|w| w[0] == w[1]) { folders.first().copied().flatten() } else { None };
        self.push_undo();
        let name = format!("Folder {}", self.map.folders.len() + 1);
        let fid = self.map.add_folder(&name, parent);
        for id in self.selection.clone() {
            if let Some(o) = self.map.get_mut(id) {
                o.folder = Some(fid);
            }
        }
        self.rename_folder = Some((fid, name.clone()));
        self.status = format!("Grouped {} object(s) into '{name}'", self.selection.len());
    }

    fn ungroup_selection(&mut self) {
        let mut fids: Vec<u64> = self.selection.iter().filter_map(|id| self.map.get(*id)).filter_map(|o| self.map.folder_of(o)).collect();
        fids.sort_unstable();
        fids.dedup();
        if fids.is_empty() {
            return;
        }
        self.push_undo();
        for f in &fids {
            self.map.remove_folder(*f);
        }
        self.status = format!("Removed {} folder(s)", fids.len());
    }

    // ----- object creation ------------------------------------------------

    pub(crate) fn brush_template(&self) -> BrushTemplate {
        if let Some(p) = self.active_preset.and_then(|i| self.settings.presets.get(i)) {
            BrushTemplate { shape: p.shape, size: p.size, color: p.color, material: p.material, steps: p.steps, transparency: p.transparency }
        } else {
            let s = &self.settings;
            BrushTemplate {
                shape: self.brush_shape,
                size: s.default_size,
                color: s.default_color,
                material: s.default_material,
                steps: s.default_steps,
                transparency: 0.0,
            }
        }
    }

    pub(crate) fn make_brush(t: &BrushTemplate, pos: Vec3, size: Vec3) -> Object {
        let mut o = Object::new(Kind::Brush { shape: t.shape, size, steps: t.steps, holes: Vec::new() }, pos, t.color, t.material);
        o.transparency = t.transparency;
        o
    }

    pub(crate) fn add_and_select(&mut self, o: Object) -> u64 {
        self.push_undo();
        let id = self.map.add(o);
        self.selection = vec![id];
        id
    }

    /// Places the current tool's object at `at`. With `on_surface` the object
    /// sits on top of the point instead of being centered on it.
    fn place(&mut self, at: Vec3, on_surface: bool) {
        match self.placement(at, on_surface) {
            Ok(o) => {
                self.add_and_select(o);
            }
            Err(e) if !e.is_empty() => self.status = e,
            Err(_) => {}
        }
    }

    /// Tools that drop one object per click (placed the moment you press).
    pub(crate) fn is_place_tool(&self) -> bool {
        matches!(self.tool, Tool::Prop | Tool::Light | Tool::Spawn | Tool::Prompt)
    }

    /// Turns what's being placed by `deg` (R / Shift+R, Alt+scroll).
    pub(crate) fn turn_placement(&mut self, deg: f32) {
        self.place_yaw = (self.place_yaw + deg).rem_euclid(360.0);
        if (self.place_yaw - 360.0).abs() < 1e-3 {
            self.place_yaw = 0.0;
        }
        self.status = format!("Placing turned {:.0}° (R / Shift+R or Alt+scroll to turn)", self.place_yaw);
    }

    /// The object a click with the current tool would add at `at`.
    pub(crate) fn placement(&self, at: Vec3, on_surface: bool) -> Result<Object, String> {
        let s = &self.settings;
        let obj = match self.tool {
            Tool::Block | Tool::Select | Tool::Wall => {
                let t = self.brush_template();
                let mut pos = at;
                if on_surface {
                    pos.y += t.size.y * 0.5;
                }
                Some(Self::make_brush(&t, pos, t.size))
            }
            Tool::Prop => match &self.active_model {
                Some(m) => Some(Object::new(
                    Kind::Prop { model: m.clone(), scale: Vec3::ONE, roblox_mesh_id: String::new() },
                    at,
                    [200, 200, 200],
                    Material::SmoothPlastic,
                )),
                None => return Err("Pick a model in the Library panel first (Models > Add…)".into()),
            },
            Tool::Light => {
                let mut pos = at;
                if on_surface {
                    pos.y += 4.0;
                }
                let mut o = Object::new(Kind::Light { range: s.light_range, brightness: s.light_brightness }, pos, [255, 236, 190], Material::Neon);
                o.collide = false;
                Some(o)
            }
            Tool::Prompt => Some(crate::entities::new_prompt(if on_surface { at + Vec3::Y * 2.5 } else { at })),
            Tool::Spawn => {
                let size = s.spawn_size;
                let mut pos = at;
                if on_surface {
                    pos.y += size.y * 0.5;
                }
                Some(Object::new(Kind::Spawn { size }, pos, [90, 200, 110], Material::SmoothPlastic))
            }
            _ => None,
        };
        let mut obj = obj.ok_or_else(String::new)?;
        if matches!(self.tool, Tool::Prop | Tool::Spawn | Tool::Prompt) {
            obj.rot.y = self.place_yaw;
        }
        Ok(obj)
    }

    /// Sticks a decal on a surface with normal `n`.
    pub(crate) fn place_decal(&mut self, at: Vec3, n: Vec3) {
        let z = n.normalize_or_zero();
        if z == Vec3::ZERO {
            return;
        }
        let up_ref = if z.y.abs() > 0.9 { Vec3::NEG_Z } else { Vec3::Y };
        let x = up_ref.cross(z).normalize();
        let y = z.cross(x);
        let mut o = Object::new(Kind::Decal { size: self.settings.decal_size }, at + z * 0.03, [255, 255, 255], Material::SmoothPlastic);
        o.collide = false;
        o.set_rotation(Quat::from_mat3(&Mat3::from_cols(x, y, z)));
        o.texture = self.active_texture.filter(|t| self.map.textures.iter().any(|d| d.id == *t));
        if let Some(t) = self.map.texture(o.texture) {
            o.name = format!("Decal_{}", t.name);
        } else {
            self.status = "Decal placed. Pick its image in Textures > 'Use for decals' (or set it in the Inspector).".into();
        }
        self.add_and_select(o);
    }

    fn create_from_rect(&mut self, view: &View2D, a: Vec2, b: Vec2, scene: &[Arc<WorldMesh>]) {
        let (ua, su, va, sv, da) = view.axes();
        if (a.x - b.x).abs() < 1e-4 || (a.y - b.y).abs() < 1e-4 {
            return;
        }
        let t = self.brush_template();
        let (dmin, dmax) = if view.kind == ViewKind::Top {
            (self.work_y, self.work_y + t.size.y)
        } else if let Some((mn, mx)) = sel_bounds(scene, &self.selection) {
            (mn[da], mx[da])
        } else {
            (-t.size[da] * 0.5, t.size[da] * 0.5)
        };
        let mut mn = Vec3::ZERO;
        let mut mx = Vec3::ZERO;
        let (u0, u1) = (a.x * su, b.x * su);
        let (v0, v1) = (a.y * sv, b.y * sv);
        mn[ua] = u0.min(u1);
        mx[ua] = u0.max(u1);
        mn[va] = v0.min(v1);
        mx[va] = v0.max(v1);
        mn[da] = dmin.min(dmax);
        mx[da] = dmin.max(dmax);
        let o = Self::make_brush(&t, (mn + mx) * 0.5, mx - mn);
        self.add_and_select(o);
    }

    fn create_wall(&mut self, a: Vec3, b: Vec3) {
        if let Some(mut o) = self.wall_object(a, b, 0.0) {
            o.name = format!("Wall_{}", self.map.next_id + 1);
            o.texture = self.build_wall_texture.filter(|t| self.map.textures.iter().any(|d| d.id == *t));
            self.add_and_select(o);
        }
    }

    /// Hammer's "Make Hollow": turns a box brush into six wall brushes.
    fn hollow(&mut self, id: u64) {
        let Some(o) = self.map.get(id).cloned() else { return };
        let Kind::Brush { shape: Shape::Box, size, .. } = o.kind else {
            self.status = "Hollow only works on Box brushes".into();
            return;
        };
        let t = self.settings.wall_thickness.min(size.min_element() * 0.45);
        let rot = o.rotation();
        let parts = [
            (Vec3::new(0.0, size.y * 0.5 - t * 0.5, 0.0), Vec3::new(size.x, t, size.z), "Ceiling"),
            (Vec3::new(0.0, -size.y * 0.5 + t * 0.5, 0.0), Vec3::new(size.x, t, size.z), "Floor"),
            (Vec3::new(size.x * 0.5 - t * 0.5, 0.0, 0.0), Vec3::new(t, size.y - 2.0 * t, size.z), "WallE"),
            (Vec3::new(-size.x * 0.5 + t * 0.5, 0.0, 0.0), Vec3::new(t, size.y - 2.0 * t, size.z), "WallW"),
            (Vec3::new(0.0, 0.0, size.z * 0.5 - t * 0.5), Vec3::new(size.x - 2.0 * t, size.y - 2.0 * t, t), "WallS"),
            (Vec3::new(0.0, 0.0, -size.z * 0.5 + t * 0.5), Vec3::new(size.x - 2.0 * t, size.y - 2.0 * t, t), "WallN"),
        ];
        self.push_undo();
        self.map.remove(&[id]);
        let mut ids = Vec::new();
        for (off, sz, n) in parts {
            let mut p = o.clone();
            p.kind = Kind::Brush { shape: Shape::Box, size: sz, steps: 1, holes: Vec::new() };
            p.pos = o.pos + rot * off;
            p.name = format!("{}_{n}", o.name);
            ids.push(self.map.add(p));
        }
        self.selection = ids;
        self.status = "Made the brush hollow (6 walls)".into();
    }

    // ----- plugins --------------------------------------------------------

    fn scene_bounds(scene: &[Arc<WorldMesh>]) -> HashMap<u64, (Vec3, Vec3)> {
        scene.iter().map(|w| (w.id, (w.min, w.max))).collect()
    }

    pub(crate) fn apply_outcome(&mut self, out: crate::plugins::Outcome) {
        if out.has_changes() {
            self.push_undo();
            out.apply_to(&mut self.map);
        }
        if let Some(sel) = out.selection {
            self.selection = sel;
            self.prune_selection();
        }
        if let Some(s) = out.status {
            self.status = s;
        }
    }

    pub(crate) fn run_plugin_command(&mut self, idx: usize) {
        let scene = build_scene(&self.map, &mut self.models);
        let out = self.plugins.run_command(idx, &self.map, &self.models, &self.selection, self.settings.grid, Self::scene_bounds(&scene));
        self.apply_outcome(out);
    }

    fn plugin_hit(&mut self, idx: usize, scene: &[Arc<WorldMesh>], hit: Hit) {
        let out = self.plugins.tool_click(idx, &self.map, &self.models, &self.selection, self.settings.grid, Self::scene_bounds(scene), &hit);
        self.apply_outcome(out);
    }

    /// Runs enabled "move" plugins over a drag delta.
    fn hook_move(&mut self, scene: &[Arc<WorldMesh>], bmin: Vec3, bmax: Vec3, mut wd: Vec3) -> Vec3 {
        let enabled: Vec<usize> = self
            .plugins
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == PluginKind::Move && self.settings.enabled_plugins.contains(&p.file))
            .map(|(i, _)| i)
            .collect();
        if enabled.is_empty() {
            return wd;
        }
        let ids = self.selection.clone();
        let bounds: HashMap<u64, (Vec3, Vec3)> = scene.iter().filter(|w| !ids.contains(&w.id)).map(|w| (w.id, (w.min, w.max))).collect();
        for i in enabled {
            wd = self.plugins.adjust_move(i, &self.map, &self.models, &ids, self.settings.grid, bounds.clone(), bmin, bmax, wd);
        }
        wd
    }

    /// The enabled gizmo plugin, if it should be active right now.
    pub(crate) fn active_gizmo(&self) -> Option<usize> {
        if self.tool != Tool::Select || self.selection.is_empty() || self.edit.is_some() || self.play.is_some() {
            return None;
        }
        self.plugins
            .plugins
            .iter()
            .position(|p| p.kind == PluginKind::Gizmo && self.settings.enabled_plugins.contains(&p.file))
    }

    /// Applies a plugin result without adding an undo step (live drags).
    fn apply_live(&mut self, out: Outcome) {
        if out.has_changes() {
            out.apply_to(&mut self.map);
            self.dirty = true;
        }
        if let Some(sel) = out.selection {
            self.selection = sel;
            self.prune_selection();
        }
        if let Some(s) = out.status {
            self.status = s;
        }
    }

    /// Routes pointer input to the gizmo plugin. Returns true when the gizmo
    /// owns the interaction and the viewport should ignore it.
    pub(crate) fn gizmo_mode_name(&self) -> &'static str {
        ["move", "rotate", "scale"][self.gizmo_mode.min(2) as usize]
    }

    fn sync_gizmo_state(&mut self) {
        let (m, l) = (self.gizmo_mode_name(), self.gizmo_local);
        self.plugins.set_gizmo(m, l);
    }

    fn gizmo_input(&mut self, slot: usize, view: ViewInfo, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) -> bool {
        self.sync_gizmo_state();
        let here = matches!(self.drag, Drag::Gizmo { slot: s } if s == slot);
        let Some(gi) = self.active_gizmo() else {
            if here {
                self.drag = Drag::None;
            }
            return false;
        };
        let mods = ctx.input(|i| i.modifiers);
        let grid = self.snap_step();
        if resp.drag_started_by(PointerButton::Primary) {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect_center(resp));
            let ev = event_map(&view, start, mods.shift, mods.command, mods.alt);
            let before = self.map.clone();
            let (out, taken) = self.plugins.gizmo_event(gi, "pointer_down", &self.map, &self.models, &self.selection, grid, Self::scene_bounds(scene), view, ev);
            if taken {
                self.gizmo_origs = self.selected_objects();
                self.undo.push(before);
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.dirty = true;
                self.apply_live(out);
                self.drag = Drag::Gizmo { slot };
                self.drag_cur = start;
                return true;
            }
            return false;
        }
        if !here {
            return false;
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            let ev = event_map(&view, self.drag_cur, mods.shift, mods.command, mods.alt);
            let (mut out, _) = self.plugins.gizmo_event(gi, "pointer_drag", &self.map, &self.models, &self.selection, grid, Self::scene_bounds(scene), view, ev);
            let origs = self.gizmo_origs.clone();
            self.collide_objects(scene, &origs, &mut out.changed);
            self.apply_live(out);
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            let ev = event_map(&view, self.drag_cur, mods.shift, mods.command, mods.alt);
            let (mut out, _) = self.plugins.gizmo_event(gi, "pointer_up", &self.map, &self.models, &self.selection, grid, Self::scene_bounds(scene), view, ev);
            let origs = self.gizmo_origs.clone();
            self.collide_objects(scene, &origs, &mut out.changed);
            self.apply_live(out);
            self.drag = Drag::None;
        }
        true
    }

    /// Draws the gizmo plugin's handles on top of a viewport.
    fn paint_gizmo(&mut self, painter: &egui::Painter, view: ViewInfo, scene: &[Arc<WorldMesh>]) {
        let Some(gi) = self.active_gizmo() else { return };
        self.sync_gizmo_state();
        let cmds = self.plugins.gizmo_draw(gi, &self.map, &self.models, &self.selection, self.snap_step(), Self::scene_bounds(scene), view);
        paint_cmds(painter, view, &cmds);
    }

    fn clear_tool_plugin_state(&mut self) {
        if let Tool::Plugin(i) = self.tool {
            if let Some(f) = self.plugins.plugins.get(i).map(|p| p.file.clone()) {
                self.plugins.clear_state_for(&f);
            }
        }
    }

    // ----- file operations -----------------------------------------------

    pub(crate) fn confirm_discard(&self) -> bool {
        if !self.dirty {
            return true;
        }
        rfd::MessageDialog::new()
            .set_title("Unsaved changes")
            .set_description("Discard unsaved changes to this map?")
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    pub(crate) fn reset_after_load(&mut self) {
        self.restore_offer = None;
        self.undo.clear();
        self.redo.clear();
        self.selection.clear();
        self.drag = Drag::None;
        self.edit = None;
        self.layout_pts.clear();
        self.active_texture = None;
    }

    fn new_map(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        self.starter_map();
        self.path = None;
        self.reset_after_load();
        self.status = "New map".into();
    }

    fn open_map(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Anvil map or package", &["anvil", "anvilpak"])
            .add_filter("Anvil map", &["anvil"])
            .add_filter("Shared package", &["anvilpak"])
            .pick_file()
        else {
            return;
        };
        self.open_path(p);
    }

    fn add_recent(&mut self, p: &Path) {
        let s = p.to_string_lossy().to_string();
        self.settings.recent_files.retain(|r| !r.eq_ignore_ascii_case(&s));
        self.settings.recent_files.insert(0, s);
        self.settings.recent_files.truncate(10);
        let _ = self.settings.save();
    }

    /// Opens an .anvil map or an .anvilpak package.
    pub(crate) fn open_path(&mut self, p: PathBuf) {
        let is_pak = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("anvilpak"));
        if is_pak {
            match crate::package::open_package(&p) {
                Ok(o) => {
                    let n = o.library.len();
                    for e in o.library {
                        self.models.forget(&e.path);
                        self.scene_cache.clear();
                        if let Some(x) = self.settings.model_library.iter_mut().find(|x| x.path == e.path) {
                            *x = e;
                        } else {
                            self.settings.model_library.push(e);
                        }
                    }
                    let _ = self.settings.save();
                    self.map = o.map;
                    self.path = None; // saving writes a normal .anvil next to the extracted files
                    self.dirty = false;
                    self.reset_after_load();
                    self.add_recent(&p);
                    self.status = format!("Opened package {} ({} model(s); files in {})", p.display(), n, o.dir.display());
                }
                Err(e) => self.status = e,
            }
            return;
        }
        match Map::load(&p) {
            Ok(mut m) => {
                let refit = crate::trim::refit_old_slabs(&mut m);
                self.map = m;
                self.path = Some(p.clone());
                self.dirty = refit > 0;
                self.reset_after_load();
                self.add_recent(&p);
                self.status = if refit > 0 {
                    format!("Opened {} (fixed {refit} floor / ceiling piece(s) of older buildings so they line up with the walls; save to keep it)", p.display())
                } else {
                    format!("Opened {}", p.display())
                };
            }
            Err(e) => self.status = e,
        }
    }

    pub(crate) fn save_package(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Shared Anvil package", &["anvilpak"])
            .set_file_name(format!("{}.anvilpak", self.map.name))
            .save_file()
        else {
            return;
        };
        self.status = match crate::package::save_package(&self.map, &self.settings, &p) {
            Ok(m) => {
                self.add_recent(&p);
                m
            }
            Err(e) => format!("Package failed: {e}"),
        };
    }

    fn save_to(&mut self, p: PathBuf) {
        match self.map.save(&p) {
            Ok(()) => {
                self.status = format!("Saved {}", p.display());
                self.add_recent(&p);
                self.path = Some(p);
                self.dirty = false;
            }
            Err(e) => self.status = e,
        }
    }

    fn save(&mut self) {
        match self.path.clone() {
            Some(p) => self.save_to(p),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Anvil map", &["anvil"])
            .set_file_name(format!("{}.anvil", self.map.name))
            .save_file()
        else {
            return;
        };
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            self.map.name = stem.to_string();
        }
        self.save_to(p);
    }

    pub(crate) fn export_unity(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Wavefront OBJ (Unity)", &["obj"])
            .set_file_name(format!("{}.obj", self.map.name))
            .save_file()
        else {
            return;
        };
        self.status = match export_unity::export(&self.map, &mut self.models, &self.settings, &p) {
            Ok(m) => m,
            Err(e) => format!("Export failed: {e}"),
        };
    }

    pub(crate) fn export_rbxmx(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Roblox XML model", &["rbxmx"])
            .set_file_name(format!("{}.rbxmx", self.map.name))
            .save_file()
        else {
            return;
        };
        self.status = match export_roblox::export_rbxmx(&self.map, &mut self.models, &self.settings, &p) {
            Ok(m) => m,
            Err(e) => format!("Export failed: {e}"),
        };
    }

    pub(crate) fn export_luau(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Luau script", &["lua", "luau"])
            .set_file_name(format!("{}_import.lua", self.map.name))
            .save_file()
        else {
            return;
        };
        self.status = match export_roblox::export_luau(&self.map, &mut self.models, &self.settings, &p) {
            Ok(m) => m,
            Err(e) => format!("Export failed: {e}"),
        };
    }

    pub(crate) fn copy_luau(&mut self, ctx: &egui::Context) {
        let s = export_roblox::luau_string(&self.map, &mut self.models, &self.settings);
        ctx.copy_text(s);
        self.status = "Roblox script copied. Paste it into Studio's Command Bar and press Enter.".into();
    }

    pub(crate) fn add_models(&mut self) {
        let Some(files) = rfd::FileDialog::new().add_filter("3D model", &["obj", "stl", "glb", "gltf"]).pick_files() else { return };
        for f in files {
            let path = f.to_string_lossy().to_string();
            self.models.forget(&path);
            self.scene_cache.clear();
            if !self.settings.model_library.iter().any(|m| m.path == path) {
                self.settings.model_library.push(ModelEntry { path: path.clone(), ..Default::default() });
            }
            match self.models.get(&path) {
                Ok(m) => self.status = format!("Added {} ({} triangles)", file_name(&path), m.tris.len()),
                Err(e) => self.status = format!("{}: {e}", file_name(&path)),
            }
            self.active_model = Some(path);
            self.tool = Tool::Prop;
        }
        let _ = self.settings.save();
    }

    // ----- playtest -------------------------------------------------------

    /// F5: a plugin's playtest if one offers it (the TRP plugin), else Anvil's own.
    fn playtest_pressed(&mut self, ctx: &egui::Context) {
        match self.plugins.playtest_plugin() {
            Some(i) => {
                let out = self.plugins.playtest(i, &self.map, &self.models, self.settings.grid);
                self.apply_outcome(out);
            }
            None => self.start_playtest(ctx),
        }
    }

    fn start_playtest(&mut self, ctx: &egui::Context) {
        if self.play.is_some() {
            return;
        }
        let scene = build_scene(&self.map, &mut self.models);
        let ups = 1.0 / self.settings.roblox_scale.max(0.0001);
        let spawn = self.map.objects.iter().filter(|o| !o.hidden).find_map(|o| match o.kind {
            Kind::Spawn { size } => Some((o.pos + Vec3::Y * (size.y * 0.5 + 0.05), o.rot.y.to_radians())),
            _ => None,
        });
        let (pos, yaw) = match spawn {
            Some((p, y)) => (p, -y),
            None => {
                let ground = pick_ray(&scene, self.cam.pos, Vec3::NEG_Y).map(|h| h.point.y).unwrap_or(self.work_y);
                (Vec3::new(self.cam.pos.x, ground + 0.05, self.cam.pos.z), self.cam.yaw)
            }
        };
        self.play = Some((Play::new(&scene, pos, yaw, ups), self.layout));
        self.layout = Layout::Single(0);
        self.drag = Drag::None;
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Confined));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(false));
        self.status = "Playtest: WASD move, mouse look, Space jump, Shift run, Esc to stop".into();
    }

    fn stop_playtest(&mut self, ctx: &egui::Context) {
        if let Some((_, layout)) = self.play.take() {
            self.layout = layout;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        self.status = "Playtest stopped".into();
    }

    fn play_frame(&mut self, ctx: &egui::Context, rect: Rect, ui: &mut egui::Ui, scene: &[Arc<WorldMesh>]) {
        let sens = 0.0025 * self.settings.mouse_sensitivity;
        let input = ctx.input(|i| {
            let mut look = Vec2::ZERO;
            let mut raw = false;
            for e in &i.raw.events {
                if let egui::Event::MouseMoved(d) = e {
                    look += Vec2::new(d.x, d.y);
                    raw = true;
                }
            }
            if !raw {
                let d = i.pointer.delta();
                look = Vec2::new(d.x, d.y);
            }
            let k = |key: Key| if i.key_down(key) { 1.0 } else { 0.0 };
            PlayInput {
                forward: k(Key::W) - k(Key::S),
                right: k(Key::D) - k(Key::A),
                jump: i.key_down(Key::Space),
                run: i.modifiers.shift,
                look: look * sens,
            }
        });
        let dt = ctx.input(|i| i.stable_dt);
        let Some((play, _)) = self.play.as_mut() else { return };
        play.step(&input, dt);
        let cam = play.camera();
        let speed = Vec2::new(play.vel.x, play.vel.z).length() * self.settings.roblox_scale;
        let grounded = play.grounded;
        let opts = RenderOpts {
            fov: self.settings.fov,
            grid: self.settings.grid,
            plane_y: self.work_y,
            show_grid: false,
            outlines: false,
            bg: self.settings.bg_3d,
            scale: self.settings.render_scale.clamp(0.25, 2.0),
            filter: self.settings.texture_filtering,
            lighting: self.lighting(),
        };
        let tex = self.renderer.render(ctx, rect, &cam, &opts, scene, &[], &HashMap::new(), &mut self.textures);
        let painter = ui.painter_at(rect);
        painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let c = rect.center();
        let w = Color32::from_white_alpha(200);
        painter.line_segment([c - vec2(7.0, 0.0), c + vec2(7.0, 0.0)], Stroke::new(1.5f32, w));
        painter.line_segment([c - vec2(0.0, 7.0), c + vec2(0.0, 7.0)], Stroke::new(1.5f32, w));
        painter.text(
            rect.min + vec2(10.0, 8.0),
            Align2::LEFT_TOP,
            format!(
                "PLAYTEST  ·  WASD move · mouse look · Space jump · Shift run · Esc exit\n{:.1} studs/s{}",
                speed,
                if grounded { "" } else { "  (airborne)" }
            ),
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        ctx.request_repaint();
    }

    // ----- keyboard -------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.play.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.stop_playtest(ctx);
            }
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let (cmd, shift, sec) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift, i.pointer.secondary_down()));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        // Ctrl+C / Ctrl+X / Ctrl+V arrive as copy / cut / paste events
        if self.edit.is_none() {
            let events = ctx.input(|i| i.events.clone());
            for ev in events {
                match ev {
                    egui::Event::Copy if shift => self.carve_selection(),
                    egui::Event::Copy => self.copy_selection(ctx, false),
                    egui::Event::Cut => self.copy_selection(ctx, true),
                    egui::Event::Paste(text) => {
                        let text = if crate::clipboard::parse(&text).is_some() { Some(text) } else { self.clip_text.clone() };
                        if let Some(t) = text {
                            self.paste_text(&t, true);
                        }
                    }
                    _ => {}
                }
            }
        }
        if pressed(Key::F5) {
            if shift {
                self.start_playtest(ctx);
            } else {
                self.playtest_pressed(ctx);
            }
            return;
        }
        if cmd {
            if pressed(Key::Z) {
                if shift {
                    self.redo()
                } else {
                    self.undo()
                }
            }
            if pressed(Key::Y) {
                self.redo();
            }
            if pressed(Key::U) {
                self.uv.open = !self.uv.open;
            }
            if pressed(Key::S) {
                if shift {
                    self.save_as()
                } else {
                    self.save()
                }
            }
            if pressed(Key::O) {
                self.open_map();
            }
            if pressed(Key::N) {
                self.new_map();
            }
            if pressed(Key::G) {
                if shift {
                    self.ungroup_selection()
                } else {
                    self.group_selection()
                }
            }
            if pressed(Key::C) && shift {
                self.carve_selection();
            }
            if self.edit.is_some() {
                if pressed(Key::A) {
                    self.edit_select_all();
                }
                if pressed(Key::E) {
                    self.edit_extrude();
                }
                return;
            }
            if pressed(Key::D) && !self.selection.is_empty() {
                self.duplicate_in_place();
            }
            if pressed(Key::A) {
                self.selection = self.map.objects.iter().filter(|o| !o.hidden).map(|o| o.id).collect();
            }
            if pressed(Key::H) {
                if let Some(id) = self.selection.first().copied() {
                    self.hollow(id);
                }
            }
            return;
        }
        if sec {
            return; // WASD flying in the 3D view
        }
        if pressed(Key::Tab) {
            self.toggle_edit();
            return;
        }
        if self.edit.is_some() {
            if pressed(Key::Escape) {
                self.edit = None;
                self.drag = Drag::None;
                self.status = "Left mesh edit mode".into();
            }
            if pressed(Key::Num1) {
                self.set_edit_mode(0);
            }
            if pressed(Key::Num2) {
                self.set_edit_mode(1);
            }
            if pressed(Key::Num3) {
                self.set_edit_mode(2);
            }
            self.gizmo_keys(ctx);
            if pressed(Key::Delete) {
                self.edit_delete();
            }
            if pressed(Key::M) {
                self.edit_merge();
            }
            if pressed(Key::N) {
                self.edit_flip();
            }
            if pressed(Key::F) {
                self.focus();
            }
            return;
        }
        if matches!(self.tool, Tool::Layout | Tool::Polygon) && !self.layout_pts.is_empty() {
            if pressed(Key::Enter) {
                self.close_layout();
            }
            if pressed(Key::Backspace) {
                self.layout_pts.pop();
            }
            if pressed(Key::Escape) {
                self.layout_pts.clear();
                self.status = "Outline cancelled".into();
                return;
            }
        }
        if pressed(Key::Delete) {
            self.delete_selection();
        }
        if pressed(Key::Escape) && self.poly_extrude.is_some() {
            self.poly_extrude = None;
            self.status = "Polygon cancelled".into();
            return;
        }
        if pressed(Key::Escape) && self.tp_pending.is_some() {
            self.tp_pending = None;
            self.status = "Teleport left without a destination (set it in the Inspector)".into();
            return;
        }
        if pressed(Key::Escape) && self.stair_start.is_some() {
            self.stair_start = None;
            self.status = "Stairs cancelled".into();
            return;
        }
        if pressed(Key::Escape) {
            if matches!(self.drag, Drag::None) {
                self.selection.clear();
            }
            self.drag = Drag::None;
            self.clear_tool_plugin_state();
        }
        if pressed(Key::OpenBracket) {
            self.settings.grid = (self.settings.grid * 0.5).max(0.125);
            let _ = self.settings.save();
        }
        if pressed(Key::CloseBracket) {
            self.settings.grid = (self.settings.grid * 2.0).min(256.0);
            let _ = self.settings.save();
        }
        if self.tool == Tool::Select {
            self.gizmo_keys(ctx);
        }
        if self.is_place_tool() && !cmd && pressed(Key::R) {
            let step = if ctx.input(|i| i.modifiers.alt) { 90.0 } else { 15.0 };
            self.turn_placement(if shift { -step } else { step });
        }
        for t in Tool::BASIC {
            if let Some((k, _)) = t.key() {
                if pressed(k) {
                    self.tool = t;
                    self.layout_pts.clear();
                }
            }
        }
        if pressed(Key::F) {
            self.focus();
        }
        if pressed(Key::H) {
            if ctx.input(|i| i.modifiers.shift) {
                self.hide_unselected();
            } else {
                self.hide_selection();
            }
        }
        if pressed(Key::U) {
            self.unhide_all();
        }
        if pressed(Key::F1) {
            self.show_help = !self.show_help;
        }
        // Nudge with arrow keys in the hovered 2D view (Top view for 3D).
        let view = match self.hovered_slot {
            Some(s @ 1..=3) => self.views[s - 1],
            _ => self.views[0],
        };
        let g = self.settings.grid;
        let mut d = Vec2::ZERO;
        if pressed(Key::ArrowLeft) {
            d.x -= g;
        }
        if pressed(Key::ArrowRight) {
            d.x += g;
        }
        if pressed(Key::ArrowUp) {
            d.y += g;
        }
        if pressed(Key::ArrowDown) {
            d.y -= g;
        }
        if d != Vec2::ZERO && !self.selection.is_empty() {
            self.push_undo();
            let wd = view.uv_delta_to_world(d);
            let origs = self.sel_positions();
            let scene = self.scene_cache.build(&self.map, &mut self.models);
            let wd = self.collide_translate(&scene, &origs, wd);
            for id in self.selection.clone() {
                if let Some(o) = self.map.get_mut(id) {
                    o.pos += wd;
                }
            }
        }
    }

    // ----- panels ---------------------------------------------------------

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                self.brand_header(ui);
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("New map").shortcut_text("Ctrl+N")).clicked() {
                        ui.close_menu();
                        self.new_map();
                    }
                    if ui.add(egui::Button::new("Open…").shortcut_text("Ctrl+O")).clicked() {
                        ui.close_menu();
                        self.open_map();
                    }
                    if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                        ui.close_menu();
                        self.save();
                    }
                    if ui.add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S")).clicked() {
                        ui.close_menu();
                        self.save_as();
                    }
                    if ui.button("Save package for sharing (.anvilpak)…").on_hover_text("One file with the map and every texture and model it uses").clicked() {
                        ui.close_menu();
                        self.save_package();
                    }
                    let recent = self.settings.recent_files.clone();
                    ui.menu_button("Open recent", |ui| {
                        if recent.is_empty() {
                            ui.label(RichText::new("Nothing yet").weak());
                        }
                        for r in recent {
                            if ui.button(file_name(&r)).on_hover_text(&r).clicked() {
                                ui.close_menu();
                                if self.confirm_discard() {
                                    self.open_path(PathBuf::from(&r));
                                }
                            }
                        }
                    });
                    if ui.button("Open autosave folder").clicked() {
                        ui.close_menu();
                        let d = crate::autosave::dir();
                        let _ = std::fs::create_dir_all(&d);
                        let _ = std::process::Command::new("explorer").arg(d).spawn();
                    }
                    ui.separator();
                    if ui.button("Half-Life / Source content…").clicked() {
                        ui.close_menu();
                        self.open_content();
                    }
                    ui.separator();
                    if ui.button("Export…").clicked() {
                        ui.close_menu();
                        self.show_export = true;
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.close_menu();
                        if self.confirm_discard() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add(egui::Button::new("UV editor…").shortcut_text("Ctrl+U")).clicked() {
                        ui.close_menu();
                        self.uv.open = true;
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                        ui.close_menu();
                        self.undo();
                    }
                    if ui.add(egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() {
                        ui.close_menu();
                        self.redo();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
                        ui.close_menu();
                        self.copy_selection(ui.ctx(), false);
                    }
                    if ui.add(egui::Button::new("Cut").shortcut_text("Ctrl+X")).clicked() {
                        ui.close_menu();
                        self.copy_selection(ui.ctx(), true);
                    }
                    if ui.add(egui::Button::new("Paste").shortcut_text("Ctrl+V")).clicked() {
                        ui.close_menu();
                        if let Some(t) = self.clip_text.clone() {
                            self.paste_text(&t, false);
                        }
                    }
                    if ui.add(egui::Button::new("Duplicate in place").shortcut_text("Ctrl+D")).clicked() {
                        ui.close_menu();
                        if !self.selection.is_empty() {
                            self.duplicate_in_place();
                        }
                    }
                    if ui.add(egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                        ui.close_menu();
                        self.delete_selection();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Group into folder").shortcut_text("Ctrl+G")).clicked() {
                        ui.close_menu();
                        self.group_selection();
                    }
                    if ui.add(egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G")).clicked() {
                        ui.close_menu();
                        self.ungroup_selection();
                    }
                    ui.separator();
                    ui.menu_button("Boolean (first selected = base)", |ui| {
                        if ui.button("Union: merge the others into it").clicked() {
                            ui.close_menu();
                            self.boolean_selection(crate::mesh_csg::Op::Union);
                        }
                        if ui.button("Subtract: cut the others out of it").clicked() {
                            ui.close_menu();
                            self.boolean_selection(crate::mesh_csg::Op::Subtract);
                        }
                        if ui.button("Intersect: keep only the overlap").clicked() {
                            ui.close_menu();
                            self.boolean_selection(crate::mesh_csg::Op::Intersect);
                        }
                    });
                    if ui.add(egui::Button::new("Boolean: carve with selection").shortcut_text("Ctrl+Shift+C")).clicked() {
                        ui.close_menu();
                        self.carve_selection();
                    }
                    if ui.add(egui::Button::new("Make hollow").shortcut_text("Ctrl+H")).clicked() {
                        ui.close_menu();
                        if let Some(id) = self.selection.first().copied() {
                            self.hollow(id);
                        }
                    }
                    if ui.add(egui::Button::new("Edit mesh (vertices / faces)").shortcut_text("Tab")).clicked() {
                        ui.close_menu();
                        self.toggle_edit();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Select all").shortcut_text("Ctrl+A")).clicked() {
                        ui.close_menu();
                        self.selection = self.map.objects.iter().filter(|o| !o.hidden).map(|o| o.id).collect();
                    }
                    if ui.add(egui::Button::new("Select none").shortcut_text("Esc")).clicked() {
                        ui.close_menu();
                        self.selection.clear();
                    }
                    if ui.add(egui::Button::new("Hide selected").shortcut_text("H")).clicked() {
                        ui.close_menu();
                        self.hide_selection();
                    }
                    if ui.button("Randomize offset / rotation…").on_hover_text("Small random nudges so clutter looks hand-placed").clicked() {
                        ui.close_menu();
                        self.scatter.open = true;
                    }
                    if ui.add(egui::Button::new("Hide everything else").shortcut_text("Shift+H")).clicked() {
                        ui.close_menu();
                        self.hide_unselected();
                    }
                    if ui.add(egui::Button::new("Unhide all").shortcut_text("U")).clicked() {
                        ui.close_menu();
                        self.unhide_all();
                    }
                });
                ui.menu_button("Team", |ui| {
                    let label = if self.team.is_some() { "Team Create (in a session)…" } else { "Team Create…" };
                    if ui.button(label).on_hover_text("Edit this map together with friends on your network").clicked() {
                        ui.close_menu();
                        self.team_ui.open = true;
                    }
                });
                ui.menu_button("View", |ui| {
                    let items = [
                        ("Four views", Layout::Quad),
                        ("3D only", Layout::Single(0)),
                        ("Top only", Layout::Single(1)),
                        ("Front only", Layout::Single(2)),
                        ("Side only", Layout::Single(3)),
                    ];
                    for (label, l) in items {
                        if ui.selectable_label(self.layout == l, label).clicked() {
                            ui.close_menu();
                            self.layout = l;
                        }
                    }
                    ui.separator();
                    ui.menu_button("Theme", |ui| {
                        for (i, name) in crate::themes::THEME_NAMES.iter().enumerate() {
                            if ui.selectable_label(self.settings.theme == i as u8, *name).clicked() {
                                self.settings.theme = i as u8;
                                let _ = self.settings.save();
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Accent");
                            if ui.color_edit_button_srgb(&mut self.settings.accent).changed() {
                                let _ = self.settings.save();
                            }
                        });
                    });
                    ui.menu_button("3D lighting", |ui| {
                        for (i, name) in crate::lighting::QUALITY_NAMES.iter().enumerate() {
                            if ui.selectable_label(self.settings.light_quality == i as u8, *name).clicked() {
                                self.settings.light_quality = i as u8;
                                let _ = self.settings.save();
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Sun, fog and sky: Settings > Viewports").small().weak());
                    });
                    if ui.add(egui::Button::new("Focus selection").shortcut_text("F")).clicked() {
                        ui.close_menu();
                        self.focus();
                    }
                    if ui.button("Reset 3D camera").clicked() {
                        ui.close_menu();
                        self.cam = Camera::looking_at(Vec3::new(-34.0, 26.0, 34.0), Vec3::new(0.0, 2.0, 0.0));
                    }
                });
                if ui.button("Content").on_hover_text("Mounted Half-Life / Garry's Mod props and textures").clicked() {
                    if self.show_content {
                        self.show_content = false;
                    } else {
                        self.open_content();
                    }
                }
                if ui.button("Textures").clicked() {
                    self.show_textures = !self.show_textures;
                }
                if ui.button("Plugins").clicked() {
                    self.show_plugins = !self.show_plugins;
                }
                if ui.button("Export").clicked() {
                    self.show_export = !self.show_export;
                }
                if ui.button("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                if ui.button("Help").clicked() {
                    self.show_help = !self.show_help;
                }
                ui.separator();
                let plugin_play = self.plugins.playtest_plugin().map(|i| self.plugins.plugins[i].name.clone());
                let tip = match &plugin_play {
                    Some(n) => format!("Playtest with the {n} plugin (Shift+F5: Anvil's own)"),
                    None => "Walk around the map".to_string(),
                };
                if ui.add(egui::Button::new(RichText::new("▶ Playtest").strong()).shortcut_text("F5")).on_hover_text(tip).clicked() {
                    self.playtest_pressed(ctx);
                }
                if plugin_play.is_some() && ui.small_button("Anvil's own").on_hover_text("Anvil's built-in playtest (Shift+F5)").clicked() {
                    self.start_playtest(ctx);
                }
                ui.separator();
                self.gizmo_toolbar(ui);
            });
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let tool = match self.tool {
                    Tool::Plugin(i) => self.plugins.plugins.get(i).map(|p| p.name.clone()).unwrap_or_default(),
                    t => t.label().to_string(),
                };
                let mode = if self.edit.is_some() { " (mesh edit)" } else { "" };
                ui.label(RichText::new(format!("{tool}{mode}")).strong());
                ui.separator();
                ui.label(format!("Grid {}  [ ]", fmt_num(self.settings.grid)));
                ui.separator();
                ui.label(format!("Work plane Y {}", fmt_num(self.work_y)));
                ui.separator();
                ui.label(&self.cursor_text);
                if self.team_status(ui) {
                    self.team_ui.open = true;
                }
                ui.separator();
                ui.label(format!("{} objects, {} selected", self.map.objects.len(), self.selection.len()));
                ui.separator();
                ui.label(RichText::new(&self.status).weak());
            });
        });
    }

    fn library_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("library").resizable(true).default_width(250.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("lib_scroll").show(ui, |ui| {
                ui.heading("Tools");
                ui.horizontal_wrapped(|ui| {
                    for t in Tool::BASIC {
                        let label = match t.key() {
                            Some((_, k)) => format!("{k} {}", t.label()),
                            None => t.label().to_string(),
                        };
                        if ui.selectable_label(self.tool == t, label).clicked() {
                            self.tool = t;
                            self.layout_pts.clear();
                        }
                    }
                    for (i, p) in self.plugins.plugins.iter().enumerate() {
                        if p.kind == PluginKind::Tool && ui.selectable_label(self.tool == Tool::Plugin(i), &p.name).on_hover_text(&p.description).clicked() {
                            self.tool = Tool::Plugin(i);
                            self.layout_pts.clear();
                        }
                    }
                });
                let hint = match self.tool {
                    Tool::Plugin(i) => self.plugins.plugins.get(i).map(|p| p.description.clone()).unwrap_or_default(),
                    t => t.hint().to_string(),
                };
                self.tool_settings_panel(ui, &hint);
                ui.add_space(4.0);
                ui.label("Brush shape");
                ui.horizontal_wrapped(|ui| {
                    for s in Shape::ALL {
                        if ui.selectable_label(self.brush_shape == s && self.active_preset.is_none(), s.label()).clicked() {
                            self.brush_shape = s;
                            self.active_preset = None;
                            self.tool = Tool::Block;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Grid");
                    if ui.small_button("-").clicked() {
                        self.settings.grid = (self.settings.grid * 0.5).max(0.125);
                        let _ = self.settings.save();
                    }
                    ui.label(fmt_num(self.settings.grid));
                    if ui.small_button("+").clicked() {
                        self.settings.grid = (self.settings.grid * 2.0).min(256.0);
                        let _ = self.settings.save();
                    }
                    if ui.checkbox(&mut self.settings.snap, "Snap").changed() {
                        let _ = self.settings.save();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Work plane Y");
                    ui.add(egui::DragValue::new(&mut self.work_y).speed(0.25));
                })
                .response
                .on_hover_text("Height used for walls, buildings, top-view placement and the 3D ground grid");

                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading("Models");
                    if ui.small_button("Add…").clicked() {
                        self.add_models();
                    }
                    if ui.small_button("Half-Life…").on_hover_text("Browse mounted Half-Life / GMod props").clicked() {
                        self.open_content();
                    }
                });
                if self.settings.model_library.is_empty() {
                    ui.label(RichText::new("Add .obj or .stl files to place them as props.").small().weak());
                }
                let mut remove: Option<usize> = None;
                let lib_paths: Vec<String> = self.settings.model_library.iter().map(|m| m.path.clone()).collect();
                let uictx = ui.ctx().clone();
                for (i, path) in lib_paths.iter().enumerate() {
                    let thumb = self.model_thumb(&uictx, path);
                    let err = matches!(self.models.peek(path), Some(Err(_)));
                    ui.horizontal(|ui| {
                        let mut name = self.settings.model_library.iter().find(|e| e.path == *path).map(|e| e.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| file_name(path));
                        if err {
                            name = format!("(!) {name}");
                        }
                        let sel = self.active_model.as_deref() == Some(path.as_str());
                        let pick = match thumb {
                            Some(t) => ui.add(egui::ImageButton::new((t, vec2(40.0, 40.0))).selected(sel)).on_hover_text(path).clicked(),
                            None => {
                                ui.add_sized([40.0, 40.0], egui::Spinner::new());
                                false
                            }
                        };
                        if ui.selectable_label(sel, name).on_hover_text(path).clicked() || pick {
                            self.active_model = Some(path.clone());
                            self.tool = Tool::Prop;
                        }
                        if ui.small_button("x").on_hover_text("Remove from library").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    let m = self.settings.model_library.remove(i);
                    if self.active_model.as_deref() == Some(m.path.as_str()) {
                        self.active_model = None;
                    }
                    let _ = self.settings.save();
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading("Textures");
                    if ui.small_button("Open…").clicked() {
                        self.show_textures = true;
                    }
                });
                let mut pick = None;
                let uictx = ui.ctx().clone();
                for t in &self.map.textures {
                    let sel = self.active_texture == Some(t.id);
                    ui.horizontal(|ui| {
                        if let Some(id) = crate::windows::preview(&uictx, &mut self.textures, &mut self.tex_previews, &t.image) {
                            ui.dnd_drag_source(egui::Id::new(("texdrag", t.id)), crate::faces_ui::TexDrop(t.id), |ui| {
                                ui.add(egui::Image::new((id, vec2(22.0, 22.0))));
                            })
                            .response
                            .on_hover_text("Drag onto an object in a view (hold Alt for just one face)");
                        }
                        if ui.selectable_label(sel, &t.name).on_hover_text("Active texture for decals and 'Apply'").clicked() {
                            pick = Some(t.id);
                        }
                    });
                }
                if let Some(id) = pick {
                    self.active_texture = Some(id);
                }
                if self.map.textures.is_empty() {
                    ui.label(RichText::new("No textures yet: Textures > Add texture…").small().weak());
                }
            });
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        if self.edit.is_some() {
            self.edit_panel(ui);
            return;
        }
        if self.tool == Tool::Face {
            self.face_sel.retain(|f| self.map.get(f.0).is_some());
            if self.face_sel.is_empty() {
                ui.label(RichText::new("Faces tool: click faces to texture them separately.").weak());
            } else {
                let targets = self.face_sel.clone();
                self.face_tex_panel(ui, targets);
                if ui.small_button("Clear face selection").clicked() {
                    self.face_sel.clear();
                }
            }
            return;
        }
        if self.selection.is_empty() {
            ui.label(RichText::new("Nothing selected.").weak());
            return;
        }
        if self.inspector_hide_row(ui) || self.selection.is_empty() {
            return;
        }
        let before_objs = self.selected_objects();
        let mut changed = false;
        let mut save_preset = false;
        let mut rotate = false;
        let mut snap_now = false;
        let mut hollow = false;
        let mut pick_model = false;
        let mut to_mesh = false;
        let mut edit_mesh = false;
        let mut clear_holes = false;
        let mut remove_hole: Option<usize> = None;
        let n = self.selection.len();
        let id = self.selection[0];
        let lib = &self.settings.model_library;
        let folders: Vec<(u64, String)> = self.map.folders.iter().map(|f| (f.id, self.map.folder_path(f.id))).collect();
        let textures: Vec<(u64, String)> = self.map.textures.iter().map(|t| (t.id, t.name.clone())).collect();
        let dests: Vec<(u64, String)> = self.teleport_dests().into_iter().filter(|d| d.0 != id).collect();
        let speed = (self.settings.grid as f64 * 0.25).max(0.01);
        if n > 1 {
            ui.label(RichText::new(format!("{n} selected. Color, material, texture, folder and flags apply to all.")).small().weak());
        }
        let mut shared = None;
        if let Some(o) = self.map.get_mut(id) {
            let shared_before = (o.color, o.material, o.transparency, o.collide, o.anchored, o.folder, o.texture);
            egui::Grid::new("insp_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Name");
                changed |= ui.text_edit_singleline(&mut o.name).changed();
                ui.end_row();
                ui.label("Type");
                ui.label(o.type_label());
                ui.end_row();
                ui.label("Folder");
                let cur = folders.iter().find(|f| Some(f.0) == o.folder).map(|f| f.1.clone()).unwrap_or_else(|| "(none)".into());
                egui::ComboBox::from_id_salt("insp_folder").selected_text(cur).width(150.0).show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut o.folder, None, "(none)").changed();
                    for (fid, path) in &folders {
                        changed |= ui.selectable_value(&mut o.folder, Some(*fid), path).changed();
                    }
                });
                ui.end_row();
                ui.label("Position");
                changed |= vec3_edit(ui, &mut o.pos, speed, None);
                ui.end_row();
                ui.label("Rotation");
                changed |= vec3_edit(ui, &mut o.rot, 1.0, None);
                ui.end_row();
                match &mut o.kind {
                    Kind::Brush { shape, size, steps, holes } => {
                        ui.label("Shape");
                        changed |= shape_combo(ui, "insp_shape", shape);
                        ui.end_row();
                        ui.label("Size");
                        changed |= vec3_edit(ui, size, speed, Some(0.01));
                        ui.end_row();
                        if *shape == Shape::Stairs {
                            ui.label("Steps");
                            changed |= ui.add(egui::DragValue::new(steps).range(1..=128)).changed();
                            ui.end_row();
                        }
                        if !holes.is_empty() {
                            ui.label(format!("Openings ({})", holes.len()));
                            ui.vertical(|ui| {
                                for (i, h) in holes.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("#{}", i + 1));
                                        if ui.small_button("x").on_hover_text("Remove this opening").clicked() {
                                            remove_hole = Some(i);
                                        }
                                    });
                                    ui.label(RichText::new("min").small());
                                    changed |= vec3_edit(ui, &mut h.min, speed, None);
                                    ui.label(RichText::new("max").small());
                                    changed |= vec3_edit(ui, &mut h.max, speed, None);
                                }
                                clear_holes = ui.small_button("Remove all openings").clicked();
                            });
                            ui.end_row();
                        }
                    }
                    Kind::Mesh { verts, faces } => {
                        ui.label("Mesh");
                        ui.label(format!("{} vertices, {} faces", verts.len(), faces.len()));
                        ui.end_row();
                    }
                    Kind::Prop { model, scale, roblox_mesh_id } => {
                        ui.label("Model");
                        ui.horizontal(|ui| {
                            ui.label(file_name(model)).on_hover_text(model.as_str());
                            if ui.small_button("…").on_hover_text("Use the selected library model").clicked() {
                                pick_model = true;
                            }
                        });
                        ui.end_row();
                        ui.label("Scale");
                        changed |= vec3_edit(ui, scale, 0.01, Some(0.001));
                        ui.end_row();
                        ui.label("Roblox Mesh ID");
                        let hint = lib
                            .iter()
                            .find(|m| m.path == *model)
                            .map(|m| m.roblox_mesh_id.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "from library".into());
                        changed |= ui.add(egui::TextEdit::singleline(roblox_mesh_id).hint_text(hint)).changed();
                        ui.end_row();
                    }
                    Kind::Decal { size } => {
                        ui.label("Size");
                        changed |= vec2_edit(ui, size, speed, Some(0.01), ["w", "h"]);
                        ui.end_row();
                    }
                    Kind::Light { range, brightness } => {
                        ui.label("Range");
                        changed |= ui.add(egui::DragValue::new(range).speed(0.25).range(0.0..=1000.0)).changed();
                        ui.end_row();
                        ui.label("Brightness");
                        changed |= ui.add(egui::DragValue::new(brightness).speed(0.05).range(0.0..=100.0)).changed();
                        ui.end_row();
                    }
                    Kind::Spawn { size } => {
                        ui.label("Size");
                        changed |= vec3_edit(ui, size, speed, Some(0.01));
                        ui.end_row();
                    }
                    k @ (Kind::Prompt { .. } | Kind::Teleport { .. } | Kind::Target {}) => {
                        changed |= crate::entities::entity_ui(ui, k, &dests, speed);
                    }
                }
                ui.label("Color");
                changed |= ui.color_edit_button_srgb(&mut o.color).changed();
                ui.end_row();
                ui.label("Material");
                changed |= material_combo(ui, "insp_mat", &mut o.material);
                ui.end_row();
                ui.label("Texture");
                let cur = textures.iter().find(|t| Some(t.0) == o.texture).map(|t| t.1.clone()).unwrap_or_else(|| "(none)".into());
                egui::ComboBox::from_id_salt("insp_tex").selected_text(cur).width(150.0).show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut o.texture, None, "(none)").changed();
                    for (tid, name) in &textures {
                        changed |= ui.selectable_value(&mut o.texture, Some(*tid), name).changed();
                    }
                });
                ui.end_row();
                if o.texture.is_some() || !o.face_tex.is_empty() {
                    ui.label("Texture settings");
                    ui.vertical(|ui| {
                        changed |= crate::faces_ui::tex_params_ui(ui, &mut o.tex_params, "obj_tex_params");
                        if ui.small_button("Open UV editor").on_hover_text("Drag the texture into place (Ctrl+U)").clicked() {
                            self.uv.open = true;
                        }
                        if !o.face_tex.is_empty() {
                            let n = o.face_tex.iter().filter(|f| f.is_some()).count();
                            ui.label(RichText::new(format!("{n} face(s) have their own texture (Faces tool)")).small().weak());
                        }
                    });
                    ui.end_row();
                }
                ui.label("Transparency");
                changed |= ui.add(egui::Slider::new(&mut o.transparency, 0.0..=1.0)).changed();
                ui.end_row();
                ui.label("Flags");
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut o.collide, "CanCollide").changed();
                    changed |= ui.checkbox(&mut o.anchored, "Anchored").changed();
                });
                ui.end_row();
            });
            let shared_after = (o.color, o.material, o.transparency, o.collide, o.anchored, o.folder, o.texture);
            if shared_after != shared_before {
                shared = Some(shared_after);
            }
            ui.horizontal_wrapped(|ui| {
                rotate = ui.button("Rotate 90° Y").clicked();
                snap_now = ui.button("Snap to grid").clicked();
                match &o.kind {
                    Kind::Brush { shape, .. } => {
                        save_preset = ui.button("Save as preset").clicked();
                        if *shape == Shape::Box {
                            hollow = ui.button("Make hollow").clicked();
                        }
                        to_mesh = ui.button("Convert to editable mesh").clicked();
                    }
                    Kind::Mesh { .. } => {
                        edit_mesh = ui.button("Edit mesh (Tab)").clicked();
                    }
                    _ => {}
                }
            });
        }
        if n == 1 {
            self.text_inspector(ui, id);
        }
        if let Some((c, m, t, col, anc, folder, tex)) = shared {
            for sid in self.selection.clone() {
                if let Some(o) = self.map.get_mut(sid) {
                    o.color = c;
                    o.material = m;
                    o.transparency = t;
                    o.collide = col;
                    o.anchored = anc;
                    o.folder = folder;
                    o.texture = tex;
                }
            }
        }
        if let Some(i) = remove_hole {
            if let Some(Object { kind: Kind::Brush { holes, .. }, .. }) = self.map.get_mut(id) {
                if i < holes.len() {
                    holes.remove(i);
                    changed = true;
                }
            }
        }
        if clear_holes {
            if let Some(Object { kind: Kind::Brush { holes, .. }, .. }) = self.map.get_mut(id) {
                holes.clear();
                changed = true;
            }
        }
        if changed {
            if !self.inspector_editing {
                self.push_undo_restoring(before_objs);
                self.inspector_editing = true;
            }
            self.dirty = true;
        }
        if rotate {
            self.push_undo();
            let origs = self.selected_objects();
            let mut turned = origs.clone();
            for o in &mut turned {
                o.rot.y = (o.rot.y + 90.0) % 360.0;
            }
            let scene = self.scene_cache.build(&self.map, &mut self.models);
            self.collide_objects(&scene, &origs, &mut turned);
            for o in turned {
                if let Some(x) = self.map.get_mut(o.id) {
                    *x = o;
                }
            }
        }
        if snap_now {
            self.push_undo();
            let g = self.settings.grid;
            for sid in self.selection.clone() {
                if let Some(o) = self.map.get_mut(sid) {
                    o.pos = Vec3::new(snap(o.pos.x, g * 0.5), snap(o.pos.y, g * 0.5), snap(o.pos.z, g * 0.5));
                }
            }
        }
        if pick_model {
            if let Some(m) = self.active_model.clone() {
                self.push_undo();
                if let Some(Object { kind: Kind::Prop { model, .. }, .. }) = self.map.get_mut(id) {
                    *model = m;
                }
            } else {
                self.status = "Select a model in the Library first".into();
            }
        }
        if save_preset {
            if let Some(o) = self.map.get(id) {
                if let Kind::Brush { shape, size, steps, .. } = o.kind {
                    self.settings.presets.push(Preset { name: o.name.clone(), shape, size, color: o.color, material: o.material, steps, transparency: o.transparency });
                    let _ = self.settings.save();
                    self.status = format!("Saved preset '{}'", o.name);
                }
            }
        }
        if hollow {
            self.hollow(id);
        }
        if to_mesh {
            self.convert_to_mesh(id);
        }
        if edit_mesh {
            self.toggle_edit();
        }
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        let add = ui.input(|i| i.modifiers.command || i.modifiers.shift);
        if let Some((fid, mut name)) = self.rename_folder.take() {
            let mut keep = true;
            ui.horizontal(|ui| {
                ui.label("Folder name");
                let r = ui.text_edit_singleline(&mut name);
                if ui.button("OK").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                    if let Some(f) = self.map.folders.iter_mut().find(|f| f.id == fid) {
                        f.name = name.clone();
                        self.dirty = true;
                    }
                    keep = false;
                }
            });
            if keep {
                self.rename_folder = Some((fid, name));
            }
        }
        let mut act = OutAct::default();
        self.outliner_level(ui, None, 0, &mut act);
        if let Some(id) = act.toggle {
            self.push_undo();
            if let Some(o) = self.map.get_mut(id) {
                o.hidden = !o.hidden;
            }
        }
        if let Some(fid) = act.select_folder {
            let ids = self.map.folder_objects(fid);
            if add {
                for id in ids {
                    if !self.selection.contains(&id) {
                        self.selection.push(id);
                    }
                }
            } else {
                self.selection = ids;
            }
        }
        if let Some(fid) = act.hide_folder {
            self.push_undo();
            let ids = self.map.folder_objects(fid);
            let hide = ids.iter().any(|id| self.map.get(*id).is_some_and(|o| !o.hidden));
            for id in ids {
                if let Some(o) = self.map.get_mut(id) {
                    o.hidden = hide;
                }
            }
        }
        if let Some(fid) = act.ungroup {
            self.push_undo();
            self.map.remove_folder(fid);
        }
        if let Some(fid) = act.rename {
            let name = self.map.folder(fid).map(|f| f.name.clone()).unwrap_or_default();
            self.rename_folder = Some((fid, name));
        }
        if let Some(id) = act.click {
            let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            let order = self.outliner_order();
            self.selection = outliner_pick(&self.selection, &order, self.outliner_anchor, id, shift, ctrl);
            self.outliner_anchor = Some(id);
        }
    }

    /// Every object in the order the object list shows them (folders first,
    /// as nested, then loose objects), whether or not a folder is collapsed.
    fn outliner_order(&self) -> Vec<u64> {
        fn walk(map: &Map, parent: Option<u64>, depth: usize, out: &mut Vec<u64>) {
            if depth > 32 {
                return;
            }
            for f in map.folders.iter().filter(|f| f.parent == parent) {
                walk(map, Some(f.id), depth + 1, out);
            }
            out.extend(map.objects.iter().filter(|o| map.folder_of(o) == parent).map(|o| o.id));
        }
        let mut out = Vec::new();
        walk(&self.map, None, 0, &mut out);
        out
    }

    fn outliner_level(&self, ui: &mut egui::Ui, parent: Option<u64>, depth: usize, act: &mut OutAct) {
        if depth > 32 {
            return;
        }
        for f in self.map.folders.iter().filter(|f| f.parent == parent) {
            let id = ui.make_persistent_id(("anvil_folder", f.id));
            let count = self.map.folder_objects(f.id).len();
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    if ui
                        .selectable_label(false, RichText::new(format!("▣ {}  ({count})", f.name)).strong())
                        .on_hover_text("Select everything in this folder")
                        .clicked()
                    {
                        act.select_folder = Some(f.id);
                    }
                    if ui.small_button("hide").on_hover_text("Hide / show the folder").clicked() {
                        act.hide_folder = Some(f.id);
                    }
                    if ui.small_button("rename").clicked() {
                        act.rename = Some(f.id);
                    }
                    if ui.small_button("ungroup").on_hover_text("Remove the folder, keep its contents").clicked() {
                        act.ungroup = Some(f.id);
                    }
                })
                .body(|ui| self.outliner_level(ui, Some(f.id), depth + 1, act));
        }
        for o in self.map.objects.iter().filter(|o| self.map.folder_of(o) == parent) {
            ui.horizontal(|ui| {
                if crate::hiding::eye_button(ui, !o.hidden).clicked() {
                    act.toggle = Some(o.id);
                }
                let sel = self.selection.contains(&o.id);
                let text = RichText::new(format!("{}  ·  {}", o.name, o.type_label()));
                if ui.selectable_label(sel, if o.hidden { text.weak() } else { text }).clicked() {
                    act.click = Some(o.id);
                }
            });
        }
    }

    fn right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector").resizable(true).default_width(320.0).show(ctx, |ui| {
            ui.heading(if self.edit.is_some() { "Mesh edit" } else { "Inspector" });
            let h = ui.available_height() * 0.58;
            egui::ScrollArea::vertical().id_salt("insp_scroll").max_height(h).show(ui, |ui| {
                self.inspector(ui);
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading(format!("Objects ({})", self.map.objects.len()));
                if ui.small_button("Group").on_hover_text("Put the selection in a new folder (Ctrl+G)").clicked() {
                    self.group_selection();
                }
            });
            egui::ScrollArea::vertical().id_salt("outliner_scroll").show(ui, |ui| {
                self.outliner(ui);
            });
        });
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("Controls").open(&mut open).default_width(480.0).show(ctx, |ui| {
            egui::Grid::new("help").num_columns(2).striped(true).show(ui, |ui| {
                let rows = [
                    ("1-0", "Select / Block / Wall / Building / Polygon / Door / Window / Prop / Decal / Light"),
                    ("2D: scroll", "Zoom at cursor"),
                    ("2D: right or middle drag", "Pan"),
                    ("Drag (Select)", "Move selection (snaps to grid; Snap To Part plugin snaps to parts)"),
                    ("Drag handles", "Resize selection"),
                    ("Shift + drag", "Clone and move"),
                    ("Shift / Ctrl + click", "Add to / remove from selection"),
                    ("Object list: Shift + click", "Add or remove one object at a time"),
                    ("Object list: Ctrl + click", "Select everything between the last clicked object and this one"),
                    ("W / E / R / X", "Transform Gizmo: move / rotate / scale handles, local-global axes (Ctrl = no snap)"),
                    ("Ctrl+G / Ctrl+Shift+G", "Group selection into a folder / ungroup"),
                    ("Building tool", "Click corners, click the first point / double-click / Enter to close"),
                    ("Ctrl+Shift+C", "Boolean: carve every Box brush the selected boxes overlap"),
                    ("Tab", "Mesh edit mode (1 vertices, 2 edges, 3 faces, Alt+click a loop, Ctrl+E extrude, M merge, N flip, Del delete)"),
                    ("F5", "Playtest (WASD, mouse, Space, Shift, Esc); with a playtest plugin, that plugin's"),
                    ("Shift+F5", "Anvil's own playtest, even with a playtest plugin"),
                    ("3D: hold right mouse", "Look around, WASD to fly, Q/E down/up, Shift faster"),
                    ("3D: middle drag / scroll", "Pan / dolly"),
                    ("[  ]", "Halve / double grid size"),
                    ("Arrow keys", "Nudge selection by one grid step in the hovered view"),
                    ("Ctrl+D", "Duplicate in place (the copies stay where the originals are, selected)"),
                    ("Ctrl+C / Ctrl+X / Ctrl+V", "Copy / cut / paste objects (pastes where the mouse is; works between Anvil windows)"),
                    ("Ctrl+H", "Make box hollow"),
                    ("Del", "Delete"),
                    ("H / U", "Hide selected / unhide all"),
                    ("F", "Focus selection (or the whole map)"),
                    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                    ("Double-click view title", "Maximize / restore that view"),
                ];
                for (k, v) in rows {
                    ui.label(RichText::new(k).strong());
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
        self.show_help = open;
    }

    // ----- viewports ------------------------------------------------------

    fn viewports(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_gray(12))).show(ctx, |ui| {
            let full = ui.max_rect();
            let scene = self.scene_cache.build(&self.map, &mut self.models);
            if self.play.is_some() {
                self.play_frame(ctx, full, ui, &scene);
                return;
            }
            if !ui.rect_contains_pointer(full) {
                self.hovered_slot = None;
            }
            let rects: Vec<(usize, Rect)> = match self.layout {
                Layout::Quad => {
                    let g = 1.5;
                    let m = full.center();
                    vec![
                        (0, Rect::from_min_max(full.min, pos2(m.x - g, m.y - g))),
                        (1, Rect::from_min_max(pos2(m.x + g, full.min.y), pos2(full.max.x, m.y - g))),
                        (2, Rect::from_min_max(pos2(full.min.x, m.y + g), pos2(m.x - g, full.max.y))),
                        (3, Rect::from_min_max(pos2(m.x + g, m.y + g), full.max)),
                    ]
                }
                Layout::Single(s) => vec![(s, full)],
            };
            for (slot, r) in rects {
                if slot == 0 {
                    self.view_3d(ui, r, &scene);
                } else {
                    self.view_2d(ui, slot - 1, r, &scene);
                }
            }
        });
    }

    fn toggle_max(&mut self, slot: usize) {
        self.layout = if self.layout == Layout::Quad { Layout::Single(slot) } else { Layout::Quad };
    }

    fn view_2d(&mut self, ui: &mut egui::Ui, vi: usize, rect: Rect, scene: &[Arc<WorldMesh>]) {
        let resp = ui.interact(rect, ui.id().with(("view2d", vi)), Sense::click_and_drag());
        let ctx = ui.ctx().clone();

        if let Some(hp) = resp.hover_pos() {
            self.hovered_slot = Some(vi + 1);
            let cv = self.views[vi];
            let top = cv.kind == ViewKind::Top;
            self.cursor_world = Some(cv.uv_to_world(cv.screen_to_uv(rect, hp), if top { self.work_y } else { 0.0 }));
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let v = &mut self.views[vi];
                let before = v.screen_to_uv(rect, hp);
                v.zoom = (v.zoom * (scroll * 0.0025).exp()).clamp(0.02, 500.0);
                let c = rect.center();
                v.center = Vec2::new(before.x - (hp.x - c.x) / v.zoom, before.y + (hp.y - c.y) / v.zoom);
            }
            let v = self.views[vi];
            let w = v.uv_to_world(v.screen_to_uv(rect, hp), f32::NAN);
            let (ua, _, va, _, _) = v.axes();
            let names = ["X", "Y", "Z"];
            self.cursor_text = format!("{} {}  {} {}", names[ua], fmt_num(w[ua]), names[va], fmt_num(w[va]));
        }
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            let d = resp.drag_delta();
            let v = &mut self.views[vi];
            v.center.x -= d.x / v.zoom;
            v.center.y += d.y / v.zoom;
        }
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), 22.0));
        if resp.double_clicked() && resp.interact_pointer_pos().is_some_and(|p| header.contains(p)) {
            self.toggle_max(vi + 1);
        }

        let view = ViewInfo::Ortho { view: self.views[vi], rect };
        if !self.gizmo_input(vi + 1, view, &resp, scene, &ctx) {
            self.input_2d(vi, rect, &resp, scene, &ctx);
        }
        self.context_pick_2d(vi, rect, &resp, scene);
        resp.context_menu(|ui| self.object_context_menu(ui));
        self.texture_drop_2d(vi, rect, &resp, scene, &ctx);
        self.paint_2d(ui, vi, rect, scene);
        let painter = ui.painter_at(rect);
        self.paint_gizmo(&painter, ViewInfo::Ortho { view: self.views[vi], rect }, scene);
    }

    fn plugin_click_2d(&mut self, idx: usize, view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>], p: Pos2) {
        let Some(id) = view2d::pick(view, rect, scene, p) else {
            self.status = "Click on a part".into();
            return;
        };
        let Some(wm) = scene.iter().find(|w| w.id == id) else { return };
        let (_, _, _, _, da) = view.axes();
        let center = (wm.min + wm.max) * 0.5;
        let pts: Vec<Pos2> = wm.pos.iter().map(|q| view.world_to_screen(rect, *q)).collect();
        let mut best: Option<(f32, [u32; 2])> = None;
        for e in &wm.mesh.edges {
            let d = view2d::dist_to_segment(p, pts[e[0] as usize], pts[e[1] as usize]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, *e));
            }
        }
        let Some((_, e)) = best else { return };
        let mut a = wm.pos[e[0] as usize];
        let mut b = wm.pos[e[1] as usize];
        a[da] = center[da];
        b[da] = center[da];
        let mut normal = Vec3::ZERO;
        normal[da] = 1.0;
        let point = (a + b) * 0.5;
        self.plugin_hit(idx, scene, Hit { id, point, normal, edge: (a, b) });
    }

    fn input_2d(&mut self, vi: usize, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        if self.edit.is_some() {
            self.edit_input_2d(vi, rect, resp, scene, ctx);
            return;
        }
        let view = self.views[vi];
        let (ua, su, va, sv, da) = view.axes();
        let mods = ctx.input(|i| i.modifiers);
        let add = mods.command || mods.shift;
        let g = self.snap_step();
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), 22.0));
        self.hover_2d(&view, rect, resp.hover_pos(), mods.command);
        if self.poly_extrude.is_some() {
            if resp.clicked() {
                self.extrude_click();
            }
            return;
        }

        if resp.double_clicked() && matches!(self.tool, Tool::Layout | Tool::Polygon) && self.layout_pts.len() >= 3 {
            self.close_layout();
            return;
        }

        // props, lights, spawns and prompts drop the moment you press
        if self.is_place_tool() && self.edit.is_none() && resp.hovered() && ctx.input(|i| i.pointer.primary_pressed()) && !mods.alt {
            if let Some(p) = resp.hover_pos().filter(|p| !header.contains(*p)) {
                let top = view.kind == ViewKind::Top;
                let at = view.uv_to_world(snap2(view.screen_to_uv(rect, p), g), if top { self.work_y } else { 0.0 });
                self.place(at, top);
            }
        }

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if !header.contains(p) || self.tool != Tool::Select {
                    let raw_uv = view.screen_to_uv(rect, p);
                    let uv = snap2(raw_uv, g);
                    match self.tool {
                        Tool::Select => {
                            let hit = view2d::pick(&view, rect, scene, p);
                            self.click_select(hit, add);
                        }
                        Tool::Wall => self.status = "Drag in the Top or 3D view to draw a wall".into(),
                        Tool::Trim => self.status = "Click a wall in the 3D view to add trim".into(),
                        Tool::Layout | Tool::Polygon => {
                            if view.kind == ViewKind::Top {
                                let _ = uv;
                                let w = self.straight_point(view.uv_to_world(raw_uv, self.work_y), mods.command);
                                let near_first = self.layout_pts.first().is_some_and(|f| (view.world_to_screen(rect, *f) - p).length() < 9.0);
                                self.layout_click(w, near_first);
                            } else {
                                self.status = "Draw outlines in the Top view (or on the ground in 3D)".into();
                            }
                        }
                        Tool::Door | Tool::Window => {
                            if let Some(id) = view2d::pick(&view, rect, scene, p) {
                                let depth = self.map.get(id).map(|o| o.pos[da]).unwrap_or(0.0);
                                let at = view.uv_to_world(raw_uv, depth);
                                self.cut_opening_at(id, at, crate::mesh_openings::view_ray(&view, at), self.tool == Tool::Window, !mods.command);
                            } else {
                                self.status = "Click on a wall".into();
                            }
                        }
                        Tool::Decal => {
                            let (at, n) = match view.kind {
                                ViewKind::Top => (view.uv_to_world(uv, self.work_y), Vec3::Y),
                                ViewKind::Front => (view.uv_to_world(uv, 0.0), Vec3::Z),
                                ViewKind::Side => (view.uv_to_world(uv, 0.0), Vec3::NEG_X),
                            };
                            self.place_decal(at, n);
                        }
                        Tool::Plugin(i) => self.plugin_click_2d(i, &view, rect, scene, p),
                        Tool::Knife => self.status = "Knife: drag a line across what you want to cut".into(),
                        Tool::Teleport => {
                            let top = view.kind == ViewKind::Top;
                            self.teleport_click(view.uv_to_world(uv, if top { self.work_y } else { 0.0 }));
                        }
                        Tool::Stairs => {
                            let pt = self.stair_point_2d(&view, rect, p);
                            self.stair_click(pt);
                        }
                        Tool::Face => {
                            let hit = view2d::pick(&view, rect, scene, p);
                            let f = match view.kind {
                                ViewKind::Top => 2,
                                ViewKind::Front => 4,
                                ViewKind::Side => 1,
                            };
                            self.face_click_2d(hit, f, add);
                        }
                        Tool::Text => {
                            let (at, yaw) = match view.kind {
                                ViewKind::Top => (view.uv_to_world(uv, self.work_y), 0.0),
                                ViewKind::Front => (view.uv_to_world(uv, 0.0), 0.0),
                                ViewKind::Side => (view.uv_to_world(uv, 0.0), -90.0),
                            };
                            self.place_text(at, yaw, None);
                        }
                        _ if self.is_place_tool() => {} // already placed on the press
                        _ => {
                            let top = view.kind == ViewKind::Top;
                            let at = view.uv_to_world(uv, if top { self.work_y } else { 0.0 });
                            self.place(at, top);
                        }
                    }
                }
            }
        }

        if resp.drag_started_by(PointerButton::Primary) {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
            self.drag_cur = start;
            let uv = view.screen_to_uv(rect, start);
            self.drag = match self.tool {
                Tool::Select => {
                    if let Some((handle, bmin, bmax)) = self.handle_at(&view, rect, scene, start) {
                        self.push_undo();
                        let (a, b) = (view.to_uv(bmin), view.to_uv(bmax));
                        Drag::Resize2D { view: vi, handle, uv_min: a.min(b), uv_max: a.max(b), wmin: bmin, wmax: bmax, origs: self.selected_objects() }
                    } else if let Some(id) = view2d::pick(&view, rect, scene, start) {
                        if !self.selection.contains(&id) {
                            if !mods.command {
                                self.selection.clear();
                            }
                            self.selection.push(id);
                        }
                        let (bmin, bmax) = sel_bounds(scene, &self.selection).unwrap_or((Vec3::ZERO, Vec3::ZERO));
                        self.push_undo();
                        if mods.shift {
                            self.duplicate(Vec3::ZERO);
                            self.status = "Cloned selection".into();
                        }
                        let (a, b) = (view.to_uv(bmin), view.to_uv(bmax));
                        Drag::Move2D { view: vi, start_uv: uv, bmin_uv: a.min(b), bmin, bmax, origs: self.sel_positions() }
                    } else {
                        Drag::Marquee { view: vi, start }
                    }
                }
                Tool::Wall if view.kind == ViewKind::Top => Drag::Wall2D { view: vi, start: view.uv_to_world(snap2(uv, g), self.work_y) },
                Tool::Block | Tool::Wall => Drag::Create2D { view: vi, start_uv: snap2(uv, g) },
                Tool::Door | Tool::Window => Drag::Opening { slot: vi + 1 },
                Tool::Layout | Tool::Polygon if view.kind == ViewKind::Top && self.layout_pts.is_empty() => {
                    Drag::Outline2D { view: vi, start: view.uv_to_world(snap2(uv, g), self.work_y), square: mods.alt }
                }
                Tool::Knife => Drag::Knife { slot: vi + 1, start: if mods.command { start } else { view.uv_to_screen(rect, snap2(uv, g)) } },
                _ => Drag::None,
            };
        }

        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            let cur_uv = view.screen_to_uv(rect, self.drag_cur);
            if let Drag::Outline2D { square, .. } | Drag::Outline3D { square, .. } = &mut self.drag {
                *square = mods.alt;
            }
            let mut moved: Option<(Vec3, Vec3, Vec3, Vec<(u64, Vec3)>)> = None;
            match &self.drag {
                Drag::Move2D { view: dv, start_uv, bmin_uv, bmin, bmax, origs } if *dv == vi => {
                    let target = *bmin_uv + (cur_uv - *start_uv);
                    let wd = view.uv_delta_to_world(snap2(target, g) - *bmin_uv);
                    moved = Some((wd, *bmin, *bmax, origs.clone()));
                }
                Drag::Resize2D { view: dv, handle, uv_min, uv_max, wmin, wmax, origs } if *dv == vi => {
                    let c = snap2(cur_uv, g);
                    let minsz = if g > 0.0 { g } else { 0.01 };
                    let (mut a, mut b) = (*uv_min, *uv_max);
                    match handle.0 {
                        1 => b.x = c.x.max(a.x + minsz),
                        -1 => a.x = c.x.min(b.x - minsz),
                        _ => {}
                    }
                    match handle.1 {
                        1 => b.y = c.y.max(a.y + minsz),
                        -1 => a.y = c.y.min(b.y - minsz),
                        _ => {}
                    }
                    let mut nmin = *wmin;
                    let mut nmax = *wmax;
                    let mut axes = vec![ua, va];
                    if mods.shift {
                        // Shift: same factor on every axis (the unseen one scales about its middle)
                        let ow = (*uv_max - *uv_min).max(Vec2::splat(1e-6));
                        let (fu, fv) = ((b.x - a.x) / ow.x, (b.y - a.y) / ow.y);
                        let f = match (handle.0 != 0, handle.1 != 0) {
                            (true, true) => fu.max(fv),
                            (true, false) => fu,
                            (false, true) => fv,
                            _ => 1.0,
                        };
                        let fit = |lo: f32, hi: f32, h: i32| match h {
                            1 => (lo, lo + (hi - lo) * f),
                            -1 => (hi - (hi - lo) * f, hi),
                            _ => {
                                let (c, half) = ((lo + hi) * 0.5, (hi - lo) * f * 0.5);
                                (c - half, c + half)
                            }
                        };
                        let (ax0, ax1) = fit(uv_min.x, uv_max.x, handle.0);
                        let (ay0, ay1) = fit(uv_min.y, uv_max.y, handle.1);
                        a = Vec2::new(ax0, ay0);
                        b = Vec2::new(ax1, ay1);
                        let (dc, dh) = ((wmin[da] + wmax[da]) * 0.5, (wmax[da] - wmin[da]) * f * 0.5);
                        nmin[da] = dc - dh;
                        nmax[da] = dc + dh;
                        axes.push(da);
                    }
                    let (x0, x1) = (a.x * su, b.x * su);
                    nmin[ua] = x0.min(x1);
                    nmax[ua] = x0.max(x1);
                    let (y0, y1) = (a.y * sv, b.y * sv);
                    nmin[va] = y0.min(y1);
                    nmax[va] = y0.max(y1);
                    apply_scale(&mut self.map, origs, *wmin, *wmax, nmin, nmax, &axes);
                }
                _ => {}
            }
            if let Some((wd, bmin, bmax, origs)) = moved {
                let wd = self.hook_move(scene, bmin, bmax, wd);
                let wd = self.collide_translate(scene, &origs, wd);
                for (id, p) in origs {
                    if let Some(o) = self.map.get_mut(id) {
                        o.pos = p + wd;
                    }
                }
            }
        }

        if resp.drag_stopped_by(PointerButton::Primary) {
            let drag = std::mem::replace(&mut self.drag, Drag::None);
            let cur_uv = view.screen_to_uv(rect, self.drag_cur);
            match drag {
                Drag::Marquee { view: dv, start } if dv == vi => {
                    let r = Rect::from_two_pos(start, self.drag_cur);
                    if !add {
                        self.selection.clear();
                    }
                    for wm in scene {
                        if r.contains_rect(view.screen_rect(rect, wm.min, wm.max)) && !self.selection.contains(&wm.id) {
                            self.selection.push(wm.id);
                        }
                    }
                }
                Drag::Create2D { view: dv, start_uv } if dv == vi => {
                    let end = snap2(cur_uv, g);
                    let end = if mods.shift { crate::previews::square_end(start_uv, end) } else { end };
                    self.create_from_rect(&view, start_uv, end, scene);
                }
                Drag::Outline2D { view: dv, start, square } if dv == vi => {
                    let uv = if mods.command { cur_uv } else { snap2(cur_uv, g) };
                    let pts = crate::previews::outline_rect(start, view.uv_to_world(uv, start.y), square);
                    if !pts.is_empty() {
                        self.layout_pts = pts;
                        self.close_layout();
                    }
                }
                Drag::Wall2D { view: dv, start } if dv == vi => {
                    let end = self.straight_end(start, view.uv_to_world(cur_uv, self.work_y), mods.command);
                    self.create_wall(start, end);
                }
                Drag::Opening { slot } if slot == vi + 1 => {
                    if let Some(id) = view2d::pick(&view, rect, scene, self.drag_cur) {
                        let depth = self.map.get(id).map(|o| o.pos[da]).unwrap_or(0.0);
                        let at = view.uv_to_world(cur_uv, depth);
                        self.cut_opening_at(id, at, crate::mesh_openings::view_ray(&view, at), self.tool == Tool::Window, !mods.command);
                    }
                }
                Drag::Knife { slot, start } if slot == vi + 1 => {
                    let end = if mods.command { self.drag_cur } else { view.uv_to_screen(rect, snap2(cur_uv, g)) };
                    self.knife_release_2d(&view, rect, scene, start, end);
                }
                _ => {}
            }
        }
    }

    fn handle_at(&self, view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>], p: Pos2) -> Option<((i32, i32), Vec3, Vec3)> {
        if self.tool != Tool::Select || self.selection.is_empty() {
            return None;
        }
        let (bmin, bmax) = sel_bounds(scene, &self.selection)?;
        let r = view.screen_rect(rect, bmin, bmax);
        view2d::handles(r).into_iter().find(|(_, hp)| (*hp - p).length() < 7.0).map(|(h, _)| (h, bmin, bmax))
    }

    fn paint_2d(&self, ui: &egui::Ui, vi: usize, rect: Rect, scene: &[Arc<WorldMesh>]) {
        let painter = ui.painter_at(rect);
        let view = self.views[vi];
        let (ua, _, va, _, _) = view.axes();
        painter.rect_filled(rect, CornerRadius::ZERO, rgb(self.settings.bg_2d));
        view.draw_grid(&painter, rect, self.settings.grid);

        // wireframe lines are costly to draw: past this many, detailed props
        // (not selected ones) show as their outline box
        let mut budget: usize = 40_000;
        for pass in 0..2 {
            for wm in scene {
                let sel = self.selection.contains(&wm.id);
                if (pass == 1) != sel {
                    continue;
                }
                let r = view.screen_rect(rect, wm.min, wm.max);
                if !r.intersects(rect) {
                    continue;
                }
                let col = if sel { Color32::from_rgb(255, 72, 72) } else { lighten(wm.color) };
                if r.width() < 3.0 && r.height() < 3.0 {
                    painter.circle_filled(r.center(), 2.0, col);
                    continue;
                }
                let stroke = Stroke::new(if sel { 1.6f32 } else { 1.0 }, col);
                let dense = wm.mesh.edges.len() > 300;
                if dense && (r.width().max(r.height()) < 120.0 || (!sel && wm.mesh.edges.len() > budget)) {
                    // dense prop drawn small: its outline box reads just as well
                    painter.rect_stroke(r, CornerRadius::ZERO, stroke, StrokeKind::Middle);
                    continue;
                }
                if dense && !sel {
                    budget -= wm.mesh.edges.len();
                }
                let pts: Vec<Pos2> = wm.pos.iter().map(|p| view.world_to_screen(rect, *p)).collect();
                for e in &wm.mesh.edges {
                    let (a, b) = (pts[e[0] as usize], pts[e[1] as usize]);
                    // skip lines that are off this view or shorter than a pixel
                    if (a - b).length_sq() < 0.25 || !rect.intersects(Rect::from_two_pos(a, b)) {
                        continue;
                    }
                    painter.line_segment([a, b], stroke);
                }
            }
        }

        let font = FontId::proportional(12.0);
        if self.tool == Tool::Select && self.edit.is_none() {
            if let Some((bmin, bmax)) = sel_bounds(scene, &self.selection) {
                let r = view.screen_rect(rect, bmin, bmax);
                painter.rect_stroke(r.expand(2.0), CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_rgb(255, 205, 70)), StrokeKind::Outside);
                for (_, hp) in view2d::handles(r.expand(2.0)) {
                    let hr = Rect::from_center_size(hp, vec2(7.0, 7.0));
                    painter.rect_filled(hr, CornerRadius::ZERO, Color32::WHITE);
                    painter.rect_stroke(hr, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::BLACK), StrokeKind::Inside);
                }
                let size = bmax - bmin;
                painter.text(
                    r.center_top() - vec2(0.0, 8.0),
                    Align2::CENTER_BOTTOM,
                    format!("{} x {}", fmt_num(size[ua]), fmt_num(size[va])),
                    font.clone(),
                    Color32::from_rgb(255, 205, 70),
                );
            }
        }

        self.paint_team_2d(&painter, &view, rect, scene);
        self.paint_entities_2d(&painter, &view, rect);
        if self.tool == Tool::Stairs {
            self.paint_stair_preview_2d(&painter, &view, rect, ui.input(|i| i.pointer.hover_pos()));
        }
        {
            let (hp, down, free) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down(), i.modifiers.command));
            self.paint_previews_2d(&painter, vi, &view, rect, scene, hp, down, free);
        }
        if self.edit.is_some() {
            self.paint_edit_2d(&painter, &view, rect);
        }

        let g = self.snap_step();
        let preview = Color32::from_rgb(120, 220, 255);
        // building / polygon outline in progress
        if view.kind == ViewKind::Top && !self.layout_pts.is_empty() {
            let pts: Vec<Pos2> = self.layout_pts.iter().map(|p| view.world_to_screen(rect, *p)).collect();
            for w in pts.windows(2) {
                painter.line_segment([w[0], w[1]], Stroke::new(2.0f32, preview));
            }
            if let Some(hp) = ui.ctx().pointer_hover_pos().filter(|p| rect.contains(*p)) {
                let free = ui.input(|i| i.modifiers.command);
                let cur = view.world_to_screen(rect, self.straight_point(view.uv_to_world(view.screen_to_uv(rect, hp), self.work_y), free));
                if let Some(last) = pts.last() {
                    painter.line_segment([*last, cur], Stroke::new(1.0f32, preview.gamma_multiply(0.6)));
                }
            }
            for (i, p) in pts.iter().enumerate() {
                painter.circle_filled(*p, if i == 0 { 6.0 } else { 4.0 }, if i == 0 { Color32::from_rgb(255, 205, 70) } else { preview });
            }
        }
        match &self.drag {
            Drag::Marquee { view: dv, start } if *dv == vi => {
                let r = Rect::from_two_pos(*start, self.drag_cur);
                painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(120, 200, 255, 24));
                painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, preview), StrokeKind::Inside);
            }
            Drag::EditMarquee { view: Some(dv), start } if *dv == vi => {
                let r = Rect::from_two_pos(*start, self.drag_cur);
                painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, preview), StrokeKind::Inside);
            }
            Drag::Create2D { view: dv, start_uv } if *dv == vi => {
                let end = snap2(view.screen_to_uv(rect, self.drag_cur), g);
                let end = if ui.input(|i| i.modifiers.shift) { crate::previews::square_end(*start_uv, end) } else { end };
                let r = Rect::from_two_pos(view.uv_to_screen(rect, *start_uv), view.uv_to_screen(rect, end));
                painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5f32, preview), StrokeKind::Inside);
                let d = (end - *start_uv).abs();
                painter.text(r.center_top() - vec2(0.0, 6.0), Align2::CENTER_BOTTOM, format!("{} x {}", fmt_num(d.x), fmt_num(d.y)), font.clone(), preview);
            }
            Drag::Wall2D { view: dv, start } if *dv == vi => {
                let end = self.straight_end(*start, view.uv_to_world(view.screen_to_uv(rect, self.drag_cur), self.work_y), ui.input(|i| i.modifiers.command));
                let (a, b) = (view.world_to_screen(rect, *start), view.world_to_screen(rect, end));
                let thick = (self.settings.wall_thickness * view.zoom).max(2.0);
                painter.line_segment([a, b], Stroke::new(thick, preview.gamma_multiply(0.5)));
                painter.line_segment([a, b], Stroke::new(1.0f32, preview));
                let len = (end - *start).length();
                painter.text(b + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, format!("{} long", fmt_num(len)), font.clone(), preview);
            }
            Drag::Knife { slot, start } if *slot == vi + 1 => {
                let snap_on = !ui.input(|i| i.modifiers.command);
                let end = if snap_on { view.uv_to_screen(rect, snap2(view.screen_to_uv(rect, self.drag_cur), g)) } else { self.drag_cur };
                paint_knife_line(&painter, *start, end, self.settings.knife_split);
            }
            _ => {}
        }

        painter.text(rect.min + vec2(8.0, 5.0), Align2::LEFT_TOP, view.label(), FontId::proportional(13.0), Color32::from_gray(170));
        let hovered = self.hovered_slot == Some(vi + 1);
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0f32, if hovered { Color32::from_gray(120) } else { Color32::from_gray(45) }),
            StrokeKind::Inside,
        );
    }

    fn view_3d(&mut self, ui: &mut egui::Ui, rect: Rect, scene: &[Arc<WorldMesh>]) {
        let resp = ui.interact(rect, ui.id().with("view3d"), Sense::click_and_drag());
        let ctx = ui.ctx().clone();
        if resp.hovered() {
            self.hovered_slot = Some(0);
        }
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), 22.0));
        if resp.double_clicked() && resp.interact_pointer_pos().is_some_and(|p| header.contains(p)) {
            self.toggle_max(0);
        }
        let gview = ViewInfo::Persp { cam: self.cam, rect, fov: self.settings.fov };
        if self.gizmo_input(0, gview, &resp, scene, &ctx) {
            self.camera_nav(rect, &resp, &ctx);
        } else {
            self.input_3d(rect, &resp, scene, &ctx);
        }
        self.context_pick_3d(rect, &resp, scene);
        resp.context_menu(|ui| self.object_context_menu(ui));
        self.texture_drop_3d(rect, &resp, scene, &ctx);
        let drop_resp = resp.clone();

        // while the camera moves, a slow view draws at lower resolution, then
        // sharp again a moment after it stops
        let now = ctx.input(|i| i.time);
        if self.last_cam != Some(self.cam) {
            self.last_cam = Some(self.cam);
            self.cam_moved_at = now;
        }
        let moving = now - self.cam_moved_at < 0.2;
        let mut scale = self.settings.render_scale.clamp(0.25, 2.0);
        if moving && self.settings.fast_motion && self.renderer.full_ms > 14.0 {
            scale *= (14.0 / self.renderer.full_ms).sqrt().clamp(0.4, 0.8);
        }
        if moving {
            ctx.request_repaint_after(std::time::Duration::from_millis(220));
        }
        let opts = RenderOpts {
            fov: self.settings.fov,
            grid: self.settings.grid,
            plane_y: self.work_y,
            show_grid: self.settings.show_grid_3d,
            outlines: self.settings.outlines_3d,
            bg: self.settings.bg_3d,
            scale,
            filter: self.settings.texture_filtering,
            lighting: self.lighting(),
        };
        let remote = self.team_remote_sel();
        let tex = self.renderer.render(&ctx, rect, &self.cam, &opts, scene, &self.selection, &remote, &mut self.textures);
        let painter = ui.painter_at(rect);
        painter.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        let fov = self.settings.fov;
        let preview = Color32::from_rgb(120, 220, 255);
        if let Drag::Wall3D { start } = &self.drag {
            let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
            if let Some(p) = ray_plane(o, d, self.work_y) {
                let end = self.straight_end(*start, Vec3::new(p.x, self.work_y, p.z), resp.ctx.input(|i| i.modifiers.command));
                let h = Vec3::Y * self.settings.wall_height;
                for (a, b) in [(*start, end), (*start + h, end + h), (*start, *start + h), (end, end + h)] {
                    if let Some((sa, sb)) = self.cam.project_segment(rect, fov, a, b) {
                        painter.line_segment([sa, sb], Stroke::new(1.5f32, preview));
                    }
                }
            }
        }
        if !self.layout_pts.is_empty() {
            let mut pts = self.layout_pts.clone();
            if let Some(hp) = resp.hover_pos() {
                let (o, d) = self.cam.ray(rect, fov, hp);
                if let Some(p) = ray_plane(o, d, self.work_y) {
                    let free = resp.ctx.input(|i| i.modifiers.command);
                    pts.push(self.straight_point(Vec3::new(p.x, self.work_y, p.z), free));
                }
            }
            for w in pts.windows(2) {
                if let Some((a, b)) = self.cam.project_segment(rect, fov, w[0], w[1]) {
                    painter.line_segment([a, b], Stroke::new(2.0f32, preview));
                }
            }
            for (i, p) in self.layout_pts.iter().enumerate() {
                if let Some(s) = self.cam.project(rect, fov, *p) {
                    painter.circle_filled(s, if i == 0 { 6.0 } else { 4.0 }, if i == 0 { Color32::from_rgb(255, 205, 70) } else { preview });
                }
            }
        }
        if self.tool == Tool::Face {
            self.paint_face_selection(&painter, rect, scene);
        }
        if let Some(start) = self.is_knife_drag(0) {
            paint_knife_line(&painter, start, self.drag_cur, self.settings.knife_split);
        }
        self.paint_team_3d(&painter, rect);
        self.paint_entities_3d(&painter, rect);
        self.paint_texture_drop_hover(&painter, rect, &drop_resp, scene);
        if self.tool == Tool::Stairs {
            self.paint_stair_preview_3d(&painter, rect, scene, resp.hover_pos());
        }
        {
            let (down, free) = ui.input(|i| (i.pointer.primary_down(), i.modifiers.command));
            self.paint_previews_3d(&painter, rect, scene, resp.hover_pos(), down, free);
        }
        self.paint_gizmo(&painter, ViewInfo::Persp { cam: self.cam, rect, fov }, scene);
        if self.edit.is_some() {
            self.paint_edit_3d(&painter, rect);
            if let Drag::EditMarquee { view: None, start } = &self.drag {
                painter.rect_stroke(Rect::from_two_pos(*start, self.drag_cur), CornerRadius::ZERO, Stroke::new(1.0f32, preview), StrokeKind::Inside);
            }
        }

        let bar = Rect::from_min_size(rect.min + vec2(4.0, 3.0), vec2((rect.width() - 8.0).max(10.0), 20.0));
        let mut pick: Option<u8> = None;
        let cur = self.view_preset;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, label) in ["Persp", "Top", "Bottom", "Front", "Back", "Left", "Right"].iter().enumerate() {
                    let r = ui.selectable_label(cur == i as u8, RichText::new(*label).small().strong());
                    if r.clicked() {
                        pick = Some(i as u8);
                    }
                }
                ui.add_space(6.0);
                let hint = if cur == 0 { "right-drag: look  ·  WASD fly" } else { "right/middle-drag: pan  ·  scroll: zoom" };
                ui.label(RichText::new(hint).small().color(Color32::from_gray(190)));
            });
        });
        if let Some(v) = pick {
            self.set_view(v, rect);
        }
        let hovered = self.hovered_slot == Some(0);
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0f32, if hovered { Color32::from_gray(140) } else { Color32::from_gray(45) }),
            StrokeKind::Inside,
        );
    }

    /// Right-mouse look + WASD fly, middle-drag pan and scroll dolly.
    fn camera_nav(&mut self, rect: Rect, resp: &egui::Response, ctx: &egui::Context) {
        let (dt, scroll, sec_down, mods) = ctx.input(|i| (i.stable_dt.min(0.1), i.raw_scroll_delta.y, i.pointer.secondary_down(), i.modifiers));
        let key = |k: Key| ctx.input(|i| i.key_down(k));
        if self.cam.is_ortho() {
            // flat views: pan with right / middle drag (or WASD), zoom to the cursor
            let upp = self.cam.ortho * 2.0 / rect.height().max(1.0);
            if resp.dragged_by(PointerButton::Secondary) || resp.dragged_by(PointerButton::Middle) {
                let d = resp.drag_delta();
                self.cam.pos += (-self.cam.right() * d.x + self.cam.up() * d.y) * upp;
            }
            if sec_down && resp.hovered() {
                let mut mv = Vec3::ZERO;
                if key(Key::W) {
                    mv += self.cam.up();
                }
                if key(Key::S) {
                    mv -= self.cam.up();
                }
                if key(Key::D) {
                    mv += self.cam.right();
                }
                if key(Key::A) {
                    mv -= self.cam.right();
                }
                self.cam.pos += mv * self.cam.ortho * dt * if mods.shift { 3.0 } else { 1.0 };
                ctx.request_repaint();
            }
            if let (true, Some(hp)) = (scroll != 0.0, resp.hover_pos()) {
                let fov = self.settings.fov;
                let before = self.cam.ray(rect, fov, hp).0;
                self.cam.ortho = (self.cam.ortho * (-scroll * 0.0025).exp()).clamp(0.25, 50_000.0);
                let after = self.cam.ray(rect, fov, hp).0;
                self.cam.pos += before - after;
            }
            return;
        }

        if resp.dragged_by(PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.cam.yaw += d.x * 0.005;
            self.cam.pitch = (self.cam.pitch - d.y * 0.005).clamp(-1.54, 1.54);
        }
        if sec_down && (resp.hovered() || resp.dragged_by(PointerButton::Secondary)) {
            let (f, r) = (self.cam.forward(), self.cam.right());
            let mut mv = Vec3::ZERO;
            if key(Key::W) {
                mv += f;
            }
            if key(Key::S) {
                mv -= f;
            }
            if key(Key::D) {
                mv += r;
            }
            if key(Key::A) {
                mv -= r;
            }
            if key(Key::E) {
                mv += Vec3::Y;
            }
            if key(Key::Q) {
                mv -= Vec3::Y;
            }
            if mv != Vec3::ZERO {
                let sp = self.settings.cam_speed * if mods.shift { 3.0 } else { 1.0 };
                self.cam.pos += mv.normalize() * sp * dt;
            }
            ctx.request_repaint();
        }
        if resp.dragged_by(PointerButton::Middle) {
            let d = resp.drag_delta();
            let k = self.settings.cam_speed * 0.01;
            self.cam.pos += (-self.cam.right() * d.x + self.cam.up() * d.y) * k;
        }
        if resp.hovered() && scroll != 0.0 {
            self.cam.pos += self.cam.forward() * scroll * 0.0025 * self.settings.cam_speed;
        }
    }

    fn input_3d(&mut self, rect: Rect, resp: &egui::Response, scene: &[Arc<WorldMesh>], ctx: &egui::Context) {
        let fov = self.settings.fov;
        let g = self.snap_step();
        let mods = ctx.input(|i| i.modifiers);
        let add = mods.command || mods.shift;
        // Alt+scroll turns what's being placed (instead of moving the camera)
        if self.is_place_tool() && mods.alt && resp.hovered() {
            let wheel = ctx.input(|i| i.raw_scroll_delta.y);
            if wheel != 0.0 {
                self.turn_placement(if wheel > 0.0 { 15.0 } else { -15.0 });
                ctx.input_mut(|i| {
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                    i.raw_scroll_delta = egui::Vec2::ZERO;
                });
            }
        }
        self.camera_nav(rect, resp, ctx);
        // one pick under the mouse per frame, for the readout and the placing ghost
        let under = resp.hover_pos().and_then(|hp| {
            let (o, d) = self.cam.ray(rect, fov, hp);
            pick_ray(scene, o, d).map(|h| h.point).or_else(|| ray_plane(o, d, self.work_y))
        });
        if let Some(p) = under {
            self.cursor_text = format!("X {}  Y {}  Z {}", fmt_num(p.x), fmt_num(p.y), fmt_num(p.z));
        }
        if resp.hovered() {
            self.cursor_world = under;
        }

        self.hover_3d(rect, under, resp.hover_pos(), mods.command);
        // props, lights, spawns and prompts drop the moment you press: no
        // waiting for the release, and a slight wiggle doesn't eat the click
        if self.is_place_tool() && self.edit.is_none() && self.poly_extrude.is_none() && resp.hovered() && ctx.input(|i| i.pointer.primary_pressed()) && !mods.alt {
            if let Some(pt) = under {
                self.place(Vec3::new(snap(pt.x, g), pt.y, snap(pt.z, g)), true);
            }
        }
        if self.poly_extrude.is_some() {
            if resp.clicked() {
                self.extrude_click();
            }
            return;
        }
        if self.uv.open {
            if mods.alt && resp.drag_started_by(PointerButton::Primary) {
                let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
                let (o, d) = self.cam.ray(rect, fov, start);
                if self.uv_view_drag_start(scene, o, d, start.x, mods.shift) {
                    return;
                }
            }
            if self.uv.vdrag.is_some() {
                if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged_by(PointerButton::Primary)) {
                    let (o, d) = self.cam.ray(rect, fov, p);
                    self.uv_view_drag(o, d, p.x, mods.command);
                }
                if resp.drag_stopped() || !ctx.input(|i| i.pointer.primary_down()) {
                    self.uv.vdrag = None;
                }
                return;
            }
        }
        if self.edit.is_some() {
            self.edit_input_3d(rect, resp, scene, ctx);
            return;
        }

        if resp.double_clicked() && matches!(self.tool, Tool::Layout | Tool::Polygon) && self.layout_pts.len() >= 3 {
            self.close_layout();
            return;
        }

        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (o, d) = self.cam.ray(rect, fov, p);
                match self.tool {
                    Tool::Select => {
                        let hit = pick_ray(scene, o, d).map(|h| h.id);
                        self.click_select(hit, add);
                    }
                    Tool::Wall => self.status = "Drag on the ground to draw a wall".into(),
                    Tool::Layout | Tool::Polygon => {
                        if let Some(q) = ray_plane(o, d, self.work_y) {
                            let w = self.straight_point(Vec3::new(q.x, self.work_y, q.z), mods.command);
                            let near_first = self
                                .layout_pts
                                .first()
                                .and_then(|f| self.cam.project(rect, fov, *f))
                                .is_some_and(|s| (s - p).length() < 9.0);
                            self.layout_click(w, near_first);
                        }
                    }
                    Tool::Door | Tool::Window => match pick_ray(scene, o, d) {
                        Some(h) => self.cut_opening_at(h.id, h.point, (o, d), self.tool == Tool::Window, !mods.command),
                        None => self.status = "Click on a wall".into(),
                    },
                    Tool::Trim => match pick_ray(scene, o, d) {
                        Some(h) => self.place_trim(h.id, h.point, h.normal, !mods.command),
                        None => self.status = "Click on a wall".into(),
                    },
                    Tool::Decal => {
                        let hit = crate::view3d::pick_ray_where(scene, o, d, |w| w.decal);
                        match hit {
                            Some(h) => self.place_decal(h.point, h.normal),
                            None => {
                                if let Some(q) = ray_plane(o, d, self.work_y) {
                                    self.place_decal(q, Vec3::Y);
                                }
                            }
                        }
                    }
                    Tool::Knife => self.status = "Knife: drag a line across what you want to cut".into(),
                    Tool::Teleport => {
                        let pt = pick_ray(scene, o, d).map(|h| h.point).or_else(|| ray_plane(o, d, self.work_y));
                        if let Some(pt) = pt {
                            self.teleport_click(Vec3::new(snap(pt.x, g), pt.y, snap(pt.z, g)));
                        }
                    }
                    Tool::Stairs => {
                        if let Some(pt) = self.stair_point_3d(rect, scene, p) {
                            self.stair_click(pt);
                        }
                    }
                    Tool::Face => {
                        let hit = pick_ray(scene, o, d).and_then(|h| self.face_at(scene, h.id, h.tri).map(|f| (h.id, f)));
                        self.click_face(hit, add);
                    }
                    Tool::Text => {
                        let f = self.cam.forward();
                        let face_cam = (-f.x).atan2(-f.z).to_degrees();
                        match pick_ray(scene, o, d) {
                            Some(h) if h.normal.y.abs() < 0.5 => {
                                // on a wall: lie flat against it, facing out
                                let yaw = h.normal.x.atan2(h.normal.z).to_degrees();
                                self.place_text(h.point, yaw, Some(h.normal));
                            }
                            Some(h) => self.place_text(h.point, face_cam, None),
                            None => {
                                if let Some(q) = ray_plane(o, d, self.work_y) {
                                    self.place_text(q, face_cam, None);
                                }
                            }
                        }
                    }
                    Tool::Plugin(i) => {
                        if let Some(h) = pick_ray(scene, o, d) {
                            if let Some(wm) = scene.iter().find(|w| w.id == h.id) {
                                let mut best: Option<(f32, Vec3, Vec3)> = None;
                                for e in &wm.mesh.edges {
                                    let (a, b) = (wm.pos[e[0] as usize], wm.pos[e[1] as usize]);
                                    let (dd, _) = dist_point_segment(h.point, a, b);
                                    if best.is_none_or(|bb| dd < bb.0) {
                                        best = Some((dd, a, b));
                                    }
                                }
                                if let Some((_, a, b)) = best {
                                    self.plugin_hit(i, scene, Hit { id: h.id, point: h.point, normal: h.normal, edge: (a, b) });
                                }
                            }
                        } else {
                            self.status = "Click on a part".into();
                        }
                    }
                    _ if self.is_place_tool() => {} // already placed on the press
                    _ => {
                        let pt = pick_ray(scene, o, d).map(|h| h.point).or_else(|| ray_plane(o, d, self.work_y));
                        if let Some(pt) = pt {
                            self.place(Vec3::new(snap(pt.x, g), pt.y, snap(pt.z, g)), true);
                        }
                    }
                }
            }
        }

        if resp.drag_started_by(PointerButton::Primary) {
            let start = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
            self.drag_cur = start;
            let (o, d) = self.cam.ray(rect, fov, start);
            match self.tool {
                Tool::Select => {
                    if let Some(h) = pick_ray(scene, o, d) {
                        if !self.selection.contains(&h.id) {
                            if !mods.command {
                                self.selection.clear();
                            }
                            self.selection.push(h.id);
                        }
                        let (bmin, bmax) = sel_bounds(scene, &self.selection).unwrap_or((Vec3::ZERO, Vec3::ZERO));
                        self.push_undo();
                        if mods.shift {
                            self.duplicate(Vec3::ZERO);
                        }
                        self.drag = Drag::Move3D { plane_y: h.point.y, start: h.point, bmin, bmax, origs: self.sel_positions() };
                    }
                }
                Tool::Knife => self.drag = Drag::Knife { slot: 0, start },
                Tool::Door | Tool::Window => self.drag = Drag::Opening { slot: 0 },
                Tool::Layout | Tool::Polygon if self.layout_pts.is_empty() => {
                    if let Some(p) = ray_plane(o, d, self.work_y) {
                        self.drag = Drag::Outline3D { start: Vec3::new(snap(p.x, g), self.work_y, snap(p.z, g)), square: mods.alt };
                    }
                }
                Tool::Wall => {
                    if let Some(p) = ray_plane(o, d, self.work_y) {
                        self.drag = Drag::Wall3D { start: Vec3::new(snap(p.x, g), self.work_y, snap(p.z, g)) };
                    }
                }
                _ => {}
            }
        }

        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                self.drag_cur = p;
            }
            if let Drag::Outline3D { square, .. } = &mut self.drag {
                *square = mods.alt;
            }
            let mut moved = None;
            if let Drag::Move3D { plane_y, start, bmin, bmax, origs } = &self.drag {
                let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
                if let Some(p) = ray_plane(o, d, *plane_y) {
                    let dd = p - *start;
                    let wd = Vec3::new(snap(dd.x, g), 0.0, snap(dd.z, g));
                    moved = Some((wd, *bmin, *bmax, origs.clone()));
                }
            }
            if let Some((wd, bmin, bmax, origs)) = moved {
                let wd = self.hook_move(scene, bmin, bmax, wd);
                let wd = self.collide_translate(scene, &origs, wd);
                for (id, p) in origs {
                    if let Some(o) = self.map.get_mut(id) {
                        o.pos = p + wd;
                    }
                }
            }
        }

        if resp.drag_stopped_by(PointerButton::Primary) {
            let drag = std::mem::replace(&mut self.drag, Drag::None);
            if let Drag::Wall3D { start } = drag {
                let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
                if let Some(p) = ray_plane(o, d, self.work_y) {
                    self.create_wall(start, self.straight_end(start, Vec3::new(p.x, self.work_y, p.z), mods.command));
                }
            } else if let Drag::Outline3D { start, square } = drag {
                let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
                if let Some(p) = ray_plane(o, d, start.y) {
                    let pts = crate::previews::outline_rect(start, Vec3::new(snap(p.x, g), start.y, snap(p.z, g)), square);
                    if !pts.is_empty() {
                        self.layout_pts = pts;
                        self.close_layout();
                    }
                }
            } else if let Drag::Opening { slot: 0 } = drag {
                let (o, d) = self.cam.ray(rect, fov, self.drag_cur);
                if let Some(h) = pick_ray(scene, o, d) {
                    self.cut_opening_at(h.id, h.point, (o, d), self.tool == Tool::Window, !mods.command);
                }
            } else if let Drag::Knife { slot: 0, start } = drag {
                let end = self.drag_cur;
                self.knife_release_3d(rect, scene, start, end);
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = format!("Anvil - {}{}", self.map.name, if self.dirty { " *" } else { "" });
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

impl AnvilApp {
    /// The text tool's font (Arial or the first installed font by default).
    fn text_font(&mut self) -> Option<(String, String)> {
        if self.settings.text_font_path.is_empty() || !Path::new(&self.settings.text_font_path).is_file() {
            let f = crate::text::default_font()?;
            self.settings.text_font_path = f.path;
            self.settings.text_font_name = f.name;
        }
        Some((self.settings.text_font_path.clone(), self.settings.text_font_name.clone()))
    }

    fn ensure_fonts(&mut self) {
        if self.fonts.is_none() && self.fonts_rx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(crate::text::system_fonts());
            });
            self.fonts_rx = Some(rx);
        }
    }

    /// Font picker with search. Returns the chosen font, if one was clicked.
    fn font_picker(&mut self, ui: &mut egui::Ui, current: &str) -> Option<crate::text::FontInfo> {
        self.ensure_fonts();
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.label("Font");
            ui.label(RichText::new(if current.is_empty() { "(default)" } else { current }).strong());
        });
        ui.add(egui::TextEdit::singleline(&mut self.font_search).hint_text("search fonts").desired_width(200.0));
        match &self.fonts {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Finding installed fonts…");
                });
            }
            Some(fonts) => {
                let q = self.font_search.to_ascii_lowercase();
                let list: Vec<&crate::text::FontInfo> = fonts.iter().filter(|f| q.is_empty() || f.name.to_ascii_lowercase().contains(&q)).collect();
                let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                egui::ScrollArea::vertical().id_salt("font_list").max_height(140.0).auto_shrink([false, true]).show_rows(ui, row, list.len(), |ui, range| {
                    for f in &list[range] {
                        if ui.selectable_label(f.name == current, &f.name).on_hover_text(&f.path).clicked() {
                            picked = Some((*f).clone());
                        }
                    }
                });
                ui.label(RichText::new(format!("{} fonts", fonts.len())).small().weak());
            }
        }
        picked
    }

    pub(crate) fn text_tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Text").strong());
            ui.add(egui::TextEdit::multiline(&mut self.settings.text_content).desired_rows(2).desired_width(f32::INFINITY));
            let cur = self.settings.text_font_name.clone();
            if let Some(f) = self.font_picker(ui, &cur) {
                self.settings.text_font_path = f.path;
                self.settings.text_font_name = f.name;
            }
            ui.horizontal(|ui| {
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut self.settings.text_height).speed(0.1).range(0.05..=1000.0));
                ui.label("Depth");
                ui.add(egui::DragValue::new(&mut self.settings.text_depth).speed(0.05).range(0.01..=1000.0));
            });
        });
    }

    /// Builds the text mesh and places it standing at `at` (or flat on a wall).
    pub(crate) fn place_text(&mut self, at: Vec3, yaw_deg: f32, wall_normal: Option<Vec3>) {
        let Some((path, name)) = self.text_font() else {
            self.status = "No fonts found on this PC".into();
            return;
        };
        let (text, h, d) = (self.settings.text_content.clone(), self.settings.text_height, self.settings.text_depth);
        match crate::text::text_mesh(&path, &text, h, d) {
            Ok((verts, faces)) => {
                let mut pos = at;
                if let Some(n) = wall_normal {
                    pos += n * (d * 0.5 + 0.01);
                    pos.y -= h * 0.35; // roughly center the letters on the click
                }
                let mut o = Object::new(Kind::Mesh { verts, faces }, pos, self.settings.default_color, self.settings.default_material);
                o.rot.y = yaw_deg;
                let short: String = text.chars().take(24).collect::<String>().replace('\n', " ");
                o.name = format!("Text: {short}");
                o.text = Some(crate::map::TextSpec { text, font: path, font_name: name, height: h, depth: d });
                self.add_and_select(o);
                self.status = "Placed 3D text. Edit or rebuild it in the Inspector.".into();
            }
            Err(e) => self.status = e,
        }
    }

    /// Inspector section for text objects: edit and rebuild.
    fn text_inspector(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(spec) = self.map.get(id).and_then(|o| o.text.clone()) else { return };
        let mut spec2 = spec.clone();
        let mut rebuild = false;
        ui.separator();
        ui.label(RichText::new("3D text").strong());
        ui.add(egui::TextEdit::multiline(&mut spec2.text).desired_rows(2).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut spec2.height).speed(0.1).range(0.05..=1000.0));
            ui.label("Depth");
            ui.add(egui::DragValue::new(&mut spec2.depth).speed(0.05).range(0.01..=1000.0));
        });
        let have_font = Path::new(&spec2.font).is_file();
        if !have_font {
            ui.label(RichText::new(format!("Font '{}' isn't installed here. The text still shows; pick a font to rebuild it.", spec2.font_name)).small().weak());
        }
        egui::CollapsingHeader::new(format!("Font: {}", spec2.font_name)).id_salt(("text_font", id)).show(ui, |ui| {
            if let Some(f) = self.font_picker(ui, &spec2.font_name) {
                spec2.font = f.path;
                spec2.font_name = f.name;
                rebuild = true;
            }
        });
        if ui.button("Rebuild text").clicked() {
            rebuild = true;
        }
        if rebuild || spec2 != spec {
            if rebuild || ui.ctx().input(|i| i.key_pressed(Key::Enter)) || spec2.height != spec.height || spec2.depth != spec.depth || spec2.font != spec.font {
                match crate::text::text_mesh(&spec2.font, &spec2.text, spec2.height, spec2.depth) {
                    Ok((verts, faces)) => {
                        self.push_undo();
                        if let Some(o) = self.map.get_mut(id) {
                            o.kind = Kind::Mesh { verts, faces };
                            o.face_tex.clear();
                            let short: String = spec2.text.chars().take(24).collect::<String>().replace('\n', " ");
                            o.name = format!("Text: {short}");
                            o.text = Some(spec2);
                        }
                    }
                    Err(e) => self.status = e,
                }
            } else if let Some(o) = self.map.get_mut(id) {
                // typing: keep the text, rebuild on Enter / button
                o.text = Some(spec2);
            }
        }
    }
}

impl AnvilApp {
    /// Switches the 3D view between perspective and the six flat views.
    pub(crate) fn set_view(&mut self, v: u8, rect: Rect) {
        const BACK: f32 = 5000.0;
        let fw = self.cam.forward();
        let target = if self.cam.is_ortho() {
            self.cam.pos + fw * BACK
        } else {
            ray_plane(self.cam.pos, fw, self.work_y)
                .filter(|p| (*p - self.cam.pos).length() < 400.0)
                .unwrap_or(self.cam.pos + fw * 40.0)
        };
        if v == 0 {
            match self.persp_cam.take() {
                Some(c) => self.cam = c,
                None => {
                    self.cam.ortho = 0.0;
                    self.cam.pitch = self.cam.pitch.clamp(-1.5, 1.5);
                    self.cam.pos = target - self.cam.forward() * 40.0;
                }
            }
            self.view_preset = 0;
            return;
        }
        let half = if self.cam.is_ortho() {
            self.cam.ortho
        } else {
            self.persp_cam = Some(self.cam);
            (target - self.cam.pos).length().max(5.0) * (self.settings.fov.to_radians() * 0.5).tan()
        };
        use std::f32::consts::{FRAC_PI_2, PI};
        let (yaw, pitch) = match v {
            1 => (0.0, -FRAC_PI_2),
            2 => (0.0, FRAC_PI_2),
            3 => (0.0, 0.0),
            4 => (PI, 0.0),
            5 => (FRAC_PI_2, 0.0),
            _ => (-FRAC_PI_2, 0.0),
        };
        self.cam.yaw = yaw;
        self.cam.pitch = pitch;
        self.cam.ortho = half.max(1.0);
        self.cam.pos = target - self.cam.forward() * BACK;
        self.view_preset = v;
        let _ = rect;
    }

    /// W / E / R pick move / rotate / scale, X toggles local / global.
    fn gizmo_keys(&mut self, ctx: &egui::Context) {
        let (w, e, r, x) = ctx.input(|i| (i.key_pressed(Key::W), i.key_pressed(Key::E), i.key_pressed(Key::R), i.key_pressed(Key::X)));
        if w {
            self.gizmo_mode = 0;
        }
        if e {
            self.gizmo_mode = 1;
        }
        if r {
            self.gizmo_mode = 2;
        }
        if x {
            self.gizmo_local = !self.gizmo_local;
        }
        if w || e || r || x {
            self.status = format!("Gizmo: {} ({} axes)", self.gizmo_mode_name(), if self.gizmo_local { "local" } else { "global" });
        }
    }

    /// Move / Rotate / Scale and Local / Global buttons.
    fn gizmo_toolbar(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(self.settings.collisions, "⛶ Collisions")
            .on_hover_text("On: moving and rotating stop flush against other parts instead of passing through them")
            .clicked()
        {
            self.settings.collisions = !self.settings.collisions;
            let _ = self.settings.save();
        }
        for (i, (label, key)) in [("✥ Move", "W"), ("⟳ Rotate", "E"), ("⤢ Scale", "R")].iter().enumerate() {
            if ui.selectable_label(self.gizmo_mode == i as u8, *label).on_hover_text(format!("{key}")).clicked() {
                self.gizmo_mode = i as u8;
                if self.edit.is_none() {
                    self.tool = Tool::Select;
                }
            }
        }
        let space = if self.gizmo_local { "Local" } else { "Global" };
        if ui
            .selectable_label(self.gizmo_local, space)
            .on_hover_text("X: move / rotate / scale along the object's own axes (Local) or the world's (Global)")
            .clicked()
        {
            self.gizmo_local = !self.gizmo_local;
        }
    }
}

/// The selection after clicking `id` in the object list:
/// plain click = just it; Shift = add or remove it (one by one);
/// Ctrl = add everything between the last clicked object and it.
pub(crate) fn outliner_pick(sel: &[u64], order: &[u64], anchor: Option<u64>, id: u64, shift: bool, ctrl: bool) -> Vec<u64> {
    let mut out = sel.to_vec();
    if ctrl {
        let a = anchor.and_then(|a| order.iter().position(|x| *x == a));
        let b = order.iter().position(|x| *x == id);
        match (a, b) {
            (Some(a), Some(b)) => {
                for x in &order[a.min(b)..=a.max(b)] {
                    if !out.contains(x) {
                        out.push(*x);
                    }
                }
            }
            _ => {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    } else if shift {
        match out.iter().position(|x| *x == id) {
            Some(i) => {
                out.remove(i);
            }
            None => out.push(id),
        }
    } else {
        out = vec![id];
    }
    out
}

#[derive(Default)]
struct OutAct {
    click: Option<u64>,
    toggle: Option<u64>,
    select_folder: Option<u64>,
    hide_folder: Option<u64>,
    ungroup: Option<u64>,
    rename: Option<u64>,
}

impl eframe::App for AnvilApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) {
            self.inspector_editing = false;
        }
        self.textures.set_max_dim(self.settings.preview_texture_size);
        crate::themes::apply(ctx, self.settings.theme, self.settings.accent, &mut self.theme_applied);
        self.thumbs.begin_frame();
        if let Some(rx) = &self.fonts_rx {
            if let Ok(f) = rx.try_recv() {
                self.fonts = Some(f);
                self.fonts_rx = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        self.poll_cloud(ctx);
        self.poll_mounts(ctx);
        self.poll_team(ctx);
        self.poll_live();
        self.poll_blend(ctx);
        self.poll_update(ctx);
        if self.live.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let now = ctx.input(|i| i.time);
        if self.last_autosave == 0.0 {
            self.last_autosave = now;
        }
        if self.dirty && self.settings.autosave_minutes > 0.0 && now - self.last_autosave > self.settings.autosave_minutes as f64 * 60.0 {
            self.autosave_now(now);
        }
        if self.dirty && self.settings.autosave_minutes > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs(30));
        }
        self.shortcuts(ctx);
        self.update_title(ctx);
        if self.play.is_none() {
            self.menu_bar(ctx);
            self.update_banner(ctx);
            self.restore_banner(ctx);
            self.status_bar(ctx);
            self.library_panel(ctx);
            self.right_panel(ctx);
        }
        self.viewports(ctx);
        if self.play.is_none() {
            self.settings_window(ctx);
            self.help_window(ctx);
            self.textures_window(ctx);
            self.plugins_window(ctx);
            self.export_window(ctx);
            self.content_window(ctx);
            self.team_window(ctx);
            self.about_window(ctx);
            self.plugin_trust_window(ctx);
            self.plugin_panels(ctx);
            self.end_ghost_frame();
            self.uv_window(ctx);
            self.scatter_window(ctx);
        }
    }
}

/// Dashed red line + scissors-ish end marks while dragging the knife.
pub(crate) fn paint_knife_line(painter: &egui::Painter, a: Pos2, b: Pos2, split: bool) {
    let col = if split { Color32::from_rgb(255, 80, 80) } else { Color32::from_rgb(255, 200, 70) };
    let d = b - a;
    let len = d.length();
    if len < 1.0 {
        return;
    }
    let dir = d / len;
    let mut t = 0.0;
    while t < len {
        let e = (t + 8.0).min(len);
        painter.line_segment([a + dir * t, a + dir * e], Stroke::new(2.0f32, col));
        t += 13.0;
    }
    painter.circle_filled(a, 3.5, col);
    painter.circle_filled(b, 3.5, col);
}
