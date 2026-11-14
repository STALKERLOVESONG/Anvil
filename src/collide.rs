//! Collisions for moving and rotating (like Roblox Studio's Collisions
//! toggle): parts stop flush against whatever they'd push into, slide along
//! walls and floors, and rotations stop at the point of contact.
//!
//! Real shapes are tested, not boxes: triangle-against-triangle, plus a
//! "fully inside" check for small closed shapes. Moving parts are shrunk by
//! a hair so parts resting flush on each other don't count as overlapping.

use crate::app::AnvilApp;
use crate::map::Object;
use crate::mesh::{local_mesh, ModelCache, WorldMesh};
use crate::view3d::ray_tri;
use glam::{Quat, Vec3};
use std::sync::Arc;

const EPS: f32 = 0.02;

/// A part being moved: its shape in object space plus where it started.
pub struct Mover {
    pub id: u64,
    pub pos0: Vec3,
    pub rot0: Quat,
    local: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    center: Vec3,
    extent: f32,
}

impl Mover {
    pub fn new(o: &Object, models: &mut ModelCache) -> Mover {
        let m = local_mesh(o, models);
        let sc = o.scale();
        let local: Vec<Vec3> = m.pos.iter().map(|p| *p * sc).collect();
        let (mn, mx) = local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let (center, ext) = if local.is_empty() { (Vec3::ZERO, Vec3::ONE) } else { ((mn + mx) * 0.5, mx - mn) };
        Mover { id: o.id, pos0: o.pos, rot0: o.rotation(), local, tris: m.tris.clone(), center, extent: ext.max_element().max(0.05) }
    }

    fn min_extent(&self) -> f32 {
        self.extent
    }

    /// World vertices at a pose, pulled in by EPS toward the middle.
    fn world(&self, pos: Vec3, rot: Quat) -> (Vec<Vec3>, Vec3, Vec3) {
        let c = pos + rot * self.center;
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        let v: Vec<Vec3> = self
            .local
            .iter()
            .map(|p| {
                let w = pos + rot * *p;
                let to_c = c - w;
                let d = to_c.length();
                let w = if d > 1e-6 { w + to_c * (EPS.min(d * 0.3) / d) } else { w };
                mn = mn.min(w);
                mx = mx.max(w);
                w
            })
            .collect();
        (v, mn, mx)
    }
}

fn aabb_hit(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> bool {
    a0.x <= b1.x && a1.x >= b0.x && a0.y <= b1.y && a1.y >= b0.y && a0.z <= b1.z && a1.z >= b0.z
}

fn seg_tri(p: Vec3, q: Vec3, t: [Vec3; 3]) -> bool {
    ray_tri(p, q - p, t[0], t[1], t[2]).is_some_and(|s| s <= 1.0)
}

/// Do two triangles cross? (Any edge of one passes through the other.)
pub fn tri_tri(a: [Vec3; 3], b: [Vec3; 3]) -> bool {
    (0..3).any(|i| seg_tri(a[i], a[(i + 1) % 3], b)) || (0..3).any(|i| seg_tri(b[i], b[(i + 1) % 3], a))
}

/// Ray-parity "is p inside this closed mesh".
fn inside(p: Vec3, verts: &[Vec3], tris: &[[u32; 3]]) -> bool {
    let d = Vec3::new(0.0123, 1.0, 0.0071).normalize();
    tris.iter().filter(|t| ray_tri(p, d, verts[t[0] as usize], verts[t[1] as usize], verts[t[2] as usize]).is_some()).count() % 2 == 1
}

/// The parts that don't move (everything solid and visible that isn't being moved).
pub struct World<'a> {
    statics: Vec<&'a WorldMesh>,
    pub movers: Vec<Mover>,
}

