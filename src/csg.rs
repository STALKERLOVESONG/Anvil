//! Box-minus-boxes boolean used for door/window openings and carving.
//!
//! The box is split along every hole boundary into a grid of cells; cells
//! inside a hole are dropped and the rest are greedily merged back into as
//! few boxes as possible. Each output box becomes one Roblox part.

use crate::map::Hole;
use glam::Vec3;

pub fn box_minus(size: Vec3, holes: &[Hole]) -> Vec<(Vec3, Vec3)> {
    let bmin = -size * 0.5;
    let bmax = size * 0.5;
    let hs: Vec<(Vec3, Vec3)> = holes
        .iter()
        .map(|h| (h.min.min(h.max).max(bmin), h.max.max(h.min).min(bmax)))
        .filter(|(a, b)| (b.x - a.x) > 1e-4 && (b.y - a.y) > 1e-4 && (b.z - a.z) > 1e-4)
        .collect();
    if hs.is_empty() {
        return vec![(bmin, bmax)];
    }
    let mut cuts: [Vec<f32>; 3] = Default::default();
    for a in 0..3 {
        let mut c = vec![bmin[a], bmax[a]];
        for (mn, mx) in &hs {
            c.push(mn[a]);
            c.push(mx[a]);
        }
        c.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        c.dedup_by(|x, y| (*x - *y).abs() < 1e-5);
        cuts[a] = c;
    }
    let (nx, ny, nz) = (cuts[0].len() - 1, cuts[1].len() - 1, cuts[2].len() - 1);
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    let mut solid = vec![false; nx * ny * nz];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let c = Vec3::new(
                    (cuts[0][i] + cuts[0][i + 1]) * 0.5,
                    (cuts[1][j] + cuts[1][j + 1]) * 0.5,
                    (cuts[2][k] + cuts[2][k + 1]) * 0.5,
                );
                solid[idx(i, j, k)] = !hs.iter().any(|(a, b)| c.cmpgt(*a).all() && c.cmplt(*b).all());
            }
        }
    }
    let mut used = vec![false; solid.len()];
    let free = |used: &Vec<bool>, i, j, k| solid[idx(i, j, k)] && !used[idx(i, j, k)];
    let mut out = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                if !free(&used, i, j, k) {
                    continue;
                }
                let mut i2 = i;
                while i2 + 1 < nx && free(&used, i2 + 1, j, k) {
                    i2 += 1;
                }
                let mut j2 = j;
                'y: while j2 + 1 < ny {
                    for ii in i..=i2 {
                        if !free(&used, ii, j2 + 1, k) {
                            break 'y;
                        }
                    }
                    j2 += 1;
                }
                let mut k2 = k;
                'z: while k2 + 1 < nz {
                    for jj in j..=j2 {
                        for ii in i..=i2 {
                            if !free(&used, ii, jj, k2 + 1) {
                                break 'z;
                            }
                        }
                    }
                    k2 += 1;
                }
                for kk in k..=k2 {
                    for jj in j..=j2 {
                        for ii in i..=i2 {
                            used[idx(ii, jj, kk)] = true;
                        }
                    }
                }
                out.push((
                    Vec3::new(cuts[0][i], cuts[1][j], cuts[2][k]),
                    Vec3::new(cuts[0][i2 + 1], cuts[1][j2 + 1], cuts[2][k2 + 1]),
                ));
            }
        }
    }
    out
}
