//! Source engine (Half-Life 2 / Garry's Mod) file formats:
//! VPK archives, GMA addons, VTF textures, VMT materials and MDL/VVD/VTX models.

use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// little-endian readers
// ---------------------------------------------------------------------------

fn u8_(b: &[u8], o: usize) -> Option<u8> {
    b.get(o).copied()
}
fn u16_(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn i16_(b: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn i32_(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32_(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn cstr(b: &[u8], o: usize) -> Option<(String, usize)> {
    let rest = b.get(o..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    Some((String::from_utf8_lossy(&rest[..end]).to_string(), o + end + 1))
}
fn at(o: usize, rel: i32) -> Option<usize> {
    usize::try_from(o as i64 + rel as i64).ok()
}

/// Game-relative path: lowercase, forward slashes, no leading slash.
pub fn norm(p: &str) -> String {
    let s = p.replace('\\', "/").to_ascii_lowercase();
    let s = s.trim_start_matches('/');
    let mut out = String::with_capacity(s.len());
    for part in s.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

// ---------------------------------------------------------------------------
// VPK
// ---------------------------------------------------------------------------

struct VpkEntry {
    archive: u16,
    offset: u32,
    len: u32,
    preload: Vec<u8>,
}

pub struct Vpk {
    pub path: PathBuf,
    base: String,
    data_start: u64,
    files: HashMap<String, VpkEntry>,
}

impl Vpk {
    pub fn open(path: &Path) -> Result<Vpk, String> {
        let mut f = File::open(path).map_err(|e| e.to_string())?;
        let mut head = [0u8; 28];
        f.read_exact(&mut head[..12]).map_err(|e| e.to_string())?;
        if u32_(&head, 0) != Some(0x55aa_1234) {
            return Err("not a VPK directory file".into());
        }
        let version = u32_(&head, 4).unwrap_or(0);
        let tree_size = u32_(&head, 8).unwrap_or(0) as usize;
        let header_len = match version {
            1 => 12,
            2 => {
                f.read_exact(&mut head[12..28]).map_err(|e| e.to_string())?;
                28
            }
            v => return Err(format!("unsupported VPK version {v}")),
        };
        let mut tree = vec![0u8; tree_size];
        f.read_exact(&mut tree).map_err(|e| e.to_string())?;
        let mut files = HashMap::new();
        let mut o = 0;
        let bad = || "corrupt VPK tree".to_string();
        loop {
            let (ext, n) = cstr(&tree, o).ok_or_else(bad)?;
            o = n;
            if ext.is_empty() {
                break;
            }
            loop {
                let (dir, n) = cstr(&tree, o).ok_or_else(bad)?;
                o = n;
                if dir.is_empty() {
                    break;
                }
                loop {
                    let (name, n) = cstr(&tree, o).ok_or_else(bad)?;
                    o = n;
                    if name.is_empty() {
                        break;
                    }
                    let preload_len = u16_(&tree, o + 4).ok_or_else(bad)? as usize;
                    let archive = u16_(&tree, o + 6).ok_or_else(bad)?;
                    let offset = u32_(&tree, o + 8).ok_or_else(bad)?;
                    let len = u32_(&tree, o + 12).ok_or_else(bad)?;
                    o += 18;
                    let preload = tree.get(o..o + preload_len).ok_or_else(bad)?.to_vec();
                    o += preload_len;
                    let full = if dir == " " { format!("{name}.{ext}") } else { format!("{dir}/{name}.{ext}") };
                    files.insert(norm(&full), VpkEntry { archive, offset, len, preload });
                }
            }
        }
        let s = path.to_string_lossy();
        let base = s.strip_suffix("_dir.vpk").unwrap_or(&s).to_string();
        Ok(Vpk { path: path.to_path_buf(), base, data_start: (header_len + tree_size) as u64, files })
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let e = self.files.get(rel)?;
        let mut out = e.preload.clone();
        if e.len > 0 {
            let (file, off) = if e.archive == 0x7fff {
                (self.path.clone(), self.data_start + e.offset as u64)
            } else {
                (PathBuf::from(format!("{}_{:03}.vpk", self.base, e.archive)), e.offset as u64)
            };
            let mut f = File::open(file).ok()?;
            f.seek(SeekFrom::Start(off)).ok()?;
            let mut buf = vec![0u8; e.len as usize];
            f.read_exact(&mut buf).ok()?;
            out.extend(buf);
        }
        Some(out)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.files.keys()
    }
}

// ---------------------------------------------------------------------------
// GMA (Garry's Mod addon)
// ---------------------------------------------------------------------------

pub struct Gma {
    pub path: PathBuf,
    pub title: String,
    files: HashMap<String, (u64, u64)>,
}

impl Gma {
    pub fn open(path: &Path) -> Result<Gma, String> {
        let mut f = File::open(path).map_err(|e| e.to_string())?;
        let total = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut want = total.min(4 << 20) as usize;
        loop {
            f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            let mut b = vec![0u8; want];
            f.read_exact(&mut b).map_err(|e| e.to_string())?;
            match Self::parse(&b) {
                Some((title, entries, data_start)) => {
                    let mut files = HashMap::new();
                    let mut off = data_start as u64;
                    for (name, size) in entries {
                        files.insert(norm(&name), (off, size));
                        off += size;
                    }
                    return Ok(Gma { path: path.to_path_buf(), title, files });
                }
                None if (want as u64) < total => want = (want * 4).min(total as usize),
                None => return Err("not a valid GMA addon".into()),
            }
        }
    }

    fn parse(b: &[u8]) -> Option<(String, Vec<(String, u64)>, usize)> {
        if b.get(0..4)? != b"GMAD" {
            return None;
        }
        let version = u8_(b, 4)?;
        let mut o = 5 + 8 + 8;
        if version > 1 {
            loop {
                let (s, n) = cstr(b, o)?;
                o = n;
                if s.is_empty() {
                    break;
                }
            }
        }
        let (title, n) = cstr(b, o)?;
        let (_desc, n) = cstr(b, n)?;
        let (_author, n) = cstr(b, n)?;
        o = n + 4;
        let mut entries = Vec::new();
        loop {
            let num = u32_(b, o)?;
            o += 4;
            if num == 0 {
                break;
            }
            let (name, n) = cstr(b, o)?;
            let size = u64::from_le_bytes(b.get(n..n + 8)?.try_into().ok()?);
            o = n + 8 + 4;
            entries.push((name, size));
        }
        Some((title, entries, o))
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let (off, size) = *self.files.get(rel)?;
        let mut f = File::open(&self.path).ok()?;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut buf = vec![0u8; size as usize];
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.files.keys()
    }
}

// ---------------------------------------------------------------------------
// VTF textures
// ---------------------------------------------------------------------------

pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

fn fmt_size(fmt: i32, w: u32, h: u32) -> Option<usize> {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let blocks = w.div_ceil(4) * h.div_ceil(4);
    Some(match fmt {
        13 | 20 => blocks * 8,
        14 | 15 => blocks * 16,
        0 | 1 | 11 | 12 | 16 | 23 | 26 => w * h * 4,
        2 | 3 | 9 | 10 => w * h * 3,
        4 | 6 | 17 | 18 | 19 | 21 | 22 => w * h * 2,
        5 | 7 | 8 => w * h,
        24 | 25 => w * h * 8,
        _ => return None,
    })
}

fn c565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

fn dxt_color_block(b: &[u8], four_color: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (c565(c0), c565(c1));
    let mix = |a: [u8; 3], b: [u8; 3], wa: u32, wb: u32| {
        let t = wa + wb;
        [((a[0] as u32 * wa + b[0] as u32 * wb) / t) as u8, ((a[1] as u32 * wa + b[1] as u32 * wb) / t) as u8, ((a[2] as u32 * wa + b[2] as u32 * wb) / t) as u8]
    };
    let pal: [[u8; 4]; 4] = if four_color || c0 > c1 {
        let (p2, p3) = (mix(p0, p1, 2, 1), mix(p0, p1, 1, 2));
        [[p0[0], p0[1], p0[2], 255], [p1[0], p1[1], p1[2], 255], [p2[0], p2[1], p2[2], 255], [p3[0], p3[1], p3[2], 255]]
    } else {
        let p2 = mix(p0, p1, 1, 1);
        [[p0[0], p0[1], p0[2], 255], [p1[0], p1[1], p1[2], 255], [p2[0], p2[1], p2[2], 255], [0, 0, 0, 0]]
    };
    let idx = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((idx >> (i * 2)) & 3) as usize];
    }
    out
}

fn decode_blocks(src: &[u8], w: u32, h: u32, fmt: i32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let bs = if fmt == 13 || fmt == 20 { 8 } else { 16 };
    let bw = w.div_ceil(4);
    for by in 0..h.div_ceil(4) {
        for bx in 0..bw {
            let o = (by * bw + bx) * bs;
            let Some(blk) = src.get(o..o + bs) else { continue };
            let mut px = match fmt {
                13 | 20 => dxt_color_block(blk, false),
                _ => dxt_color_block(&blk[8..], true),
            };
            if fmt == 14 {
                for (i, p) in px.iter_mut().enumerate() {
                    let a = (blk[i / 2] >> ((i % 2) * 4)) & 15;
                    p[3] = a * 17;
                }
            } else if fmt == 15 {
                let (a0, a1) = (blk[0] as u32, blk[1] as u32);
                let mut pal = [0u8; 8];
                pal[0] = a0 as u8;
                pal[1] = a1 as u8;
                if a0 > a1 {
                    for i in 1..7 {
                        pal[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
                    }
                } else {
                    for i in 1..5 {
                        pal[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
                    }
                    pal[6] = 0;
                    pal[7] = 255;
                }
                let bits = u64::from_le_bytes([blk[2], blk[3], blk[4], blk[5], blk[6], blk[7], 0, 0]);
                for (i, p) in px.iter_mut().enumerate() {
                    p[3] = pal[((bits >> (i * 3)) & 7) as usize];
                }
            }
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let d = (y * w + x) * 4;
                    out[d..d + 4].copy_from_slice(p);
                }
            }
        }
    }
    out
}

fn decode_plain(src: &[u8], w: u32, h: u32, fmt: i32) -> Option<Vec<u8>> {
    let n = (w * h) as usize;
    let mut out = vec![0u8; n * 4];
    let half = |b: &[u8]| -> f32 {
        let h = u16::from_le_bytes([b[0], b[1]]);
        let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let e = ((h >> 10) & 31) as i32;
        let m = (h & 1023) as f32;
        if e == 0 {
            s * m * 2f32.powi(-24)
        } else {
            s * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
        }
    };
    for i in 0..n {
        let px: [u8; 4] = match fmt {
            0 => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]],
            1 => [src[i * 4 + 3], src[i * 4 + 2], src[i * 4 + 1], src[i * 4]],
            11 => [src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3], src[i * 4]],
            12 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], src[i * 4 + 3]],
            16 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], 255],
            23 | 26 => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], 255],
            2 | 9 => [src[i * 3], src[i * 3 + 1], src[i * 3 + 2], 255],
            3 | 10 => [src[i * 3 + 2], src[i * 3 + 1], src[i * 3], 255],
            4 => {
                let c = c565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                [c[0], c[1], c[2], 255]
            }
            17 => {
                let c = c565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                [c[2], c[1], c[0], 255]
            }
            18 | 21 => {
                let c = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                let f = |v: u16| ((v & 31) as u32 * 255 / 31) as u8;
                [f(c >> 10), f(c >> 5), f(c), if fmt == 21 && c & 0x8000 == 0 { 0 } else { 255 }]
            }
            19 => {
                let c = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                let f = |v: u16| ((v & 15) * 17) as u8;
                [f(c >> 8), f(c >> 4), f(c), f(c >> 12)]
            }
            5 => [src[i], src[i], src[i], 255],
            6 => [src[i * 2], src[i * 2], src[i * 2], src[i * 2 + 1]],
            7 => [src[i], src[i], src[i], 255],
            8 => [255, 255, 255, src[i]],
            22 => [src[i * 2], src[i * 2 + 1], 255, 255],
            24 => {
                let b = &src[i * 8..i * 8 + 8];
                let t = |v: f32| ((v / (1.0 + v)).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
                [t(half(&b[0..2])), t(half(&b[2..4])), t(half(&b[4..6])), (half(&b[6..8]).clamp(0.0, 1.0) * 255.0) as u8]
            }
            25 => {
                let b = &src[i * 8..i * 8 + 8];
                [b[1], b[3], b[5], b[7]]
            }
            _ => return None,
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    Some(out)
}

