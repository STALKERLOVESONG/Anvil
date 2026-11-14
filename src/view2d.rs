//! Orthographic Top / Front / Side views (Hammer-style grid views).

use std::sync::Arc;
use crate::mesh::WorldMesh;
use eframe::egui::{pos2, Color32, Painter, Pos2, Rect, Stroke};
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Top,
    Front,
    Side,
}

/// A 2D view maps two world axes onto the screen ("u" right, "v" up).
#[derive(Clone, Copy, Debug)]
pub struct View2D {
    pub kind: ViewKind,
    pub center: Vec2,
    /// Screen points per world unit.
    pub zoom: f32,
}

impl View2D {
    pub fn new(kind: ViewKind) -> Self {
        let center = match kind {
            ViewKind::Top => Vec2::ZERO,
            _ => Vec2::new(0.0, 8.0),
        };
        View2D { kind, center, zoom: 8.0 }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            ViewKind::Top => "Top (X / Z)",
            ViewKind::Front => "Front (X / Y)",
            ViewKind::Side => "Side (Z / Y)",
        }
    }

    /// (u axis, u sign, v axis, v sign, depth axis)
    pub fn axes(&self) -> (usize, f32, usize, f32, usize) {
        match self.kind {
            ViewKind::Top => (0, 1.0, 2, -1.0, 1),
            ViewKind::Front => (0, 1.0, 1, 1.0, 2),
            ViewKind::Side => (2, 1.0, 1, 1.0, 0),
        }
    }

    pub fn to_uv(&self, p: Vec3) -> Vec2 {
        let (u, su, v, sv, _) = self.axes();
        Vec2::new(p[u] * su, p[v] * sv)
    }

    pub fn uv_to_screen(&self, rect: Rect, uv: Vec2) -> Pos2 {
        let c = rect.center();
        pos2(c.x + (uv.x - self.center.x) * self.zoom, c.y - (uv.y - self.center.y) * self.zoom)
    }

    pub fn world_to_screen(&self, rect: Rect, p: Vec3) -> Pos2 {
        self.uv_to_screen(rect, self.to_uv(p))
    }

    pub fn screen_to_uv(&self, rect: Rect, s: Pos2) -> Vec2 {
        let c = rect.center();
        Vec2::new(self.center.x + (s.x - c.x) / self.zoom, self.center.y - (s.y - c.y) / self.zoom)
    }

    pub fn uv_to_world(&self, uv: Vec2, depth: f32) -> Vec3 {
        let (u, su, v, sv, d) = self.axes();
        let mut p = Vec3::ZERO;
        p[u] = uv.x * su;
        p[v] = uv.y * sv;
        p[d] = depth;
        p
    }

    pub fn uv_delta_to_world(&self, d: Vec2) -> Vec3 {
        self.uv_to_world(d, 0.0)
    }

    /// Screen-space bounding rect of a world AABB.
    pub fn screen_rect(&self, rect: Rect, mn: Vec3, mx: Vec3) -> Rect {
        Rect::from_two_pos(self.world_to_screen(rect, mn), self.world_to_screen(rect, mx))
    }

    pub fn draw_grid(&self, painter: &Painter, rect: Rect, grid: f32) {
        let mut step = grid.max(1e-3);
        while step * self.zoom < 6.0 {
            step *= 2.0;
        }
        let a = self.screen_to_uv(rect, rect.left_bottom());
        let b = self.screen_to_uv(rect, rect.right_top());
        let (u0, u1) = (a.x.min(b.x), a.x.max(b.x));
        let (v0, v1) = (a.y.min(b.y), a.y.max(b.y));
        let minor = Color32::from_gray(40);
        let major = Color32::from_gray(62);
        let axis = Color32::from_rgb(40, 95, 110);
        let pick = |k: i64| {
            if k == 0 {
                axis
            } else if k % 8 == 0 {
                major
            } else {
                minor
            }
        };
        let (k0, k1) = ((u0 / step).floor() as i64, (u1 / step).ceil() as i64);
        if k1 - k0 < 4000 {
            for k in k0..=k1 {
                let x = self.uv_to_screen(rect, Vec2::new(k as f32 * step, 0.0)).x;
                painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0f32, pick(k)));
            }
        }
        let (k0, k1) = ((v0 / step).floor() as i64, (v1 / step).ceil() as i64);
        if k1 - k0 < 4000 {
            for k in k0..=k1 {
                let y = self.uv_to_screen(rect, Vec2::new(0.0, k as f32 * step)).y;
                painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0f32, pick(k)));
            }
        }
    }
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

pub fn in_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let s = |p1: Pos2, p2: Pos2, p3: Pos2| (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y);
    let d1 = s(p, a, b);
    let d2 = s(p, b, c);
    let d3 = s(p, c, a);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Picks the object under the cursor. Objects whose outline is near the
/// cursor win over objects merely containing it; ties go to the smallest.
pub fn pick(view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>], p: Pos2) -> Option<u64> {
    let mut best: Option<(u8, f32, u64)> = None;
    for wm in scene {
        let r = view.screen_rect(rect, wm.min, wm.max);
        if !r.expand(5.0).contains(p) {
            continue;
        }
        let pts: Vec<Pos2> = wm.pos.iter().map(|q| view.world_to_screen(rect, *q)).collect();
        let mut pri = None;
        if wm.mesh.edges.iter().any(|e| dist_to_segment(p, pts[e[0] as usize], pts[e[1] as usize]) < 5.0) {
            pri = Some(0u8);
        } else if wm
            .mesh
            .tris
            .iter()
            .any(|t| in_triangle(p, pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]))
        {
            pri = Some(1u8);
        }
        if let Some(pr) = pri {
            let area = r.area().max(1.0);
            let better = match best {
                None => true,
                Some((bp, ba, _)) => pr < bp || (pr == bp && area < ba),
            };
            if better {
                best = Some((pr, area, wm.id));
            }
        }
    }
    best.map(|b| b.2)
}

/// Selection handles: (u dir, v dir, screen pos). v = +1 is the top edge.
pub fn handles(r: Rect) -> Vec<((i32, i32), Pos2)> {
    let mut out = Vec::new();
    let small = r.width() < 24.0 || r.height() < 24.0;
    for hu in [-1, 0, 1] {
        for hv in [-1, 0, 1] {
            if hu == 0 && hv == 0 {
                continue;
            }
            if small && (hu == 0 || hv == 0) {
                continue;
            }
            let x = match hu {
                -1 => r.left(),
                0 => r.center().x,
                _ => r.right(),
            };
            let y = match hv {
                1 => r.top(),
                0 => r.center().y,
                _ => r.bottom(),
            };
            out.push(((hu, hv), pos2(x, y)));
        }
    }
    out
}
