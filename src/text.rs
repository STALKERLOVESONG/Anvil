//! 3D text: turns a string in any installed font into an extruded mesh.
//! The result is a normal editable mesh, so a map with custom-font text
//! opens correctly on PCs that don't have the font.

use glam::Vec3;
use std::path::PathBuf;
use ttf_parser::{Face, OutlineBuilder};

#[derive(Clone, Debug)]
pub struct FontInfo {
    pub name: String,
    pub path: String,
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from(r"C:\Windows\Fonts")];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    v
}

/// Family + style name from a font file ("Arial Bold").
fn font_name(data: &[u8]) -> Option<String> {
    let face = Face::parse(data, 0).ok()?;
    let mut family = None;
    let mut sub = None;
    for n in face.names() {
        if !n.is_unicode() {
            continue;
        }
        match n.name_id {
            ttf_parser::name_id::FAMILY if family.is_none() => family = n.to_string(),
            ttf_parser::name_id::SUBFAMILY if sub.is_none() => sub = n.to_string(),
            _ => {}
        }
    }
    let family = family?;
    Some(match sub {
        Some(s) if !s.eq_ignore_ascii_case("regular") => format!("{family} {s}"),
        _ => family,
    })
}

/// Every installed .ttf / .otf / .ttc font, sorted by name.
pub fn system_fonts() -> Vec<FontInfo> {
    let mut out = Vec::new();
    for d in font_dirs() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let name = font_name(&data).unwrap_or_else(|| p.file_stem().and_then(|s| s.to_str()).unwrap_or("font").to_string());
            out.push(FontInfo { name, path: p.to_string_lossy().to_string() });
        }
    }
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Arial if installed, else the first font found.
pub fn default_font() -> Option<FontInfo> {
    let arial = PathBuf::from(r"C:\Windows\Fonts\arial.ttf");
    if arial.is_file() {
        return Some(FontInfo { name: "Arial".into(), path: arial.to_string_lossy().to_string() });
    }
    system_fonts().into_iter().next()
}

/// Flattens glyph outlines into closed polygons.
struct Flatten {
    contours: Vec<Vec<(f32, f32)>>,
    cur: Vec<(f32, f32)>,
    last: (f32, f32),
    off: (f32, f32),
    s: f32,
}

impl Flatten {
    fn pt(&self, x: f32, y: f32) -> (f32, f32) {
        (self.off.0 + x * self.s, self.off.1 + y * self.s)
    }
    fn push(&mut self, p: (f32, f32)) {
        if let Some(l) = self.cur.last() {
            if (l.0 - p.0).abs() < 1e-5 && (l.1 - p.1).abs() < 1e-5 {
                return;
            }
        }
        self.cur.push(p);
    }
    fn end(&mut self) {
        let mut c = std::mem::take(&mut self.cur);
        while c.len() > 1 {
            let (f, l) = (c[0], c[c.len() - 1]);
            if (f.0 - l.0).abs() < 1e-5 && (f.1 - l.1).abs() < 1e-5 {
                c.pop();
            } else {
                break;
            }
        }
        if c.len() >= 3 && area(&c).abs() > 1e-9 {
            self.contours.push(c);
        }
    }
}

impl OutlineBuilder for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.end();
        let p = self.pt(x, y);
        self.push(p);
        self.last = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.push(p);
        self.last = (x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x0, y0) = self.last;
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            let p = self.pt(u * u * x0 + 2.0 * u * t * x1 + t * t * x, u * u * y0 + 2.0 * u * t * y1 + t * t * y);
            self.push(p);
        }
        self.last = (x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x0, y0) = self.last;
        for i in 1..=10 {
            let t = i as f32 / 10.0;
            let u = 1.0 - t;
            let p = self.pt(
                u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x,
                u * u * u * y0 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y,
            );
            self.push(p);
        }
        self.last = (x, y);
    }
    fn close(&mut self) {
        self.end();
    }
}

fn area(c: &[(f32, f32)]) -> f32 {
    let n = c.len();
    (0..n).map(|i| c[i].0 * c[(i + 1) % n].1 - c[(i + 1) % n].0 * c[i].1).sum::<f32>() * 0.5
}

