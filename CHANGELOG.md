# Changelog

## 0.4.0

### Team Create
- **Room codes.** *Create room* gives you one code, like `ABCD-EF23`. Friends type it under
  *Join a room* and press Join; nobody types an IP address. You still click Allow for each
  person.
  - The code finds the room on your home network and on VPNs: Tailscale, ZeroTier,
    Radmin and Hamachi.
  - There is no Anvil server, so a code can't reach a PC across the plain internet. For
    that, *Advanced* still has join-by-address (with a forwarded port).
  - Older Anvil versions can't join 0.4.0 rooms; both sides get a clear version message.
- **Importing a map while in a room works.** Before, a file dialog open for over 10 seconds
  dropped you from the room. The host was dropping guests after just 10 seconds of
  silence, not the 90 it was meant to.
  - Connections now send a keep-alive ping in the background.
  - The *Import Source map* file picker no longer freezes the window.
  - Host or guest can import. Everyone gets every brush, prop, light, spawn, folder and
    texture, can edit it, and stays connected.
  - Texture and model downloads after a big import no longer trip the flood limit.
- Undo history stays light when someone else imports a whole map (it no longer copies
  thousands of objects into every undo step).

### Source map (.bsp) import
- **Fixed textures that looked like a mess.** Overlapping brushes left two visible faces
  in the same spot, and they flickered and striped into each other. Where faces overlap
  in a plane, the covered part is now cut away, like Source's own compiler does. The test
  map had 87 such pairs; now it has none.
- Each face's Hammer alignment is now checked against Source's formula at every triangle
  corner: rotated, scaled, mirrored and offset textures, non-square textures, far-negative
  coordinates and slanted faces. The UV editor shows exactly what the 3D view draws.
- **Missing content is named.** The report says what's missing and what uses it: the
  material, or the texture file it points at. For example: `de_cbble/grassdirt: its texture
  de_cbble/grassfloor01 isn't installed (used by 40 brush faces, prop tree_small x2)`.
  The status says when an import had missing textures.
- Roblox exports of imported faces keep the right tile size (Roblox textures can't rotate
  or shear, so offsets start at the part's corner).

### Transparency
- **Inspector: Transparency for props.** Solid, Cut out (leaves, fences, grates) or
  See-through (glass), for the whole model. Use it for props imported before transparency
  existed, or whose material didn't say.
- **Inspector: Texture transparency** for walls and meshes, using the texture's own setting
  (the same one as in the Textures window).
- Half-Life materials with `$alphatest` / `$translucent` keep it on imported map brushes and
  props. A picture with alpha but no such setting stays solid; Anvil doesn't guess.

### Wall tool
- **Automatic trim.** *Add trim to new walls* in the Wall tool settings adds a baseboard
  and/or crown to both sides of each wall you draw.
  - It uses the Trim settings' size and color.
  - Where walls meet, the strips run on so the corners close.
  - Doors and windows cut through it like building trim.
  - The wall and its trim are one undo step, and the settings are saved.

### Sketchfab
- **Models no longer come in white.** Many Sketchfab models use the older
  specular/glossiness material, or plain colors with no picture; both were ignored.
  - Their textures and colors load now.
  - A texture's color tint is applied too.
- **Faster downloads:**
  - Sketchfab offers a .glb and a zipped .gltf of each model, and the plugin now takes the
    smaller one. On tested models that was 4 MB instead of 30, and 18 MB instead of 48.
  - It remembers which kind of token works, so there's no failed request before every
    download.
  - Connections are reused instead of a new secure connection per request.
  - Unzipping happens in the background.
  - The model is parsed and its textures decoded in the background before you place it, so
    placing it doesn't freeze the editor.
- Downloads show progress and have a **Cancel** button. A dropped connection or a busy
  server is retried twice. A cancelled or failed download never leaves a partial file.

### TRP playtest export
- The player now starts facing the way the spawn pad faces. It used to face the mirror
  direction.
- Every spawn pad is exported as a start (`start anvil`, `start anvil 2`, …).
- A map with no spawn starts on top of whatever is under its middle.

## 0.3.0

- **Import Source maps (.bsp).** Use *File → Import Source map (.bsp)…* for Garry's Mod and
  Half-Life 2 maps.
  - Every brush becomes its own editable mesh, with each face's Hammer texture alignment
    kept exactly.
  - Displacements come in as terrain meshes.
  - Static props and `prop_*` entities come in as textured props.
  - Lights, spawns and brush entities come in too.
  - Clips, invisible walls and the 3D skybox go into hidden folders.
  - Textures packed into the map are used.
  - A report lists anything your mounted content doesn't have, and the whole import is one
    undo step.
- **MCP:** a new `import_bsp` tool.
- **Faces can keep an exact texture alignment** (planar axes). Imported Source faces use it.
  The UV editor and every export follow it.
- **Security:** paths read from game files can no longer contain `..` or drive letters. A
  bad addon or map could otherwise have written files outside Anvil's content folder.

## 0.2.9

- **Transparent textures work everywhere now.** Before, only decals used a picture's
  transparency; fences, leaves, grates and glass on props drew solid.
  - **Cut out:** the picture's see-through pixels are holes. Use it for chain-link, foliage
    and lattice.
  - **See-through:** blended, like glass.
  - Each texture in the Textures window has a *Transparency* setting.
  - Half-Life / GMod materials bring their own (`$alphatest` / `$translucent`).
  - OBJ models use `map_d` and glTF models use `alphaMode`.
  - Props imported before this update: import them once more to pick up their
    transparency.
- **MCP:** `set_texture_scale` also sets a texture's transparency, and `import_texture`
  reports it.

## 0.2.8

- **MCP server (`anvil.exe --mcp`)** so an AI assistant can build maps.
  - **Maps:** new, open and save.
  - **Building:** buildings from an outline, door and window cuts, and gable, hip or flat
    roofs.
  - **Objects:** brushes, props, lights and spawns.
  - **Content:** search the mounted Half-Life / GMod content, and import its textures and
    models.
  - **Checking:** render a view.

## 0.2.7

- **Copy and paste:**
  - **Ctrl+C / Ctrl+X / Ctrl+V** copy, cut and paste objects, with their textures.
  - Pasting puts them where your mouse is (on the surface under it in 3D), or one
    grid step over when the mouse isn't over a view.
  - It goes through the system clipboard, so it works between two Anvil windows too.
- **Ctrl+D duplicates in place.** The copies stay exactly where the originals are,
  selected, ready to move (it used to shift them a grid step).
- **Mesh edit mode (Tab):**
  - **Edge mode:** 1 Vertices, 2 Edges, 3 Faces. What's selected carries over when you
    switch.
  - **Alt+click selects a loop:** an edge loop in edge mode (its vertices in vertex
    mode), or a ring of faces in face mode.
    - The loop goes straight on through four-edge vertices and follows a hole's border.
    - Shift+Alt+click adds to the selection.
  - Deleting edges deletes the faces they border.

## 0.2.6

- **Placing props:**
  - Props, lights, spawns and prompts drop the moment you press the mouse. Before, they
    waited for the release, and a slight wiggle while clicking turned the click into a
    drag that placed nothing.
  - **R / Shift+R** turn what you're placing by 15° (Alt+R: 90°), or **Alt+scroll** in
    the 3D view. The preview shows it.
  - The preview no longer rebuilds the model on every mouse move.
  - Picking under the mouse checks the nearest objects first.
- **Autosave can't lose work any more.** Every untitled map used to autosave to the same
  file, so starting a new map overwrote the last one's autosave (and anything imported
  into it) within minutes.
  - Each untitled session now has its own autosave; the newest 15 are kept.
  - At start, Anvil offers to reopen unsaved work that's newer than your last saved map.
