//! Image loading for the textured 3D preview: mipmapped, with bilinear
//! sampling so textures stay smooth up close and don't shimmer far away.

use std::collections::HashMap;
use std::sync::Arc;

pub struct TexImage {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    /// Smaller levels: mips[0] is half size, and so on down to 1x1.
    mips: Vec<(usize, usize, Vec<u8>)>,
}

fn downsample(w: usize, h: usize, src: &[u8]) -> (usize, usize, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let (x0, y0) = ((x * 2).min(w - 1), (y * 2).min(h - 1));
            let (x1, y1) = ((x * 2 + 1).min(w - 1), (y * 2 + 1).min(h - 1));
            for c in 0..4 {
                let s = src[(y0 * w + x0) * 4 + c] as u32
                    + src[(y0 * w + x1) * 4 + c] as u32
                    + src[(y1 * w + x0) * 4 + c] as u32
                    + src[(y1 * w + x1) * 4 + c] as u32;
                out[(y * nw + x) * 4 + c] = ((s + 2) / 4) as u8;
            }
        }
    }
    (nw, nh, out)
}

impl TexImage {
    pub fn new(w: usize, h: usize, rgba: Vec<u8>) -> Self {
        let mut mips: Vec<(usize, usize, Vec<u8>)> = Vec::new();
        let (mut cw, mut ch) = (w, h);
        while cw > 1 || ch > 1 {
            let next = {
                let src: &[u8] = mips.last().map(|m| m.2.as_slice()).unwrap_or(&rgba);
                downsample(cw, ch, src)
            };
            cw = next.0;
            ch = next.1;
            mips.push(next);
        }
        TexImage { w, h, rgba, mips }
    }

    /// Samples mip level `lod` (0 = full size), bilinear or nearest.
    #[inline]
    pub fn sample_lod(&self, u: f32, v: f32, lod: f32, bilinear: bool) -> [u8; 4] {
        let level = if lod > 0.0 { (lod as usize).min(self.mips.len()) } else { 0 };
        let (w, h, d): (usize, usize, &[u8]) = if level == 0 {
            (self.w, self.h, &self.rgba)
        } else {
            let m = &self.mips[level - 1];
            (m.0, m.1, &m.2)
        };
        // one bounds check per texel, read as a whole pixel
        let texel = |i: usize| -> [u8; 4] {
            match d.get(i..i + 4) {
                Some(s) => [s[0], s[1], s[2], s[3]],
                None => [0; 4],
            }
        };
        let fu = u - ffloor(u);
        let fv = v - ffloor(v);
        if !bilinear || (w == 1 && h == 1) {
            let x = ((fu * w as f32) as usize).min(w - 1);
            let y = ((fv * h as f32) as usize).min(h - 1);
            return texel((y * w + x) * 4);
        }
        let x = fu * w as f32 - 0.5;
        let y = fv * h as f32 - 0.5;
        let (xf, yf) = (ffloor(x), ffloor(y));
        // 8-bit fixed point weights: integer math instead of 16 float blends
        let (tx, ty) = (((x - xf) * 256.0) as u32, ((y - yf) * 256.0) as u32);
        // xf is in -1..=w-1 here, so wrapping needs no division
        let (xi, yi) = (xf as i64, yf as i64);
        let x0 = if xi < 0 { w - 1 } else { (xi as usize).min(w - 1) };
        let x1 = if xi + 1 >= w as i64 { 0 } else { (xi + 1) as usize };
        let y0 = if yi < 0 { h - 1 } else { (yi as usize).min(h - 1) };
        let y1 = if yi + 1 >= h as i64 { 0 } else { (yi + 1) as usize };
        let p = |x: usize, y: usize| (y * w + x) * 4;
        let (a, b, c, e) = (texel(p(x0, y0)), texel(p(x1, y0)), texel(p(x0, y1)), texel(p(x1, y1)));
        let (ix, iy) = (256 - tx, 256 - ty);
        let mut out = [0u8; 4];
        for ch in 0..4 {
            let top = a[ch] as u32 * ix + b[ch] as u32 * tx;
            let bot = c[ch] as u32 * ix + e[ch] as u32 * tx;
            out[ch] = ((top * iy + bot * ty + 32768) >> 16) as u8;
        }
        out
    }
}

