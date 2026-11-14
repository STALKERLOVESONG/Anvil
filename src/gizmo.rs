//! Built-in move / rotate / scale gizmo used for vertices and faces in mesh
//! edit mode (objects use the scriptable Transform Gizmo plugin, which
//! behaves the same way).

use crate::plugins::{DrawCmd, ViewInfo};
use crate::view2d::dist_to_segment;
use eframe::egui::Pos2;
use glam::{Quat, Vec3};

pub const MOVE: u8 = 0;
pub const ROTATE: u8 = 1;
pub const SCALE: u8 = 2;

const COLS: [[u8; 3]; 3] = [[235, 70, 70], [110, 215, 80], [80, 140, 255]];
const HOT: [u8; 3] = [255, 220, 60];
const HANDLE_PX: f32 = 85.0;
const PICK_PX: f32 = 9.0;

fn perp(a: Vec3) -> (Vec3, Vec3) {
    let rf = if a.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = a.cross(rf).normalize_or_zero();
    (u, a.cross(u))
}

pub fn ring(c: Vec3, a: Vec3, r: f32) -> Vec<Vec3> {
    let (u, v) = perp(a);
    (0..=48).map(|i| {
        let t = i as f32 / 48.0 * std::f32::consts::TAU;
        c + (u * t.cos() + v * t.sin()) * r
    })
    .collect()
}

pub fn handle_len(view: &ViewInfo, c: Vec3) -> f32 {
    view.units_per_pixel(c) * HANDLE_PX
}

/// Plane handle `k` (3, 4, 5): the square between the two axes other than
/// `k - 3`, so it moves in the plane facing that axis.
pub fn plane_quad(c: Vec3, axes: [Vec3; 3], k: usize, s: f32) -> [Vec3; 4] {
    let n = k - 3;
    let (i, j) = ((n + 1) % 3, (n + 2) % 3);
    let (a, b) = (s * 0.22, s * 0.42);
    let (u, v) = (axes[i], axes[j]);
    [c + u * a + v * a, c + u * b + v * a, c + u * b + v * b, c + u * a + v * b]
}

/// The plane square on screen, if it isn't nearly edge-on.
fn plane_screen(view: &ViewInfo, c: Vec3, axes: [Vec3; 3], k: usize, s: f32) -> Option<[Pos2; 4]> {
    let q = plane_quad(c, axes, k, s);
    let p = [view.project(q[0])?, view.project(q[1])?, view.project(q[2])?, view.project(q[3])?];
    let (d1, d2) = (p[2] - p[0], p[3] - p[1]);
    let area = (d1.x * d2.y - d1.y * d2.x).abs() * 0.5;
    (area > 60.0).then_some(p)
}

fn in_quad(m: Pos2, p: [Pos2; 4]) -> bool {
    let side = |a: Pos2, b: Pos2| (b.x - a.x) * (m.y - a.y) - (b.y - a.y) * (m.x - a.x);
    let s: Vec<f32> = (0..4).map(|i| side(p[i], p[(i + 1) % 4])).collect();
    s.iter().all(|x| *x >= 0.0) || s.iter().all(|x| *x <= 0.0)
}

fn ray_plane_n(o: Vec3, d: Vec3, c: Vec3, n: Vec3) -> Option<Vec3> {
    let den = d.dot(n);
    if den.abs() < 1e-5 {
        return None;
    }
    Some(o + d * ((c - o).dot(n) / den))
}