fn inside(p: (f32, f32), c: &[(f32, f32)]) -> bool {
    let mut odd = false;
    let n = c.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (c[i], c[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// Builds an extruded text mesh: `height` = em size in units, `depth` =
/// thickness. Text stands in the XY plane facing +Z, centered on X and Z,
/// with its lowest point at Y = 0. Returns (verts, faces).
pub fn text_mesh(font_path: &str, text: &str, height: f32, depth: f32) -> Result<(Vec<Vec3>, Vec<Vec<u32>>), String> {
    if crate::texture::is_network_path(font_path) {
        return Err("Fonts on network paths aren't opened, for safety".into());
    }
    let data = std::fs::read(font_path).map_err(|e| format!("Could not read font: {e}"))?;
    let face = Face::parse(&data, 0).map_err(|e| format!("Not a usable font: {e}"))?;
    let s = height / face.units_per_em().max(1) as f32;
    let line_h = (face.ascender() as f32 - face.descender() as f32 + face.line_gap() as f32) * s;
    let space = face.glyph_index(' ').and_then(|g| face.glyph_hor_advance(g)).unwrap_or(face.units_per_em() / 3) as f32 * s;

    // one list of contours per glyph (classify holes per glyph so
    // overlapping italic glyphs don't confuse each other)
    let mut glyphs: Vec<Vec<Vec<(f32, f32)>>> = Vec::new();
    let mut y = 0.0;
    for line in text.lines() {
        let mut x = 0.0;
        for ch in line.chars() {
            let Some(gid) = face.glyph_index(ch) else {
                x += space;
                continue;
            };
            let mut fl = Flatten { contours: Vec::new(), cur: Vec::new(), last: (0.0, 0.0), off: (x, y), s };
            face.outline_glyph(gid, &mut fl);
            fl.end();
            if !fl.contours.is_empty() {
                glyphs.push(fl.contours);
            }
            x += face.glyph_hor_advance(gid).map(|a| a as f32 * s).unwrap_or(space);
        }
        y -= line_h;
    }
    if glyphs.is_empty() {
        return Err("Nothing to build (empty text, or the font has none of these characters)".into());
    }

    let hz = depth.max(0.001) * 0.5;
    let mut verts: Vec<Vec3> = Vec::new();
    let mut faces: Vec<Vec<u32>> = Vec::new();
    for contours in glyphs {
        // containment depth: even = outline, odd = hole
        let depth_of: Vec<usize> = contours.iter().enumerate().map(|(i, c)| contours.iter().enumerate().filter(|(j, o)| *j != i && inside(c[0], o)).count()).collect();
        for (oi, outer) in contours.iter().enumerate() {
            if depth_of[oi] % 2 == 1 {
                continue;
            }
            let mut rings: Vec<Vec<(f32, f32)>> = Vec::new();
            let mut o = outer.clone();
            if area(&o) < 0.0 {
                o.reverse(); // outlines counter-clockwise
            }
            rings.push(o);
            for (hi, h) in contours.iter().enumerate() {
                if depth_of[hi] == depth_of[oi] + 1 && inside(h[0], outer) {
                    let mut h = h.clone();
                    if area(&h) > 0.0 {
                        h.reverse(); // holes clockwise
                    }
                    rings.push(h);
                }
            }
            // triangulate the cap (outline with holes)
            let mut flat: Vec<f64> = Vec::new();
            let mut holes = Vec::new();
            for (ri, r) in rings.iter().enumerate() {
                if ri > 0 {
                    holes.push(flat.len() / 2);
                }
                for p in r {
                    flat.push(p.0 as f64);
                    flat.push(p.1 as f64);
                }
            }
            let tris = earcutr::earcut(&flat, &holes, 2).map_err(|e| format!("Could not triangulate a glyph: {e:?}"))?;
            let base = verts.len() as u32;
            let n = flat.len() / 2;
            for i in 0..n {
                verts.push(Vec3::new(flat[i * 2] as f32, flat[i * 2 + 1] as f32, hz));
            }
            for i in 0..n {
                verts.push(Vec3::new(flat[i * 2] as f32, flat[i * 2 + 1] as f32, -hz));
            }
            for t in tris.chunks_exact(3) {
                let (a, b, c) = (t[0] as u32, t[1] as u32, t[2] as u32);
                let (pa, pb, pc) = (verts[(base + a) as usize], verts[(base + b) as usize], verts[(base + c) as usize]);
                let ccw = (pb - pa).cross(pc - pa).z > 0.0;
                let (a, b, c) = if ccw { (a, b, c) } else { (a, c, b) };
                faces.push(vec![base + a, base + b, base + c]); // front (+Z)
                faces.push(vec![base + n as u32 + a, base + n as u32 + c, base + n as u32 + b]); // back (-Z)
            }
            // side walls along every ring
            let mut start = 0u32;
            for r in &rings {
                let m = r.len() as u32;
                for i in 0..m {
                    let (fi, fj) = (base + start + i, base + start + (i + 1) % m);
                    let (bi, bj) = (fi + n as u32, fj + n as u32);
                    faces.push(vec![bi, bj, fj, fi]);
                }
                start += m;
            }
        }
    }
    // center on X and Z, stand on Y = 0
    let (mn, mx) = verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    let shift = Vec3::new(-(mn.x + mx.x) * 0.5, -mn.y, 0.0);
    for v in &mut verts {
        *v += shift;
    }
    Ok((verts, faces))
}
