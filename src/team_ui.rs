//! Team Create in the editor: the Team window, other people's floating
//! cameras and their selections drawn in their color.

use crate::app::AnvilApp;
use crate::map::Object;
use crate::mesh::WorldMesh;
use crate::team::{self, Presence, Session};
use crate::view2d::View2D;
use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, FontId, Painter, Rect, RichText, Stroke, StrokeKind};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TeamUi {
    pub open: bool,
    pub join_code: String,
    pub message: String,
    pub pending_seen: usize,
    /// A room-code search in progress (the code, and where the answer comes).
    finding: Option<(String, std::sync::mpsc::Receiver<Result<std::net::SocketAddr, String>>)>,
}

enum Act {
    Host,
    Join,
    JoinAddr,
    Leave,
    Approve(u32),
    Deny(u32),
    Kick(u32),
}

/// How many object copies patching the undo history for others' edits may make.
pub(crate) const HISTORY_COPY_BUDGET: usize = 300_000;

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn pretty_code(c: &str) -> String {
    if c.len() == 8 { format!("{}-{}", &c[..4], &c[4..]) } else { c.to_string() }
}

impl AnvilApp {
    /// Network work for this frame (call once per frame).
    pub(crate) fn poll_team(&mut self, ctx: &egui::Context) {
        self.poll_room_search(ctx);
        let Some(s) = &mut self.team else { return };
        let tool = match self.tool {
            crate::app::Tool::Plugin(_) => "Plugin".to_string(),
            t => t.label().to_string(),
        };
        let presence = Presence { cam: self.cam.pos, look: self.cam.forward(), sel: self.selection.clone(), tool };
        let r = s.pump(&mut self.map, &presence);
        let pending = s.pending.len();
        let ended = s.ended.clone();
        if pending > self.team_ui.pending_seen {
            self.team_ui.open = true; // someone is knocking
        }
        self.team_ui.pending_seen = pending;

        for p in &r.block {
            self.textures.alias(p, None);
            self.models.alias(p, None);
            self.tex_previews.remove(p);
        }
        if !r.assets.is_empty() {
            for (p, local) in &r.assets {
                let l = local.to_string_lossy().to_string();
                self.textures.alias(p, Some(l.clone()));
                self.models.alias(p, Some(l));
                self.tex_previews.remove(p);
            }
            self.scene_cache.clear();
        }
        if r.map_replaced {
            self.undo.clear();
            self.redo.clear();
            self.selection.clear();
            self.face_sel.clear();
            self.edit = None;
            self.path = None;
            self.dirty = false;
            self.scene_cache.clear();
            self.status = "Joined the Team Create session".into();
        } else if !r.changed.is_empty() || !r.removed.is_empty() || r.meta_changed {
            self.patch_history(r.changed, &r.removed, r.meta_changed);
            self.prune_selection();
            self.dirty = true;
        }
        if let Some(why) = ended {
            team::end_ranges(&mut self.map);
            self.team = None;
            self.team_ui.message = why.clone();
            self.status = why;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// A room-code search finished: connect to the host it found.
    fn poll_room_search(&mut self, ctx: &egui::Context) {
        let Some((code, rx)) = &self.team_ui.finding else { return };
        match rx.try_recv() {
            Ok(Ok(addr)) => {
                let code = code.clone();
                self.team_ui.finding = None;
                match Session::join(&addr.to_string(), &code, &self.settings.team_name, self.settings.team_color, team::default_cache_dir()) {
                    Ok(s) => self.team = Some(s),
                    Err(e) => self.team_ui.message = format!("Found the room but couldn't connect to it: {e}"),
                }
            }
            Ok(Err(e)) => {
                self.team_ui.finding = None;
                self.team_ui.message = e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(_) => self.team_ui.finding = None,
        }
    }

    /// Someone else's change shouldn't come back when you press undo: put
    /// their version into every undo / redo snapshot too.
    pub(crate) fn patch_history(&mut self, mut changed: Vec<u64>, removed: &[u64], meta: bool) {
        changed.sort_unstable();
        changed.dedup();
        let cur: Vec<Object> = changed.iter().filter_map(|id| self.map.get(*id).cloned()).collect();
        // a huge change (someone imported a whole map) would be copied into
        // every undo step; keep only as many recent steps as stay cheap
        let steps = self.undo.len() + self.redo.len();
        if cur.len() * steps > HISTORY_COPY_BUDGET {
            let keep = (HISTORY_COPY_BUDGET / cur.len().max(1)).max(1);
            if self.undo.len() > keep {
                let cut = self.undo.len() - keep;
                self.undo.drain(..cut);
            }
            self.redo.truncate(keep.saturating_sub(self.undo.len()));
        }
        let (name, folders, textures) = (self.map.name.clone(), self.map.folders.clone(), self.map.textures.clone());
        for snap in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            if cur.len() <= 16 {
                for o in &cur {
                    match snap.objects.iter().position(|x| x.id == o.id) {
                        Some(i) => snap.objects[i] = o.clone(),
                        None => snap.objects.push(o.clone()),
                    }
                }
            } else {
                let idx: HashMap<u64, usize> = snap.objects.iter().enumerate().map(|(i, o)| (o.id, i)).collect();
                for o in &cur {
                    match idx.get(&o.id) {
                        Some(&i) => snap.objects[i] = o.clone(),
                        None => snap.objects.push(o.clone()),
                    }
                }
            }
            if !removed.is_empty() {
                snap.objects.retain(|o| !removed.contains(&o.id));
            }
            if meta {
                snap.name = name.clone();
                snap.folders = folders.clone();
                snap.textures = textures.clone();
            }
        }
    }

    pub(crate) fn team_remote_sel(&self) -> HashMap<u64, [u8; 3]> {
        self.team.as_ref().map(|s| s.remote_selection()).unwrap_or_default()
    }

    /// Other people's cameras floating in the 3D view.
    pub(crate) fn paint_team_3d(&self, painter: &Painter, rect: Rect) {
        let Some(s) = &self.team else { return };
        let fov = self.settings.fov;
        for r in s.peers.values() {
            let Some(p) = &r.presence else { continue };
            let fwd = self.cam.forward();
            if !self.cam.is_ortho() && (p.cam - self.cam.pos).dot(fwd) < 0.3 {
                continue;
            }
            let Some(c) = self.cam.project(rect, fov, p.cam) else { continue };
            if !rect.expand(40.0).contains(c) {
                continue;
            }
            let upp = self.cam.units_per_pixel(rect, fov, p.cam).max(1e-4);
            let rad = (1.1 / upp).clamp(5.0, 60.0);
            let col = c32(r.info.color);
            if let Some(tip) = self.cam.project(rect, fov, p.cam + p.look.normalize_or_zero() * 2.6) {
                let d = tip - c;
                let len = d.length().max(1e-3);
                let tip = c + d / len * len.clamp(rad * 0.6, rad * 2.2);
                painter.line_segment([c, tip], Stroke::new((rad * 0.25).clamp(2.0, 8.0), col.gamma_multiply(0.85)));
            }
            painter.circle_filled(c, rad, col);
            painter.circle_stroke(c, rad, Stroke::new(1.5f32, Color32::from_black_alpha(160)));
            // a little visor so it reads as "someone looking"
            painter.circle_filled(c + vec2(0.0, -rad * 0.15), rad * 0.38, Color32::from_white_alpha(200));
            let label = format!("{}  ·  {}", r.info.name, p.tool);
            let at = c - vec2(0.0, rad + 4.0);
            painter.text(at + vec2(1.0, 1.0), Align2::CENTER_BOTTOM, &label, FontId::proportional(12.0), Color32::from_black_alpha(200));
            painter.text(at, Align2::CENTER_BOTTOM, &label, FontId::proportional(12.0), col);
        }
    }

    /// Other people's selections and positions in a 2D view.
    pub(crate) fn paint_team_2d(&self, painter: &Painter, view: &View2D, rect: Rect, scene: &[Arc<WorldMesh>]) {
        let Some(s) = &self.team else { return };
        let sel = s.remote_selection();
        if !sel.is_empty() {
            for wm in scene {
                if let Some(c) = sel.get(&wm.id) {
                    let r = view.screen_rect(rect, wm.min, wm.max).expand(2.0);
                    if r.intersects(rect) {
                        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5f32, c32(*c)), StrokeKind::Outside);
                    }
                }
            }
        }
        for r in s.peers.values() {
            let Some(p) = &r.presence else { continue };
            let c = view.world_to_screen(rect, p.cam);
            if !rect.contains(c) {
                continue;
            }
            let col = c32(r.info.color);
            let d = view.world_to_screen(rect, p.cam + p.look) - c;
            if d.length() > 0.5 {
                painter.line_segment([c, c + d / d.length() * 16.0], Stroke::new(2.0f32, col));
            }
            painter.circle_filled(c, 6.0, col);
            painter.circle_stroke(c, 6.0, Stroke::new(1.0f32, Color32::from_black_alpha(160)));
            painter.text(c + vec2(9.0, -9.0), Align2::LEFT_BOTTOM, &r.info.name, FontId::proportional(11.0), col);
        }
    }

