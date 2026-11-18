//! The level data model: brushes, meshes, props, decals, entities, folders
//! and texture definitions.

use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Primitive brush shapes. Every one of them maps onto a Roblox part type
/// (or a small group of parts) so exports stay native.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Shape {
    Box,
    Wedge,
    Cylinder,
    Sphere,
    Stairs,
}

impl Shape {
    pub const ALL: [Shape; 5] = [Shape::Box, Shape::Wedge, Shape::Cylinder, Shape::Sphere, Shape::Stairs];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Box => "Box",
            Shape::Wedge => "Wedge",
            Shape::Cylinder => "Cylinder",
            Shape::Sphere => "Sphere",
            Shape::Stairs => "Stairs",
        }
    }
}

/// Surface materials. Names match Roblox's `Enum.Material` items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Material {
    Plastic,
    SmoothPlastic,
    Neon,
    Wood,
    WoodPlanks,
    Marble,
    Slate,
    Concrete,
    Granite,
    Brick,
    Pebble,
    Cobblestone,
    CorrodedMetal,
    DiamondPlate,
    Foil,
    Metal,
    Grass,
    Sand,
    Fabric,
    Ice,
    Glass,
}

impl Material {
    pub const ALL: [Material; 21] = [
        Material::Plastic,
        Material::SmoothPlastic,
        Material::Neon,
        Material::Wood,
        Material::WoodPlanks,
        Material::Marble,
        Material::Slate,
        Material::Concrete,
        Material::Granite,
        Material::Brick,
        Material::Pebble,
        Material::Cobblestone,
        Material::CorrodedMetal,
        Material::DiamondPlate,
        Material::Foil,
        Material::Metal,
        Material::Grass,
        Material::Sand,
        Material::Fabric,
        Material::Ice,
        Material::Glass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Plastic => "Plastic",
            Self::SmoothPlastic => "SmoothPlastic",
            Self::Neon => "Neon",
            Self::Wood => "Wood",
            Self::WoodPlanks => "WoodPlanks",
            Self::Marble => "Marble",
            Self::Slate => "Slate",
            Self::Concrete => "Concrete",
            Self::Granite => "Granite",
            Self::Brick => "Brick",
            Self::Pebble => "Pebble",
            Self::Cobblestone => "Cobblestone",
            Self::CorrodedMetal => "CorrodedMetal",
            Self::DiamondPlate => "DiamondPlate",
            Self::Foil => "Foil",
            Self::Metal => "Metal",
            Self::Grass => "Grass",
            Self::Sand => "Sand",
            Self::Fabric => "Fabric",
            Self::Ice => "Ice",
            Self::Glass => "Glass",
        }
    }

    pub fn from_label(s: &str) -> Option<Material> {
        Self::ALL.iter().copied().find(|m| m.label().eq_ignore_ascii_case(s))
    }

    /// Value of the `Material` token in Roblox XML files.
    pub fn roblox_token(self) -> u32 {
        match self {
            Self::Plastic => 256,
            Self::SmoothPlastic => 272,
            Self::Neon => 288,
            Self::Wood => 512,
            Self::WoodPlanks => 528,
            Self::Marble => 784,
            Self::Slate => 800,
            Self::Concrete => 816,
            Self::Granite => 832,
            Self::Brick => 848,
            Self::Pebble => 864,
            Self::Cobblestone => 880,
            Self::CorrodedMetal => 1040,
            Self::DiamondPlate => 1056,
            Self::Foil => 1072,
            Self::Metal => 1088,
            Self::Grass => 1280,
            Self::Sand => 1296,
            Self::Fabric => 1312,
            Self::Ice => 1536,
            Self::Glass => 1568,
        }
    }
}

/// A local-space box subtracted from a Box brush (door / window openings,
/// stairwells, anything carved with Boolean > Carve).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    pub min: Vec3,
    pub max: Vec3,
}

/// Outliner folder. Exported as a Model in Roblox and a name prefix in Unity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub parent: Option<u64>,
}

fn plastic() -> Material {
    Material::Plastic
}

fn tile() -> Vec2 {
    Vec2::new(4.0, 4.0)
}

