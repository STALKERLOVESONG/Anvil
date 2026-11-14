# Where work paused (resume here)

The code compiles (`cargo build --release`; close Anvil first so the exe can be replaced).
Already in the code but not wired up yet:

- **Speed**
  - `mesh::SceneCache` rebuilds only changed objects. Not used yet: `viewports()` still calls `build_scene`.
  - The new multithreaded renderer (`view3d.rs`, rayon row bands) is already live.
  - The texture cache no longer allocates on cache hits.
- ~~DONE~~ **Texture settings / tint / per-face textures** (Inspector + Faces tool + exports)
  - Data model is done: `TexParams` (scale, offset, rotation, tint) and `Object::face_tex`.
  - `world_mesh` already renders them, using texture slots and a per-triangle `tri_tex`.
  - Missing: Inspector UI, a Face tool, and the Roblox/Unity export of face textures and tint.
- ~~DONE~~ **2D views on the 3D viewport** (buttons in the 3D view header; ortho pan/zoom)
  - `Camera::ortho`, plus ortho projection, picking and rays, are done.
  - Missing: the header buttons (Persp/Top/Bottom/Front/Back/Left/Right), ortho panning/zooming in `camera_nav`, and `plugins::ViewInfo::units_per_pixel` → `cam.units_per_pixel`.
- **Team Create id ranges**: `Map::id_base` / `local_next` in `Map::new_id` is done.

Remaining, in order:

0. ~~**DO FIRST: lag fixes / optimizations**~~ **DONE.** On a 901-object textured map: scene 4.5-7.5 ms → 0.15 ms per frame (cached), render 53 ms → 25 ms (multithreaded, now with mipmaps and smoothing), no whole-map copies per frame, gizmo plugins get only the selection, dense props drawn as boxes in 2D when small, preview texture size + smoothing in Settings > Viewports.
   - Step 7 below: use `SceneCache` in `viewports()`; Inspector clones only the selected objects; gizmo plugin gets a map of just the selection and returns per-object changes; 2D views draw a bounding box for dense props when zoomed out.
   - Plus texture mipmaps + filtering (items 20 and 26). This also fixes the far-distance dithering.
   - Measure before and after with a heavy textured test map.
0b. ~~**THEN: 3D text / font maker**~~ **DONE.** Text tool (last in the tool list) with text box, font search over installed fonts, height and depth. Click a wall to lay it flat on the wall, or the floor to stand it up facing the camera. Saved as a mesh; Inspector can edit and rebuild it.
   - Tool + side-panel settings: text, font picker with search, size, depth.
   - Builds a mesh from any installed font.
   - Saved as a mesh so others see it without the font; rebuildable via `Object::text`.

1. ~~DONE~~ Gizmo mode/local owned by the app (toolbar buttons + W/E/R/X).
   - Add plugin host fns `gizmo_mode()` / `gizmo_local()`, and update `plugins/transform_gizmo.rhai` to use them.
   - Auto-upgrade the installed copy when its FNV-1a hash is `0x732b1b3158d12ad5` (the unmodified old version).
   - Edit-mode extrude moves to Ctrl+E.
2. ~~DONE~~ A native gizmo for selected vertices/faces in mesh edit mode (new `gizmo.rs`: shapes, hit test, drag → translate/rotate/scale).
3. ~~DONE~~ Face tool, plus face texture UI (also in edit-mode face mode). Exports: Roblox `Texture` per NormalId with `Color3` tint; Unity one material per slot.
4. ~~DONE~~ Knife tool: split, or slice-only (new `knife.rs`, `polymesh::plane_cut`).
   - Plane from a 2D line plus the view axis, or from two 3D rays.
   - An axis-aligned cut on a Box brush gives two boxes (holes kept); otherwise mesh plane split with caps.
