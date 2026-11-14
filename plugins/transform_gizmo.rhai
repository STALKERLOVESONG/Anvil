// Transform Gizmo (gizmo plugin)
// Unity-style handles on the selection, in every viewport.
//
//   W  move    - drag an arrow to move along that axis, a small colored square to move
//                in that plane only (the third axis stays put), the center square to move freely
//   E  rotate  - drag a ring to rotate around that axis (snaps to 15 degrees)
//   R  scale   - drag a cube to scale along that axis, the center cube to scale uniformly
//   X  toggle global / local axes
//   Hold Ctrl while dragging to turn snapping off.
//
// The Move / Rotate / Scale and Local / Global buttons in the top bar drive it too.
// Edit this file and press Reload in the Plugins window - no recompiling needed.

fn info() {
    #{
        name: "Transform Gizmo",
        kind: "gizmo",
        description: "Unity-style move (W), rotate (E) and scale (R) handles on the selection. X toggles local/global axes, Ctrl disables snapping."
    }
}

// ---- settings you can tweak -------------------------------------------------
fn handle_pixels() { 85.0 }       // handle length on screen
fn pick_pixels() { 9.0 }          // how close the mouse must be to grab a handle
fn rotate_snap() { 15.0 }         // degrees
fn colors() { [[235, 70, 70], [110, 215, 80], [80, 140, 255]] }   // X Y Z

// ---- state helpers ----------------------------------------------------------
// Mode and space come from the editor (toolbar buttons and W / E / R / X).
fn mode() { gizmo_mode() }

fn is_local() { gizmo_local() }

fn gizmo_axes() {
    let sel = selection();
    if is_local() && sel.len() > 0 {
        [vnorm(axis(sel[0], 0)), vnorm(axis(sel[0], 1)), vnorm(axis(sel[0], 2))]
    } else {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    }
}

fn center() {
    let b = selection_bounds();
    vscale(vadd(b.min, b.max), 0.5)
}

fn is_unit(v) { type_of(v) == "()" }

