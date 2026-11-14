// My plugin
// kind: "command" (run once), "tool" (receives viewport clicks), "move"
// (adjusts the drag while moving the selection) or "gizmo" (draws handles on
// the selection and owns drags that start on them; see transform_gizmo.rhai).
//
// API:
//   objects() parts() selection() set_selection(ids) exists(id)
//   get_name(id) get_type(id) get_pos(id) get_rot(id) get_size(id) get_color(id)
//   bounds(id) -> #{min, max}   to_local(id, p) to_world(id, p) axis(id, k)
//   set_pos(id, p) set_rot(id, r) set_size(id, s) set_name(id, s)
//   set_color(id, [r,g,b]) set_material(id, "Brick")
//   add_box(pos, size) -> id   delete(id)   grid()   status(text)   print(text)
//   state_get(key) state_set(key, value)     (persists between calls)
//   vadd vsub vscale vdot vcross vlen vnorm  (vectors are [x, y, z] arrays)
//   selection_bounds() -> #{min, max}   rotate_world(id, axis, degrees, pivot)
//   gizmo only: view_name() project(p) -> [x, y] or ()   units_per_pixel(p)
//     draw_line(a, b, [r,g,b], width) draw_point(p, col, size) draw_box(p, col, size)
//     dist2d(a, b) seg_dist(p, a, b) atan2(y, x) rad2deg(r) deg2rad(d)
//   gizmo hooks: draw(), pointer_down(ev) -> bool, pointer_drag(ev), pointer_up(ev)
//     gizmo_mode() -> "move" | "rotate" | "scale"   gizmo_local() -> bool
//     ev = #{ view, mouse: [x, y], origin, dir, shift, ctrl, alt }

fn info() {
    #{ name: "My Plugin", kind: "command", description: "Describe what it does." }
}

// command plugins
fn run() {
    for id in selection() {
        let p = get_pos(id);
        p[1] += grid();
        set_pos(id, p);
    }
    status("Moved the selection up one grid step");
}

// tool plugins: hit = #{ id, point, normal, edge: [a, b] }
fn on_click(hit) {
    print("clicked " + get_name(hit.id));
}

// move plugins: return the (possibly changed) delta
fn adjust_move(ids, box, delta) {
    delta
}
