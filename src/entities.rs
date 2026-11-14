//! Prompts (Roblox ProximityPrompt), teleports and teleport targets: the
//! Teleport tool, Inspector fields, and the lines / labels drawn in the views.

use crate::app::{vec2_edit, vec3_edit, AnvilApp};
use crate::map::{Kind, Material, Object, PromptSpec};
use crate::view2d::View2D;
use eframe::egui::{self, vec2, Align2, Color32, FontId, Painter, Rect, RichText, Stroke};
use glam::Vec3;

pub const KEYS: [&str; 30] = [
    "E", "F", "Q", "R", "T", "G", "H", "X", "C", "V", "B", "Z", "Y", "U", "I", "O", "P", "J", "K", "L", "N", "M", "One", "Two", "Three", "Four", "Space", "Return", "LeftShift", "Tab",
];
pub const PADS: [&str; 10] = ["ButtonX", "ButtonY", "ButtonA", "ButtonB", "ButtonR1", "ButtonL1", "ButtonR2", "ButtonL2", "DPadUp", "DPadDown"];

/// Enum.KeyCode value for a key name (letters are their lowercase ASCII code).
pub fn keycode_value(name: &str) -> u32 {
    let n = name.trim();
    if n.len() == 1 && n.chars().all(|c| c.is_ascii_alphabetic()) {
        return n.to_ascii_lowercase().as_bytes()[0] as u32;
    }
    match n {
        "Zero" => 48,
        "One" => 49,
        "Two" => 50,
        "Three" => 51,
        "Four" => 52,
        "Five" => 53,
        "Space" => 32,
        "Return" => 13,
        "Tab" => 9,
        "LeftShift" => 304,
        "ButtonX" => 1000,
        "ButtonY" => 1001,
        "ButtonA" => 1002,
        "ButtonB" => 1003,
        "ButtonR1" => 1004,
        "ButtonL1" => 1005,
        "ButtonR2" => 1006,
        "ButtonL2" => 1007,
        "DPadLeft" => 1012,
        "DPadRight" => 1013,
        "DPadUp" => 1014,
        "DPadDown" => 1015,
        _ => 101, // E
    }
}

/// Inspector fields for a prompt. Returns true if anything changed.
pub(crate) fn prompt_ui(ui: &mut egui::Ui, p: &mut PromptSpec, id: &str) -> bool {
    let mut ch = false;
    ui.label("Action text");
    ch |= ui.text_edit_singleline(&mut p.action_text).changed();
    ui.end_row();
    ui.label("Object text");
    ch |= ui.text_edit_singleline(&mut p.object_text).changed();
    ui.end_row();
    ui.label("Key / gamepad");
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt((id, "key")).selected_text(&p.key).width(70.0).show_ui(ui, |ui| {
            for k in KEYS {
                ch |= ui.selectable_value(&mut p.key, k.to_string(), k).changed();
            }
        });
        egui::ComboBox::from_id_salt((id, "pad")).selected_text(&p.gamepad).width(80.0).show_ui(ui, |ui| {
            for k in PADS {
                ch |= ui.selectable_value(&mut p.gamepad, k.to_string(), k).changed();
            }
        });
    });
    ui.end_row();
    ui.label("Hold (s) / range");
    ui.horizontal(|ui| {
        ch |= ui.add(egui::DragValue::new(&mut p.hold).speed(0.05).range(0.0..=60.0)).changed();
        ch |= ui.add(egui::DragValue::new(&mut p.distance).speed(0.25).range(0.0..=1000.0)).changed();
    });
    ui.end_row();
    ui.label("Options");
    ui.horizontal_wrapped(|ui| {
        ch |= ui.checkbox(&mut p.enabled, "Enabled").changed();
        ch |= ui.checkbox(&mut p.line_of_sight, "Needs line of sight").changed();
        ch |= ui.checkbox(&mut p.custom_style, "Custom style").changed();
    });
    ui.end_row();
    ui.label("UI offset");
    ch |= vec2_edit(ui, &mut p.ui_offset, 1.0, None, ["x", "y"]);
    ui.end_row();
    ch
}