/// A custom texture: image, optional normal map, metal flag and tiling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextureDef {
    pub id: u64,
    pub name: String,
    /// Path to the color image (.png / .jpg / .bmp / .tga).
    pub image: String,
    /// Optional normal map image.
    #[serde(default)]
    pub normal_map: String,
    #[serde(default)]
    pub metallic: bool,
    /// 0 solid, 1 cut-out (the picture's see-through pixels are holes),
    /// 2 see-through (blended, like glass).
    #[serde(default)]
    pub alpha: u8,
    /// Material the texture sits on (Roblox MaterialVariant base material).
    #[serde(default = "plastic")]
    pub base_material: Material,
    /// Units per tile along U / V.
    #[serde(default = "tile")]
    pub scale: Vec2,
    /// Offset in units along U / V.
    #[serde(default)]
    pub offset: Vec2,
    #[serde(default)]
    pub roblox_image_id: String,
    #[serde(default)]
    pub roblox_normal_id: String,
    /// PBR maps (Roblox MaterialVariant RoughnessMap / MetalnessMap).
    #[serde(default)]
    pub roughness_map: String,
    #[serde(default)]
    pub metalness_map: String,
    #[serde(default)]
    pub roblox_roughness_id: String,
    #[serde(default)]
    pub roblox_metalness_id: String,
}

impl TextureDef {
    pub fn new(name: String, image: String) -> Self {
        TextureDef {
            id: 0,
            name,
            image,
            normal_map: String::new(),
            metallic: false,
            alpha: 0,
            base_material: Material::Plastic,
            scale: tile(),
            offset: Vec2::ZERO,
            roblox_image_id: String::new(),
            roblox_normal_id: String::new(),
            roughness_map: String::new(),
            metalness_map: String::new(),
            roblox_roughness_id: String::new(),
            roblox_metalness_id: String::new(),
        }
    }

    /// Normal maps and metal need a Roblox MaterialVariant; plain images use Texture faces.
    pub fn needs_material_variant(&self) -> bool {
        !self.normal_map.trim().is_empty() || self.metallic || !self.roughness_map.trim().is_empty() || !self.metalness_map.trim().is_empty()
    }
}

/// Per-object / per-face texture adjustments (on top of the TextureDef).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TexParams {
    /// Multiplies the texture's tile size.
    pub scale: Vec2,
    /// Added to the texture's offset (units).
    pub offset: Vec2,
    /// Degrees (preview + Unity; Roblox textures can't rotate).
    pub rotation: f32,
    /// Color the texture is multiplied by (Roblox Texture.Color3).
    pub tint: [u8; 3],
}

impl Default for TexParams {
    fn default() -> Self {
        TexParams { scale: Vec2::ONE, offset: Vec2::ZERO, rotation: 0.0, tint: [255, 255, 255] }
    }
}

/// A texture painted on one face. `texture: None` = an untextured face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceTex {
    pub texture: Option<u64>,
    #[serde(default)]
    pub params: TexParams,
}

/// Source of a 3D text mesh, so it can be rebuilt (the mesh itself is saved,
/// so people without the font still see it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpec {
    pub text: String,
    pub font: String,
    pub font_name: String,
    pub height: f32,
    pub depth: f32,
}

/// Everything a Roblox ProximityPrompt has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptSpec {
    pub action_text: String,
    pub object_text: String,
    /// Keyboard key name, e.g. "E", "F", "Space".
    pub key: String,
    /// Gamepad button name, e.g. "ButtonX".
    pub gamepad: String,
    pub hold: f32,
    pub distance: f32,
    pub line_of_sight: bool,
    pub enabled: bool,
    pub custom_style: bool,
    pub ui_offset: Vec2,
}

