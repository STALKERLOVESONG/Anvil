//! Model thumbnails, drawn with the editor's own software renderer and
//! cached in memory and on disk (`thumbs/` next to anvil.exe).

use crate::app::AnvilApp;
use crate::map::{Kind, Map, Material, Object};
use crate::mesh::{build_scene, ModelCache, TriMesh, WorldMesh};
use crate::texture::TexCache;
use crate::view3d::{Camera, RenderOpts, Renderer};
use eframe::egui::{self, TextureHandle, TextureOptions};
use glam::Vec3;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const THUMB: usize = 96;
/// Bump when thumbnails should be redrawn (look changed).
const VERSION: u32 = 1;

#[derive(Default)]
pub(crate) struct Thumbs {
    mem: HashMap<String, Option<TextureHandle>>,
    renderer: Renderer,
    /// New thumbnails left to draw this frame.
    budget: usize,
}

impl Thumbs {
    pub fn begin_frame(&mut self) {
        self.budget = 2;
    }
}

pub fn thumbs_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from(".")).join("thumbs")
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Disk cache name: changes whenever the file changes.
pub fn cache_key(key: &str) -> String {
    let stamp = std::fs::metadata(key)
        .ok()
        .map(|m| format!("{}:{:?}", m.len(), m.modified().ok()))
        .unwrap_or_default();
    format!("{:016x}.png", fnv(&format!("{VERSION}|{key}|{stamp}")))
}

/// Renders a scene to a THUMB x THUMB RGBA image from a 3/4 view, framed to fit.
pub fn render_scene(ctx: &egui::Context, r: &mut Renderer, scene: &[Arc<WorldMesh>], textures: &mut TexCache) -> Option<Vec<u8>> {
    let (mn, mx) = scene.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), w| (a.min(w.min), b.max(w.max)));
    if scene.is_empty() || !mn.is_finite() || !mx.is_finite() {
        return None;
    }
    let c = (mn + mx) * 0.5;
    let radius = ((mx - mn).length() * 0.5).max(0.05);
    let fov: f32 = 35.0;
    let dist = radius / (fov.to_radians() * 0.5).sin() * 1.05;
    let dir = Vec3::new(0.85, 0.6, 1.0).normalize();
    let cam = Camera::looking_at(c + dir * dist, c);
    let ppp = ctx.pixels_per_point();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(THUMB as f32, THUMB as f32));
    let opts = RenderOpts { fov, grid: 1.0, plane_y: mn.y, show_grid: false, outlines: false, bg: [38, 42, 50], scale: 1.0 / ppp, filter: true, lighting: Default::default() };
    r.render(ctx, rect, &cam, &opts, scene, &[], &HashMap::new(), textures);
    let (w, h, px) = r.pixels();
    (w == THUMB && h == THUMB).then(|| px.to_vec())
}

fn prop_scene(path: &str, models: &mut ModelCache) -> Vec<Arc<WorldMesh>> {
    let mut m = Map::default();
    m.add(Object::new(Kind::Prop { model: path.to_string(), scale: Vec3::ONE, roblox_mesh_id: String::new() }, Vec3::ZERO, [205, 205, 210], Material::Plastic));
    build_scene(&m, models)
}

/// TriMesh from a Source model (geometry only, for the content browser preview).
pub fn source_mesh(model: &crate::source::SourceModel, scale: f32) -> TriMesh {
    TriMesh { pos: model.pos.iter().map(|p| *p * scale).collect(), tris: model.tris.clone(), ..Default::default() }
}

impl AnvilApp {
    fn thumb_from(&mut self, ctx: &egui::Context, key: &str, disk: bool, scene: impl FnOnce(&mut Self) -> Option<Vec<Arc<WorldMesh>>>) -> Option<egui::TextureId> {
        if let Some(h) = self.thumbs.mem.get(key) {
            return h.as_ref().map(|h| h.id());
        }
        let file = thumbs_dir().join(cache_key(key));
        if disk {
            if let Some(img) = crate::texture::open_image(&file.to_string_lossy()) {
                let img = img.to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                let h = ctx.load_texture(format!("thumb:{key}"), ci, TextureOptions::LINEAR);
                let id = h.id();
                self.thumbs.mem.insert(key.to_string(), Some(h));
                return Some(id);
            }
        }
        if self.thumbs.budget == 0 {
            ctx.request_repaint();
            return None;
        }
        self.thumbs.budget -= 1;
        let Some(sc) = scene(self) else {
            self.thumbs.mem.insert(key.to_string(), None);
            return None;
        };
        let mut r = std::mem::take(&mut self.thumbs.renderer);
        let px = render_scene(ctx, &mut r, &sc, &mut self.textures);
        self.thumbs.renderer = r;
        let Some(px) = px else {
            self.thumbs.mem.insert(key.to_string(), None);
            return None;
        };
        if disk {
            let _ = std::fs::create_dir_all(thumbs_dir());
            if let Some(img) = image::RgbaImage::from_raw(THUMB as u32, THUMB as u32, px.clone()) {
                let _ = img.save(&file);
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([THUMB, THUMB], &px);
        let h = ctx.load_texture(format!("thumb:{key}"), ci, TextureOptions::LINEAR);
        let id = h.id();
        self.thumbs.mem.insert(key.to_string(), Some(h));
        Some(id)
    }

    /// Thumbnail of a model file in the library (None while it's queued or if it can't load).
    pub(crate) fn model_thumb(&mut self, ctx: &egui::Context, path: &str) -> Option<egui::TextureId> {
        let p = path.to_string();
        self.thumb_from(ctx, path, true, move |app| {
            let scene = prop_scene(&p, &mut app.models);
            (!matches!(app.models.peek(&p), Some(Err(_)))).then_some(scene)
        })
    }

    /// Thumbnail of a Half-Life / Source prop straight from the mounted content.
    pub(crate) fn source_prop_thumb(&mut self, ctx: &egui::Context, rel: &str) -> Option<egui::TextureId> {
        let key = format!("mdl:{rel}");
        let r = rel.to_string();
        self.thumbs.budget = self.thumbs.budget.max(1);
        self.thumb_from(ctx, &key, true, move |app| {
            let m = app.mounts.clone()?;
            let model = crate::source::load_mdl(&|x| m.read(x), &crate::source::norm(&r)).ok()?;
            let mut cache = ModelCache::default();
            let k = format!("mdl:{r}");
            cache.insert(&k, source_mesh(&model, app.settings.source_scale));
            Some(prop_scene(&k, &mut cache))
        })
    }

    /// Forget a model's thumbnail (after re-importing it).
    pub(crate) fn forget_thumb(&mut self, key: &str) {
        self.thumbs.mem.remove(key);
    }
}
