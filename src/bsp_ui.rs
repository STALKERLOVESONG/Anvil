//! File > Import Source map: reads a .bsp on a background thread (with
//! progress), then adds it to the open map as one undo step.

use crate::app::AnvilApp;
use crate::bsp::{self, Imported, Report};
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub(crate) struct BspJob {
    rx: Receiver<Result<Imported, String>>,
    progress: Arc<Mutex<String>>,
    name: String,
}

/// What the last import brought in (and what it couldn't find).
pub(crate) struct BspReport {
    name: String,
    report: Report,
}

impl AnvilApp {
    pub(crate) fn import_bsp_dialog(&mut self) {
        if self.bsp_job.is_some() {
            self.status = "A map is already being imported".into();
            return;
        }
        let Some(path) = rfd::FileDialog::new().set_title("Import a Source engine map").add_filter("Source map (Garry's Mod, Half-Life 2)", &["bsp"]).pick_file() else { return };
        self.import_bsp(path);
    }

    pub(crate) fn import_bsp(&mut self, path: PathBuf) {
        if crate::texture::is_network_path(&path.to_string_lossy()) {
            self.status = "Copy the map to this PC first (network files aren't opened)".into();
            return;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("map").to_string();
        let progress = Arc::new(Mutex::new("Starting…".to_string()));
        let (tx, rx) = std::sync::mpsc::channel();
        let mounts = self.mounts.clone();
        let mount_paths = self.settings.mounts.clone();
        let opts = bsp::Options { scale: self.settings.source_scale, spawn_size: self.settings.spawn_size, props: true };
        let p2 = progress.clone();
        std::thread::spawn(move || {
            let say = |s: String| {
                if let Ok(mut g) = p2.lock() {
                    *g = s;
                }
            };
            // the game content (textures, models) the map uses
            let built;
            let m = match &mounts {
                Some(m) => Some(&**m),
                None if !mount_paths.is_empty() => {
                    say("Loading mounted game content…".into());
                    built = crate::mount::Mounts::build(&mount_paths);
                    Some(&built)
                }
                None => None,
            };
            let r = bsp::import(&path, m, &opts, &say);
            let _ = tx.send(r);
        });
        self.status = format!("Importing {name}…");
        self.bsp_job = Some(BspJob { rx, progress, name });
    }

    pub(crate) fn poll_bsp_import(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.bsp_job else { return };
        match job.rx.try_recv() {
            Ok(Ok(imp)) => {
                let name = job.name.clone();
                self.bsp_job = None;
                let report = imp.report.clone();
                for (o, _) in &imp.objects {
                    if let crate::map::Kind::Prop { model, .. } = &o.kind {
                        self.models.forget(model);
                    }
                }
                self.push_undo();
                let ids = bsp::apply(&mut self.map, imp);
                self.scene_cache.clear();
                // look at what came in, without leaving it all selected
                self.selection = ids.iter().copied().filter(|id| self.map.get(*id).is_some_and(|o| !o.hidden)).collect();
                self.focus();
                self.selection.clear();
                self.status = format!("Imported {name}: {}", report.summary());
                self.bsp_report = Some(BspReport { name, report });
            }
            Ok(Err(e)) => {
                self.status = format!("Couldn't import {}: {e}", job.name);
                self.bsp_job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let text = job.progress.lock().map(|g| g.clone()).unwrap_or_default();
                let name = job.name.clone();
                egui::Window::new("Importing map").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(name).strong());
                    });
                    ui.label(text);
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                self.status = "The map import stopped unexpectedly".into();
                self.bsp_job = None;
            }
        }
    }

    pub(crate) fn bsp_report_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.bsp_report else { return };
        let mut open = true;
        egui::Window::new(format!("Imported {}", r.name)).open(&mut open).default_width(420.0).show(ctx, |ui| {
            let rep = &r.report;
            ui.label(format!("{} brushes ({} faces) — each one is an editable mesh (Tab to edit)", rep.brushes, rep.faces));
            if rep.displacements > 0 {
                ui.label(format!("{} terrain pieces", rep.displacements));
            }
            ui.label(format!("{} brush entities, {} props, {} lights, {} spawns, {} textures", rep.brush_entities, rep.props, rep.lights, rep.spawns, rep.textures));
            if rep.hidden + rep.skybox > 0 {
                ui.label(RichText::new(format!("{} clips / invisible walls / 3D skybox pieces are in hidden folders", rep.hidden + rep.skybox)).weak());
            }
            if !rep.missing_materials.is_empty() || !rep.missing_models.is_empty() {
                ui.separator();
                ui.label(RichText::new("Not found in your mounted content").strong());
                ui.label(
                    RichText::new("Mount the game they come from (File > Half-Life / Source content), then import again. Brushes without their texture show plain grey; missing props are left out.")
                        .small()
                        .weak(),
                );
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for m in &rep.missing_materials {
                        ui.label(RichText::new(format!("texture  {m}")).monospace().small());
                    }
                    for m in &rep.missing_models {
                        ui.label(RichText::new(format!("model    {m}")).monospace().small());
                    }
                });
            }
            for w in &rep.warnings {
                ui.label(RichText::new(w).color(egui::Color32::from_rgb(230, 170, 60)));
            }
        });
        if !open {
            self.bsp_report = None;
        }
    }
}
