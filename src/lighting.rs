//! Lighting for the 3D preview (software renderer): a sun with a shadow map,
//! sky light, point lights, a sky gradient, distance fog and screen-space
//! ambient occlusion. Quality presets keep slower PCs fast.

use crate::mesh::WorldMesh;
use glam::Vec3;
use rayon::prelude::*;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub pos: Vec3,
    pub range: f32,
    pub brightness: f32,
    pub color: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Lighting {
    /// Off = the old flat shading.
    pub on: bool,
    /// Direction toward the sun.
    pub sun: Vec3,
    pub sun_strength: f32,
    pub ambient: f32,
    pub shadows: bool,
    pub shadow_res: usize,
    /// Shadows are drawn this far around the camera (studs).
    pub shadow_radius: f32,
    /// Soft shadow edges (3x3 samples).
    pub soft: bool,
    pub ao: bool,
    pub ao_strength: f32,
    /// 1 = one ring of samples (High), 2 = two rings (Ultra).
    pub ao_rings: u8,
    pub fog: bool,
    pub fog_start: f32,
    pub fog_end: f32,
    pub sky: bool,
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub ground: [f32; 3],
    pub lights: Vec<PointLight>,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            on: false,
            sun: sun_dir(-55.0, 45.0),
            sun_strength: 0.62,
            ambient: 0.4,
            shadows: false,
            shadow_res: 1024,
            shadow_radius: 160.0,
            soft: false,
            ao: false,
            ao_strength: 0.55,
            ao_rings: 1,
            fog: false,
            fog_start: 250.0,
            fog_end: 900.0,
            sky: false,
            sky_top: [62.0, 104.0, 168.0],
            sky_horizon: [168.0, 190.0, 214.0],
            ground: [92.0, 98.0, 108.0],
            lights: Vec::new(),
        }
    }
}

pub const QUALITY_NAMES: [&str; 5] = ["Off (flat, fastest)", "Low", "Medium", "High", "Ultra"];

/// Direction toward the sun from compass angle and height above the horizon (degrees).
pub fn sun_dir(azimuth: f32, elevation: f32) -> Vec3 {
    let (a, e) = (azimuth.to_radians(), elevation.clamp(1.0, 90.0).to_radians());
    Vec3::new(a.sin() * e.cos(), e.sin(), a.cos() * e.cos()).normalize()
}

/// Lighting for a quality level (0 = off .. 4 = ultra).
pub fn preset(quality: u8, sun: Vec3, fog: bool, fog_end: f32, sky: bool) -> Lighting {
    let mut l = Lighting { sun, ..Default::default() };
    if quality == 0 {
        return l;
    }
    l.on = true;
    l.sky = sky;
    l.fog = fog;
    l.fog_end = fog_end.max(50.0);
    l.fog_start = l.fog_end * 0.3;
    match quality {
        1 => {}
        2 => {
            l.shadows = true;
            l.shadow_res = 1024;
            l.shadow_radius = 160.0;
        }
        3 => {
            l.shadows = true;
            l.shadow_res = 2048;
            l.shadow_radius = 220.0;
            l.soft = true;
            l.ao = true;
        }
        _ => {
            l.shadows = true;
            l.shadow_res = 4096;
            l.shadow_radius = 320.0;
            l.soft = true;
            l.ao = true;
            l.ao_rings = 2;
        }
    }
    l
}

/// Sky color for a view direction with this much "up" (-1..1).
pub fn sky_color(l: &Lighting, up: f32) -> [f32; 3] {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    if up >= 0.0 {
        mix(l.sky_horizon, l.sky_top, up.min(1.0).powf(0.55))
    } else {
        mix(l.sky_horizon, l.ground, (-up * 4.0).min(1.0))
    }
}

/// Light arriving at a point (per color channel, ~1 = normal brightness).
#[inline]
pub fn light_at(l: &Lighting, shadow: Option<&ShadowMap>, p: Vec3, n: Vec3, metallic: bool, lmask: u64) -> [f32; 3] {
    let ndl = n.dot(l.sun);
    let lit = if ndl > 0.0 {
        match shadow {
            Some(s) => s.light(p, n, l.soft),
            None => 1.0,
        }
    } else {
        0.0
    };
    let ndl = ndl.max(0.0);
    let back = Vec3::new(-l.sun.x, 0.25, -l.sun.z).normalize_or_zero();
    let mut k = l.ambient * (0.78 + 0.22 * n.y) + 0.1 * n.dot(back).max(0.0) + l.sun_strength * ndl * lit;
    if metallic {
        k = k * 0.75 + 0.8 * ndl.powi(12) * lit;
    }
    let mut rgb = [k; 3];
    let mut m = lmask;
    while m != 0 {
        let i = m.trailing_zeros() as usize;
        m &= m - 1;
        let Some(pl) = l.lights.get(i) else { continue };
        let d = pl.pos - p;
        let dist = d.length();
        if dist >= pl.range || dist < 1e-4 {
            continue;
        }
        let f = 1.0 - dist / pl.range;
        let a = f * f * pl.brightness * (n.dot(d / dist) * 0.85 + 0.15).max(0.0);
        for c in 0..3 {
            rgb[c] += a * pl.color[c];
        }
    }
    rgb
}

