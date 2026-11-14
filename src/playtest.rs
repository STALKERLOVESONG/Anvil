//! Playtest mode: a Roblox-sized first-person character (2 x 5 studs,
//! 16 studs/s walk, 50 studs/s jump, 196.2 studs/s^2 gravity) that collides
//! with the level so you can check scale.

use std::sync::Arc;
use crate::mesh::WorldMesh;
use crate::view3d::Camera;
use glam::{Vec2, Vec3};

struct Collider {
    min: Vec3,
    max: Vec3,
    tris: Vec<[Vec3; 3]>,
}

pub struct Play {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    spawn: Vec3,
    /// editor units per stud
    s: f32,
    colliders: Vec<Collider>,
}

#[derive(Default, Clone, Copy)]
pub struct PlayInput {
    pub forward: f32,
    pub right: f32,
    pub jump: bool,
    pub run: bool,
    pub look: Vec2,
}

/// Closest point on triangle abc to p (Ericson, Real-Time Collision Detection).
pub fn closest_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

impl Play {
    /// `units_per_stud` = 1 / roblox_scale.
    pub fn new(scene: &[Arc<WorldMesh>], spawn: Vec3, yaw: f32, units_per_stud: f32) -> Self {
        let colliders = scene
            .iter()
            .filter(|w| w.solid)
            .map(|w| Collider {
                min: w.min,
                max: w.max,
                tris: w
                    .mesh
                    .tris
                    .iter()
                    .map(|t| [w.pos[t[0] as usize], w.pos[t[1] as usize], w.pos[t[2] as usize]])
                    .collect(),
            })
            .collect();
        Play { pos: spawn, vel: Vec3::ZERO, yaw, pitch: 0.0, grounded: false, spawn, s: units_per_stud, colliders }
    }

    pub fn eye_height(&self) -> f32 {
        4.5 * self.s
    }

    pub fn camera(&self) -> Camera {
        Camera { pos: self.pos + Vec3::Y * self.eye_height(), yaw: self.yaw, pitch: self.pitch, ortho: 0.0 }
    }

    pub fn step(&mut self, input: &PlayInput, dt: f32) {
        self.yaw += input.look.x;
        self.pitch = (self.pitch - input.look.y).clamp(-1.5, 1.5);
        let dt = dt.min(0.1);
        let steps = ((dt / (1.0 / 120.0)).ceil() as usize).max(1);
        let h = dt / steps as f32;
        for _ in 0..steps {
            let s = self.s;
            let walk = 16.0 * s * if input.run { 1.75 } else { 1.0 };
            let f = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
            let r = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
            let wish = (f * input.forward + r * input.right).normalize_or_zero() * walk;
            self.vel.x = wish.x;
            self.vel.z = wish.z;
            if input.jump && self.grounded {
                self.vel.y = 50.0 * s;
                self.grounded = false;
            }
            self.vel.y -= 196.2 * s * h;
            self.vel.y = self.vel.y.max(-300.0 * s);
            self.pos += self.vel * h;
            self.collide();
            if self.pos.y < self.spawn.y - 600.0 * s {
                self.pos = self.spawn;
                self.vel = Vec3::ZERO;
            }
        }
    }

    fn collide(&mut self) {
        let r = 1.0 * self.s;
        let height = 5.0 * self.s;
        self.grounded = false;
        let offsets = [r, height * 0.5, height - r];
        for _ in 0..4 {
            for off in offsets {
                let mut c = self.pos + Vec3::Y * off;
                for col in &self.colliders {
                    if c.x < col.min.x - r
                        || c.y < col.min.y - r
                        || c.z < col.min.z - r
                        || c.x > col.max.x + r
                        || c.y > col.max.y + r
                        || c.z > col.max.z + r
                    {
                        continue;
                    }
                    for t in &col.tris {
                        let q = closest_on_tri(c, t[0], t[1], t[2]);
                        let d = c - q;
                        let d2 = d.length_squared();
                        if d2 >= r * r || d2 < 1e-12 {
                            continue;
                        }
                        let dist = d2.sqrt();
                        let n = d / dist;
                        let push = n * (r - dist);
                        self.pos += push;
                        c += push;
                        if n.y > 0.55 {
                            self.grounded = true;
                            if self.vel.y < 0.0 {
                                self.vel.y = 0.0;
                            }
                        } else if n.y < -0.55 {
                            if self.vel.y > 0.0 {
                                self.vel.y = 0.0;
                            }
                        } else {
                            let vn = self.vel.dot(n);
                            if vn < 0.0 {
                                self.vel -= n * vn;
                            }
                        }
                    }
                }
            }
        }
    }
}