/// Decodes the first frame of a VTF, picking the largest mip no bigger than `max_dim`.
pub fn decode_vtf(b: &[u8], max_dim: u32) -> Result<Rgba, String> {
    if b.get(0..4) != Some(b"VTF\0") {
        return Err("not a VTF texture".into());
    }
    let bad = || "truncated VTF".to_string();
    let minor = u32_(b, 8).ok_or_else(bad)?;
    let header_size = u32_(b, 12).ok_or_else(bad)? as usize;
    let w = u16_(b, 16).ok_or_else(bad)? as u32;
    let h = u16_(b, 18).ok_or_else(bad)? as u32;
    let flags = u32_(b, 20).ok_or_else(bad)?;
    let frames = u16_(b, 24).ok_or_else(bad)?.max(1) as usize;
    let first_frame = u16_(b, 26).ok_or_else(bad)?;
    let fmt = i32_(b, 52).ok_or_else(bad)?;
    let mips = u8_(b, 56).ok_or_else(bad)?.max(1) as u32;
    let lo_fmt = i32_(b, 57).ok_or_else(bad)?;
    let (lo_w, lo_h) = (u8_(b, 61).ok_or_else(bad)? as u32, u8_(b, 62).ok_or_else(bad)? as u32);
    let depth = if minor >= 2 { u16_(b, 63).unwrap_or(1).max(1) as usize } else { 1 };
    let faces = if flags & 0x4000 != 0 {
        if minor < 5 && first_frame != 0xffff {
            7
        } else {
            6
        }
    } else {
        1
    };
    let mut data_off = if minor >= 3 {
        let n = u32_(b, 68).ok_or_else(bad)? as usize;
        (0..n)
            .find_map(|i| {
                let o = 80 + i * 8;
                (b.get(o..o + 3)? == [0x30, 0, 0]).then(|| u32_(b, o + 4).map(|v| v as usize)).flatten()
            })
            .ok_or("VTF has no image data")?
    } else {
        header_size + if lo_fmt < 0 { 0 } else { fmt_size(lo_fmt, lo_w, lo_h).unwrap_or(0) }
    };
    fmt_size(fmt, 1, 1).ok_or_else(|| format!("unsupported VTF format {fmt}"))?;
    let mut mip = 0;
    while mip + 1 < mips && (w >> mip).max(h >> mip) > max_dim {
        mip += 1;
    }
    for m in (mip + 1..mips).rev() {
        data_off += fmt_size(fmt, w >> m, h >> m).unwrap_or(0) * frames * faces * depth;
    }
    let (mw, mh) = ((w >> mip).max(1), (h >> mip).max(1));
    let size = fmt_size(fmt, mw, mh).unwrap_or(0);
    let src = b.get(data_off..data_off + size).ok_or_else(bad)?;
    let data = match fmt {
        13 | 14 | 15 | 20 => decode_blocks(src, mw, mh, fmt),
        _ => decode_plain(src, mw, mh, fmt).ok_or_else(|| format!("unsupported VTF format {fmt}"))?,
    };
    Ok(Rgba { w: mw, h: mh, data })
}

