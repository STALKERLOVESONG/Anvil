#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autosave;
mod clipboard;
mod branding;
mod blender_export;
mod building;
mod content_ui;
mod cloud;
mod collide;
mod csg;
mod edge_loops;
mod editmode;
mod entities;
mod export_roblox;
mod faces_ui;
mod export_unity;
mod gizmo;
mod glb;
mod gltf;
mod hiding;
mod knife;
mod lighting;
mod livesync;
mod map;
mod mesh;
mod mesh_csg;
mod mesh_openings;
mod mcp;
mod play_export;
mod playtest;
mod plugin_panels;
mod plugin_web;
mod plugins;
mod polymesh;
mod previews;
mod scatter;
mod secure;
mod settings;
mod source;
mod stairs;
mod mount;
mod package;
mod team;
mod team_ui;
mod text;
mod updater;
mod themes;
mod tool_panel;
mod trim;
mod thumbs;
mod uv_editor;
mod texture;
mod view2d;
mod view3d;
mod windows;

#[cfg(test)]
mod tests;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // `anvil.exe --mcp`: the map-building server for AI assistants (no window)
    if std::env::args().any(|a| a == "--mcp") {
        mcp::serve();
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1500.0, 920.0])
        .with_min_inner_size([900.0, 600.0])
        .with_title("Anvil");
    if let Some(icon) = branding::window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Anvil",
        options,
        Box::new(|cc| Ok(Box::new(app::AnvilApp::new(cc)))),
    )
}