impl<'a> World<'a> {
    pub fn new(scene: &'a [Arc<WorldMesh>], movers: Vec<Mover>) -> World<'a> {
        let ids: Vec<u64> = movers.iter().map(|m| m.id).collect();
        let statics = scene.iter().filter(|w| w.solid && !ids.contains(&w.id)).map(|w| &**w).collect();
        World { statics, movers }
    }

    /// The first part hit with every mover at the given poses.
    pub fn overlap(&self, poses: &[(Vec3, Quat)]) -> Option<u64> {
        for (m, &(pos, rot)) in self.movers.iter().zip(poses) {
            let (wv, mn, mx) = m.world(pos, rot);
            for s in &self.statics {
                if !aabb_hit(mn, mx, s.min, s.max) {
                    continue;
                }
                for t in &m.tris {
                    let a = [wv[t[0] as usize], wv[t[1] as usize], wv[t[2] as usize]];
                    let (ta0, ta1) = (a[0].min(a[1]).min(a[2]), a[0].max(a[1]).max(a[2]));
                    if !aabb_hit(ta0, ta1, s.min, s.max) {
                        continue;
                    }
                    for u in &s.mesh.tris {
                        let b = [s.pos[u[0] as usize], s.pos[u[1] as usize], s.pos[u[2] as usize]];
                        let (tb0, tb1) = (b[0].min(b[1]).min(b[2]), b[0].max(b[1]).max(b[2]));
                        if aabb_hit(ta0, ta1, tb0, tb1) && tri_tri(a, b) {
                            return Some(s.id);
                        }
                    }
                }
                // fully inside each other (small closed shapes only)
                if s.mesh.tris.len() <= 256 && !wv.is_empty() {
                    let c = (mn + mx) * 0.5;
                    if inside(c, &s.pos, &s.mesh.tris) {
                        return Some(s.id);
                    }
                }
                if m.tris.len() <= 256 && inside((s.min + s.max) * 0.5, &wv, &m.tris) && aabb_hit(s.min, s.max, mn, mx) {
                    let sc = (s.min + s.max) * 0.5;
                    if sc.cmpge(mn).all() && sc.cmple(mx).all() {
                        return Some(s.id);
                    }
                }
            }
        }
        None
    }

    fn offset(&self, d: Vec3) -> Vec<(Vec3, Quat)> {
        self.movers.iter().map(|m| (m.pos0 + d, m.rot0)).collect()
    }

    fn step_len(&self) -> f32 {
        self.movers.iter().map(|m| m.min_extent()).fold(f32::MAX, f32::min).max(0.2) * 0.4
    }

    /// How far along `wd` everything can go: each axis on its own (so parts
    /// slide along walls), stopping flush at the first contact.
    pub fn resolve_translation(&self, wd: Vec3) -> (Vec3, Option<u64>) {
        if self.movers.is_empty() || self.overlap(&self.offset(wd)).is_none() && self.swept_free(Vec3::ZERO, wd) {
            return (wd, None);
        }
        if self.overlap(&self.offset(Vec3::ZERO)).is_some() {
            return (wd, None); // already stuck inside something: don't trap it
        }
        let mut cur = Vec3::ZERO;
        let mut hit = None;
        for k in 0..3 {
            if wd[k] == 0.0 {
                continue;
            }
            let mut step = Vec3::ZERO;
            step[k] = wd[k];
            let (f, h) = self.first_contact(cur, step);
            cur += step * f;
            hit = hit.or(h);
        }
        (cur, hit)
    }

    fn swept_free(&self, from: Vec3, d: Vec3) -> bool {
        let n = ((d.length() / self.step_len()).ceil() as usize).clamp(1, 64);
        (1..=n).all(|i| self.overlap(&self.offset(from + d * (i as f32 / n as f32))).is_none())
    }

    /// Largest fraction of `step` (from `from`) with no overlap, sampled so
    /// thin walls aren't skipped, then refined by bisection.
    fn first_contact(&self, from: Vec3, step: Vec3) -> (f32, Option<u64>) {
        let n = ((step.length() / self.step_len()).ceil() as usize).clamp(1, 64);
        let mut lo = 0.0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            if let Some(id) = self.overlap(&self.offset(from + step * t)) {
                let mut hi = t;
                for _ in 0..14 {
                    let mid = (lo + hi) * 0.5;
                    if self.overlap(&self.offset(from + step * mid)).is_some() {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                return (lo, Some(id));
            }
            lo = t;
        }
        (1.0, None)
    }

    /// For a move + turn to `targets`: the largest fraction (0..1) of the way
    /// that's free, so a rotation stops where the part touches something.
    pub fn resolve_fraction(&self, targets: &[(Vec3, Quat)]) -> (f32, Option<u64>) {
        let pose = |s: f32| -> Vec<(Vec3, Quat)> { self.movers.iter().zip(targets).map(|(m, &(p, q))| (m.pos0.lerp(p, s), m.rot0.slerp(q, s))).collect() };
        if self.overlap(&pose(0.0)).is_some() {
            return (1.0, None);
        }
        let angle = self.movers.iter().zip(targets).map(|(m, (_, q))| m.rot0.angle_between(*q).to_degrees()).fold(0.0, f32::max);
        let dist = self.movers.iter().zip(targets).map(|(m, (p, _))| (*p - m.pos0).length()).fold(0.0, f32::max);
        let n = ((angle / 3.0).max(dist / self.step_len()).ceil() as usize).clamp(1, 96);
        let mut lo = 0.0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            if let Some(id) = self.overlap(&pose(t)) {
                let mut hi = t;
                for _ in 0..14 {
                    let mid = (lo + hi) * 0.5;
                    if self.overlap(&pose(mid)).is_some() {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                return (lo, Some(id));
            }
            lo = t;
        }
        (1.0, None)
    }
}

impl AnvilApp {
    fn movers_for(&mut self, objs: &[Object]) -> Vec<Mover> {
        objs.iter().map(|o| Mover::new(o, &mut self.models)).collect()
    }

    fn hit_name(&self, id: Option<u64>) -> Option<String> {
        id.and_then(|i| self.map.get(i)).map(|o| o.name.clone())
    }

    /// A drag / nudge of the parts that started at `origs` by `wd`, cut short
    /// where they'd run into something (when Collisions is on).
    pub(crate) fn collide_translate(&mut self, scene: &[Arc<WorldMesh>], origs: &[(u64, Vec3)], wd: Vec3) -> Vec3 {
        if !self.settings.collisions || wd == Vec3::ZERO {
            return wd;
        }
        let objs: Vec<Object> = origs
            .iter()
            .filter_map(|(id, p)| {
                self.map.get(*id).cloned().map(|mut o| {
                    o.pos = *p;
                    o
                })
            })
            .collect();
        let movers = self.movers_for(&objs);
        let w = World::new(scene, movers);
        let (d, hit) = w.resolve_translation(wd);
        if let Some(n) = self.hit_name(hit) {
            self.status = format!("Stopped against {n} (Collisions on)");
        }
        d
    }

    /// Plugin / button results that move or turn the selection: keep the
    /// furthest pose that doesn't push into anything. Size changes pass.
    pub(crate) fn collide_objects(&mut self, scene: &[Arc<WorldMesh>], origs: &[Object], proposed: &mut [Object]) {
        if !self.settings.collisions || origs.is_empty() {
            return;
        }
        let pairs: Vec<(Object, usize)> = origs.iter().filter_map(|o| proposed.iter().position(|p| p.id == o.id).map(|i| (o.clone(), i))).collect();
        if pairs.is_empty() || pairs.iter().any(|(o, i)| o.kind != proposed[*i].kind) {
            return; // a resize: let it through
        }
        let objs: Vec<Object> = pairs.iter().map(|(o, _)| o.clone()).collect();
        let turned = pairs.iter().any(|(o, i)| o.rotation().angle_between(proposed[*i].rotation()) > 1e-4);
        let movers = self.movers_for(&objs);
        let w = World::new(scene, movers);
        if !turned {
            let wd = proposed[pairs[0].1].pos - pairs[0].0.pos;
            let (d, hit) = w.resolve_translation(wd);
            for (o, i) in &pairs {
                proposed[*i].pos = o.pos + d;
            }
            if let Some(n) = self.hit_name(hit) {
                self.status = format!("Stopped against {n} (Collisions on)");
            }
            return;
        }
        let targets: Vec<(Vec3, Quat)> = pairs.iter().map(|(_, i)| (proposed[*i].pos, proposed[*i].rotation())).collect();
        let (s, hit) = w.resolve_fraction(&targets);
        if s < 1.0 {
            for (o, i) in &pairs {
                let q = o.rotation().slerp(targets.iter().zip(&pairs).find(|(_, p)| p.1 == *i).map(|(t, _)| t.1).unwrap_or(o.rotation()), s);
                proposed[*i].pos = o.pos.lerp(proposed[*i].pos, s);
                proposed[*i].set_rotation(q);
            }
            if let Some(n) = self.hit_name(hit) {
                self.status = format!("Rotation stopped against {n} (Collisions on)");
            }
        }
    }
}