// ---------------------------------------------------------------------------
// VMT materials
// ---------------------------------------------------------------------------

pub struct Vmt {
    pub shader: String,
    /// `$params` (lowercase keys), flattened from all nested blocks.
    pub params: HashMap<String, String>,
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let c: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '{' || ch == '}' {
            out.push(ch.to_string());
            i += 1;
        } else if ch == '"' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(s);
        } else {
            let mut s = String::new();
            while i < c.len() && !c[i].is_whitespace() && c[i] != '{' && c[i] != '}' && c[i] != '"' {
                s.push(c[i]);
                i += 1;
            }
            out.push(s);
        }
    }
    out
}

pub fn parse_vmt(text: &str) -> Vmt {
    let t = tokens(text);
    let shader = t.first().cloned().unwrap_or_default().to_ascii_lowercase();
    let mut params = HashMap::new();
    let mut i = 1;
    while i < t.len() {
        if t[i] == "{" || t[i] == "}" {
            i += 1;
            continue;
        }
        if t.get(i + 1).is_some_and(|n| n == "{") {
            i += 1; // nested block name
            continue;
        }
        if let Some(v) = t.get(i + 1) {
            params.entry(t[i].to_ascii_lowercase()).or_insert_with(|| v.clone());
            i += 2;
        } else {
            break;
        }
    }
    Vmt { shader, params }
}

