//! 3D view: fly / orthographic camera and a multithreaded software
//! rasterizer (depth buffer, perspective-correct textures, normal maps,
//! decals, tinting). The screen is split into row bands that are drawn in
//! parallel with rayon.

use crate::mesh::{WorldMesh, NO_TEX};
use crate::texture::{TexCache, TexImage};
use eframe::egui::{self, pos2, Pos2, Rect};
use glam::{Mat3, Vec2, Vec3};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

pub const NEAR: f32 = 0.05;
/// Depth range used by orthographic views (depth = 1 - z / ORTHO_FAR).
const ORTHO_FAR: f32 = 100_000.0;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// 0 = perspective, otherwise orthographic with this half-height (units).
    pub ortho: f32,
}

impl Camera {
    pub fn looking_at(pos: Vec3, target: Vec3) -> Self {
        let d = (target - pos).normalize_or_zero();
        Camera { pos, yaw: d.x.atan2(-d.z), pitch: d.y.clamp(-1.0, 1.0).asin(), ortho: 0.0 }
    }

    pub fn is_ortho(&self) -> bool {
        self.ortho > 0.0
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.sin(), self.pitch.sin(), -self.pitch.cos() * self.yaw.cos())
    }

    pub fn right(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin())
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// World -> view space (x right, y up, z forward).
    pub fn to_view(&self, p: Vec3, basis: &(Vec3, Vec3, Vec3)) -> Vec3 {
        let d = p - self.pos;
        Vec3::new(d.dot(basis.0), d.dot(basis.1), d.dot(basis.2))
    }

    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        (self.right(), self.up(), self.forward())
    }

    pub fn focal(height: f32, fov_deg: f32) -> f32 {
        height * 0.5 / (fov_deg.to_radians() * 0.5).tan()
    }

    /// Pixels per unit: at depth 1 (perspective) or everywhere (ortho).
    pub fn px_scale(&self, height: f32, fov: f32) -> f32 {
        if self.is_ortho() {
            height * 0.5 / self.ortho
        } else {
            Self::focal(height, fov)
        }
    }

    fn proj_view(&self, v: Vec3, c: Pos2, k: f32) -> Pos2 {
        if self.is_ortho() {
            pos2(c.x + v.x * k, c.y - v.y * k)
        } else {
            pos2(c.x + v.x / v.z * k, c.y - v.y / v.z * k)
        }
    }

    pub fn ray(&self, rect: Rect, fov: f32, s: Pos2) -> (Vec3, Vec3) {
        let k = self.px_scale(rect.height(), fov);
        let c = rect.center();
        let (r, u, fw) = self.basis();
        let x = (s.x - c.x) / k;
        let y = -(s.y - c.y) / k;
        if self.is_ortho() {
            (self.pos + r * x + u * y, fw)
        } else {
            (self.pos, (fw + r * x + u * y).normalize())
        }
    }

    /// Projects a world point into UI points (None if behind the camera).
    pub fn project(&self, rect: Rect, fov: f32, p: Vec3) -> Option<Pos2> {
        let v = self.to_view(p, &self.basis());
        if v.z < NEAR {
            return None;
        }
        Some(self.proj_view(v, rect.center(), self.px_scale(rect.height(), fov)))
    }

    /// Projects a world-space segment into UI points (clipped to the near plane).
    pub fn project_segment(&self, rect: Rect, fov: f32, a: Vec3, b: Vec3) -> Option<(Pos2, Pos2)> {
        let basis = self.basis();
        let (a, b) = clip_segment(self.to_view(a, &basis), self.to_view(b, &basis))?;
        let k = self.px_scale(rect.height(), fov);
        Some((self.proj_view(a, rect.center(), k), self.proj_view(b, rect.center(), k)))
    }

    /// World units covered by one screen point at `p`.
    pub fn units_per_pixel(&self, rect: Rect, fov: f32, p: Vec3) -> f32 {
        let k = self.px_scale(rect.height(), fov);
        if self.is_ortho() {
            1.0 / k
        } else {
            (p - self.pos).dot(self.forward()).max(0.1) / k
        }
    }
}

fn clip_segment(a: Vec3, b: Vec3) -> Option<(Vec3, Vec3)> {
    let (ain, bin) = (a.z >= NEAR, b.z >= NEAR);
    match (ain, bin) {
        (true, true) => Some((a, b)),
        (false, false) => None,
        _ => {
            let t = (NEAR - a.z) / (b.z - a.z);
            let m = a + (b - a) * t;
            if ain {
                Some((a, m))
            } else {
                Some((m, b))
            }
        }
    }
}

/// Clips a triangle (with UVs) against the near plane. Returns up to 4 verts.
fn clip_near(t: [(Vec3, Vec2); 3], out: &mut [(Vec3, Vec2); 4]) -> usize {
    let mut n = 0;
    for i in 0..3 {
        let a = t[i];
        let b = t[(i + 1) % 3];
        let (ain, bin) = (a.0.z >= NEAR, b.0.z >= NEAR);
        if ain {
            out[n] = a;
            n += 1;
        }
        if ain != bin {
            let s = (NEAR - a.0.z) / (b.0.z - a.0.z);
            out[n] = (a.0 + (b.0 - a.0) * s, a.1 + (b.1 - a.1) * s);
            n += 1;
        }
    }
    n
}

