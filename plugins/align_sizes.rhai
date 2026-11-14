// Align Sizes (tool)
// 1. Click an edge of the part you want to stretch (wall, floor, any box).
// 2. Click an edge on the target part.
// The first part is resized along its own axis so the clicked edge lines up
// with the target edge; the opposite side stays where it was.

fn info() {
    #{
        name: "Align Sizes",
        kind: "tool",
        description: "Click an edge on the part to resize, then an edge on the target. The first part stretches so the edges line up."
    }
}

fn on_click(hit) {
    let first = state_get("first");
    if type_of(first) == "()" {
        state_set("first", hit);
        status("Align Sizes: now click the edge to line up with (Esc to cancel)");
        return;
    }
    state_set("first", ());
    let id = first.id;
    if id == hit.id {
        status("Align Sizes: pick an edge on a different part");
        return;
    }
    let ea = first.edge;
    let mid_a = vscale(vadd(ea[0], ea[1]), 0.5);
    let mid_b = vscale(vadd(hit.edge[0], hit.edge[1]), 0.5);
    let la = to_local(id, mid_a);
    let lb = to_local(id, mid_b);
    let size = get_size(id);

    // The clicked edge lies on the box faces where |local coord| == half size.
    // Of those, stretch along the one that moves the edge the most.
    let best = -1;
    let bestd = 0.0;
    for k in 0..3 {
        let half = size[k] / 2.0;
        if (la[k].abs() - half).abs() < 0.05 {
            let d = lb[k] - la[k];
            if d.abs() > bestd.abs() {
                best = k;
                bestd = d;
            }
        }
    }
    if best < 0 || bestd.abs() < 0.0001 {
        status("Align Sizes: those edges are already lined up (or the first edge isn't on a box face)");
        return;
    }
    let sign = if la[best] > 0.0 { 1.0 } else { -1.0 };
    let new_size = size;
    new_size[best] = size[best] + bestd * sign;
    if new_size[best] < 0.01 {
        status("Align Sizes: the target is on the far side of the part");
        return;
    }
    set_size(id, new_size);
    let ax = axis(id, best);
    set_pos(id, vadd(get_pos(id), vscale(ax, bestd / 2.0)));
    set_selection([id]);
    status("Align Sizes: resized " + get_name(id) + " by " + bestd);
}
