//! Tool settings panel: the active tool's options in one docked box under
//! the tool list (instead of buried in the big Settings window).

use crate::app::{material_combo, vec2_edit, vec3_edit, AnvilApp, Tool};
use eframe::egui::{self, RichText};

impl AnvilApp {
    pub(crate) fn tool_settings_panel(&mut self, ui: &mut egui::Ui, hint: &str) {
        egui::Frame::group(ui.style()).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = match self.tool {
                Tool::Plugin(i) => self.plugins.plugins.get(i).map(|p| p.name.clone()).unwrap_or_default(),
                t => t.label().to_string(),
            };
            ui.label(RichText::new(format!("{title} settings")).strong());
            ui.label(RichText::new(hint).small().weak());
            let mut ch = false;
            match self.tool {
                Tool::Wall | Tool::Layout => ch |= self.building_options(ui),
                Tool::Door | Tool::Window => ch |= self.opening_options(ui),
                Tool::Trim => {
                    let s = &mut self.settings;
                    egui::Grid::new("tp_trim").num_columns(2).spacing([6.0, 3.0]).show(ui, |ui| {
                        ui.label("Height");
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_height).speed(0.05).range(0.05..=50.0)).changed();
                        ui.end_row();
                        ui.label("Sticks out");
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_depth).speed(0.02).range(0.02..=10.0)).changed();
                        ui.end_row();
                        ui.label("Color");
                        ch |= ui.color_edit_button_srgb(&mut s.trim_color).changed();
                        ui.end_row();
                    });
                    ui.label(RichText::new("Click near the bottom of a wall for a baseboard, near the top for a crown, anywhere between for a stripe. Doors and windows cut through it.").small().weak());
                }
                Tool::Polygon => {
                    ui.horizontal(|ui| {
                        ui.label("Start height");
                        ch |= ui.add(egui::DragValue::new(&mut self.settings.polygon_height).speed(0.1).range(0.01..=10000.0)).changed();
                    });
                    ui.label(RichText::new("Drag for a rectangle, or click points (lines stay straight, Ctrl = any angle). After closing the outline, move the mouse to set the height.").small().weak());
                }
                Tool::Light => {
                    egui::Grid::new("tp_light").num_columns(2).show(ui, |ui| {
                        ui.label("Range");
                        ch |= ui.add(egui::DragValue::new(&mut self.settings.light_range).speed(0.25).range(0.0..=1000.0)).changed();
                        ui.end_row();
                        ui.label("Brightness");
                        ch |= ui.add(egui::DragValue::new(&mut self.settings.light_brightness).speed(0.05).range(0.0..=100.0)).changed();
                        ui.end_row();
                    });
                }
                Tool::Spawn => {
                    ui.horizontal(|ui| {
                        ui.label("Size");
                        ch |= vec3_edit(ui, &mut self.settings.spawn_size, 0.1, Some(0.01));
                    });
                }
                Tool::Decal => {
                    ui.horizontal(|ui| {
                        ui.label("Size");
                        ch |= vec2_edit(ui, &mut self.settings.decal_size, 0.1, Some(0.01), ["w", "h"]);
                    });
                }
                Tool::Knife => {
                    ui.horizontal(|ui| {
                        ch |= ui.selectable_value(&mut self.settings.knife_split, true, "Split").on_hover_text("Cut into two separate objects").changed();
                        ch |= ui.selectable_value(&mut self.settings.knife_split, false, "Slice").on_hover_text("Keep one object, just add an edge across its faces").changed();
                    });
                }
                Tool::Text => self.text_tool_panel(ui),
                Tool::Teleport => ch |= self.teleport_panel(ui),
                Tool::Stairs => self.stairs_panel(ui),
                Tool::Select => {
                    ui.horizontal(|ui| {
                        ui.label("Grid");
                        ch |= ui.add(egui::DragValue::new(&mut self.settings.grid).speed(0.05).range(0.01..=1024.0)).changed();
                        ch |= ui.checkbox(&mut self.settings.snap, "Snap").changed();
                    });
                    ch |= ui.checkbox(&mut self.settings.collisions, "Collisions").on_hover_text("Moving and rotating stop flush against other parts").changed();
                    ui.label(RichText::new("W / E / R: move, rotate, scale handles. X: local / global. The small squares move in one plane.").small().weak());
                }
                _ => {}
            }
            if ch {
                let _ = self.settings.save();
            }
        });
    }

    fn building_options(&mut self, ui: &mut egui::Ui) -> bool {
        let mut ch = false;
        let textures: Vec<(u64, String)> = self.map.textures.iter().map(|t| (t.id, t.name.clone())).collect();
        let s = &mut self.settings;
        egui::Grid::new("tp_build").num_columns(2).spacing([6.0, 3.0]).show(ui, |ui| {
            ui.label("Wall height");
            ch |= ui.add(egui::DragValue::new(&mut s.wall_height).speed(0.1).range(0.01..=10000.0)).changed();
            ui.end_row();
            ui.label("Thickness");
            ch |= ui.add(egui::DragValue::new(&mut s.wall_thickness).speed(0.05).range(0.01..=1000.0)).changed();
            ui.end_row();
            ui.label("Walls");
            ui.horizontal(|ui| {
                ch |= ui.color_edit_button_srgb(&mut s.wall_color).changed();
                ch |= material_combo(ui, "tp_wall_mat", &mut s.wall_material);
            });
            ui.end_row();
        });
        if self.tool == Tool::Wall {
            let s = &mut self.settings;
            ch |= ui.checkbox(&mut s.wall_trim, "Add trim to new walls").on_hover_text("Baseboard and/or crown strips on both sides of each wall you draw. Corners where walls meet join up.").changed();
            if s.wall_trim {
                egui::Grid::new("tp_wall_trim").num_columns(2).spacing([6.0, 3.0]).show(ui, |ui| {
                    ui.label("Trim");
                    ui.horizontal(|ui| {
                        ch |= ui.checkbox(&mut s.wall_trim_baseboard, "Baseboard").on_hover_text("Along the bottom of the wall").changed();
                        ch |= ui.checkbox(&mut s.wall_trim_crown, "Crown").on_hover_text("Along the top of the wall").changed();
                    });
                    ui.end_row();
                    ui.label("Trim size");
                    ui.horizontal(|ui| {
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_height).speed(0.05).range(0.05..=50.0).prefix("h ")).changed();
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_depth).speed(0.02).range(0.02..=10.0).prefix("d ")).changed();
                        ch |= ui.color_edit_button_srgb(&mut s.trim_color).changed();
                    });
                    ui.end_row();
                });
                if !s.wall_trim_baseboard && !s.wall_trim_crown {
                    ui.label(RichText::new("Pick Baseboard and/or Crown").small().weak());
                }
            }
        }
        if self.tool == Tool::Layout {
            let s = &mut self.settings;
            egui::Grid::new("tp_build2").num_columns(2).spacing([6.0, 3.0]).show(ui, |ui| {
                ui.label("Adds");
                ui.horizontal(|ui| {
                    ch |= ui.checkbox(&mut s.layout_floor, "Floor").changed();
                    ch |= ui.checkbox(&mut s.layout_ceiling, "Ceiling").changed();
                });
                ui.end_row();
                ui.label("Floor");
                ui.horizontal(|ui| {
                    ch |= ui.color_edit_button_srgb(&mut s.floor_color).changed();
                    ch |= material_combo(ui, "tp_floor_mat", &mut s.floor_material);
                    ch |= ui.add(egui::DragValue::new(&mut s.floor_thickness).speed(0.05).range(0.01..=1000.0).prefix("thick ")).changed();
                });
                ui.end_row();
                ui.label("Trim");
                ui.horizontal(|ui| {
                    ch |= ui.checkbox(&mut s.trim_baseboard, "Baseboard").on_hover_text("Strip along the bottom of the inside walls").changed();
                    ch |= ui.checkbox(&mut s.trim_crown, "Crown").on_hover_text("Molding along the top of the inside walls").changed();
                });
                ui.end_row();
                if s.trim_baseboard || s.trim_crown {
                    ui.label("Trim size");
                    ui.horizontal(|ui| {
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_height).speed(0.05).range(0.05..=50.0).prefix("h ")).changed();
                        ch |= ui.add(egui::DragValue::new(&mut s.trim_depth).speed(0.02).range(0.02..=10.0).prefix("d ")).changed();
                        ch |= ui.color_edit_button_srgb(&mut s.trim_color).changed();
                    });
                    ui.end_row();
                }
            });
        }
        // textures for new walls / floors (this map's textures)
        let tex_combo = |ui: &mut egui::Ui, id: &str, cur: &mut Option<u64>| -> bool {
            let name = cur.and_then(|c| textures.iter().find(|t| t.0 == c)).map(|t| t.1.clone()).unwrap_or_else(|| "(none)".into());
            let mut changed = false;
            egui::ComboBox::from_id_salt(id).selected_text(name).width(110.0).show_ui(ui, |ui| {
                changed |= ui.selectable_value(cur, None, "(none)").changed();
                for (tid, n) in &textures {
                    changed |= ui.selectable_value(cur, Some(*tid), n).changed();
                }
            });
            changed
        };
        if !textures.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Wall texture");
                tex_combo(ui, "tp_wall_tex", &mut self.build_wall_texture);
            });
            if self.tool == Tool::Layout {
                ui.horizontal(|ui| {
                    ui.label("Floor texture");
                    tex_combo(ui, "tp_floor_tex", &mut self.build_floor_texture);
                });
            }
        }
        ch
    }

    fn opening_options(&mut self, ui: &mut egui::Ui) -> bool {
        let mut ch = false;
        let s = &mut self.settings;
        egui::Grid::new("tp_open").num_columns(2).spacing([6.0, 3.0]).show(ui, |ui| {
            if self.tool == Tool::Door {
                ui.label("Door size");
                ui.horizontal(|ui| {
                    ch |= ui.add(egui::DragValue::new(&mut s.door_width).speed(0.1).range(0.1..=1000.0).prefix("w ")).changed();
                    ch |= ui.add(egui::DragValue::new(&mut s.door_height).speed(0.1).range(0.1..=1000.0).prefix("h ")).changed();
                });
                ui.end_row();
            } else {
                ui.label("Window size");
                ui.horizontal(|ui| {
                    ch |= ui.add(egui::DragValue::new(&mut s.window_width).speed(0.1).range(0.1..=1000.0).prefix("w ")).changed();
                    ch |= ui.add(egui::DragValue::new(&mut s.window_height).speed(0.1).range(0.1..=1000.0).prefix("h ")).changed();
                });
                ui.end_row();
                ui.label("Sill height");
                ch |= ui.add(egui::DragValue::new(&mut s.window_sill).speed(0.1).range(0.0..=1000.0)).changed();
                ui.end_row();
                ui.label("Glass");
                ch |= ui.checkbox(&mut s.window_glass, "Add a glass pane").changed();
                ui.end_row();
            }
        });
        ch
    }
}