// points of a circle around axis `a`
fn ring(c, a, r) {
    let rf = if a[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let u = vnorm(vcross(a, rf));
    let v = vcross(a, u);
    let pts = [];
    for i in 0..49 {
        let t = i.to_float() * 2.0 * PI() / 48.0;
        pts.push(vadd(c, vadd(vscale(u, cos(t) * r), vscale(v, sin(t) * r))));
    }
    pts
}

// parameter along the axis line (c + a*t) closest to the mouse ray
fn axis_param(ev, c, a) {
    let d = ev.dir;
    let b = vdot(a, d);
    let den = 1.0 - b * b;
    if den < 0.0001 { return 0.0; }
    let w = vsub(c, ev.origin);
    (b * vdot(d, w) - vdot(a, w)) / den
}

// where the mouse ray meets the plane through c facing the viewer
fn plane_point(ev, c) {
    let t = vdot(vsub(c, ev.origin), ev.dir);
    vadd(ev.origin, vscale(ev.dir, t))
}

fn snap_to(v, step) { (v / step).round() * step }

// where the mouse ray meets the plane through c with normal n (() if edge-on)
fn ray_plane(ev, c, n) {
    let den = vdot(ev.dir, n);
    if den.abs() < 0.00001 { return (); }
    let t = vdot(vsub(c, ev.origin), n) / den;
    vadd(ev.origin, vscale(ev.dir, t))
}

// the plane square for normal axis k (moves in the other two axes)
fn plane_quad(c, ax, k, s) {
    let u = ax[(k + 1) % 3];
    let v = ax[(k + 2) % 3];
    let a = s * 0.22;
    let b = s * 0.42;
    [vadd(c, vadd(vscale(u, a), vscale(v, a))), vadd(c, vadd(vscale(u, b), vscale(v, a))),
     vadd(c, vadd(vscale(u, b), vscale(v, b))), vadd(c, vadd(vscale(u, a), vscale(v, b)))]
}

// the square on screen, or () when it's nearly edge-on
fn plane_screen(c, ax, k, s) {
    let q = plane_quad(c, ax, k, s);
    let p = [];
    for w in q {
        let sp = project(w);
        if is_unit(sp) { return (); }
        p.push(sp);
    }
    let d1 = [p[2][0] - p[0][0], p[2][1] - p[0][1]];
    let d2 = [p[3][0] - p[1][0], p[3][1] - p[1][1]];
    let area = (d1[0] * d2[1] - d1[1] * d2[0]).abs() * 0.5;
    if area < 60.0 { return (); }
    p
}

fn in_quad(m, p) {
    let pos = 0;
    let neg = 0;
    for i in 0..4 {
        let a = p[i];
        let b = p[(i + 1) % 4];
        let side = (b[0] - a[0]) * (m[1] - a[1]) - (b[1] - a[1]) * (m[0] - a[0]);
        if side >= 0.0 { pos += 1; }
        if side <= 0.0 { neg += 1; }
    }
    pos == 4 || neg == 4
}

// ---- drawing ----------------------------------------------------------------
fn draw() {
    if selection().len() == 0 { return; }
    let c = center();
    let pc = project(c);
    if is_unit(pc) { return; }
    let s = units_per_pixel(c) * handle_pixels();
    let ax = gizmo_axes();
    let cols = colors();
    let hot = state_get("hot");
    let m = mode();
    for k in 0..3 {
        let col = if !is_unit(hot) && hot == k { [255, 220, 60] } else { cols[k] };
        if m == "rotate" {
            let pts = ring(c, ax[k], s * 0.8);
            for i in 0..48 { draw_line(pts[i], pts[i + 1], col, 2.0); }
        } else {
            let tip = vadd(c, vscale(ax[k], s));
            let pt = project(tip);
            if is_unit(pt) || dist2d(pt, pc) < 10.0 { continue; }   // axis points at the camera
            draw_line(c, tip, col, 2.5);
            if m == "scale" {
                draw_box(tip, col, 9.0);
            } else {
                // arrow head
                let rf = if ax[k][1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
                let u = vnorm(vcross(ax[k], rf));
                let v = vcross(ax[k], u);
                let back = vsub(tip, vscale(ax[k], s * 0.18));
                for side in [u, vscale(u, -1.0), v, vscale(v, -1.0)] {
                    draw_line(tip, vadd(back, vscale(side, s * 0.07)), col, 2.5);
                }
            }
        }
    }
    if m == "move" {
        for k in 0..3 {
            if is_unit(plane_screen(c, ax, k, s)) { continue; }
            let q = plane_quad(c, ax, k, s);
            let col = if !is_unit(hot) && hot == 10 + k { [255, 220, 60] } else { cols[k] };
            for i in 0..4 { draw_line(q[i], q[(i + 1) % 4], col, 2.0); }
            draw_line(q[0], q[2], col, 1.0);
            draw_line(q[1], q[3], col, 1.0);
        }
    }
    let hot_c = !is_unit(hot) && hot == -1;
    if m == "move" { draw_box(c, if hot_c { [255, 220, 60] } else { [235, 235, 235] }, 9.0); }
    if m == "scale" { draw_box(c, if hot_c { [255, 220, 60] } else { [235, 235, 235] }, 11.0); }
    if m == "rotate" { draw_point(c, [235, 235, 235], 4.0); }
}

// ---- input ------------------------------------------------------------------
fn pointer_down(ev) {
    let sel = selection();
    if sel.len() == 0 { return false; }
    let c = center();
    let pc = project(c);
    if is_unit(pc) { return false; }
    let s = units_per_pixel(c) * handle_pixels();
    let ax = gizmo_axes();
    let m = mode();
    let mouse = ev.mouse;
    let hit = -2;          // -2 nothing, -1 center, 0/1/2 axis, 10/11/12 plane
    let best = pick_pixels();
    if m == "move" {
        for k in 0..3 {
            let q = plane_screen(c, ax, k, s);
            if !is_unit(q) && in_quad(mouse, q) { hit = 10 + k; }
        }
    }
    if hit >= 10 {
        // a plane square
    } else if m != "rotate" && dist2d(mouse, pc) < best {
        hit = -1;
    } else {
        for k in 0..3 {
            if m == "rotate" {
                let pts = ring(c, ax[k], s * 0.8);
                for i in 0..48 {
                    let a = project(pts[i]);
                    let b = project(pts[i + 1]);
                    if is_unit(a) || is_unit(b) { continue; }
                    let d = seg_dist(mouse, a, b);
                    if d < best { best = d; hit = k; }
                }
            } else {
                let pt = project(vadd(c, vscale(ax[k], s)));
                if is_unit(pt) || dist2d(pt, pc) < 10.0 { continue; }
                let d = seg_dist(mouse, pc, pt);
                if d < best { best = d; hit = k; }
            }
        }
    }
    if hit == -2 { return false; }

    // remember where everything started so each drag frame is absolute
    let start = #{};
    for id in sel {
        start[id.to_string()] = #{ pos: get_pos(id), rot: get_rot(id), size: get_size(id) };
    }
    let a = if hit >= 0 && hit < 3 { ax[hit] } else { [0.0, 0.0, 0.0] };
    let p0 = plane_point(ev, c);
    if hit >= 10 {
        let pp = ray_plane(ev, c, ax[hit - 10]);
        if is_unit(pp) { return false; }
        p0 = pp;
    }
    state_set("drag", #{
        axis: hit, mode: m, c: c, s: s, ax: ax, ids: sel, start: start,
        mouse: mouse, t0: axis_param(ev, c, a), p0: p0
    });
    state_set("hot", hit);
    true
}

fn pointer_drag(ev) {
    let st = state_get("drag");
    if is_unit(st) { return; }
    let snap = !ev.ctrl;
    let g = grid();
    let k = st.axis;

    if st.mode == "move" {
        let delta = [0.0, 0.0, 0.0];
        if k >= 10 {
            // slide in one plane; the third axis stays put
            let n = k - 10;
            let p = ray_plane(ev, st.c, st.ax[n]);
            if is_unit(p) { return; }
            let v = vsub(p, st.p0);
            let u = st.ax[(n + 1) % 3];
            let w = st.ax[(n + 2) % 3];
            let tu = vdot(v, u);
            let tw = vdot(v, w);
            if snap && g > 0.0 { tu = snap_to(tu, g); tw = snap_to(tw, g); }
            delta = vadd(vscale(u, tu), vscale(w, tw));
        } else if k >= 0 {
            let t = axis_param(ev, st.c, st.ax[k]) - st.t0;
            if snap && g > 0.0 { t = snap_to(t, g); }
            delta = vscale(st.ax[k], t);
        } else {
            delta = vsub(plane_point(ev, st.c), st.p0);
            if snap && g > 0.0 {
                for i in 0..3 { delta[i] = snap_to(delta[i], g); }
            }
        }
        for id in st.ids {
            let s0 = st.start[id.to_string()];
            set_pos(id, vadd(s0.pos, delta));
        }
        status(`Move  x ${delta[0].round()} y ${delta[1].round()} z ${delta[2].round()}  (Ctrl = free)`);
    } else if st.mode == "rotate" {
        let pc = project(st.c);
        if is_unit(pc) { return; }
        let a0 = atan2(st.mouse[1] - pc[1], st.mouse[0] - pc[0]);
        let a1 = atan2(ev.mouse[1] - pc[1], ev.mouse[0] - pc[0]);
        let deg = rad2deg(a1 - a0);
        // screen angles grow clockwise; flip when the axis points at the viewer
        if vdot(st.ax[k], ev.dir) < 0.0 { deg = -deg; }
        while deg > 180.0 { deg -= 360.0; }
        while deg < -180.0 { deg += 360.0; }
        if snap { deg = snap_to(deg, rotate_snap()); }
        for id in st.ids {
            let s0 = st.start[id.to_string()];
            set_pos(id, s0.pos);
            set_rot(id, s0.rot);
            rotate_world(id, st.ax[k], deg, st.c);
        }
        status(`Rotate ${deg.round()} degrees  (Ctrl = free)`);
    } else {
        let f = 1.0;
        if k >= 0 {
            f = 1.0 + (axis_param(ev, st.c, st.ax[k]) - st.t0) / st.s;
        } else {
            f = 1.0 + ((ev.mouse[0] - st.mouse[0]) - (ev.mouse[1] - st.mouse[1])) / 120.0;
        }
        if f < 0.02 { f = 0.02; }
        let single = st.ids.len() == 1;
        for id in st.ids {
            let s0 = st.start[id.to_string()];
            let size = s0.size;
            let rel = vsub(s0.pos, st.c);
            if k >= 0 {
                let a = st.ax[k];
                // the object's own axis that lines up best with the handle
                let j = 0;
                let bestd = -1.0;
                for i in 0..3 {
                    let d = vdot(axis(id, i), a).abs();
                    if d > bestd { bestd = d; j = i; }
                }
                size[j] = size[j] * f;
                if single && snap && g > 0.0 {
                    size[j] = snap_to(size[j], g);
                    if size[j] < g { size[j] = g; }
                }
                set_pos(id, vadd(s0.pos, vscale(a, vdot(rel, a) * (f - 1.0))));
            } else {
                for i in 0..3 { size[i] = size[i] * f; }
                set_pos(id, vadd(st.c, vscale(rel, f)));
            }
            set_size(id, size);
        }
        status(`Scale x${(f * 100.0).round() / 100.0}  (Ctrl = no size snapping)`);
    }
}

fn pointer_up(ev) {
    state_set("drag", ());
    state_set("hot", ());
    false
}
