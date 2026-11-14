//! Easier hiding: eye icons, Hide / Isolate buttons, and the right-click menu
//! in the viewports.

use crate::app::AnvilApp;
use crate::mesh::WorldMesh;
use crate::view2d;
use eframe::egui::{self, pos2, vec2, Color32, Rect, Response, Sense, Shape, Stroke};
use std::sync::Arc;

/// A small drawn eye (crossed out when hidden). Click toggles.
pub(crate) fn eye_button(ui: &mut egui::Ui, visible: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 14.0), Sense::click());
    let col = if resp.hovered() { Color32::WHITE } else if visible { Color32::from_gray(200) } else { Color32::from_gray(95) };
    let p = ui.painter();
    let c = rect.center();
    let (w, h) = (7.0, 4.2);
    let mut pts = Vec::new();
    for i in 0..=12 {
        let t = i as f32 / 12.0;
        let x = -w + 2.0 * w * t;
        pts.push(pos2(c.x + x, c.y - h * (1.0 - (x / w).powi(2))));
    }
    for i in (1..12).rev() {
        let t = i as f32 / 12.0;
        let x = -w + 2.0 * w * t;
        pts.push(pos2(c.x + x, c.y + h * (1.0 - (x / w).powi(2))));
    }
    p.add(Shape::closed_line(pts, Stroke::new(1.3f32, col)));
    if visible {
        p.circle_filled(c, 2.3, col);
    } else {
        p.line_segment([pos2(c.x - 7.0, c.y + 5.0), pos2(c.x + 7.0, c.y - 5.0)], Stroke::new(1.5f32, col));
    }
    resp.on_hover_text(if visible { "Visible (click to hide)" } else { "Hidden (click to show)" })
}

impl AnvilApp {
    /// Hide everything that isn't selected.
    pub(crate) fn hide_unselected(&mut self) {
        if self.selection.is_empty() {
            self.status = "Select what you want to keep visible, then isolate".into();
            return;
        }
        self.push_undo();
        let keep = self.selection.clone();
        let mut n = 0;
        for o in &mut self.map.objects {
            if !keep.contains(&o.id) && !o.hidden {
                o.hidden = true;
                n += 1;
            }
        }
        self.status = format!("Isolated the selection ({n} other object(s) hidden; U shows everything again)");
    }

    /// Hide / show the selection (shows it if all of it is hidden).
    pub(crate) fn toggle_hide_selection(&mut self) {
        let all_hidden = self.selection.iter().all(|id| self.map.get(*id).is_some_and(|o| o.hidden));
        if all_hidden {
            self.push_undo();
            for id in self.selection.clone() {
                if let Some(o) = self.map.get_mut(id) {
                    o.hidden = false;
                }
            }
        } else {
            self.hide_selection();
        }
    }

    /// Hide / Isolate row at the top of the Inspector. True if it changed the selection.
    pub(crate) fn inspector_hide_row(&mut self, ui: &mut egui::Ui) -> bool {
        let all_hidden = self.selection.iter().all(|id| self.map.get(*id).is_some_and(|o| o.hidden));
        let mut act = 0;
        ui.horizontal(|ui| {
            if ui.small_button(if all_hidden { "Show" } else { "Hide" }).on_hover_text("H hides the selection, U shows everything").clicked() {
                act = 1;
            }
            if ui.small_button("Isolate").on_hover_text("Hide everything else (Shift+H)").clicked() {
                act = 2;
            }
            if ui.small_button("Unhide all").on_hover_text("U").clicked() {
                act = 3;
            }
        });
        match act {
            1 => {
                self.toggle_hide_selection();
                true
            }
            2 => {
                self.hide_unselected();
                false
            }
            3 => {
                self.unhide_all();
                false
            }
            _ => false,
        }
    }

    /// Right-click in a viewport: select what's under the mouse (if it isn't already).
    pub(crate) fn context_pick_3d(&mut self, rect: Rect, resp: &Response, scene: &[Arc<WorldMesh>]) {
        if !resp.secondary_clicked() || self.edit.is_some() {
            return;
        }
        let Some(p) = resp.interact_pointer_pos() else { return };
        let (o, d) = self.cam.ray(rect, self.settings.fov, p);
        if let Some(h) = crate::view3d::pick_ray(scene, o, d) {
            if !self.selection.contains(&h.id) {
                self.selection = vec![h.id];
            }
        }
    }

    pub(crate) fn context_pick_2d(&mut self, vi: usize, rect: Rect, resp: &Response, scene: &[Arc<WorldMesh>]) {
        if !resp.secondary_clicked() || self.edit.is_some() {
            return;
        }
        let Some(p) = resp.interact_pointer_pos() else { return };
        if let Some(id) = view2d::pick(&self.views[vi], rect, scene, p) {
            if !self.selection.contains(&id) {
                self.selection = vec![id];
            }
        }
    }

    /// The right-click menu itself.
    pub(crate) fn object_context_menu(&mut self, ui: &mut egui::Ui) {
        if self.edit.is_some() {
            ui.close_menu();
            return;
        }
        let has = !self.selection.is_empty();
        ui.label(egui::RichText::new(format!("{} selected", self.selection.len())).weak());
        if ui.add_enabled(has, egui::Button::new("Hide").shortcut_text("H")).clicked() {
            ui.close_menu();
            self.hide_selection();
        }
        if ui.add_enabled(has, egui::Button::new("Hide everything else").shortcut_text("Shift+H")).clicked() {
            ui.close_menu();
            self.hide_unselected();
        }
        if ui.add(egui::Button::new("Unhide all").shortcut_text("U")).clicked() {
            ui.close_menu();
            self.unhide_all();
        }
        ui.separator();
        if ui.add_enabled(has, egui::Button::new("Focus").shortcut_text("F")).clicked() {
            ui.close_menu();
            self.focus();
        }
        if ui.add_enabled(has, egui::Button::new("Duplicate").shortcut_text("Ctrl+D")).clicked() {
            ui.close_menu();
            self.push_undo();
            self.duplicate(glam::Vec3::ZERO);
        }
        if ui.add_enabled(has && self.active_texture.is_some(), egui::Button::new("Apply active texture")).clicked() {
            ui.close_menu();
            self.push_undo();
            for id in self.selection.clone() {
                if let Some(o) = self.map.get_mut(id) {
                    o.texture = self.active_texture;
                }
            }
        }
        if ui.add_enabled(has, egui::Button::new("Delete").shortcut_text("Del")).clicked() {
            ui.close_menu();
            self.delete_selection();
        }
    }
}