    /// Status bar badge. True when clicked.
    pub(crate) fn team_status(&self, ui: &mut egui::Ui) -> bool {
        let Some(s) = &self.team else { return false };
        ui.separator();
        let n = s.peers.len() + 1;
        let txt = if s.host {
            format!("● Team: hosting, {n} here{}", if s.pending.is_empty() { String::new() } else { format!(", {} waiting", s.pending.len()) })
        } else if s.welcomed {
            format!("● Team: {n} here")
        } else {
            "● Team: joining…".into()
        };
        ui.add(egui::Label::new(RichText::new(txt).color(c32(s.me.color))).sense(egui::Sense::click())).on_hover_text("Open Team Create").clicked()
    }

    pub(crate) fn team_window(&mut self, ctx: &egui::Context) {
        if !self.team_ui.open {
            return;
        }
        let mut open = true;
        let mut act: Option<Act> = None;
        egui::Window::new("Team Create").open(&mut open).resizable(false).default_width(380.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Your name");
                ui.add_enabled(self.team.is_none(), egui::TextEdit::singleline(&mut self.settings.team_name).desired_width(140.0).char_limit(32));
                ui.label("Color");
                ui.add_enabled_ui(self.team.is_none(), |ui| ui.color_edit_button_srgb(&mut self.settings.team_color));
            });
            ui.separator();
            match &self.team {
                None => {
                    ui.label(RichText::new("Create a room").strong());
                    ui.label(RichText::new("You get a room code to send your friends, and you click Allow for each one who knocks.").small().weak());
                    if ui.button("Create room").clicked() {
                        act = Some(Act::Host);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(RichText::new("Join a room").strong());
                    let searching = self.team_ui.finding.is_some();
                    ui.horizontal(|ui| {
                        ui.label("Room code");
                        let r = ui.add_enabled(!searching, egui::TextEdit::singleline(&mut self.team_ui.join_code).hint_text("ABCD-EF23").desired_width(110.0).char_limit(16));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.add_enabled(!searching, egui::Button::new("Join")).clicked() || (enter && !searching) {
                            act = Some(Act::Join);
                        }
                    });
                    if searching {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Looking for the room on your network and VPN...");
                        });
                    }
                    ui.label(RichText::new("Works when you're on the same home network, or the same VPN (Tailscale, ZeroTier, Radmin, Hamachi). For other setups see Advanced.").small().weak());
                    if self.dirty {
                        ui.label(RichText::new("Joining replaces the open map with the room's. Save first if you want to keep it.").small().color(Color32::from_rgb(255, 170, 60)));
                    }
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new("Advanced (other networks)").default_open(false).show(ui, |ui| {
                        ui.label(RichText::new("Only if the room code can't find the room: connect straight to the host's address (they see it under Advanced in their room), using the room code above as the password.").small().weak());
                        ui.horizontal(|ui| {
                            ui.label("Host address");
                            ui.add(egui::TextEdit::singleline(&mut self.settings.team_join_addr).hint_text("192.168.1.20").desired_width(150.0));
                            if ui.button("Join by address").clicked() {
                                act = Some(Act::JoinAddr);
                            }
                        });
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Port when hosting");
                            ui.add(egui::DragValue::new(&mut self.settings.team_port).range(1024..=65535));
                        });
                        ui.checkbox(&mut self.settings.team_allow_internet, "Let people from the internet in (not just LAN / VPN)");
                        if self.settings.team_allow_internet {
                            ui.label(RichText::new("They'd need your public address and a forwarded port, and they can see your IP. Safer: both install Tailscale or ZeroTier and leave this off.").small().color(Color32::from_rgb(255, 170, 60)));
                        }
                    });
                }
                Some(s) => {
                    if s.host {
                        ui.horizontal(|ui| {
                            ui.label("Room code");
                            ui.label(RichText::new(pretty_code(&s.code)).monospace().strong().size(18.0));
                            if ui.button("Copy").clicked() {
                                ui.ctx().copy_text(pretty_code(&s.code));
                            }
                        });
                        if s.beacon.is_some() {
                            ui.label(RichText::new("Send this code to your friends. They type it under Join a room (same network or VPN).").small().weak());
                        } else {
                            ui.label(RichText::new("The room code can't be looked up from this PC right now (another program has its ports). Friends can still join with your address under Advanced.").small().color(Color32::from_rgb(255, 170, 60)));
                        }
                        let addr = team::lan_ip().map(|ip| if s.port == team::DEFAULT_PORT { ip.to_string() } else { format!("{ip}:{}", s.port) });
                        egui::CollapsingHeader::new("Advanced").id_salt("team_host_adv").default_open(false).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Your address");
                                match &addr {
                                    Some(a) => {
                                        ui.label(RichText::new(a).monospace());
                                        if ui.small_button("Copy").clicked() {
                                            ui.ctx().copy_text(a.clone());
                                        }
                                    }
                                    None => {
                                        ui.label(format!("this PC, port {}", s.port));
                                    }
                                }
                            });
                            ui.label(RichText::new(if s.allow_internet { "Internet joins allowed (people can see your IP)." } else { "Only LAN / VPN addresses can connect." }).small().weak());
                        });
                    } else if s.welcomed {
                        ui.label("Connected to the host. Everyone's edits show up live.");
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Waiting for the host to let you in…");
                        });
                    }
                    if !s.pending.is_empty() {
                        ui.separator();
                        for p in &s.pending {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("■").color(c32(p.color)));
                                ui.label(RichText::new(&p.name).strong());
                                ui.label(RichText::new(format!("from {}", p.addr.ip())).weak());
                                if ui.button("Allow").clicked() {
                                    act = Some(Act::Approve(p.conn));
                                }
                                if ui.button("Deny").clicked() {
                                    act = Some(Act::Deny(p.conn));
                                }
                            });
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("People").strong());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(c32(s.me.color)));
                        ui.label(format!("{} (you{})", s.me.name, if s.host { ", host" } else { "" }));
                    });
                    for (id, r) in &s.peers {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("●").color(c32(r.info.color)));
                            ui.label(&r.info.name);
                            if *id == team::HOST_ID {
                                ui.label(RichText::new("host").weak());
                            }
                            if let Some(p) = &r.presence {
                                ui.label(RichText::new(&p.tool).weak());
                            }
                            if let Some(addr) = r.addr {
                                ui.label(RichText::new(addr.ip().to_string()).small().weak()).on_hover_text("Only you (the host) can see this");
                            }
                            if s.host && ui.small_button("Remove").clicked() {
                                act = Some(Act::Kick(*id));
                            }
                        });
                    }
                    if !s.log.is_empty() {
                        ui.separator();
                        for l in s.log.iter().rev().take(5) {
                            ui.label(RichText::new(l).small().weak());
                        }
                    }
                    ui.separator();
                    if ui.button(if s.host { "Stop hosting" } else { "Leave session" }).clicked() {
                        act = Some(Act::Leave);
                    }
                }
            }
            if !self.team_ui.message.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.team_ui.message).color(Color32::from_rgb(255, 170, 60)));
            }
        });
        if !open {
            self.team_ui.open = false;
        }
        let Some(a) = act else { return };
        self.team_ui.message.clear();
        match a {
            Act::Host => {
                self.settings.team_name = team::clean_name(&self.settings.team_name);
                match Session::host(&mut self.map, &self.settings.team_name, self.settings.team_color, self.settings.team_port, self.settings.team_allow_internet, team::default_cache_dir()) {
                    Ok(s) => {
                        self.status = format!("Team Create room open. Room code {}", pretty_code(&s.code));
                        self.team = Some(s);
                    }
                    Err(e) => self.team_ui.message = e,
                }
                let _ = self.settings.save();
            }
            Act::Join => {
                self.settings.team_name = team::clean_name(&self.settings.team_name);
                let _ = self.settings.save();
                let Some(code) = crate::room::normalize(&self.team_ui.join_code) else {
                    self.team_ui.message = "That isn't a room code. Room codes are 8 letters and numbers, like ABCD-EF23.".into();
                    return;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                let c2 = code.clone();
                std::thread::spawn(move || {
                    let targets = crate::room::targets();
                    let _ = tx.send(crate::room::find(&c2, &targets, &crate::room::PORTS, std::time::Duration::from_secs(4)));
                });
                self.team_ui.finding = Some((code, rx));
            }
            Act::JoinAddr => {
                self.settings.team_name = team::clean_name(&self.settings.team_name);
                if self.team_ui.join_code.trim().is_empty() {
                    self.team_ui.message = "Type the room code too: it's the password for the room.".into();
                    return;
                }
                match Session::join(&self.settings.team_join_addr, &self.team_ui.join_code, &self.settings.team_name, self.settings.team_color, team::default_cache_dir()) {
                    Ok(s) => self.team = Some(s),
                    Err(e) => self.team_ui.message = e,
                }
                let _ = self.settings.save();
            }
            Act::Leave => {
                if let Some(mut s) = self.team.take() {
                    s.leave(if s.host { "The host stopped the session." } else { "left" });
                    team::end_ranges(&mut self.map);
                    self.status = "Left Team Create".into();
                }
            }
            Act::Approve(c) => {
                if let Some(s) = &mut self.team {
                    s.approve(c, &self.map);
                }
            }
            Act::Deny(c) => {
                if let Some(s) = &mut self.team {
                    s.deny(c);
                }
            }
            Act::Kick(c) => {
                if let Some(s) = &mut self.team {
                    s.kick(c);
                }
            }
        }
    }
}
