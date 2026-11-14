// Snap To Part (move hook)
// While you drag a selection, its faces snap flush against nearby parts and it
// is pushed back out instead of overlapping them. Toggle it in the Plugins window.

fn info() {
    #{
        name: "Snap To Part",
        kind: "move",
        description: "While moving, snaps the selection flush against nearby parts and stops it overlapping them."
    }
}

fn adjust_move(ids, box, delta) {
    let g = grid();
    let th = if g > 0.5 { g } else { 0.5 };
    let eps = 0.001;
    let mn = vadd(box.min, delta);
    let mx = vadd(box.max, delta);

    // 1. Flush snapping: for each axis, find the closest face we could touch.
    let best = [th, th, th];
    let found = [false, false, false];
    for id in parts() {
        if ids.contains(id) { continue; }
        let b = bounds(id);
        for a in 0..3 {
            let o1 = (a + 1) % 3;
            let o2 = (a + 2) % 3;
            // only parts that overlap us on the other two axes can be "next to" us
            if mx[o1] <= b.min[o1] + eps || mn[o1] >= b.max[o1] - eps { continue; }
            if mx[o2] <= b.min[o2] + eps || mn[o2] >= b.max[o2] - eps { continue; }
            for c in [b.min[a] - mx[a], b.max[a] - mn[a]] {
                if c.abs() < best[a].abs() {
                    best[a] = c;
                    found[a] = true;
                }
            }
        }
    }
    let out = delta;
    for a in 0..3 {
        if found[a] { out[a] = delta[a] + best[a]; }
    }

    // 2. Never overlap: push out along the axis of least penetration.
    let mn = vadd(box.min, out);
    let mx = vadd(box.max, out);
    for id in parts() {
        if ids.contains(id) { continue; }
        let b = bounds(id);
        let pen = [0.0, 0.0, 0.0];
        let inside = true;
        for a in 0..3 {
            let p1 = mx[a] - b.min[a];
            let p2 = b.max[a] - mn[a];
            if p1 <= eps || p2 <= eps { inside = false; break; }
            pen[a] = if p1 < p2 { -p1 } else { p2 };
        }
        if !inside { continue; }
        let k = 0;
        for a in 1..3 {
            if pen[a].abs() < pen[k].abs() { k = a; }
        }
        out[k] += pen[k];
        mn[k] += pen[k];
        mx[k] += pen[k];
    }
    out
}
