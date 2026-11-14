// Random Colors (command)
// A tiny example plugin: gives every selected part a random color.

fn info() {
    #{
        name: "Random Colors",
        kind: "command",
        description: "Example plugin: gives every selected part a random color."
    }
}

fn run() {
    let sel = selection();
    if sel.len() == 0 {
        status("Random Colors: select some parts first");
        return;
    }
    let seed = 12345;
    for id in sel {
        seed = (seed * 1103515245 + 12345 + id) % 2147483648;
        let r = 60 + seed % 196;
        seed = (seed * 1103515245 + 12345) % 2147483648;
        let g = 60 + seed % 196;
        seed = (seed * 1103515245 + 12345) % 2147483648;
        let b = 60 + seed % 196;
        set_color(id, [r, g, b]);
    }
    status("Random Colors: recolored " + sel.len() + " part(s)");
}