pub struct RenderOpts {
    pub fov: f32,
    pub grid: f32,
    pub plane_y: f32,
    pub show_grid: bool,
    pub outlines: bool,
    pub bg: [u8; 3],
    pub scale: f32,
    /// Bilinear texture filtering (mipmaps are always used).
    pub filter: bool,
    /// Sun, shadows, sky, fog, point lights, AO (off = flat shading).
    pub lighting: crate::lighting::Lighting,
}

#[inline]
fn light(n: Vec3, metallic: bool) -> f32 {
    let l1 = Vec3::new(0.4, 0.85, 0.3).normalize();
    let l2 = Vec3::new(-0.5, 0.3, -0.6).normalize();
    let d1 = n.dot(l1).max(0.0);
    let d2 = n.dot(l2).max(0.0);
    if metallic {
        0.22 + 0.45 * d1 + 0.85 * d1.powi(12) + 0.12 * d2
    } else {
        0.36 + 0.56 * d1 + 0.16 * d2
    }
}

#[inline]
fn finish3(c: [f32; 3], selected: bool) -> [u8; 3] {
    let mut o = [0u8; 3];
    for i in 0..3 {
        let mut v = c[i];
        if selected {
            v = v * 0.55 + [255.0, 70.0, 50.0][i] * 0.45;
        }
        o[i] = v.clamp(0.0, 255.0) as u8;
    }
    o
}

/// Everything per-pixel lighting needs to turn a depth sample back into a
/// world position.
struct LitCtx<'a> {
    l: &'a crate::lighting::Lighting,
    shadow: Option<&'a crate::lighting::ShadowMap>,
    cam: Vec3,
    r: Vec3,
    u: Vec3,
    f: Vec3,
    cx: f32,
    cy: f32,
    k: f32,
    ortho: bool,
}

impl LitCtx<'_> {
    #[inline]
    fn view_z(&self, z: f32) -> f32 {
        if self.ortho { (1.0 - z) * ORTHO_FAR } else { 1.0 / z }
    }
    #[inline]
    fn world(&self, px: f32, py: f32, z: f32) -> Vec3 {
        let vz = self.view_z(z);
        let (sx, sy) = ((px - self.cx) / self.k, -(py - self.cy) / self.k);
        let (vx, vy) = if self.ortho { (sx, sy) } else { (sx * vz, sy * vz) };
        self.cam + self.r * vx + self.u * vy + self.f * vz
    }
    #[inline]
    fn shade(&self, base: [f32; 3], px: f32, py: f32, z: f32, n: Vec3, metallic: bool, lmask: u64) -> [f32; 3] {
        let m = crate::lighting::light_at(self.l, self.shadow, self.world(px, py, z), n, metallic, lmask);
        [base[0] * m[0], base[1] * m[1], base[2] * m[2]]
    }
}

#[inline]
fn finish(c: [f32; 3], k: f32, selected: bool) -> [u8; 3] {
    let mut o = [0u8; 3];
    for i in 0..3 {
        let mut v = c[i] * k;
        if selected {
            v = v * 0.55 + [255.0, 70.0, 50.0][i] * 0.45;
        }
        o[i] = v.clamp(0.0, 255.0) as u8;
    }
    o
}

/// Shared per texture slot.
struct Paint {
    tint: [f32; 3],
    alpha: f32,
    tex: Arc<TexImage>,
    nrm: Option<Arc<TexImage>>,
    metallic: bool,
    decal: bool,
}

enum Shade {
    /// fast = lit the old way; base = unlit color for per-pixel lighting.
    Flat { fast: [u8; 3], base: [f32; 3], alpha: f32, sel: bool },
    Tex { paint: Arc<Paint>, k: f32, selected: bool, tbn: Mat3 },
}

/// A screen-space triangle: p = (x, y, depth), w = perspective weight.
struct RTri {
    p: [Vec3; 3],
    w: [f32; 3],
    uvw: [Vec2; 3],
    shade: Shade,
    /// Screen-space gradients (per pixel) of U*w, V*w (in texels) and w:
    /// [dU/dx, dU/dy, dV/dx, dV/dy, dW/dx, dW/dy]. Gives the exact texel
    /// footprint per pixel for choosing the mip level.
    grad: [f32; 6],
    /// Texture size, to convert uv to texels.
    tsize: Vec2,
    /// World normal, and which point lights can reach this triangle.
    n: Vec3,
    metallic: bool,
    lmask: u64,
}

struct RLine {
    a: Vec3,
    b: Vec3,
    col: [u8; 3],
    bias: f32,
    test: bool,
    /// Grid lines: (strength 0-1, fade starts, fully gone) by view distance.
    fade: Option<(f32, f32, f32)>,
}

#[derive(Default)]
struct Frame {
    grid: Vec<RLine>,
    opaque: Vec<RTri>,
    outlines: Vec<RLine>,
    deferred: Vec<(f32, RTri)>,
    sel: Vec<RLine>,
}

/// A horizontal slice of the frame buffers, drawn by one thread.
struct Band<'a> {
    w: usize,
    y0: usize,
    rows: usize,
    color: &'a mut [u8],
    depth: &'a mut [f32],
    ortho: bool,
    bilinear: bool,
    lit: Option<&'a LitCtx<'a>>,
}