/// floor() without a library call (plain builds have no SSE4.1 rounding
/// instruction, and this runs several times per pixel). For |x| < 2^31.
#[inline(always)]
pub fn ffloor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

/// Mip level for a squared texel footprint: floor(0.5 * log2(rho2)), read
/// straight from the float's exponent.
#[inline(always)]
pub fn mip_level(rho2: f32) -> usize {
    if rho2 <= 1.0 || !rho2.is_finite() {
        return 0;
    }
    let e = ((rho2.to_bits() >> 23) & 0xff) as i32 - 127;
    (e.max(0) >> 1) as usize
}

/// Images keyed by path, downscaled to the preview quality limit.
pub struct TexCache {
    items: HashMap<String, Option<Arc<TexImage>>>,
    max_dim: u32,
    /// Team Create: map path -> local copy (None = not downloaded yet, don't open).
    alias: HashMap<String, Option<String>>,
    /// Bumped whenever a cached image may have changed (so a finished 3D
    /// frame isn't reused with old textures).
    pub version: u64,
}

impl Default for TexCache {
    fn default() -> Self {
        TexCache { items: HashMap::new(), max_dim: 1024, alias: HashMap::new(), version: 0 }
    }
}

impl TexCache {
    pub fn get(&mut self, path: &str) -> Option<Arc<TexImage>> {
        if path.trim().is_empty() {
            return None;
        }
        if let Some(v) = self.items.get(path) {
            return v.clone();
        }
        let v = match self.alias.get(path) {
            Some(None) => return None,
            Some(Some(real)) => load(real, self.max_dim),
            None => load(path, self.max_dim),
        };
        self.items.insert(path.to_string(), v.clone());
        v
    }

    pub fn forget(&mut self, path: &str) {
        self.items.remove(path);
        self.version += 1;
    }

    /// Makes `path` load from `real` instead (None: never open `path`).
    pub fn alias(&mut self, path: &str, real: Option<String>) {
        self.items.remove(path);
        self.alias.insert(path.to_string(), real);
        self.version += 1;
    }

    pub fn max_dim(&self) -> u32 {
        self.max_dim
    }

    /// Stores a picture decoded elsewhere (on a background thread) at the
    /// size limit it was made for; ignored if the limit changed since.
    pub fn put(&mut self, path: &str, img: Option<Arc<TexImage>>, max_dim: u32) {
        if max_dim == self.max_dim && !self.alias.contains_key(path) {
            self.items.insert(path.to_string(), img);
            self.version += 1;
        }
    }

    /// Changes the preview resolution limit (reloads everything when it changes).
    pub fn set_max_dim(&mut self, max_dim: u32) {
        let max_dim = max_dim.clamp(64, 4096);
        if max_dim != self.max_dim {
            self.max_dim = max_dim;
            self.items.clear();
            self.version += 1;
        }
    }
}

/// True for Windows network / device paths (`\\server\share`, `//server`,
/// `\\?\`). Opening one makes Windows connect to that machine and offer your
/// login hash, so paths that come from map files are never opened that way.
pub fn is_network_path(p: &str) -> bool {
    let t = p.trim_start().as_bytes();
    t.len() >= 2 && (t[0] == b'\\' || t[0] == b'/') && (t[1] == b'\\' || t[1] == b'/')
}

/// Decoder limits so a hostile image can't eat all memory.
pub fn image_limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(16384);
    l.max_image_height = Some(16384);
    l.max_alloc = Some(768 * 1024 * 1024);
    l
}

/// Opens an image file safely (no network paths, size limits).
pub fn open_image(path: &str) -> Option<image::DynamicImage> {
    if is_network_path(path) {
        return None;
    }
    let mut r = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    r.limits(image_limits());
    r.decode().ok()
}

pub fn load(path: &str, max_dim: u32) -> Option<Arc<TexImage>> {
    let img = open_image(path)?;
    let img = if img.width() > max_dim || img.height() > max_dim { img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    Some(Arc::new(TexImage::new(w, h, rgba.into_raw())))
}

pub fn content_type(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "tga" => "image/tga",
        _ => "image/png",
    }
}