// ---------------------------------------------------------------------------
// MDL / VVD / VTX models
// ---------------------------------------------------------------------------

pub struct SourceModel {
    pub pos: Vec<Vec3>,
    pub normal: Vec<Vec3>,
    pub uv: Vec<Vec2>,
    pub tris: Vec<[u32; 3]>,
    pub tri_mat: Vec<u16>,
    /// Material paths relative to `materials/`, without extension.
    pub materials: Vec<String>,
}

/// Source is Z-up (X forward, Y left); Anvil is Y-up.
pub fn src_to_anvil(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}

/// Loads a model. `read` fetches game-relative files ("models/x.mdl").
pub fn load_mdl(read: &dyn Fn(&str) -> Option<Vec<u8>>, rel: &str) -> Result<SourceModel, String> {
    let rel = norm(rel);
    let mdl = read(&rel).ok_or_else(|| format!("{rel} not found"))?;
    if mdl.get(0..4) != Some(b"IDST") {
        return Err("not a Source MDL".into());
    }
    let version = i32_(&mdl, 4).unwrap_or(0);
    if !(44..=49).contains(&version) {
        return Err(format!("unsupported MDL version {version} (Half-Life 2 / GMod models are 44-49)"));
    }
    let base = rel.trim_end_matches(".mdl").to_string();
    let vvd = read(&format!("{base}.vvd")).ok_or("missing .vvd vertex file")?;
    let vtx = ["dx90.vtx", "vtx", "dx80.vtx", "sw.vtx"]
        .iter()
        .find_map(|e| read(&format!("{base}.{e}")))
        .ok_or("missing .vtx mesh file")?;
    let bad = |w: &str| format!("corrupt model ({w})");

    // materials: texture names + search dirs, mapped through skin family 0
    let numtex = i32_(&mdl, 204).ok_or_else(|| bad("header"))?.max(0) as usize;
    let texidx = i32_(&mdl, 208).ok_or_else(|| bad("header"))? as usize;
    let numcd = i32_(&mdl, 212).ok_or_else(|| bad("header"))?.max(0) as usize;
    let cdidx = i32_(&mdl, 216).ok_or_else(|| bad("header"))? as usize;
    let numskinref = i32_(&mdl, 220).ok_or_else(|| bad("header"))?.max(0) as usize;
    let skinidx = i32_(&mdl, 228).ok_or_else(|| bad("header"))? as usize;
    let mut cds = Vec::new();
    for i in 0..numcd {
        let o = i32_(&mdl, cdidx + i * 4).ok_or_else(|| bad("cdtextures"))? as usize;
        cds.push(norm(&cstr(&mdl, o).map(|s| s.0).unwrap_or_default()));
    }
    if cds.is_empty() {
        cds.push(String::new());
    }
    let mut materials = Vec::new();
    for i in 0..numtex {
        let to = texidx + i * 64;
        let name_off = at(to, i32_(&mdl, to).ok_or_else(|| bad("textures"))?).ok_or_else(|| bad("textures"))?;
        let name = norm(&cstr(&mdl, name_off).map(|s| s.0).unwrap_or_default());
        let candidates: Vec<String> = cds.iter().map(|cd| norm(&format!("{cd}/{name}"))).collect();
        let found = candidates.iter().find(|c| read(&format!("materials/{c}.vmt")).is_some() || read(&format!("materials/{c}.vtf")).is_some());
        materials.push(found.cloned().unwrap_or_else(|| candidates[0].clone()));
    }
    let skin0 = |r: usize| -> usize {
        if r < numskinref {
            i16_(&mdl, skinidx + r * 2).map(|v| v.max(0) as usize).unwrap_or(r)
        } else {
            r
        }
    };

    // VVD vertices for LOD 0 (with fixups)
    if vvd.get(0..4) != Some(b"IDSV") {
        return Err("bad .vvd file".into());
    }
    let lod0_count = i32_(&vvd, 16).ok_or_else(|| bad("vvd"))?.max(0) as usize;
    let num_fix = i32_(&vvd, 48).ok_or_else(|| bad("vvd"))?.max(0) as usize;
    let fix_start = i32_(&vvd, 52).ok_or_else(|| bad("vvd"))? as usize;
    let vstart = i32_(&vvd, 56).ok_or_else(|| bad("vvd"))? as usize;
    let read_vert = |i: usize| -> Option<(Vec3, Vec3, Vec2)> {
        let o = vstart + i * 48;
        let p = Vec3::new(f32_(&vvd, o + 16)?, f32_(&vvd, o + 20)?, f32_(&vvd, o + 24)?);
        let n = Vec3::new(f32_(&vvd, o + 28)?, f32_(&vvd, o + 32)?, f32_(&vvd, o + 36)?);
        let uv = Vec2::new(f32_(&vvd, o + 40)?, f32_(&vvd, o + 44)?);
        Some((p, n, uv))
    };
    let mut verts: Vec<(Vec3, Vec3, Vec2)> = Vec::with_capacity(lod0_count);
    if num_fix == 0 {
        for i in 0..lod0_count {
            verts.push(read_vert(i).ok_or_else(|| bad("vertices"))?);
        }
    } else {
        for f in 0..num_fix {
            let o = fix_start + f * 12;
            let lod = i32_(&vvd, o).ok_or_else(|| bad("fixups"))?;
            let src = i32_(&vvd, o + 4).ok_or_else(|| bad("fixups"))?.max(0) as usize;
            let n = i32_(&vvd, o + 8).ok_or_else(|| bad("fixups"))?.max(0) as usize;
            if lod >= 0 {
                for i in src..src + n {
                    verts.push(read_vert(i).ok_or_else(|| bad("vertices"))?);
                }
            }
        }
    }

    // MDL body parts -> model 0 -> meshes (material, vertex offset)
    let numbp = i32_(&mdl, 232).ok_or_else(|| bad("header"))?.max(0) as usize;
    let bpidx = i32_(&mdl, 236).ok_or_else(|| bad("header"))? as usize;
    struct MeshInfo {
        material: usize,
        vertex_base: usize,
    }
    let mut mdl_meshes: Vec<Vec<MeshInfo>> = Vec::new(); // per body part (model 0)
    for bp in 0..numbp {
        let bo = bpidx + bp * 16;
        let nmodels = i32_(&mdl, bo + 4).ok_or_else(|| bad("bodyparts"))?;
        let mut list = Vec::new();
        if nmodels > 0 {
            let mo = at(bo, i32_(&mdl, bo + 12).ok_or_else(|| bad("bodyparts"))?).ok_or_else(|| bad("bodyparts"))?;
            let nmesh = i32_(&mdl, mo + 72).ok_or_else(|| bad("model"))?.max(0) as usize;
            let meshidx = i32_(&mdl, mo + 76).ok_or_else(|| bad("model"))?;
            let vbase = (i32_(&mdl, mo + 84).ok_or_else(|| bad("model"))?.max(0) / 48) as usize;
            for m in 0..nmesh {
                let me = at(mo, meshidx).ok_or_else(|| bad("mesh"))? + m * 116;
                let material = i32_(&mdl, me).ok_or_else(|| bad("mesh"))?.max(0) as usize;
                let voff = i32_(&mdl, me + 12).ok_or_else(|| bad("mesh"))?.max(0) as usize;
                list.push(MeshInfo { material: skin0(material), vertex_base: vbase + voff });
            }
        }
        mdl_meshes.push(list);
    }

    // VTX: try the classic 25-byte strip group header, then the newer 33-byte one
    let mut out: Option<SourceModel> = None;
    for sg_size in [25usize, 33] {
        if let Some(m) = read_vtx(&vtx, sg_size, &mdl_meshes.iter().map(|l| l.iter().map(|m| (m.material, m.vertex_base)).collect()).collect::<Vec<Vec<_>>>(), &verts, &materials) {
            out = Some(m);
            break;
        }
    }
    let mut m = out.ok_or_else(|| bad("vtx"))?;
    // Source winds front faces clockwise; Anvil uses counter-clockwise.
    for t in &mut m.tris {
        t.swap(1, 2);
    }
    Ok(m)
}