impl Band<'_> {
    fn fill(&mut self, t: &RTri, write_depth: bool) {
        let (a, b, c) = (t.p[0], t.p[1], t.p[2]);
        let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if area.abs() < 1e-6 || !area.is_finite() {
            return;
        }
        let wf = self.w as f32;
        let minx = a.x.min(b.x).min(c.x).floor().max(0.0);
        let maxx = a.x.max(b.x).max(c.x).ceil().min(wf - 1.0);
        let miny = a.y.min(b.y).min(c.y).floor().max(self.y0 as f32);
        let maxy = a.y.max(b.y).max(c.y).ceil().min((self.y0 + self.rows) as f32 - 1.0);
        if minx > maxx || miny > maxy {
            return;
        }
        let inv = 1.0 / area;
        let (x0, x1, y0, y1) = (minx as usize, maxx as usize, miny as usize, maxy as usize);
        for y in y0..=y1 {
            let py = y as f32 + 0.5;
            let row = (y - self.y0) * self.w;
            for x in x0..=x1 {
                let px = x as f32 + 0.5;
                let w0 = ((c.x - b.x) * (py - b.y) - (c.y - b.y) * (px - b.x)) * inv;
                let w1 = ((a.x - c.x) * (py - c.y) - (a.y - c.y) * (px - c.x)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a.z + w1 * b.z + w2 * c.z;
                let i = row + x;
                if z <= self.depth[i] {
                    continue;
                }
                let (out, alpha) = match &t.shade {
                    Shade::Flat { fast, base, alpha, sel } => match self.lit {
                        None => (*fast, *alpha),
                        Some(lc) => (finish3(lc.shade(*base, px, py, z, t.n, t.metallic, t.lmask), *sel), *alpha),
                    },
                    Shade::Tex { paint, k, selected, tbn } => {
                        let ww = w0 * t.w[0] + w1 * t.w[1] + w2 * t.w[2];
                        if ww <= 0.0 {
                            continue;
                        }
                        let uv = (t.uvw[0] * w0 + t.uvw[1] * w1 + t.uvw[2] * w2) / ww;
                        // exact texels-per-pixel here -> mip level (no shimmer / moire far away)
                        let (ut, vt) = (uv.x * t.tsize.x, uv.y * t.tsize.y);
                        let g = &t.grad;
                        let iw = 1.0 / ww;
                        let dudx = (g[0] - ut * g[4]) * iw;
                        let dudy = (g[1] - ut * g[5]) * iw;
                        let dvdx = (g[2] - vt * g[4]) * iw;
                        let dvdy = (g[3] - vt * g[5]) * iw;
                        let rho2 = (dudx * dudx + dvdx * dvdx).max(dudy * dudy + dvdy * dvdy);
                        let lod = if rho2 > 1.0 { 0.5 * rho2.log2() } else { 0.0 };
                        // bilinear only matters when magnified; mips already smooth the rest
                        let s = paint.tex.sample_lod(uv.x, uv.y, lod, self.bilinear && lod < 1.0);
                        let mut alpha = paint.alpha;
                        if paint.decal {
                            alpha *= s[3] as f32 / 255.0;
                        }
                        if alpha < 0.02 {
                            continue;
                        }
                        let col = [paint.tint[0] * s[0] as f32 / 255.0, paint.tint[1] * s[1] as f32 / 255.0, paint.tint[2] * s[2] as f32 / 255.0];
                        let bumped = paint.nrm.as_ref().map(|nm| {
                            let s = nm.sample_lod(uv.x, uv.y, lod, self.bilinear && lod < 1.0);
                            let tn = Vec3::new(s[0] as f32 / 127.5 - 1.0, s[1] as f32 / 127.5 - 1.0, s[2] as f32 / 127.5 - 1.0);
                            (*tbn * tn).normalize_or_zero()
                        });
                        match self.lit {
                            None => {
                                let kk = bumped.map(|n| light(n, paint.metallic)).unwrap_or(*k);
                                (finish(col, kk, *selected), alpha)
                            }
                            Some(lc) => (finish3(lc.shade(col, px, py, z, bumped.unwrap_or(t.n), paint.metallic, t.lmask), *selected), alpha),
                        }
                    }
                };
                if write_depth {
                    self.depth[i] = z;
                }
                let o = i * 4;
                if alpha >= 0.999 {
                    self.color[o..o + 3].copy_from_slice(&out);
                } else {
                    for ch in 0..3 {
                        let d = self.color[o + ch] as f32;
                        self.color[o + ch] = (d * (1.0 - alpha) + out[ch] as f32 * alpha) as u8;
                    }
                }
            }
        }
    }

    fn line(&mut self, l: &RLine) {
        let (a, b) = (l.a, l.b);
        let (wf, y0f, y1f) = (self.w as f32 - 1.0, self.y0 as f32, (self.y0 + self.rows) as f32 - 0.001);
        let d = b - a;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for (p, q) in [(-d.x, a.x), (d.x, wf - a.x), (-d.y, a.y - y0f), (d.y, y1f - a.y)] {
            if p.abs() < 1e-9 {
                if q < 0.0 {
                    return;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    if r > t1 {
                        return;
                    }
                    t0 = t0.max(r);
                } else {
                    if r < t0 {
                        return;
                    }
                    t1 = t1.min(r);
                }
            }
        }
        let (a2, b2) = (a + d * t0, a + d * t1);
        let steps = (b2.x - a2.x).abs().max((b2.y - a2.y).abs()).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = a2 + (b2 - a2) * t;
            if p.x < 0.0 || p.y < y0f {
                continue;
            }
            let (x, y) = (p.x as usize, p.y as usize);
            if x >= self.w || y >= self.y0 + self.rows {
                continue;
            }
            let idx = (y - self.y0) * self.w + x;
            if l.test {
                let dz = if self.ortho { p.z + (l.bias - 1.0) * 5e-5 } else { p.z * l.bias };
                if dz < self.depth[idx] {
                    continue;
                }
            }
            match l.fade {
                None => self.color[idx * 4..idx * 4 + 3].copy_from_slice(&l.col),
                Some((k, f0, f1)) => {
                    let dist = if self.ortho || p.z <= 0.0 { 0.0 } else { 1.0 / p.z };
                    let a = if dist <= f0 { k } else { k * ((f1 - dist) / (f1 - f0).max(1e-3)).clamp(0.0, 1.0) };
                    if a <= 0.0 {
                        continue;
                    }
                    let px = &mut self.color[idx * 4..idx * 4 + 3];
                    for c in 0..3 {
                        px[c] = (px[c] as f32 + (l.col[c] as f32 - px[c] as f32) * a) as u8;
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct Renderer {
    w: usize,
    h: usize,
    color: Vec<u8>,
    /// Larger = closer (1/z in perspective).
    depth: Vec<f32>,
    tex: Option<egui::TextureHandle>,
    /// Sun shadow map, kept until something that casts a shadow changes.
    shadow: Option<(u64, Arc<crate::lighting::ShadowMap>)>,
}

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        cam: &Camera,
        o: &RenderOpts,
        scene: &[Arc<WorldMesh>],
        selected: &[u64],
        remote_sel: &HashMap<u64, [u8; 3]>,
        textures: &mut TexCache,
    ) -> egui::TextureId {
        let ppp = ctx.pixels_per_point();
        let w = ((rect.width() * ppp * o.scale).round() as usize).clamp(1, 4096);
        let h = ((rect.height() * ppp * o.scale).round() as usize).clamp(1, 4096);
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.color = vec![0; w * h * 4];
            self.depth = vec![0.0; w * h];
        }
        let frame = build_frame(w, h, cam, o, scene, selected, remote_sel, textures);

        let ortho = cam.is_ortho();
        let lg = &o.lighting;
        let fwd = cam.forward();
        // sun shadow map around where the camera looks (rebuilt only when needed)
        let shadow = if lg.on && lg.shadows {
            let step = lg.shadow_radius * 0.125;
            let c = cam.pos + Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero() * lg.shadow_radius * 0.45;
            let center = Vec3::new((c.x / step).round() * step, 0.0, (c.z / step).round() * step);
            let key = crate::lighting::shadow_key(scene, lg.sun, center, lg.shadow_radius, lg.shadow_res);
            match &self.shadow {
                Some((k, m)) if *k == key => Some(m.clone()),
                _ => {
                    let m = Arc::new(crate::lighting::ShadowMap::build(scene, lg.sun, center, lg.shadow_radius, lg.shadow_res));
                    self.shadow = Some((key, m.clone()));
                    Some(m)
                }
            }
        } else {
            None
        };
        let (br, bu, bf) = cam.basis();
        let lc = lg.on.then(|| LitCtx { l: lg, shadow: shadow.as_deref(), cam: cam.pos, r: br, u: bu, f: bf, cx: w as f32 * 0.5, cy: h as f32 * 0.5, k: cam.px_scale(h as f32, o.fov), ortho });
        let lc = lc.as_ref();
        let sky = lg.on && lg.sky && !ortho;

        // pass 1 (row bands in parallel): background, grid, solid surfaces
        let bg = [o.bg[0], o.bg[1], o.bg[2], 255];
        let rows = (h / (rayon::current_num_threads() * 4)).clamp(4, 64);
        // which triangles / lines touch each band, so a band only looks at those
        let nb = h.div_ceil(rows);
        let tri_y = |t: &RTri| (t.p[0].y.min(t.p[1].y).min(t.p[2].y), t.p[0].y.max(t.p[1].y).max(t.p[2].y));
        let line_y = |l: &RLine| (l.a.y.min(l.b.y), l.a.y.max(l.b.y));
        let b_grid = bin(&frame.grid, nb, rows, line_y);
        let b_opaque = bin(&frame.opaque, nb, rows, tri_y);
        let b_outlines = bin(&frame.outlines, nb, rows, line_y);
        let b_deferred = bin(&frame.deferred, nb, rows, |d| tri_y(&d.1));
        let b_sel = bin(&frame.sel, nb, rows, line_y);
        let frame = &frame;
        let o = &*o;
        self.color.par_chunks_mut(w * 4 * rows).zip(self.depth.par_chunks_mut(w * rows)).enumerate().for_each(|(bi, (cb, db))| {
            let y0 = bi * rows;
            if sky {
                for (ry, row) in cb.chunks_exact_mut(w * 4).enumerate() {
                    let lc = lc.expect("sky needs lighting");
                    let d = (lc.f + lc.u * (-((y0 + ry) as f32 + 0.5 - lc.cy) / lc.k)).normalize_or_zero();
                    let s = crate::lighting::sky_color(lg, d.y);
                    let px = [s[0] as u8, s[1] as u8, s[2] as u8, 255];
                    for p in row.chunks_exact_mut(4) {
                        p.copy_from_slice(&px);
                    }
                }
            } else {
                for px in cb.chunks_exact_mut(4) {
                    px.copy_from_slice(&bg);
                }
            }
            db.fill(0.0);
            let mut band = Band { w, y0, rows: db.len() / w, color: cb, depth: db, ortho, bilinear: o.filter, lit: lc };
            for &i in &b_grid[bi] {
                band.line(&frame.grid[i as usize]);
            }
            for &i in &b_opaque[bi] {
                band.fill(&frame.opaque[i as usize], true);
            }
        });

        // pass 2: ambient occlusion and fog, reading the finished depth buffer
        if let Some(lc) = lc {
            if lg.ao || (lg.fog && !ortho) {
                let fog_col = if sky { lg.sky_horizon } else { [o.bg[0] as f32, o.bg[1] as f32, o.bg[2] as f32] };
                post_light(&mut self.color, &self.depth, w, rows, lc, fog_col);
            }
        }

        // pass 3: outlines, see-through objects, selection
        self.color.par_chunks_mut(w * 4 * rows).zip(self.depth.par_chunks_mut(w * rows)).enumerate().for_each(|(bi, (cb, db))| {
            let mut band = Band { w, y0: bi * rows, rows: db.len() / w, color: cb, depth: db, ortho, bilinear: o.filter, lit: lc };
            for &i in &b_outlines[bi] {
                band.line(&frame.outlines[i as usize]);
            }
            for &i in &b_deferred[bi] {
                band.fill(&frame.deferred[i as usize].1, false);
            }
            for &i in &b_sel[bi] {
                band.line(&frame.sel[i as usize]);
            }
        });

        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &self.color);
        match &mut self.tex {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture("anvil-3d-view", image, egui::TextureOptions::LINEAR)),
        }
        self.tex.as_ref().map(|t| t.id()).unwrap_or_default()
    }

    /// The last rendered frame (RGBA).
    pub fn pixels(&self) -> (usize, usize, &[u8]) {
        (self.w, self.h, &self.color)
    }
}

