//! Update check: looks at the latest GitHub release of the repo set in
//! Settings, offers it if it's newer, and installs it after checking its
//! SHA-256. Downloads only come from GitHub over HTTPS, the hash must be
//! published with the release, and the old exe is kept as anvil.old.exe.

use crate::app::AnvilApp;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where official releases are published.
pub const DEFAULT_REPO: &str = "STALKERLOVESONG/Anvil";
const MAX_ZIP: u64 = 300 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub zip_url: String,
    pub zip_name: String,
    pub sha256: Option<String>,
    pub sha_url: Option<String>,
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut it = s.split(['.', '-', '+']).map(|p| p.parse::<u32>());
    Some((it.next()?.ok()?, it.next().and_then(|x| x.ok()).unwrap_or(0), it.next().and_then(|x| x.ok()).unwrap_or(0)))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Only GitHub, only HTTPS.
pub fn allowed_url(u: &str) -> bool {
    ["https://github.com/", "https://api.github.com/", "https://objects.githubusercontent.com/", "https://release-assets.githubusercontent.com/"].iter().any(|p| u.starts_with(p))
}

pub fn valid_repo(r: &str) -> bool {
    let mut parts = r.split('/');
    let ok = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.len() <= 100 && !p.starts_with('.') && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// The first 64-hex-digit word in some text (a SHA-256).
pub fn find_sha256(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_hexdigit()).find(|w| w.len() == 64).map(|w| w.to_ascii_lowercase())
}

/// Reads GitHub's "latest release" JSON.
pub fn parse_release(v: &serde_json::Value) -> Option<Release> {
    let version = v.get("tag_name")?.as_str()?.trim_start_matches(['v', 'V']).to_string();
    let page = v.get("html_url").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let assets = v.get("assets")?.as_array()?;
    let zip = assets.iter().find(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.to_ascii_lowercase().ends_with(".zip") && n.to_ascii_lowercase().contains("anvil")))?;
    let zip_name = zip.get("name")?.as_str()?.to_string();
    let zip_url = zip.get("browser_download_url")?.as_str()?.to_string();
    if !allowed_url(&zip_url) {
        return None;
    }
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let sha256 = body.lines().find(|l| l.contains(&zip_name) || l.to_ascii_lowercase().contains("sha")).and_then(find_sha256).or_else(|| find_sha256(body));
    let sha_url = assets
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(&format!("{zip_name}.sha256"))))
        .and_then(|a| a.get("browser_download_url")?.as_str().map(|s| s.to_string()))
        .filter(|u| allowed_url(u));
    Some(Release { version, page, zip_url, zip_name, sha256, sha_url })
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !allowed_url(url) {
        return Err("Refusing to download from outside GitHub".into());
    }
    let resp = ureq::get(url).set("User-Agent", "Anvil-updater").set("Accept", "application/vnd.github+json").timeout(std::time::Duration::from_secs(60)).call().map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !allowed_url(resp.get_url()) {
        return Err("GitHub redirected somewhere unexpected; not downloading".into());
    }
    let mut buf = Vec::new();
    resp.into_reader().take(limit + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > limit {
        return Err("The download is bigger than expected".into());
    }
    Ok(buf)
}

/// The newest release if it's newer than this one.
pub fn check(repo: &str) -> Result<Option<Release>, String> {
    if !valid_repo(repo) {
        return Err("Set the update repository as owner/name".into());
    }
    let bytes = get(&format!("https://api.github.com/repos/{repo}/releases/latest"), 4 * 1024 * 1024)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "GitHub sent something unexpected".to_string())?;
    let rel = parse_release(&v).ok_or("The latest release has no Anvil .zip")?;
    Ok(is_newer(&rel.version, CURRENT).then_some(rel))
}

/// Pulls anvil.exe out of a release zip after checking the zip's hash.
pub fn verify_and_extract(zip: &[u8], expected_sha: &str) -> Result<Vec<u8>, String> {
    let got = crate::secure::hex(&crate::secure::sha256(zip));
    if !got.eq_ignore_ascii_case(expected_sha.trim()) {
        return Err(format!("The download doesn't match its published SHA-256 (expected {}, got {got}); not installing", expected_sha.trim()));
    }
    let mut za = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| format!("Bad update zip: {e}"))?;
    for i in 0..za.len() {
        let mut f = za.by_index(i).map_err(|e| e.to_string())?;
        let is_exe = f.enclosed_name().is_some_and(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("anvil.exe")));
        if is_exe {
            let mut exe = Vec::new();
            (&mut f).take(MAX_ZIP).read_to_end(&mut exe).map_err(|e| e.to_string())?;
            if exe.len() < 2 || &exe[..2] != b"MZ" {
                return Err("The update's anvil.exe isn't a Windows program".into());
            }
            return Ok(exe);
        }
    }
    Err("The update zip has no anvil.exe".into())
}