/// Inspector fields for teleports / prompts / targets (inside the Inspector grid).
pub(crate) fn entity_ui(ui: &mut egui::Ui, kind: &mut Kind, dests: &[(u64, String)], speed: f64) -> bool {
    let mut ch = false;
    match kind {
        Kind::Prompt { prompt } => ch |= prompt_ui(ui, prompt, "insp_prompt"),
        Kind::Teleport { size, touch, dest, cooldown, exit_offset, prompt } => {
            ui.label("Pad size");
            ch |= vec3_edit(ui, size, speed, Some(0.05));
            ui.end_row();
            ui.label("Type");
            ui.horizontal(|ui| {
                ch |= ui.selectable_value(touch, true, "Touch pad").changed();
                ch |= ui.selectable_value(touch, false, "Prompt").changed();
            });
            ui.end_row();
            ui.label("Destination");
            let name = dest.and_then(|d| dests.iter().find(|x| x.0 == d)).map(|x| x.1.clone()).unwrap_or_else(|| "(none)".into());
            egui::ComboBox::from_id_salt("insp_tp_dest").selected_text(name).width(160.0).show_ui(ui, |ui| {
                ch |= ui.selectable_value(dest, None, "(none)").changed();
                for (id, n) in dests {
                    ch |= ui.selectable_value(dest, Some(*id), n).changed();
                }
            });
            ui.end_row();
            ui.label("Cooldown (s)");
            ch |= ui.add(egui::DragValue::new(cooldown).speed(0.1).range(0.0..=600.0)).changed();
            ui.end_row();
            ui.label("Exit offset");
            ch |= vec3_edit(ui, exit_offset, speed, None);
            ui.end_row();
            if !*touch {
                ch |= prompt_ui(ui, prompt, "insp_tp_prompt");
            }
        }
        Kind::Target {} => {
            ui.label("Target");
            ui.label(RichText::new("Teleports can send players here. Invisible in the game.").small().weak());
            ui.end_row();
        }
        _ => {}
    }
    ch
}

pub(crate) fn new_prompt(at: Vec3) -> Object {
    let mut o = Object::new(Kind::Prompt { prompt: PromptSpec::default() }, at, [255, 205, 60], Material::Neon);
    o.collide = false;
    o
}

impl AnvilApp {
    /// Teleports and targets players can be sent to.
    pub(crate) fn teleport_dests(&self) -> Vec<(u64, String)> {
        self.map.objects.iter().filter(|o| matches!(o.kind, Kind::Teleport { .. } | Kind::Target {})).map(|o| (o.id, format!("{} · {}", o.name, o.type_label()))).collect()
    }

    fn new_teleport(&self, at: Vec3, dest: Option<u64>) -> Object {
        let s = &self.settings;
        let size = Vec3::new(5.0, 0.4, 5.0);
        let mut o = Object::new(
            Kind::Teleport { size, touch: s.teleport_touch, dest, cooldown: s.teleport_cooldown, exit_offset: Vec3::ZERO, prompt: PromptSpec { action_text: "Teleport".into(), ..Default::default() } },
            at + Vec3::Y * size.y * 0.5,
            [150, 90, 235],
            Material::Neon,
        );
        o.transparency = 0.25;
        o
    }

