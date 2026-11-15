# Changelog

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