/// Downloads, checks and swaps in the new exe. Returns its path.
pub fn install(rel: &Release) -> Result<PathBuf, String> {
    let expected = match (&rel.sha256, &rel.sha_url) {
        (Some(s), _) => s.clone(),
        (None, Some(u)) => find_sha256(&String::from_utf8_lossy(&get(u, 4096)?)).ok_or("The release's .sha256 file is empty")?,
        (None, None) => return Err("This release doesn't publish a SHA-256, so it can't be checked. Download it by hand from the release page instead.".into()),
    };
    let zip = get(&rel.zip_url, MAX_ZIP)?;
    let exe = verify_and_extract(&zip, &expected)?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = me.parent().ok_or("Can't find where Anvil is")?;
    let new = dir.join("anvil.new.exe");
    std::fs::write(&new, exe).map_err(|e| format!("Couldn't write the update: {e}"))?;
    let old = dir.join("anvil.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&me, &old).map_err(|e| format!("Couldn't move the old version aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &me) {
        let _ = std::fs::rename(&old, &me); // put it back
        return Err(format!("Couldn't put the new version in place: {e}"));
    }
    Ok(me)
}

pub(crate) enum UpdState {
    Idle,
    Checking(Receiver<Result<Option<Release>, String>>),
    Available(Release),
    Installing(Receiver<Result<PathBuf, String>>),
    Ready(PathBuf),
    Failed(String),
    UpToDate,
}

impl Default for UpdState {
    fn default() -> Self {
        UpdState::Idle
    }
}

impl AnvilApp {
    pub(crate) fn start_update_check(&mut self) {
        let repo = self.settings.update_repo.trim().to_string();
        if repo.is_empty() {
            self.update = UpdState::Failed("No update repository set (Settings > Updates)".into());
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check(&repo));
        });
        self.update = UpdState::Checking(rx);
    }

    pub(crate) fn poll_update(&mut self, ctx: &eframe::egui::Context) {
        if !self.update_checked {
            self.update_checked = true;
            if self.settings.check_updates && !self.settings.update_repo.trim().is_empty() {
                self.start_update_check();
            }
        }
        let next = match &self.update {
            UpdState::Checking(rx) => match rx.try_recv() {
                Ok(Ok(Some(r))) => Some(UpdState::Available(r)),
                Ok(Ok(None)) => Some(UpdState::UpToDate),
                Ok(Err(e)) => Some(UpdState::Failed(e)),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                    None
                }
                Err(_) => Some(UpdState::Idle),
            },
            UpdState::Installing(rx) => match rx.try_recv() {
                Ok(Ok(p)) => Some(UpdState::Ready(p)),
                Ok(Err(e)) => Some(UpdState::Failed(e)),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                    None
                }
                Err(_) => Some(UpdState::Idle),
            },
            _ => None,
        };
        if let Some(n) = next {
            self.update = n;
        }
    }

    /// The "new version" banner under the menu bar.
    pub(crate) fn update_banner(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let show = matches!(self.update, UpdState::Available(_) | UpdState::Installing(_) | UpdState::Ready(_)) && !self.update_hidden;
        if !show {
            return;
        }
        let mut act = 0;
        egui::TopBottomPanel::top("update_banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &self.update {
                    UpdState::Available(r) => {
                        ui.label(RichText::new(format!("You're on an outdated version ({CURRENT} → {})", r.version)).color(Color32::from_rgb(255, 200, 90)));
                        if ui.button("Update").on_hover_text("Downloads the release from GitHub, checks its SHA-256, and keeps the old version as anvil.old.exe").clicked() {
                            act = 1;
                        }
                        if !r.page.is_empty() {
                            ui.hyperlink_to("Release notes", &r.page);
                        }
                    }
                    UpdState::Installing(_) => {
                        ui.spinner();
                        ui.label("Downloading and checking the update…");
                    }
                    UpdState::Ready(_) => {
                        ui.label(RichText::new("Update installed.").color(Color32::from_rgb(140, 220, 140)));
                        if ui.button("Restart now").clicked() {
                            act = 2;
                        }
                    }
                    _ => {}
                }
                if ui.small_button("Later").clicked() {
                    act = 3;
                }
            });
        });
        match act {
            1 => {
                if let UpdState::Available(r) = &self.update {
                    let r = r.clone();
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(install(&r));
                    });
                    self.update = UpdState::Installing(rx);
                }
            }
            2 => {
                if let UpdState::Ready(p) = &self.update {
                    if self.dirty {
                        self.status = "Save your map first, then restart".into();
                    } else if std::process::Command::new(p).spawn().is_ok() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            3 => self.update_hidden = true,
            _ => {}
        }
    }

    /// Update section for the About window.
    pub(crate) fn update_about_ui(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{self, RichText};
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Updates from GitHub repo");
            if ui.add(egui::TextEdit::singleline(&mut self.settings.update_repo).hint_text("owner/name").desired_width(150.0)).changed() {
                let _ = self.settings.save();
            }
        });
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.settings.check_updates, "Check when Anvil starts").changed() {
                let _ = self.settings.save();
            }
            if ui.button("Check now").clicked() {
                self.update_hidden = false;
                self.start_update_check();
            }
        });
        let msg = match &self.update {
            UpdState::Checking(_) => "Checking…".to_string(),
            UpdState::UpToDate => format!("You're up to date ({CURRENT})."),
            UpdState::Failed(e) => e.clone(),
            UpdState::Available(r) => format!("{} is available.", r.version),
            _ => String::new(),
        };
        if !msg.is_empty() {
            ui.label(RichText::new(msg).small());
        }
    }
}