    /// Teleport tool: first click places the pad, the second its destination
    /// (a linked pad back if two-way is on, otherwise a target point).
    pub(crate) fn teleport_click(&mut self, at: Vec3) {
        match self.tp_pending.take().filter(|id| self.map.get(*id).is_some()) {
            None => {
                self.push_undo();
                let mut o = self.new_teleport(at, None);
                o.name = format!("Teleport_{}", self.map.next_id + 1);
                let id = self.map.add(o);
                self.selection = vec![id];
                self.tp_pending = Some(id);
                self.status = "Teleport placed. Now click where it should send players (Esc = decide later).".into();
            }
            Some(a) => {
                self.push_undo();
                let b = if self.settings.teleport_two_way {
                    let mut o = self.new_teleport(at, Some(a));
                    o.name = format!("Teleport_{}", self.map.next_id + 1);
                    self.map.add(o)
                } else {
                    let mut o = Object::new(Kind::Target {}, at + Vec3::Y * 0.5, [150, 90, 235], Material::Neon);
                    o.collide = false;
                    o.name = format!("Target_{}", self.map.next_id + 1);
                    self.map.add(o)
                };
                if let Some(Object { kind: Kind::Teleport { dest, .. }, .. }) = self.map.get_mut(a) {
                    *dest = Some(b);
                }
                self.selection = vec![a, b];
                self.status = if self.settings.teleport_two_way { "Linked two teleports (both ways)".into() } else { "Teleport linked to its target".into() };
            }
        }
    }

    pub(crate) fn teleport_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let mut ch = false;
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ch |= ui.selectable_value(&mut s.teleport_touch, true, "Touch pad").on_hover_text("Touching the pad teleports the player").changed();
            ch |= ui.selectable_value(&mut s.teleport_touch, false, "Prompt").on_hover_text("A ProximityPrompt (press E) teleports the player").changed();
        });
        ch |= ui.checkbox(&mut s.teleport_two_way, "Two-way (the second click makes a pad back)").changed();
        ui.horizontal(|ui| {
            ui.label("Cooldown");
            ch |= ui.add(egui::DragValue::new(&mut s.teleport_cooldown).speed(0.1).range(0.0..=600.0).suffix(" s")).changed();
        });
        ch
    }

    /// Teleport → destination lines and prompt labels, 3D view.
    pub(crate) fn paint_entities_3d(&self, painter: &Painter, rect: Rect) {
        let fov = self.settings.fov;
        let col = Color32::from_rgb(170, 110, 255);
        for o in self.map.objects.iter().filter(|o| !o.hidden) {
            match &o.kind {
                Kind::Teleport { dest: Some(d), .. } => {
                    if let Some(t) = self.map.get(*d).filter(|t| !t.hidden) {
                        let (a, b) = (o.pos + Vec3::Y, t.pos + Vec3::Y);
                        if let Some((p, q)) = self.cam.project_segment(rect, fov, a, b) {
                            painter.line_segment([p, q], Stroke::new(1.5f32, col));
                            let dir = (q - p).normalized();
                            let n = vec2(-dir.y, dir.x);
                            let tip = p + (q - p) * 0.55;
                            painter.line_segment([tip, tip - dir * 9.0 + n * 5.0], Stroke::new(1.5f32, col));
                            painter.line_segment([tip, tip - dir * 9.0 - n * 5.0], Stroke::new(1.5f32, col));
                        }
                    }
                }
                Kind::Prompt { prompt } => {
                    if let Some(p) = self.cam.project(rect, fov, o.pos + Vec3::Y * 1.2) {
                        let txt = format!("[{}] {}", prompt.key, prompt.action_text);
                        let r = painter.text(p, Align2::CENTER_BOTTOM, &txt, FontId::proportional(12.0), Color32::from_rgb(255, 225, 120));
                        painter.rect_stroke(r.expand(3.0), 3.0, Stroke::new(1.0f32, Color32::from_rgb(255, 205, 60)), egui::StrokeKind::Outside);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn paint_entities_2d(&self, painter: &Painter, view: &View2D, rect: Rect) {
        let col = Color32::from_rgb(170, 110, 255);
        for o in self.map.objects.iter().filter(|o| !o.hidden) {
            if let Kind::Teleport { dest: Some(d), .. } = &o.kind {
                if let Some(t) = self.map.get(*d) {
                    let (p, q) = (view.world_to_screen(rect, o.pos), view.world_to_screen(rect, t.pos));
                    painter.line_segment([p, q], Stroke::new(1.2f32, col));
                }
            }
        }
    }
}