- **Object list selection:**
  - **Shift+click** adds or removes one object at a time.
  - **Ctrl+click** selects everything between the last object you clicked and this one.
- **Playtest plugins:** a plugin with `fn playtest()` takes over F5. Shift+F5 is still
  Anvil's own playtest.
  - Such a plugin can export the textured map (`export_scene`).
  - It can also export the collision boxes (`export_boxes`), with door holes kept open
    and ramps turned into steps.
- **Plugin permission `programs: true`:** the plugin may ask you to pick folders and start
  `.exe` programs inside them, asking you Yes / No the first time for each program.
  - It can't start scripts, programs on network shares, or anything it downloaded
    itself.

## 0.2.5

- **3D view much faster again:**
  - It isn't redrawn at all when nothing in it changed (clicking panels, typing, editing
    in the Inspector).
  - While the camera moves, a slow view draws at lower resolution and sharpens the
    moment it stops (Settings > *Faster while moving*).
  - Fog is added while drawing instead of in an extra pass over the screen.
  - Near objects are drawn first, so hidden pixels are skipped.
  - Faster texture sampling, and the finished frame reaches the screen about 5x faster.
  - On a real map, a typical view went from about 40 ms to 26 ms per frame at full
    resolution.
- **Fixed:** jagged steps along the horizon where a big ground ended. The coverage math
  now runs in double precision when a triangle reaches behind the camera.
- **Fixed:** detailed props got dark wireframe outlines in the 3D view; now only
  selected ones do.
- **Older buildings** (made before 0.2.4) get their floor and ceiling refit to the
  outside of the walls when the map is opened. Save to keep it.

## 0.2.4

- **Much faster 3D view on big maps.** A map with 150 detailed props (3.3 million
  triangles) went from 1.2 seconds per frame to about 10-20 ms.
  - Objects off screen or too small to see are skipped.
  - Far away, detailed models are drawn from simpler versions (no visible difference).
  - Triangles are set up on all CPU cores.
  - Each part of the screen only looks at the triangles that touch it.
  - The 2D views draw detailed props as boxes once there are too many lines to draw.
