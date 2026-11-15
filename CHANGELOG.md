# Changelog

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