impl Default for PromptSpec {
    fn default() -> Self {
        PromptSpec {
            action_text: "Interact".into(),
            object_text: String::new(),
            key: "E".into(),
            gamepad: "ButtonX".into(),
            hold: 0.0,
            distance: 10.0,
            line_of_sight: true,
            enabled: true,
            custom_style: false,
            ui_offset: Vec2::ZERO,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// Solid primitive. `size` is the full extent along each local axis.
    Brush {
        shape: Shape,
        size: Vec3,
        steps: u32,
        #[serde(default)]
        holes: Vec<Hole>,
    },
    /// Editable polygon mesh in local space. Faces are CCW seen from outside.
    Mesh { verts: Vec<Vec3>, faces: Vec<Vec<u32>> },
    /// External model (.obj / .stl). `roblox_mesh_id` overrides the library's id.
    Prop { model: String, scale: Vec3, roblox_mesh_id: String },
    /// Flat textured plane (local XY, facing +Z).
    Decal { size: Vec2 },
    /// Point light.
    Light { range: f32, brightness: f32 },
    /// Player spawn pad.
    Spawn { size: Vec3 },
    /// A ProximityPrompt at this point (Roblox).
    Prompt { prompt: PromptSpec },
    /// Sends players to `dest` (another teleport or a target) by touching
    /// the pad, or by using its prompt.
    Teleport {
        size: Vec3,
        touch: bool,
        #[serde(default)]
        dest: Option<u64>,
        #[serde(default)]
        cooldown: f32,
        #[serde(default)]
        exit_offset: Vec3,
        #[serde(default)]
        prompt: PromptSpec,
    },
    /// An invisible point teleports can send players to.
    Target {},
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: u64,
    pub name: String,
    pub kind: Kind,
    pub pos: Vec3,
    /// Euler angles in degrees, applied yaw (Y), then pitch (X), then roll (Z).
    #[serde(default)]
    pub rot: Vec3,
    pub color: [u8; 3],
    pub material: Material,
    #[serde(default)]
    pub transparency: f32,
    #[serde(default = "yes")]
    pub collide: bool,
    #[serde(default = "yes")]
    pub anchored: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub folder: Option<u64>,
    #[serde(default)]
    pub texture: Option<u64>,
    #[serde(default)]
    pub tex_params: TexParams,
    /// Per-face textures: brush faces 0..6 = +X -X +Y -Y +Z -Z, mesh faces by index.
    #[serde(default)]
    pub face_tex: Vec<Option<FaceTex>>,
    #[serde(default)]
    pub text: Option<TextSpec>,
}

impl Object {
    pub fn new(kind: Kind, pos: Vec3, color: [u8; 3], material: Material) -> Self {
        Object {
            id: 0,
            name: String::new(),
            kind,
            pos,
            rot: Vec3::ZERO,
            color,
            material,
            transparency: 0.0,
            collide: true,
            anchored: true,
            hidden: false,
            folder: None,
            texture: None,
            tex_params: TexParams::default(),
            face_tex: Vec::new(),
            text: None,
        }
    }

    pub fn face_tex_mut(&mut self, face: usize) -> &mut Option<FaceTex> {
        if self.face_tex.len() <= face {
            self.face_tex.resize(face + 1, None);
        }
        &mut self.face_tex[face]
    }

    pub fn brush(shape: Shape, size: Vec3, pos: Vec3, color: [u8; 3], material: Material) -> Self {
        Self::new(Kind::Brush { shape, size, steps: 8, holes: Vec::new() }, pos, color, material)
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(
            EulerRot::YXZ,
            self.rot.y.to_radians(),
            self.rot.x.to_radians(),
            self.rot.z.to_radians(),
        )
    }

    pub fn set_rotation(&mut self, q: Quat) {
        let (y, x, z) = q.to_euler(EulerRot::YXZ);
        self.rot = Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees());
    }

    pub fn scale(&self) -> Vec3 {
        match &self.kind {
            Kind::Prop { scale, .. } => *scale,
            _ => Vec3::ONE,
        }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale(), self.rotation(), self.pos)
    }

    pub fn type_label(&self) -> &'static str {
        match &self.kind {
            Kind::Brush { shape, .. } => shape.label(),
            Kind::Mesh { .. } => "Mesh",
            Kind::Prop { .. } => "Prop",
            Kind::Decal { .. } => "Decal",
            Kind::Light { .. } => "Light",
            Kind::Spawn { .. } => "Spawn",
            Kind::Prompt { .. } => "Prompt",
            Kind::Teleport { .. } => "Teleport",
            Kind::Target { .. } => "Target",
        }
    }

    /// World point -> object space (ignores prop scale).
    pub fn to_local(&self, p: Vec3) -> Vec3 {
        self.rotation().inverse() * (p - self.pos)
    }

    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.pos + self.rotation() * p
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Map {
    #[serde(default)]
    pub version: u32,
    pub name: String,
    pub objects: Vec<Object>,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub textures: Vec<TextureDef>,
    #[serde(default)]
    pub next_id: u64,
    /// Team Create: every participant allocates ids in its own range.
    #[serde(skip)]
    pub id_base: u64,
    #[serde(skip)]
    pub local_next: u64,
}