- **Building tool:**
  - Floor and ceiling now cover the walls' full outline, flush with the outside (they
    stopped half a wall short before, leaving a step).
  - Drag a box for a rectangular building (Shift isn't needed any more).
  - Clicked corners keep lines straight (every 45°); Ctrl for any angle. The Wall tool
    works the same way.
- **New Trim tool:** click a wall to add trim at that height. On a building it wraps the
  whole way around, inside or outside.
- The Presets list is gone from the side panel.

## 0.2.3

- **Team Create:** props now arrive textured. Each prop's `.mtl` and texture images are
  sent with its `.obj`, including Half-Life props imported from a mounted game, so
  friends without the game see them properly.
  - The sender only sends `.mtl` and image files from the model's own folder area or
    Anvil's content folder.
  - The receiver only accepts plain file names, written into the model's own sandbox
    folder.
- **Team Create:** Sketchfab models (and any `.glb` / `.gltf`) now arrive textured.
  - A `.gltf` from a zip used to not arrive at all, and a `.glb` over 24 MB didn't either.
    Now they go as one `.gltf` plus its `.bin` and textures.
  - If a model is too big for a transfer, its textures are shrunk step by step
    (2048, then 1024 px…) until it fits.
  - The receiver only accepts a `.gltf` whose every file came with it.
- **Packages** (`.anvilpak`) now include a `.gltf` prop's `.bin` and textures.
- **Sketchfab downloads** no longer get cut off.
  - Every download had 30 seconds in total, so bigger models came out half-written
    (and couldn't be loaded). Now a download takes as long as it needs while data
    keeps coming.
  - A download that stops halfway is reported as failed and leaves no half file behind.

## 0.2.2

- **Fixed:** clicking *Download* in the Sketchfab window failed with "Function not found:
  is_alphanumeric".
- **Fixed:** long model names no longer lose their file extension when downloaded.
- **Sign-in:** downloads work with a personal API token or an OAuth token. Sketchfab's
  download docs only show OAuth, so the plugin tries both.
- **Upgrade:** a broken copy of the plugin from 0.2.0 or 0.2.1 is replaced automatically
  when 0.2.2 starts.
- **Tests:**
  - The whole Sketchfab flow (token, search, .glb and zipped glTF downloads, unzip,
    import) is now tested end to end.
  - A new test makes sure every released plugin version gets upgraded.

## 0.2.1

- **Fixed:** saving your Sketchfab API token failed with
  "Function not found: secret_set". The plugin updates itself the first time 0.2.1
  starts.
- Tests now cover saving and forgetting the token, and check that the updater reads real
  GitHub releases.

## 0.2.0 — first public version

**Building and editing**
- Brushes, presets and walls in four viewports (Top / Front / Side / 3D, plus flat views inside the 3D view).
- Buildings from an outline:
  - walls with clean corners, plus floor and ceiling
  - baseboard and crown trim
  - Shift+drag for exact rectangles
- Doors and windows as boolean holes on walls, floors and meshes, with a live preview and glass panes.
- Polygon tool with a live height drag; editable meshes (vertices / faces, extrude, merge).
- Stairs with stringers and handrails, with prefabs for steps and posts.
- Knife (split / slice), mesh union / subtract / intersect, and 3D text from installed fonts.
- Move / rotate / scale gizmo:
  - Local / Global axes and plane handles
  - optional Collisions that stop parts flush against others
- Per-face textures with tint, scale, offset and rotation; a UV editor; drag-and-drop textures.
- Hide / isolate, right-click menus, randomize scatter, Shift-constrain everywhere.

**Looks**
- Multithreaded software renderer with mipmaps.
- Sun shadows, point lights, ambient occlusion, sky and fog, with quality presets.
- Five UI themes with an accent color.
- Thumbnails for models, textures and Half-Life props.

**Content**
- OBJ / STL / glTF (.glb, .gltf) models.
- Half-Life / Source content mounting (VPK, GMA, MDL, VTF, VMT).
- Shareable `.anvilpak` packages.
- Sketchfab browser plugin.

**Export**
- Roblox: `.rbxmx`, command-bar Luau, and Open Cloud upload.
  - MaterialVariants with normal, roughness and metalness maps.
  - ProximityPrompts and teleports with working Scripts.
  - Invisible spawns.
- Unity: `.obj` + `.mtl` and an entity script.
- Blender: `.glb`, or a real `.blend` when Blender is installed.
- Experimental live sync into Roblox Studio and Blender.

**Together**
- Team Create (host / join) with live edits, floating cameras and colored selections.

**Plugins**
- Rhai scripts: commands, tools, move hooks, gizmos and panels.
- Every new or changed plugin has to be approved before it runs.

**Safety**
- Team Create:
  - invite codes and host approval
  - LAN / VPN only by default
  - guests never see each other's IP
  - strict message checks
- Network share paths are never opened from maps.
- The Roblox API key is encrypted with DPAPI.
- Package size caps, and SHA-256-checked self-updates.