/// Handles to draw. `hot` = the handle being dragged (-1 center).
pub fn draw(view: &ViewInfo, mode: u8, c: Vec3, axes: [Vec3; 3], hot: Option<i32>) -> Vec<DrawCmd> {
    let mut out = Vec::new();
    let Some(pc) = view.project(c) else { return out };
    let s = handle_len(view, c);
    for k in 0..3 {
        let col = if hot == Some(k as i32) { HOT } else { COLS[k] };
        if mode == ROTATE {
            let pts = ring(c, axes[k], s * 0.8);
            for w in pts.windows(2) {
                out.push(DrawCmd::Line { a: w[0], b: w[1], color: col, width: 2.0 });
            }
        } else {
            let tip = c + axes[k] * s;
            match view.project(tip) {
                Some(pt) if (pt - pc).length() >= 10.0 => {}
                _ => continue,
            }
            out.push(DrawCmd::Line { a: c, b: tip, color: col, width: 2.5 });
            if mode == SCALE {
                out.push(DrawCmd::Point { p: tip, color: col, size: 9.0, square: true });
            } else {
                let (u, v) = perp(axes[k]);
                let back = tip - axes[k] * (s * 0.18);
                for side in [u, -u, v, -v] {
                    out.push(DrawCmd::Line { a: tip, b: back + side * (s * 0.07), color: col, width: 2.5 });
                }
            }
        }
    }
    if mode == MOVE {
        for k in 3..6 {
            if plane_screen(view, c, axes, k, s).is_none() {
                continue;
            }
            let q = plane_quad(c, axes, k, s);
            let col = if hot == Some(k as i32) { HOT } else { COLS[k - 3] };
            for i in 0..4 {
                out.push(DrawCmd::Line { a: q[i], b: q[(i + 1) % 4], color: col, width: 2.0 });
            }
            out.push(DrawCmd::Line { a: q[0], b: q[2], color: col, width: 1.0 });
            out.push(DrawCmd::Line { a: q[1], b: q[3], color: col, width: 1.0 });
        }
    }
    let center = if hot == Some(-1) { HOT } else { [235, 235, 235] };
    match mode {
        MOVE => out.push(DrawCmd::Point { p: c, color: center, size: 9.0, square: true }),
        SCALE => out.push(DrawCmd::Point { p: c, color: center, size: 11.0, square: true }),
        _ => out.push(DrawCmd::Point { p: c, color: center, size: 4.0, square: false }),
    }
    out
}

/// Which handle is under the mouse: -1 center, 0..2 an axis.
pub fn hit(view: &ViewInfo, mode: u8, c: Vec3, axes: [Vec3; 3], mouse: Pos2) -> Option<i32> {
    let pc = view.project(c)?;
    let s = handle_len(view, c);
    if mode != ROTATE && (mouse - pc).length() < PICK_PX {
        return Some(-1);
    }
    if mode == MOVE {
        for k in 3..6 {
            if plane_screen(view, c, axes, k, s).is_some_and(|q| in_quad(mouse, q)) {
                return Some(k as i32);
            }
        }
    }
    let mut best = (PICK_PX, None);
    for (k, a) in axes.iter().enumerate() {
        if mode == ROTATE {
            let pts = ring(c, *a, s * 0.8);
            for w in pts.windows(2) {
                if let (Some(p), Some(q)) = (view.project(w[0]), view.project(w[1])) {
                    let d = dist_to_segment(mouse, p, q);
                    if d < best.0 {
                        best = (d, Some(k as i32));
                    }
                }
            }
        } else if let Some(pt) = view.project(c + *a * s) {
            if (pt - pc).length() < 10.0 {
                continue;
            }
            let d = dist_to_segment(mouse, pc, pt);
            if d < best.0 {
                best = (d, Some(k as i32));
            }
        }
    }
    best.1
}

fn axis_param(o: Vec3, d: Vec3, c: Vec3, a: Vec3) -> f32 {
    let b = a.dot(d);
    let den = 1.0 - b * b;
    if den < 1e-4 {
        return 0.0;
    }
    let w = c - o;
    (b * d.dot(w) - a.dot(w)) / den
}

fn plane_point(o: Vec3, d: Vec3, c: Vec3) -> Vec3 {
    o + d * (c - o).dot(d)
}

/// A transform produced by dragging a handle.
#[derive(Clone, Copy, Debug)]
pub enum Xf {
    Move(Vec3),
    Rotate { q: Quat, pivot: Vec3, deg: f32 },
    /// axis = ZERO means uniform.
    Scale { axis: Vec3, f: f32, pivot: Vec3 },
}

