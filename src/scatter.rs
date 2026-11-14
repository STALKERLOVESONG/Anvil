//! Randomize offset & rotation: small random nudges so clutter looks
//! placed by hand (chairs around a table, crates in a corner).

use crate::app::AnvilApp;
use crate::map::Object;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterSpec {
    pub pos_axes: [bool; 3],
    pub pos_min: f32,
    pub pos_max: f32,
    pub rot_axes: [bool; 3],
    pub rot_min: f32,
    pub rot_max: f32,
}

#[derive(Default)]
pub(crate) struct ScatterUi {
    pub open: bool,
    /// The objects as they were before the last Randomize (for Re-roll).
    last: Option<Vec<Object>>,
}

/// Small fast RNG (xorshift64*), seeded.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// 0..1
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// Nudges every object on the switched-on axes only.
pub fn randomize(objs: &mut [Object], seed: u64, s: &ScatterSpec) {
    let mut r = Rng::new(seed);
    for o in objs {
        for k in 0..3 {
            if s.pos_axes[k] {
                o.pos[k] += r.range(s.pos_min, s.pos_max);
            }
        }
        for k in 0..3 {
            if s.rot_axes[k] {
                o.rot[k] += r.range(s.rot_min, s.rot_max);
            }
        }
    }
}

fn fresh_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)
}

impl AnvilApp {
    fn scatter_spec(&self) -> ScatterSpec {
        let s = &self.settings;
        ScatterSpec { pos_axes: s.rand_pos_axes, pos_min: s.rand_pos_min, pos_max: s.rand_pos_max, rot_axes: s.rand_rot_axes, rot_min: s.rand_rot_min, rot_max: s.rand_rot_max }
    }

    fn apply_scatter(&mut self, originals: Vec<Object>) {
        let seed = if self.settings.rand_seed == 0 { fresh_seed() } else { self.settings.rand_seed };
        let mut objs = originals.clone();
        randomize(&mut objs, seed, &self.scatter_spec());
        for o in objs {
            if let Some(x) = self.map.get_mut(o.id) {
                *x = o;
            }
        }
        self.scatter.last = Some(originals);
        self.dirty = true;
    }

    pub(crate) fn scatter_window(&mut self, ctx: &egui::Context) {
        if !self.scatter.open {
            return;
        }
        let mut open = true;
        let mut act = 0;
        egui::Window::new("Randomize").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("Small random changes to each selected object, only on the axes you tick. E.g. X/Z offset + Y rotation makes chairs look placed by hand without floating or tilting.").small().weak());
            let s = &mut self.settings;
            egui::Grid::new("scatter").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Offset on");
                ui.horizontal(|ui| {
                    for (k, n) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.checkbox(&mut s.rand_pos_axes[k], *n);
                    }
                });
                ui.end_row();
                ui.label("Offset range");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.rand_pos_min).speed(0.05).prefix("min "));
                    ui.add(egui::DragValue::new(&mut s.rand_pos_max).speed(0.05).prefix("max "));
                });
                ui.end_row();
                ui.label("Rotate on");
                ui.horizontal(|ui| {
                    for (k, n) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.checkbox(&mut s.rand_rot_axes[k], *n);
                    }
                });
                ui.end_row();
                ui.label("Angle range");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.rand_rot_min).speed(0.5).suffix("°").prefix("min "));
                    ui.add(egui::DragValue::new(&mut s.rand_rot_max).speed(0.5).suffix("°").prefix("max "));
                });
                ui.end_row();
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut s.rand_seed).prefix("").speed(1.0)).on_hover_text("0 = different every time. Any other number gives the same result each time.");
                ui.end_row();
            });
            let n = self.selection.len();
            ui.horizontal(|ui| {
                if ui.add_enabled(n > 0, egui::Button::new(format!("Randomize {n} object(s)"))).clicked() {
                    act = 1;
                }
                let can_reroll = self.scatter.last.as_ref().is_some_and(|l| l.len() == n && l.iter().all(|o| self.selection.contains(&o.id)));
                if ui.add_enabled(can_reroll, egui::Button::new("Re-roll")).on_hover_text("Try again from where they were before (same undo step)").clicked() {
                    act = 2;
                }
            });
        });
        if !open {
            self.scatter.open = false;
        }
        match act {
            1 => {
                let _ = self.settings.save();
                self.push_undo();
                let originals = self.selected_objects();
                self.apply_scatter(originals);
                self.status = "Randomized. Not quite right? Re-roll.".into();
            }
            2 => {
                if let Some(orig) = self.scatter.last.take() {
                    if self.settings.rand_seed != 0 {
                        self.settings.rand_seed = self.settings.rand_seed.wrapping_add(1);
                    }
                    self.apply_scatter(orig);
                }
            }
            _ => {}
        }
    }
}