/// For each band of `rows` rows, the indices of the items whose y range
/// (in pixels) reaches it, in their original order.
fn bin<T>(items: &[T], nb: usize, rows: usize, yr: impl Fn(&T) -> (f32, f32)) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); nb.max(1)];
    let last = out.len() - 1;
    for (i, it) in items.iter().enumerate() {
        let (a, b) = yr(it);
        if !(a <= b) {
            continue;
        }
        let lo = ((a.floor().max(0.0) as usize) / rows).min(last);
        let hi = ((b.ceil().max(0.0) as usize) / rows).min(last);
        for band in &mut out[lo..=hi] {
            band.push(i as u32);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build_frame(
    w: usize,
    h: usize,
    cam: &Camera,
    o: &RenderOpts,
    scene: &[Arc<WorldMesh>],
    selected: &[u64],
    remote_sel: &HashMap<u64, [u8; 3]>,
    textures: &mut TexCache,
) -> Frame {
    let k = cam.px_scale(h as f32, o.fov);
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let ortho = cam.is_ortho();
    let proj = move |v: Vec3| -> (Vec3, f32) {
        if ortho {
            (Vec3::new(cx + v.x * k, cy - v.y * k, 1.0 - v.z / ORTHO_FAR), 1.0)
        } else {
            let iz = 1.0 / v.z;
            (Vec3::new(cx + v.x * iz * k, cy - v.y * iz * k, iz), iz)
        }
    };
    let basis = cam.basis();
    let fwd = cam.forward();
    let mut f = Frame::default();

    // ground grid on the work plane (background, drawn first)
    if o.show_grid {
        let mut step = o.grid.max(0.01);
        let span = if ortho { cam.ortho } else { (cam.pos.y - o.plane_y).abs().max(1.0) };
        while step < span / 12.0 {
            step *= 2.0;
        }
        let n = 40i64;
        let cxw = (cam.pos.x / step).round() as i64;
        let czw = (cam.pos.z / step).round() as i64;
        let ext = n as f32 * step;
        for i in -n..=n {
            for axis in 0..2 {
                let kk = if axis == 0 { cxw + i } else { czw + i };
                let v = kk as f32 * step;
                let (a, b) = if axis == 0 {
                    (Vec3::new(v, o.plane_y, czw as f32 * step - ext), Vec3::new(v, o.plane_y, czw as f32 * step + ext))
                } else {
                    (Vec3::new(cxw as f32 * step - ext, o.plane_y, v), Vec3::new(cxw as f32 * step + ext, o.plane_y, v))
                };
                let col = if kk == 0 {
                    if axis == 0 {
                        [70, 110, 220]
                    } else {
                        [220, 80, 70]
                    }
                } else if kk % 8 == 0 {
                    [118, 124, 136]
                } else {
                    [88, 95, 108]
                };
                if let Some((a, b)) = clip_segment(cam.to_view(a, &basis), cam.to_view(b, &basis)) {
                    // minor lines fade out early, major ones near the edge, so far
                    // away there's no shimmer and the edge doesn't cut off hard
                    let major = kk == 0 || kk % 8 == 0;
                    let fade = if ortho {
                        Some((if major { 1.0 } else { 0.7 }, f32::MAX, f32::MAX))
                    } else if major {
                        Some((1.0, ext * 0.25, ext * 0.8))
                    } else {
                        Some((0.7, ext * 0.1, ext * 0.35))
                    };
                    f.grid.push(RLine { a: proj(a).0, b: proj(b).0, col, bias: 1.0, test: false, fade });
                }
            }
        }
    }

    // what's on screen: each object's box against the view frustum, and
    // objects too far away to cover a pixel are skipped (selected ones stay)
    let (wf, hf) = (w as f32, h as f32);
    let (hx, hy) = (cx / k, cy / k);
    let on_screen = |wm: &WorldMesh| -> bool {
        let corners: Vec<Vec3> = (0..8)
            .map(|i| cam.to_view(Vec3::new(if i & 1 != 0 { wm.max.x } else { wm.min.x }, if i & 2 != 0 { wm.max.y } else { wm.min.y }, if i & 4 != 0 { wm.max.z } else { wm.min.z }), &basis))
            .collect();
        let out = |f: &dyn Fn(Vec3) -> bool| corners.iter().all(|v| f(*v));
        if ortho {
            if out(&|v| v.x > hx) || out(&|v| v.x < -hx) || out(&|v| v.y > hy) || out(&|v| v.y < -hy) || out(&|v| v.z < NEAR) {
                return false;
            }
        } else if out(&|v| v.z < NEAR) || out(&|v| v.x > v.z * hx) || out(&|v| -v.x > v.z * hx) || out(&|v| v.y > v.z * hy) || out(&|v| -v.y > v.z * hy) {
            return false;
        }
        if selected.contains(&wm.id) || remote_sel.contains_key(&wm.id) {
            return true;
        }
        let r = (wm.max - wm.min).length() * 0.5;
        let c = cam.to_view((wm.min + wm.max) * 0.5, &basis);
        let px = if ortho { r * k } else if c.z > r + NEAR { r * k / c.z } else { f32::MAX };
        px >= 0.35
    };
    // one shared Paint per texture slot (loading textures needs the cache, so
    // this part runs first, one object after another)
    let visible: Vec<(&Arc<WorldMesh>, Vec<Option<Arc<Paint>>>)> = scene
        .iter()
        .filter(|wm| on_screen(wm))
        .map(|wm| {
            let paints = wm
                .texs
                .iter()
                .map(|t| {
                    textures.get(&t.image).map(|tex| {
                        Arc::new(Paint {
                            tint: [t.tint[0] as f32, t.tint[1] as f32, t.tint[2] as f32],
                            alpha: wm.alpha,
                            tex,
                            nrm: if wm.tri_tbn.len() == wm.mesh.tris.len() { textures.get(&t.normal) } else { None },
                            metallic: t.metallic,
                            decal: wm.decal,
                        })
                    })
                })
                .collect();
            (wm, paints)
        })
        .collect();
    // triangle setup for all objects in parallel
    let parts: Vec<Frame> = visible
        .into_par_iter()
        .map(|(wm, paints)| {
            let mut part = Frame::default();
        let sel = selected.contains(&wm.id);
        let vs: Vec<Vec3> = wm.pos.iter().map(|p| cam.to_view(*p, &basis)).collect();
        let has_uv = wm.tri_uv.len() == wm.mesh.tris.len();
        // point lights that can reach this object at all
        let obj_lights: u64 = if o.lighting.on {
            let (c, r) = ((wm.min + wm.max) * 0.5, (wm.max - wm.min).length() * 0.5);
            o.lighting.lights.iter().take(64).enumerate().filter(|(_, l)| (l.pos - c).length() < l.range + r).fold(0u64, |m, (i, _)| m | (1u64 << i))
        } else {
            0
        };
        let deferred_obj = wm.alpha < 0.999 || wm.decal;
        let base = [wm.color[0] as f32, wm.color[1] as f32, wm.color[2] as f32];
        // far away, a detailed model is drawn from a simpler version whose
        // merged vertices are at most ~1.5 pixels apart on screen
        let near_z = if ortho {
            1.0
        } else {
            let c = cam.to_view((wm.min + wm.max) * 0.5, &basis);
            (c.z - (wm.max - wm.min).length() * 0.5).max(NEAR)
        };
        let lod = wm.lods.iter().rev().find(|l| l.cell * wm.lod_scale * k / near_z < 1.5);
        let all: Vec<(u32, [u32; 3])>;
        let tri_list: &[(u32, [u32; 3])] = match lod {
            Some(l) => &l.tris,
            None => {
                all = wm.mesh.tris.iter().enumerate().map(|(i, t)| (i as u32, *t)).collect();
                &all
            }
        };
        for &(ti, t) in tri_list {
            let ti = ti as usize;
            let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (a, b, c) = (wm.pos[i0], wm.pos[i1], wm.pos[i2]);
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len < 1e-9 {
                continue;
            }
            let n = n / len;
            let facing = if ortho { n.dot(fwd) } else { n.dot(a - cam.pos) };
            if facing >= 0.0 {
                continue; // back face
            }
            let tri = [vs[i0], vs[i1], vs[i2]];
            if tri.iter().all(|v| v.z < NEAR) {
                continue;
            }
            let slot = wm.tri_tex.get(ti).copied().unwrap_or(if wm.texs.is_empty() { NO_TEX } else { 0 });
            let paint = if has_uv && slot != NO_TEX { paints.get(slot as usize).cloned().flatten() } else { None };
            let metallic = paint.as_ref().is_some_and(|p| p.metallic);
            let uv = if paint.is_some() { wm.tri_uv[ti] } else { [Vec2::ZERO; 3] };
            let mut poly = [(Vec3::ZERO, Vec2::ZERO); 4];
            let cnt = clip_near([(tri[0], uv[0]), (tri[1], uv[1]), (tri[2], uv[2])], &mut poly);
            if cnt < 3 {
                continue;
            }
            let mut pp = [Vec3::ZERO; 4];
            let mut ww = [0.0f32; 4];
            let mut uvw = [Vec2::ZERO; 4];
            for q in 0..cnt {
                let (p, wq) = proj(poly[q].0);
                pp[q] = p;
                ww[q] = wq;
                uvw[q] = poly[q].1 * wq;
            }
            // off screen, or so small it covers no pixel center: nothing to draw
            let (mut mnx, mut mxx, mut mny, mut mxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for q in pp.iter().take(cnt) {
                mnx = mnx.min(q.x);
                mxx = mxx.max(q.x);
                mny = mny.min(q.y);
                mxy = mxy.max(q.y);
            }
            if mxx < 0.0 || mxy < 0.0 || mnx > wf || mny > hf || (mnx - 0.5).ceil() > (mxx - 0.5).floor() || (mny - 0.5).ceil() > (mxy - 0.5).floor() {
                continue;
            }
            let kf = light(n, metallic);
            let depth = (tri[0].z + tri[1].z + tri[2].z) / 3.0;
            let tsize = paint.as_ref().map(|p| Vec2::new(p.tex.w as f32, p.tex.h as f32)).unwrap_or(Vec2::ONE);
            for q in 1..cnt - 1 {
                // gradients of the affine attributes (U*w, V*w in texels, and w)
                let grad = {
                    let (a, b, c) = (pp[0], pp[q], pp[q + 1]);
                    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                    if area.abs() < 1e-9 {
                        [0.0; 6]
                    } else {
                        let inv = 1.0 / area;
                        let (d0x, d0y) = (-(c.y - b.y) * inv, (c.x - b.x) * inv);
                        let (d1x, d1y) = (-(a.y - c.y) * inv, (a.x - c.x) * inv);
                        let (d2x, d2y) = (-(d0x + d1x), -(d0y + d1y));
                        let f = |f0: f32, f1: f32, f2: f32| (f0 * d0x + f1 * d1x + f2 * d2x, f0 * d0y + f1 * d1y + f2 * d2y);
                        let (ux, uy) = f(uvw[0].x * tsize.x, uvw[q].x * tsize.x, uvw[q + 1].x * tsize.x);
                        let (vx, vy) = f(uvw[0].y * tsize.y, uvw[q].y * tsize.y, uvw[q + 1].y * tsize.y);
                        let (wx, wy) = f(ww[0], ww[q], ww[q + 1]);
                        [ux, uy, vx, vy, wx, wy]
                    }
                };
                let shade = match &paint {
                    Some(p) => Shade::Tex { paint: p.clone(), k: kf, selected: sel, tbn: wm.tri_tbn.get(ti).copied().unwrap_or(Mat3::IDENTITY) },
                    None => Shade::Flat { fast: finish(base, kf, sel), base, alpha: wm.alpha, sel },
                };
                let lmask = if obj_lights == 0 {
                    0
                } else {
                    let tc = (a + b + c) / 3.0;
                    let tr = (a - tc).length().max((b - tc).length()).max((c - tc).length());
                    let mut m = 0u64;
                    let mut bits = obj_lights;
                    while bits != 0 {
                        let i = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        let l = &o.lighting.lights[i];
                        if (l.pos - tc).length() < l.range + tr && n.dot(l.pos - tc) > -tr {
                            m |= 1 << i;
                        }
                    }
                    m
                };
                let rt = RTri { p: [pp[0], pp[q], pp[q + 1]], w: [ww[0], ww[q], ww[q + 1]], uvw: [uvw[0], uvw[q], uvw[q + 1]], shade, grad, tsize, n, metallic, lmask };
                if deferred_obj {
                    part.deferred.push((depth, rt));
                } else {
                    part.opaque.push(rt);
                }
            }
        }
        // outlines and selection wireframes
        let sel_col = if sel { Some([255, 215, 70]) } else { remote_sel.get(&wm.id).copied() };
        if sel_col.is_some() || (o.outlines && !wm.decal) {
            let (col, bias, list) = match sel_col {
                Some(c) => (c, 1.02, &mut part.sel),
                None => ([(wm.color[0] as f32 * 0.4) as u8, (wm.color[1] as f32 * 0.4) as u8, (wm.color[2] as f32 * 0.4) as u8], 1.004, &mut part.outlines),
            };
            for e in &wm.mesh.edges {
                if let Some((a, b)) = clip_segment(vs[e[0] as usize], vs[e[1] as usize]) {
                    list.push(RLine { a: proj(a).0, b: proj(b).0, col, bias, test: true, fade: None });
                }
            }
        }
            part
        })
        .collect();
    for p in parts {
        f.opaque.extend(p.opaque);
        f.deferred.extend(p.deferred);
        f.outlines.extend(p.outlines);
        f.sel.extend(p.sel);
    }
    f.deferred.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    f
}

/// Screen-space ambient occlusion + distance fog over the solid pixels.
///
/// AO compares pairs of opposite neighbors: on a flat surface the depth
/// buffer (1/z) is linear, so the pair's average matches the middle; in a
/// crease (floor meeting a wall) the neighbors are closer than that, which
/// darkens it. Flat floors seen at an angle don't get false shadows.
fn post_light(color: &mut [u8], depth: &[f32], w: usize, rows: usize, lc: &LitCtx, fog_col: [f32; 3]) {
    let l = lc.l;
    let h = depth.len() / w.max(1);
    const R: f32 = 1.6;
    let dirs: [(f32, f32); 4] = [(1.0, 0.0), (0.7071, 0.7071), (0.0, 1.0), (-0.7071, 0.7071)];
    color.par_chunks_mut(w * 4 * rows).enumerate().for_each(|(bi, cb)| {
        let y0 = bi * rows;
        let n = cb.len() / (w * 4);
        for yy in 0..n {
            let y = y0 + yy;
            for x in 0..w {
                let i = y * w + x;
                let z = depth[i];
                if z <= 0.0 {
                    continue;
                }
                let vz = lc.view_z(z);
                let mut k = 1.0f32;
                if l.ao {
                    let rpx = if lc.ortho { lc.k * R } else { lc.k * R / vz };
                    if rpx >= 1.5 {
                        let rpx = rpx.min(28.0);
                        let rot = ((x * 7 + y * 13) % 8) as f32 * 0.098;
                        let (sr, cr) = rot.sin_cos();
                        let mut occ = 0.0;
                        let mut cnt = 0.0;
                        for (dx, dy) in dirs {
                            let (dx, dy) = (dx * cr - dy * sr, dx * sr + dy * cr);
                            for rad in [rpx * 0.7, rpx * 0.35].into_iter().take(l.ao_rings.max(1) as usize) {
                                let (ox, oy) = ((dx * rad).round() as i64, (dy * rad).round() as i64);
                                let (ax, ay, bx, by) = (x as i64 + ox, y as i64 + oy, x as i64 - ox, y as i64 - oy);
                                if ax < 0 || bx < 0 || ay < 0 || by < 0 || ax >= w as i64 || bx >= w as i64 || ay >= h as i64 || by >= h as i64 {
                                    continue;
                                }
                                let (za, zb) = (depth[ay as usize * w + ax as usize], depth[by as usize * w + bx as usize]);
                                if za <= 0.0 || zb <= 0.0 {
                                    continue;
                                }
                                cnt += 1.0;
                                let mid = lc.view_z((za + zb) * 0.5);
                                let d = vz - mid;
                                if d > 0.02 && d < R * 3.0 {
                                    occ += (d / R).min(1.0) * (1.0 - d / (R * 3.0));
                                }
                            }
                        }
                        if cnt > 0.0 {
                            k = (1.0 - l.ao_strength * (occ / cnt) * 2.2).clamp(0.35, 1.0);
                        }
                    }
                }
                let o = (yy * w + x) * 4;
                let mut c = [cb[o] as f32 * k, cb[o + 1] as f32 * k, cb[o + 2] as f32 * k];
                if l.fog && !lc.ortho && vz > l.fog_start {
                    let t = ((vz - l.fog_start) / (l.fog_end - l.fog_start).max(1.0)).clamp(0.0, 1.0);
                    let t = t * t * (3.0 - 2.0 * t) * 0.92;
                    for ch in 0..3 {
                        c[ch] += (fog_col[ch] - c[ch]) * t;
                    }
                }
                cb[o] = c[0] as u8;
                cb[o + 1] = c[1] as u8;
                cb[o + 2] = c[2] as u8;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Picking
// ---------------------------------------------------------------------------

pub fn ray_aabb(o: Vec3, d: Vec3, mn: Vec3, mx: Vec3, tmax: f32) -> bool {
    let (mn, mx) = (mn - Vec3::splat(1e-3), mx + Vec3::splat(1e-3));
    let mut t0 = 0.0f32;
    let mut t1 = tmax;
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if o[i] < mn[i] || o[i] > mx[i] {
                return false;
            }
            continue;
        }
        let inv = 1.0 / d[i];
        let (mut a, mut b) = ((mn[i] - o[i]) * inv, (mx[i] - o[i]) * inv);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return false;
        }
    }
    true
}

pub fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-4).then_some(t)
}

#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub id: u64,
    pub t: f32,
    pub tri: usize,
    pub normal: Vec3,
    pub point: Vec3,
}

/// Nearest object hit by a ray. `skip` filters objects out.
pub fn pick_ray_where(scene: &[Arc<WorldMesh>], o: Vec3, d: Vec3, skip: impl Fn(&WorldMesh) -> bool) -> Option<RayHit> {
    let mut best: Option<RayHit> = None;
    for wm in scene {
        if skip(wm.as_ref()) {
            continue;
        }
        let tmax = best.map(|b| b.t).unwrap_or(f32::MAX);
        if !ray_aabb(o, d, wm.min, wm.max, tmax) {
            continue;
        }
        for (ti, t) in wm.mesh.tris.iter().enumerate() {
            let (a, b, c) = (wm.pos[t[0] as usize], wm.pos[t[1] as usize], wm.pos[t[2] as usize]);
            if let Some(tt) = ray_tri(o, d, a, b, c) {
                if best.map_or(true, |bb| tt < bb.t) {
                    let mut n = (b - a).cross(c - a).normalize_or_zero();
                    if n.dot(d) > 0.0 {
                        n = -n;
                    }
                    best = Some(RayHit { id: wm.id, t: tt, tri: ti, normal: n, point: o + d * tt });
                }
            }
        }
    }
    best
}

pub fn pick_ray(scene: &[Arc<WorldMesh>], o: Vec3, d: Vec3) -> Option<RayHit> {
    pick_ray_where(scene, o, d, |_| false)
}

pub fn ray_plane(o: Vec3, d: Vec3, y: f32) -> Option<Vec3> {
    if d.y.abs() < 1e-6 {
        return None;
    }
    let t = (y - o.y) / d.y;
    (t > 0.0 && t < 1.0e5).then(|| o + d * t)
}