fn read_vtx(vtx: &[u8], sg_size: usize, meshes: &[Vec<(usize, usize)>], verts: &[(Vec3, Vec3, Vec2)], materials: &[String]) -> Option<SourceModel> {
    let nbp = i32_(vtx, 28)?.max(0) as usize;
    let bpoff = i32_(vtx, 32)? as usize;
    let mut out = SourceModel { pos: Vec::new(), normal: Vec::new(), uv: Vec::new(), tris: Vec::new(), tri_mat: Vec::new(), materials: materials.to_vec() };
    let mut remap: HashMap<usize, u32> = HashMap::new();
    for bp in 0..nbp.min(meshes.len()) {
        let bh = bpoff + bp * 8;
        if i32_(vtx, bh)? <= 0 {
            continue;
        }
        let mh = at(bh, i32_(vtx, bh + 4)?)?; // model 0
        let lod0 = at(mh, i32_(vtx, mh + 4)?)?;
        let nmesh = i32_(vtx, lod0)?.max(0) as usize;
        let meshoff = i32_(vtx, lod0 + 4)?;
        for m in 0..nmesh.min(meshes[bp].len()) {
            let (material, vbase) = meshes[bp][m];
            let me = at(lod0, meshoff)? + m * 9;
            let nsg = i32_(vtx, me)?.max(0) as usize;
            let sgoff = i32_(vtx, me + 4)?;
            for g in 0..nsg {
                let sg = at(me, sgoff)? + g * sg_size;
                let nverts = i32_(vtx, sg)?.max(0) as usize;
                let voff = at(sg, i32_(vtx, sg + 4)?)?;
                let nidx = i32_(vtx, sg + 8)?.max(0) as usize;
                let ioff = at(sg, i32_(vtx, sg + 12)?)?;
                if voff + nverts * 9 > vtx.len() || ioff + nidx * 2 > vtx.len() || nidx % 3 != 0 {
                    return None;
                }
                for t in 0..nidx / 3 {
                    let mut tri = [0u32; 3];
                    for k in 0..3 {
                        let i = u16_(vtx, ioff + (t * 3 + k) * 2)? as usize;
                        if i >= nverts {
                            return None;
                        }
                        let orig = u16_(vtx, voff + i * 9 + 4)? as usize;
                        let gi = vbase + orig;
                        let v = verts.get(gi)?;
                        let id = *remap.entry(gi).or_insert_with(|| {
                            out.pos.push(src_to_anvil(v.0));
                            out.normal.push(src_to_anvil(v.1));
                            out.uv.push(v.2);
                            (out.pos.len() - 1) as u32
                        });
                        tri[k] = id;
                    }
                    out.tris.push(tri);
                    out.tri_mat.push(material.min(u16::MAX as usize) as u16);
                }
            }
        }
    }
    (!out.tris.is_empty()).then_some(out)
}
