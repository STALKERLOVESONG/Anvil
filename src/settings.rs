//! Editor settings: grid, default brush/wall/entity values, the preset
//! library ("default meshes"), the model library and export scales.

use crate::map::{Material, Shape};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn one_step() -> u32 {
    8
}

/// A reusable brush template shown in the Library panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub shape: Shape,
    pub size: Vec3,
    pub color: [u8; 3],
    pub material: Material,
    #[serde(default = "one_step")]
    pub steps: u32,
    #[serde(default)]
    pub transparency: f32,
}

impl Preset {
    fn new(name: &str, shape: Shape, size: [f32; 3], color: [u8; 3], material: Material) -> Self {
        Preset {
            name: name.into(),
            shape,
            size: Vec3::from(size),
            color,
            material,
            steps: 8,
            transparency: 0.0,
        }
    }
}

/// A model file available to the Prop tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub path: String,
    /// Display name (empty = the file name).
    #[serde(default)]
    pub name: String,
    /// Roblox asset id of the uploaded mesh (used by the Roblox exporters).
    #[serde(default)]
    pub roblox_mesh_id: String,
    /// Model asset id from an Open Cloud upload (inserted via InsertService).
    #[serde(default)]
    pub roblox_model_id: String,
}

pub fn default_presets() -> Vec<Preset> {
    let mut stairs = Preset::new("Stairs 6x8x12", Shape::Stairs, [6.0, 8.0, 12.0], [170, 170, 175], Material::Concrete);
    stairs.steps = 8;
    let mut glass = Preset::new("Glass pane 8x8", Shape::Box, [8.0, 8.0, 0.4], [160, 205, 235], Material::Glass);
    glass.transparency = 0.5;
    vec![
        Preset::new("Wall 16x12", Shape::Box, [16.0, 12.0, 1.0], [205, 200, 190], Material::Concrete),
        Preset::new("Wall 32x12", Shape::Box, [32.0, 12.0, 1.0], [205, 200, 190], Material::Concrete),
        Preset::new("Brick wall 16x12", Shape::Box, [16.0, 12.0, 1.0], [150, 75, 60], Material::Brick),
        Preset::new("Floor 32x32", Shape::Box, [32.0, 1.0, 32.0], [110, 112, 118], Material::Slate),
        Preset::new("Platform 8x8", Shape::Box, [8.0, 1.0, 8.0], [160, 112, 70], Material::WoodPlanks),
        Preset::new("Crate 4", Shape::Box, [4.0, 4.0, 4.0], [150, 105, 60], Material::Wood),
        Preset::new("Ramp 8x4x8", Shape::Wedge, [8.0, 4.0, 8.0], [170, 170, 175], Material::Concrete),
        stairs,
        Preset::new("Pillar", Shape::Cylinder, [2.0, 12.0, 2.0], [225, 222, 215], Material::Marble),
        Preset::new("Ball", Shape::Sphere, [4.0, 4.0, 4.0], [200, 60, 60], Material::Plastic),
        glass,
        Preset::new("Neon strip", Shape::Box, [8.0, 0.4, 0.4], [0, 200, 255], Material::Neon),
    ]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub grid: f32,
    pub snap: bool,

    pub default_shape: Shape,
    pub default_size: Vec3,
    pub default_color: [u8; 3],
    pub default_material: Material,
    pub default_steps: u32,

    pub wall_height: f32,
    pub wall_thickness: f32,
    pub wall_color: [u8; 3],
    pub wall_material: Material,

    pub light_range: f32,
    pub light_brightness: f32,
    pub spawn_size: Vec3,

    // building tools
    pub door_width: f32,
    pub door_height: f32,
    pub window_width: f32,
    pub window_height: f32,
    pub window_sill: f32,
    pub window_glass: bool,
    pub layout_floor: bool,
    pub layout_ceiling: bool,
    pub floor_thickness: f32,
    pub floor_color: [u8; 3],
    pub floor_material: Material,
    pub polygon_height: f32,
    pub decal_size: Vec2,
    // 3D text tool
    pub text_content: String,
    pub text_font_path: String,
    pub text_font_name: String,
    pub text_height: f32,
    pub text_depth: f32,
    /// Knife: true = cut into two objects, false = only add an edge.
    pub knife_split: bool,
    /// GitHub "owner/name" to check for new releases (empty = off).
    pub update_repo: String,
    pub check_updates: bool,
    /// Plugins the user allowed to run ("file:hash" of their exact contents).
    pub trusted_plugins: Vec<String>,
    /// blender.exe (empty = find it automatically).
    pub blender_path: String,
    // Experimental live sync
    pub live_sync: bool,
    pub live_port: u16,
    pub live_token: String,
    // Teleport tool
    pub teleport_touch: bool,
    pub teleport_two_way: bool,
    pub teleport_cooldown: f32,
    /// Moving / rotating stops flush against other parts.
    pub collisions: bool,
    // Building trim
    pub trim_baseboard: bool,
    pub trim_crown: bool,
    /// Wall tool: add trim strips to each new wall (both sides).
    pub wall_trim: bool,
    pub wall_trim_baseboard: bool,
    pub wall_trim_crown: bool,
    pub trim_height: f32,
    pub trim_depth: f32,
    pub trim_color: [u8; 3],
    // UI theme
    pub theme: u8,
    pub accent: [u8; 3],
    // 3D view lighting
    /// 0 off, 1 low, 2 medium, 3 high, 4 ultra
    pub light_quality: u8,
    pub sun_azimuth: f32,
    pub sun_elevation: f32,
    pub fog_enabled: bool,
    pub fog_distance: f32,
    pub sky_enabled: bool,
    // Randomize (scatter)
    pub rand_pos_axes: [bool; 3],
    pub rand_pos_min: f32,
    pub rand_pos_max: f32,
    pub rand_rot_axes: [bool; 3],
    pub rand_rot_min: f32,
    pub rand_rot_max: f32,
    pub rand_seed: u64,
    // Stair maker
    pub stair_width: f32,
    pub stair_step_height: f32,
    pub stair_rise: f32,
    pub stair_rail: u8,
    pub stair_rail_height: f32,
    pub stair_stringers: bool,
    pub stair_color: [u8; 3],
    pub stair_trim_color: [u8; 3],
    pub stair_step_prefab: Vec<crate::map::Object>,
    pub stair_post_prefab: Vec<crate::map::Object>,
    // Team Create
    pub team_name: String,
    pub team_color: [u8; 3],
    pub team_port: u16,
    pub team_allow_internet: bool,
    pub team_join_addr: String,

    pub presets: Vec<Preset>,
    pub model_library: Vec<ModelEntry>,

    /// File names of "move" plugins that are switched on.
    pub enabled_plugins: Vec<String>,
    pub roblox_creator_group: bool,
    pub roblox_creator_id: String,
    /// Thickness of the wedge pairs used to export editable meshes to Roblox.
    pub mesh_wedge_thickness: f32,
    pub mouse_sensitivity: f32,

    /// Mounted game content (folders, *_dir.vpk, .gma).
    pub mounts: Vec<String>,
    /// Editor units per Source unit when importing Half-Life props.
    pub source_scale: f32,
    pub recent_files: Vec<String>,
    /// 0 = off.
    pub autosave_minutes: f32,

    /// Meters per editor unit for Unity (Roblox's own conversion is 0.28 m/stud).
    pub unity_scale: f32,
    /// Studs per editor unit for Roblox.
    pub roblox_scale: f32,

    pub cam_speed: f32,
    pub fov: f32,
    pub show_grid_3d: bool,
    pub outlines_3d: bool,
    pub render_scale: f32,
    /// Draw the 3D view at lower resolution while the camera moves (if it's slow).
    pub fast_motion: bool,
    /// Largest texture size used by the 3D preview (exports always use the originals).
    pub preview_texture_size: u32,
    pub texture_filtering: bool,
    pub bg_2d: [u8; 3],
    pub bg_3d: [u8; 3],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            grid: 1.0,
            snap: true,
            default_shape: Shape::Box,
            default_size: Vec3::new(8.0, 8.0, 8.0),
            default_color: [163, 162, 165],
            default_material: Material::Plastic,
            default_steps: 8,
            wall_height: 12.0,
            wall_thickness: 1.0,
            wall_color: [205, 200, 190],
            wall_material: Material::Concrete,
            light_range: 24.0,
            light_brightness: 2.0,
            spawn_size: Vec3::new(6.0, 0.2, 6.0),
            door_width: 4.0,
            door_height: 7.0,
            window_width: 4.0,
            window_height: 4.0,
            window_sill: 3.0,
            window_glass: true,
            layout_floor: true,
            layout_ceiling: false,
            floor_thickness: 1.0,
            floor_color: [140, 110, 80],
            floor_material: Material::WoodPlanks,
            polygon_height: 1.0,
            decal_size: Vec2::new(4.0, 4.0),
            text_content: "ANVIL".into(),
            text_font_path: String::new(),
            text_font_name: String::new(),
            text_height: 4.0,
            knife_split: true,
            update_repo: crate::updater::DEFAULT_REPO.into(),
            check_updates: true,
            trusted_plugins: Vec::new(),
            blender_path: String::new(),
            live_sync: false,
            live_port: crate::livesync::DEFAULT_PORT,
            live_token: String::new(),
            teleport_touch: true,
            teleport_two_way: true,
            teleport_cooldown: 2.0,
            collisions: false,
            trim_baseboard: false,
            trim_crown: false,
            wall_trim: false,
            wall_trim_baseboard: true,
            wall_trim_crown: false,
            trim_height: 0.8,
            trim_depth: 0.25,
            trim_color: [236, 232, 222],
            theme: 0,
            accent: [70, 140, 230],
            light_quality: 2,
            sun_azimuth: -55.0,
            sun_elevation: 45.0,
            fog_enabled: true,
            fog_distance: 900.0,
            sky_enabled: true,
            rand_pos_axes: [true, false, true],
            rand_pos_min: -0.5,
            rand_pos_max: 0.5,
            rand_rot_axes: [false, true, false],
            rand_rot_min: -20.0,
            rand_rot_max: 20.0,
            rand_seed: 0,
            stair_width: 4.0,
            stair_step_height: 1.0,
            stair_rise: 12.0,
            stair_rail: 3,
            stair_rail_height: 3.0,
            stair_stringers: true,
            stair_color: [190, 185, 175],
            stair_trim_color: [95, 72, 55],
            stair_step_prefab: Vec::new(),
            stair_post_prefab: Vec::new(),
            team_name: "Player".into(),
            team_color: [70, 170, 255],
            team_port: crate::team::DEFAULT_PORT,
            team_allow_internet: false,
            team_join_addr: String::new(),
            text_depth: 0.6,
            presets: default_presets(),
            model_library: Vec::new(),
            enabled_plugins: vec!["snap_to_part.rhai".into()],
            roblox_creator_group: false,
            roblox_creator_id: String::new(),
            mesh_wedge_thickness: 0.1,
            mouse_sensitivity: 1.0,
            mounts: Vec::new(),
            source_scale: 0.07,
            recent_files: Vec::new(),
            autosave_minutes: 3.0,
            unity_scale: 0.28,
            roblox_scale: 1.0,
            cam_speed: 24.0,
            fov: 70.0,
            show_grid_3d: true,
            outlines_3d: true,
            render_scale: 1.0,
            fast_motion: true,
            preview_texture_size: 1024,
            texture_filtering: true,
            bg_2d: [24, 25, 28],
            bg_3d: [58, 66, 80],
        }
    }
}

impl Settings {
    /// Settings live next to the executable so the editor stays portable.
    pub fn path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("anvil_settings.json")
    }

    pub fn load() -> Self {
        let mut s: Settings = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // spawns used to default to a 1-stud-thick block; they're flat pads now
        if s.update_repo.trim().is_empty() {
            s.update_repo = crate::updater::DEFAULT_REPO.into();
        }
        if s.spawn_size == Vec3::new(6.0, 1.0, 6.0) {
            s.spawn_size.y = 0.2;
        }
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(), text).map_err(|e| format!("Could not save settings: {e}"))
    }
}