impl Default for Map {
    fn default() -> Self {
        Map {
            version: 2,
            name: "untitled".into(),
            objects: Vec::new(),
            folders: Vec::new(),
            textures: Vec::new(),
            next_id: 0,
            id_base: 0,
            local_next: 0,
        }
    }
}

impl Map {
    pub fn new_id(&mut self) -> u64 {
        if self.id_base == 0 {
            self.next_id += 1;
            return self.next_id;
        }
        let top = self.id_base + (1u64 << 40);
        if self.local_next < self.id_base {
            let used = self
                .objects
                .iter()
                .map(|o| o.id)
                .chain(self.folders.iter().map(|f| f.id))
                .chain(self.textures.iter().map(|t| t.id))
                .filter(|i| *i >= self.id_base && *i < top)
                .max();
            self.local_next = used.unwrap_or(self.id_base);
        }
        self.local_next += 1;
        self.next_id = self.next_id.max(self.local_next);
        self.local_next
    }

    /// Adds an object, assigning it a fresh id (and a name if it has none).
    pub fn add(&mut self, mut obj: Object) -> u64 {
        let id = self.new_id();
        obj.id = id;
        if obj.name.trim().is_empty() {
            obj.name = format!("{}_{}", obj.type_label(), obj.id);
        }
        self.objects.push(obj);
        id
    }

    pub fn add_folder(&mut self, name: &str, parent: Option<u64>) -> u64 {
        let id = self.new_id();
        self.folders.push(Folder { id, name: name.into(), parent });
        id
    }

    pub fn add_texture(&mut self, mut t: TextureDef) -> u64 {
        let id = self.new_id();
        t.id = id;
        self.textures.push(t);
        id
    }

    pub fn get(&self, id: u64) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn texture(&self, id: Option<u64>) -> Option<&TextureDef> {
        let id = id?;
        self.textures.iter().find(|t| t.id == id)
    }

    pub fn folder(&self, id: u64) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    /// The folder an object actually lives in (None if its folder was deleted).
    pub fn folder_of(&self, o: &Object) -> Option<u64> {
        o.folder.filter(|f| self.folder(*f).is_some())
    }

    /// All objects inside a folder, including sub-folders.
    pub fn folder_objects(&self, fid: u64) -> Vec<u64> {
        let mut out: Vec<u64> = self.objects.iter().filter(|o| o.folder == Some(fid)).map(|o| o.id).collect();
        for f in self.folders.iter().filter(|f| f.parent == Some(fid)) {
            out.extend(self.folder_objects(f.id));
        }
        out
    }

    /// "Parent/Child" path of a folder.
    pub fn folder_path(&self, fid: u64) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(fid);
        let mut guard = 0;
        while let Some(id) = cur {
            let Some(f) = self.folder(id) else { break };
            parts.push(f.name.clone());
            cur = f.parent;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    }

    /// Removes a folder, moving its contents to its parent.
    pub fn remove_folder(&mut self, fid: u64) {
        let parent = self.folder(fid).and_then(|f| f.parent);
        for o in &mut self.objects {
            if o.folder == Some(fid) {
                o.folder = parent;
            }
        }
        for f in &mut self.folders {
            if f.parent == Some(fid) {
                f.parent = parent;
            }
        }
        self.folders.retain(|f| f.id != fid);
    }

    pub fn remove(&mut self, ids: &[u64]) {
        self.objects.retain(|o| !ids.contains(&o.id));
    }

    pub fn load(path: &Path) -> Result<Map, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let mut map: Map = serde_json::from_str(&text).map_err(|e| format!("Not a valid Anvil map: {e}"))?;
        let max = map
            .objects
            .iter()
            .map(|o| o.id)
            .chain(map.folders.iter().map(|f| f.id))
            .chain(map.textures.iter().map(|t| t.id))
            .max()
            .unwrap_or(0);
        map.next_id = map.next_id.max(max);
        Ok(map)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }
}