5. ~~Text tool~~ (done, see 0b): `ttf-parser` + `earcutr` (already in Cargo.toml).
   - Fonts come from `C:\Windows\Fonts` and `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
   - Saved as `Kind::Mesh`, with `Object::text` kept for rebuilding.
6. ~~DONE~~ Team Create (`team.rs` engine + `team_ui.rs`), with the security items from 41:
   - Hosting is a TCP listener; peers exchange newline-JSON messages (Hello/Welcome/Presence/Upsert/Remove/Meta/AssetReq/AssetData).
   - Sync works by diffing against what was last sent, about 10 times a second.
   - Other players show as floating avatars with their color, and their selections are drawn in their color (the renderer's `remote_sel` argument).
   - Asset transfer only sends files the map references.
7. Speed leftovers:
   - Use `SceneCache` in `viewports()`.
   - Inspector should only clone the selected objects, not the whole map, for undo.
   - Gizmo plugin calls should get a map of just the selected objects, and plugins should return per-object changes.
   - 2D views should draw just a bounding box for dense props when zoomed out.
8. Tests for all of the above, README update, rebuild, relaunch.

## Added by request (after the list above)

9. ~~DONE~~ **Stair maker** (`stairs.rs`; Stairs tool. Prefabs stretch to fit each step / post, and the slanted rail stays generated)
   - Click points on or along walls; it generates a staircase between them, with trim/stringers and a handrail (posts + rail).
   - Options: width, step height, rail on left/right/both.
   - Steps and handrail can each be swapped for a custom prefab (any selection saved as a "step" or "handrail" prefab).
10. ~~DONE~~ **Texture / UV editor** (`uv_editor.rs`, Ctrl+U; Alt+drag in 3D)
    - Drag to move, rotate and scale a texture's placement in the viewport, plus a numeric panel.
    - Works per object or on a single selected face (uses `TexParams` / `face_tex`).
    - Snapping and "fit to face".
11. ~~DONE~~ **Thumbnails** (`thumbs.rs`; library + textures list + selected Half-Life prop)
    - Models library, Half-Life content props, and the Textures tab/window.
    - Render each model thumbnail with the software renderer and cache it.
12. ~~DONE~~ **Door/window preview:** press-and-hold shows the outline of the opening (and glass pane) before cutting. Snaps to the grid unless Ctrl is held.
13. ~~DONE~~ **Prop placement preview:** a ghost of the prop under the cursor before clicking (also for presets, lights and spawns).
14. ~~DONE~~ **Boolean union / subtract tool**
    - Add a cube or sphere and union it into, or subtract it from, a mesh to sculpt models (mesh CSG).
    - Not only box-minus-box like walls now.
15. ~~DONE~~ **ProBuilder-style polygon tool:** draw the outline on the floor, then mouse movement sets the height live (snapped, Ctrl = free), and click to finish.
16. ~~DONE~~ **Shift-constrain everywhere**
    - Block drag → perfect square.
    - Building/polygon → hold Shift and drag two corners for an exact rectangle/square (no lopsided 4-point outlines).
    - Resize handles → uniform scale.
17. ~~DONE~~ **Fix grid flicker in the 3D view** at far distances: fade lines with distance, thin out far lines, and stop the shimmer at the edges.
18. **More quality-of-life** (to discuss): e.g. measure tool, copy/paste between maps, align/distribute, an object search box.
19. ~~DONE~~ **Much better 3D lighting** (`lighting.rs`; shadow map, per-pixel sun/sky/point lights, SSAO, fog, sky, presets)
    - Sun shadows: a shadow map in the software renderer.
    - Ambient occlusion: screen-space, or baked per vertex for static brushes.
    - Point lights actually lighting the preview.
    - Sky gradient and soft distance fog.
    - Quality presets, so slower PCs can turn it down.
20. ~~DONE~~ **Fix the far-distance "dithering"/moiré** (per-pixel mipmaps + bilinear + distance fog) (seen on textured floors near the horizon): texture mipmaps chosen per triangle from screen-space UV size, plus filtered sampling, plus fog to hide the far edge.
21. ~~DONE~~ **Hiding is easier to find** (hiding already works: H / U, the Objects list checkbox, the folder "hide" button)
    - Add a Hide/Show button in the Inspector and an eye icon in the Objects list.
    - Add "Hide unselected" (isolate) and right-click → Hide.
22. ~~DONE~~ **Drag-and-drop textures** from the Textures window/list straight onto an object (or onto one face with Alt held) in any viewport.
23. ~~DONE~~ **Tool settings side panel**
    - A docked panel showing the active tool's settings and toggles.
    - Building: wall height/thickness, floor, ceiling, trim (baseboard + crown), wall/floor textures.
    - Door/Window: sizes, glass.
    - Knife: split/slice. Text: font/size/depth. Stairs: steps/rails. And so on.
    - Moves those settings out of the big Settings window.
24. ~~DONE~~ **Building trim:** optional baseboard/crown trim pieces generated along the walls.
25. ~~DONE~~ **Themes:** dark (default), light, plus a few color schemes (e.g. Hammer grey, blue, high contrast) and an accent color picker, saved in settings.
26. **Sharper textures in the 3D preview** (exports are already full quality; the preview is the problem)
    - Raise the preview cap from 512 px (setting: 512 / 1024 / 2048).
    - Bilinear filtering up close, plus the mipmaps from item 20 for far away.
    - Import Half-Life textures at full resolution (currently capped at 1024).
27. ~~DONE~~ **One-click Blender export**
    - Export the whole scene as a textured .glb (one object per Anvil object; folders become collections).
    - If Blender is installed (auto-detect; it's in this PC's Steam library), run it headless to save a real `.blend` in one click.
    - Otherwise just write the .glb (Blender opens it via File > Import).
28. ~~DONE~~ **Spawns: invisible and flat**
    - Default spawn size becomes flat (e.g. 6 x 0.2 x 6).
    - Exports make it invisible: Roblox SpawnLocation Transparency 1 with no decal; Unity has no geometry already.
    - Still visible in the editor.
29. ~~DONE~~ **Experimental live sync** (Settings > Experimental)
    - **Roblox:**
      - Anvil runs a small local HTTP server on a chosen port. A bundled Roblox Studio plugin (.rbxmx or .lua, installed to the Plugins folder) polls it.
      - Studio mirrors the map live into a Model (create/update/delete parts as you edit). Needs "Allow HTTP Requests" for localhost.
    - **Blender:**
      - A bundled add-on (.zip of .py files: Edit > Preferences > Add-ons > Install).
      - It connects to the same port and mirrors the scene live into a collection.
    - Both are one-way (Anvil → Studio/Blender) at first.
30. ~~DONE~~ **Prompt objects (Roblox ProximityPrompt)**
    - New entity, drawn in the editor as a billboard icon.
    - Inspector fields for everything a ProximityPrompt has: ActionText, ObjectText, KeyboardKeyCode, GamepadKeyCode, HoldDuration, MaxActivationDistance, RequiresLineOfSight, Enabled, Style, UIOffset.
    - Roblox: an invisible, anchored, non-colliding Part (or Attachment) holding the ProximityPrompt, via both .rbxmx and the Luau script.
    - Unity/Blender: just an empty at that point (no behavior needed).
31. ~~DONE~~ **Teleports (Roblox)**
    - Each teleport links to a destination: another teleport or a target point object. Settings: destination, cooldown, exit offset.
    - Two types:
      1. **Touch:** a pad; touching it teleports the player (exported with a Script using Touched → move the HumanoidRootPart to the destination CFrame, with a cooldown).
      2. **Prompt:** a ProximityPrompt at the point; Triggered teleports the player (same Script logic, prompt fields as in item 30).
    - Exported Scripts carry their Source in .rbxmx (ProtectedString) and in the command-bar Luau.
    - Editor draws a line from each teleport to its destination.
32. ~~DONE~~ **`build.bat` for a clean release exe**
    - Runs `cargo build --release` with no debug info: release profile `strip = true`, `debug = false`, `lto = true`, `codegen-units = 1`, `panic = "abort"`; already no console window.
    - Then copies `anvil.exe` + `plugins/` + README into a `dist/` folder, ready to zip and share.
33. ~~DONE~~ **Collision snap & stop-block for moving and rotating** (like Roblox Studio's Collisions toggle)
    - **Edge-touch stop (moving):**
      - While dragging (mouse, gizmo, arrow keys), test the moving parts' real shapes against nearby objects.
      - The part stops flush against the surface it hits instead of clipping into it.
      - Use oriented boxes for brushes and convex hulls/triangles for meshes and props; broad-phase with the cached scene bounds.
      - This replaces the box-only check in the Snap To Part plugin for this mode.
    - **Rotation stop-block:** while rotating (gizmo ring or Rotate 90°), sweep the angle in small steps and stop at the last angle with no overlap, so the part rests at the point of contact.
    - **Toggle:** a "Collisions" switch in the tool panel (item 23) and the 3D view toolbar, saved in settings. Off = free movement as today.
34. ~~DONE~~ **Planar move handles on the gizmo (Unity-style)**
    - Small colored squares between the move arrows for the XY, XZ and YZ planes; dragging one slides the object in that plane with the third axis locked.
    - Works in local and global space; snaps to grid unless Ctrl is held.
    - With Collisions on (item 33), it slides along a floor or wall until its edges hit a neighbor.
    - Applies to the object gizmo and the vertex/face gizmo.
35. ~~DONE~~ **Randomize offset & rotation ("lived-in" clutter scatter)**
    - Tool panel section with X/Y/Z toggles and min/max ranges, separately for position offset and rotation angle; optional seed.
    - "Randomize" applies small random changes to each selected object, only on the toggled-on axes (e.g. X/Z offset + Y rotation, so chairs around a table look natural without floating or tilting).
    - One undo step; "Re-roll" button.
36. ~~DONE~~ **Sketchfab browser & importer** (as a plugin, not built into the core)
    - Needs plugin API additions first:
      - HTTP get/download (limited to allowed hosts)
      - unzip
      - a way for plugins to draw their own panel (search box, list, thumbnail images, buttons)
      - "import model into library"
    - The plugin itself:
      - search Sketchfab and preview thumbnails
      - download downloadable models (glTF) into `content/sketchfab/`
      - add them to the model library
    - The user enters their own Sketchfab API token; downloads need it.
    - Auto-materials: read the glTF's PBR textures (base color, normal, roughness, metalness) and assign them on import.
    - Needs glTF import support in the core model loader (currently .obj/.stl only).
37. **Roblox export with full `SurfaceAppearance` (PBR)** (PARTLY DONE: roughness/metalness maps on textures, uploaded and exported on MaterialVariants. Still open: MeshPart + SurfaceAppearance for props, which needs checking in Studio)
    - Each material gets a SurfaceAppearance with ColorMap / NormalMap / RoughnessMap / MetalnessMap. Maps are uploaded via Open Cloud (Decal → image ids), reusing existing ids.
    - Applies to custom meshes, mounted Half-Life props, and imported (e.g. Sketchfab) models.
    - Caveat: SurfaceAppearance only works on **MeshParts**. So those models must go to Roblox as uploaded meshes:
      - Model upload from .glb → MeshParts.
      - Editable meshes/text get the same option instead of wedge triangles.
    - Plain brushes keep MaterialVariant / Texture faces.
    - To verify: whether SurfaceAppearance map properties can be set from the command-bar script (some aren't script-writable). The .rbxmx route can always set them.
    - Requires roughness/metalness map slots on textures (add to TextureDef; the Source importer can fill them from $phong / $envmapmask where available).
38. ~~DONE~~ **Branding** (`branding.rs`, `build.rs` + `assets/anvil.ico`)
    - `assets/anvil_logo.png` (256x256, the blue anvil badge) becomes the window/taskbar icon, the .exe icon (embed via a build script + .ico), and the logo in the menu bar / About.
    - The "ANVIL" title in the top bar uses OCR A Extended, italic, smoothed (see `assets/title_reference.png`).
    - Load the font from `C:\Windows\Fonts\OCRAEXT.TTF` at runtime, falling back to the default font. Don't ship the font file: it's a licensed Microsoft/Monotype font.
39. ~~DONE~~ **Versions & updates**
    - Show the app version (Cargo version) in the title/About.
    - Team Create handshake sends app + protocol versions. A mismatch gets a clear refusal on both sides ("Host is on 1.5, you're on 1.3. Update to join") instead of a broken sync.
    - On start (opt-out in settings), check the latest GitHub release.
    - If newer: banner "You're on an outdated version (1.3 → 1.5)" with an **Update** button that downloads the release zip over HTTPS from GitHub only, checks its SHA-256 against the release notes, swaps the exe and restarts. Plus a "Release notes" link.
40. **GitHub repository** (needs your OK before anything is published)
    - README with the logo, screenshots/GIFs, feature list and build instructions.
    - LICENSE, CHANGELOG, issue/PR templates.
    - GitHub Actions that build and publish the release zip with `build.bat`-equivalent settings and SHA-256 sums.
41. ~~DONE~~ **Security hardening**
    - ~~DONE~~ **Team Create / IP privacy:** star layout (guests never see each other's IPs), LAN/VPN-only by default, warning for internet joins. Relay mode is still open.
      - Direct peer-to-peer always shows your IP to the people you connect to; it can't be hidden without a relay.
      - So: host on LAN only by default, with a warning when hosting publicly.
      - Recommend a VPN mesh (Tailscale/ZeroTier) for playing with friends, so only those friends can reach you and real IPs stay private.
      - Optional relay mode later.
    - ~~DONE~~ **Team Create protection:** invite code + approval, lockout after 5 wrong codes, version check, size/rate limits, validation, sandboxed assets, network-path (NTLM) blocking for all loaders, image decode limits.
      - Invite code / password required to join; host approves each join.
      - Version check (item 39), size limits on every message and asset, rate limits.
      - Strict typed messages only, so peers can never run code.
      - Assets: only files the map references are ever sent, and received files are saved with sanitized names (no path tricks) in a sandbox folder.
      - Images and models are checked for size limits before decoding.
    - **Plugins:**
      - Rhai scripts can't touch files, network or processes, and have operation limits.
      - Ask before running a new or changed plugin (hash allow-list).
      - Any future HTTP access for plugins goes only to allow-listed hosts, with a prompt.
      - Packages (.anvilpak) never contain plugins.
    - **Packages:** zip-slip protection (already done), plus caps on uncompressed size and file count against zip bombs.
    - **Secrets:** encrypt the Roblox API key with Windows DPAPI instead of plain JSON.
    - **Live sync servers** (item 29): bind to 127.0.0.1 only, with a random session token.

42. ~~DONE~~ **Doors and windows on meshes** (`mesh_openings.rs`): lined up with the clicked face, cut with the mesh booleans.