impl Xf {
    pub fn apply(&self, p: Vec3) -> Vec3 {
        match *self {
            Xf::Move(d) => p + d,
            Xf::Rotate { q, pivot, .. } => pivot + q * (p - pivot),
            Xf::Scale { axis, f, pivot } => {
                if axis == Vec3::ZERO {
                    pivot + (p - pivot) * f
                } else {
                    p + axis * ((p - pivot).dot(axis) * (f - 1.0))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GDrag {
    pub mode: u8,
    pub axis: i32,
    pub c: Vec3,
    pub s: f32,
    pub axes: [Vec3; 3],
    mouse0: Pos2,
    t0: f32,
    p0: Vec3,
}

impl GDrag {
    pub fn begin(view: &ViewInfo, mode: u8, axis: i32, c: Vec3, axes: [Vec3; 3], mouse: Pos2) -> Self {
        let (o, d) = view.ray(mouse);
        let a = if (0..3).contains(&axis) { axes[axis as usize] } else { Vec3::ZERO };
        let p0 = if axis >= 3 { ray_plane_n(o, d, c, axes[axis as usize - 3]).unwrap_or(c) } else { plane_point(o, d, c) };
        GDrag { mode, axis, c, s: handle_len(view, c), axes, mouse0: mouse, t0: axis_param(o, d, c, a), p0 }
    }

    /// `grid` > 0 snaps moves; `snap` also snaps rotation to 15°.
    pub fn xf(&self, view: &ViewInfo, mouse: Pos2, grid: f32, snap: bool) -> Xf {
        let (o, d) = view.ray(mouse);
        let sn = |v: f32, g: f32| if snap && g > 0.0 { (v / g).round() * g } else { v };
        match self.mode {
            MOVE => {
                if self.axis >= 3 {
                    // plane handle: slide in the plane, third axis locked
                    let n = self.axes[self.axis as usize - 3];
                    let Some(p) = ray_plane_n(o, d, self.c, n) else { return Xf::Move(Vec3::ZERO) };
                    let v = p - self.p0;
                    let (u, w) = (self.axes[(self.axis as usize - 2) % 3], self.axes[(self.axis as usize - 1) % 3]);
                    Xf::Move(u * sn(v.dot(u), grid) + w * sn(v.dot(w), grid))
                } else if self.axis >= 0 {
                    let a = self.axes[self.axis as usize];
                    Xf::Move(a * sn(axis_param(o, d, self.c, a) - self.t0, grid))
                } else {
                    let v = plane_point(o, d, self.c) - self.p0;
                    Xf::Move(Vec3::new(sn(v.x, grid), sn(v.y, grid), sn(v.z, grid)))
                }
            }
            ROTATE => {
                let a = self.axes[self.axis.max(0) as usize];
                let pc = view.project(self.c).unwrap_or(self.mouse0);
                let a0 = (self.mouse0.y - pc.y).atan2(self.mouse0.x - pc.x);
                let a1 = (mouse.y - pc.y).atan2(mouse.x - pc.x);
                let mut deg = (a1 - a0).to_degrees();
                if a.dot(d) < 0.0 {
                    deg = -deg;
                }
                while deg > 180.0 {
                    deg -= 360.0;
                }
                while deg < -180.0 {
                    deg += 360.0;
                }
                let deg = sn(deg, 15.0);
                Xf::Rotate { q: Quat::from_axis_angle(a, deg.to_radians()), pivot: self.c, deg }
            }
            _ => {
                let f = if self.axis >= 0 {
                    let a = self.axes[self.axis as usize];
                    1.0 + (axis_param(o, d, self.c, a) - self.t0) / self.s.max(1e-4)
                } else {
                    1.0 + ((mouse.x - self.mouse0.x) - (mouse.y - self.mouse0.y)) / 120.0
                };
                let axis = if self.axis >= 0 { self.axes[self.axis as usize] } else { Vec3::ZERO };
                Xf::Scale { axis, f: f.max(0.02), pivot: self.c }
            }
        }
    }
}
