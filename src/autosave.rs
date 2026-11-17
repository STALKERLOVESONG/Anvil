//! Autosave that can't lose work: each unsaved (untitled) session writes its
//! own file (they used to share one, so starting a new map overwrote the last
//! one's autosave within minutes), and on start Anvil offers to reopen the
//! newest autosave when it's newer than the last map you saved.

use crate::app::AnvilApp;
use crate::map::Map;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many untitled sessions' autosaves are kept.
const KEEP_UNTITLED: usize = 15;

pub fn dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from(".")).join("autosave")
}

/// A name safe for a file.
fn clean(s: &str) -> String {
    let c: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if c.is_empty() { "map".into() } else { c }
}

/// The autosave file for a map: a saved map's own (by its file name), or
/// for an untitled one, one per session (`session`: when Anvil started).
pub fn file_for(dir: &Path, saved_as: Option<&Path>, map_name: &str, session: u64) -> PathBuf {
    match saved_as.and_then(|p| p.file_stem()).and_then(|s| s.to_str()) {
        Some(stem) => dir.join(format!("{}_autosave.anvil", clean(stem))),
        None => dir.join(format!("{}-{session}_autosave.anvil", clean(map_name))),
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Untitled sessions' autosaves, newest first.
fn untitled_files(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let mut v: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_autosave.anvil") && n.contains('-')))
        .filter_map(|p| modified(&p).map(|t| (t, p)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// Removes all but the newest `KEEP_UNTITLED` untitled-session autosaves.
pub fn prune(dir: &Path) {
    for (_, p) in untitled_files(dir).into_iter().skip(KEEP_UNTITLED) {
        let _ = std::fs::remove_file(p);
    }
}

/// The newest autosave, if it's newer than every map in `recent` (so it
/// holds work that was never saved to a map file).
pub fn unsaved_work(dir: &Path, recent: &[String]) -> Option<(PathBuf, SystemTime)> {
    let newest = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_autosave.anvil")))
        .filter_map(|p| modified(&p).map(|t| (p, t)))
        .max_by_key(|(_, t)| *t)?;
    let last_saved = recent.iter().filter_map(|r| modified(Path::new(r))).max();
    match last_saved {
        Some(s) if s >= newest.1 => None,
        _ => Some(newest),
    }
}

/// "5 minutes ago", "3 hours ago", "2 days ago".
pub fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} minutes ago", (s + 30) / 60),
        5400..=129_599 => format!("{} hours ago", (s + 1800) / 3600),
        _ => format!("{} days ago", (s + 43_200) / 86_400),
    }
}

impl AnvilApp {
    pub(crate) fn autosave_now(&mut self, now: f64) {
        self.last_autosave = now;
        let dir = dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let p = file_for(&dir, self.path.as_deref(), &self.map.name, self.session_id);
        if self.map.save(&p).is_ok() {
            self.status = format!("Autosaved to {}", p.display());
            if self.path.is_none() {
                prune(&dir);
            }
        }
    }

    /// The "reopen your unsaved work?" bar shown at start.
    pub(crate) fn restore_banner(&mut self, ctx: &egui::Context) {
        let Some((path, when)) = self.restore_offer.clone() else { return };
        let mut act = 0;
        egui::TopBottomPanel::top("restore_banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("You have unsaved work from {} (autosave).", ago(when))).color(Color32::from_rgb(255, 200, 90)));
                if ui.button("Open it").on_hover_text(path.display().to_string()).clicked() {
                    act = 1;
                }
                if ui.button("Open autosave folder").clicked() {
                    act = 2;
                }
                if ui.small_button("Dismiss").clicked() {
                    act = 3;
                }
            });
        });
        match act {
            1 => {
                if !self.confirm_discard() {
                    return;
                }
                self.restore_offer = None;
                match Map::load(&path) {
                    Ok(mut m) => {
                        crate::trim::refit_old_slabs(&mut m);
                        self.map = m;
                        self.reset_after_load();
                        // as an unsaved map: Ctrl+S asks where to keep it
                        self.path = None;
                        self.dirty = true;
                        self.status = "Reopened your unsaved work. Press Ctrl+S to save it as a map.".into();
                    }
                    Err(e) => self.status = format!("Couldn't open the autosave: {e}"),
                }
            }
            2 => {
                let _ = std::process::Command::new("explorer").arg(dir()).spawn();
            }
            3 => self.restore_offer = None,
            _ => {}
        }
    }
}
