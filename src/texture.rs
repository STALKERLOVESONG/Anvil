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
        let fu = u - u.floor();
        let fv = v - v.floor();
        if !bilinear || (w == 1 && h == 1) {
            let x = ((fu * w as f32) as usize).min(w - 1);
            let y = ((fv * h as f32) as usize).min(h - 1);
            let i = (y * w + x) * 4;
            return [d[i], d[i + 1], d[i + 2], d[i + 3]];
        }
        let x = fu * w as f32 - 0.5;
        let y = fv * h as f32 - 0.5;
        let (xf, yf) = (x.floor(), y.floor());
        let (tx, ty) = (x - xf, y - yf);
        let wrap = |a: i64, n: usize| a.rem_euclid(n as i64) as usize;
        let (x0, x1) = (wrap(xf as i64, w), wrap(xf as i64 + 1, w));
        let (y0, y1) = (wrap(yf as i64, h), wrap(yf as i64 + 1, h));
        let p = |x: usize, y: usize| (y * w + x) * 4;
        let (a, b, c, e) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        let mut out = [0u8; 4];
        for ch in 0..4 {
            let top = d[a + ch] as f32 * (1.0 - tx) + d[b + ch] as f32 * tx;
            let bot = d[c + ch] as f32 * (1.0 - tx) + d[e + ch] as f32 * tx;
            out[ch] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
        }
        out
    }
}

/// Images keyed by path, downscaled to the preview quality limit.
pub struct TexCache {
    items: HashMap<String, Option<Arc<TexImage>>>,
    max_dim: u32,
    /// Team Create: map path -> local copy (None = not downloaded yet, don't open).
    alias: HashMap<String, Option<String>>,
}

impl Default for TexCache {
    fn default() -> Self {
        TexCache { items: HashMap::new(), max_dim: 1024, alias: HashMap::new() }
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
    }

    /// Makes `path` load from `real` instead (None: never open `path`).
    pub fn alias(&mut self, path: &str, real: Option<String>) {
        self.items.remove(path);
        self.alias.insert(path.to_string(), real);
    }

    /// Changes the preview resolution limit (reloads everything when it changes).
    pub fn set_max_dim(&mut self, max_dim: u32) {
        let max_dim = max_dim.clamp(64, 4096);
        if max_dim != self.max_dim {
            self.max_dim = max_dim;
            self.items.clear();
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