/// Depth from the sun over a square area around the camera.
pub struct ShadowMap {
    res: usize,
    center: Vec3,
    ax: Vec3,
    ay: Vec3,
    az: Vec3,
    radius: f32,
    texel: f32,
    depth: Vec<f32>,
}

impl ShadowMap {
    pub fn build(scene: &[Arc<WorldMesh>], sun: Vec3, center: Vec3, radius: f32, res: usize) -> ShadowMap {
        let az = -sun.normalize_or_zero();
        let ax = if az.y.abs() > 0.99 { Vec3::X } else { Vec3::Y.cross(az).normalize() };
        let ay = az.cross(ax);
        let res = res.clamp(64, 8192);
        let texel = radius * 2.0 / res as f32;
        let to_tex = |p: Vec3| {
            let d = p - center;
            Vec3::new((d.dot(ax) + radius) / texel, (d.dot(ay) + radius) / texel, d.dot(az))
        };
        // casters in shadow-map space
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        for wm in scene.iter().filter(|w| w.cast_shadow && w.alpha >= 0.5) {
            // cheap reject: the object's bounding sphere must reach the area
            let c = (wm.min + wm.max) * 0.5;
            let r = (wm.max - wm.min).length() * 0.5;
            let d = c - center;
            if d.dot(ax).abs() > radius + r || d.dot(ay).abs() > radius + r {
                continue;
            }
            for t in &wm.mesh.tris {
                tris.push([to_tex(wm.pos[t[0] as usize]), to_tex(wm.pos[t[1] as usize]), to_tex(wm.pos[t[2] as usize])]);
            }
        }
        let mut depth = vec![f32::INFINITY; res * res];
        let rows = (res / (rayon::current_num_threads() * 4)).clamp(8, 256);
        depth.par_chunks_mut(res * rows).enumerate().for_each(|(bi, band)| {
            let y0 = bi * rows;
            let y1 = y0 + band.len() / res;
            for t in &tris {
                let (a, b, c) = (t[0], t[1], t[2]);
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if area.abs() < 1e-9 {
                    continue;
                }
                let miny = a.y.min(b.y).min(c.y).floor().max(y0 as f32);
                let maxy = a.y.max(b.y).max(c.y).ceil().min(y1 as f32 - 1.0);
                if miny > maxy {
                    continue;
                }
                let minx = a.x.min(b.x).min(c.x).floor().max(0.0);
                let maxx = a.x.max(b.x).max(c.x).ceil().min(res as f32 - 1.0);
                if minx > maxx {
                    continue;
                }
                let inv = 1.0 / area;
                for y in miny as usize..=maxy as usize {
                    let py = y as f32 + 0.5;
                    let row = (y - y0) * res;
                    for x in minx as usize..=maxx as usize {
                        let px = x as f32 + 0.5;
                        let w0 = ((c.x - b.x) * (py - b.y) - (c.y - b.y) * (px - b.x)) * inv;
                        let w1 = ((a.x - c.x) * (py - c.y) - (a.y - c.y) * (px - c.x)) * inv;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let z = w0 * a.z + w1 * b.z + w2 * c.z;
                        let i = row + x;
                        if z < band[i] {
                            band[i] = z;
                        }
                    }
                }
            }
        });
        ShadowMap { res, center, ax, ay, az, radius, texel, depth }
    }

    /// 1 = in sunlight, 0 = in shadow (in between at soft edges).
    #[inline]
    pub fn light(&self, p: Vec3, n: Vec3, soft: bool) -> f32 {
        let q = p + n * self.texel * 1.2 - self.center;
        let (u, v, d) = ((q.dot(self.ax) + self.radius) / self.texel, (q.dot(self.ay) + self.radius) / self.texel, q.dot(self.az));
        let bias = self.texel * 1.5;
        let test = |x: i64, y: i64| -> f32 {
            if x < 0 || y < 0 || x >= self.res as i64 || y >= self.res as i64 {
                return 1.0;
            }
            if self.depth[y as usize * self.res + x as usize] < d - bias {
                0.0
            } else {
                1.0
            }
        };
        let (x, y) = (u.floor() as i64, v.floor() as i64);
        if !soft {
            return test(x, y);
        }
        let mut s = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                s += test(x + dx, y + dy);
            }
        }
        s / 9.0
    }
}

/// Cache key for a shadow map: changes when anything that casts moves.
pub fn shadow_key(scene: &[Arc<WorldMesh>], sun: Vec3, center: Vec3, radius: f32, res: usize) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x100_0000_01b3);
    for w in scene {
        mix(Arc::as_ptr(w) as usize as u64);
        mix(w.min.x.to_bits() as u64 ^ ((w.max.z.to_bits() as u64) << 32));
    }
    for f in [sun.x, sun.y, sun.z, center.x, center.y, center.z, radius] {
        mix(f.to_bits() as u64);
    }
    mix(res as u64);
    h
}
