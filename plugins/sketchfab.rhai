// Sketchfab browser (panel plugin)
// Search Sketchfab for downloadable models, see their pictures, and download
// one straight into the model library (glTF, with its textures).
//
// Downloading needs your own Sketchfab API token:
//   sketchfab.com > your avatar > Settings > Password & API > API token.
// It's stored encrypted for your Windows account (never in the map or the
// settings file). Models are under their authors' licenses: check them on
// the model's Sketchfab page before you use one.

fn info() {
    #{
        name: "Sketchfab",
        kind: "panel",
        description: "Search Sketchfab and download models into the library (needs your Sketchfab API token).",
        hosts: ["api.sketchfab.com", "media.sketchfab.com", "sketchfab.com", "sketchfab-prod-media.s3.amazonaws.com"]
    }
}

fn st(k, d) {
    let v = state_get(k);
    if type_of(v) == "()" { d } else { v }
}

fn thumb(r) {
    let best = "";
    let bw = 99999;
    if type_of(r.thumbnails) == "map" {
        for im in r.thumbnails.images {
            // smallest picture that's still at least 200 px wide
            if im.width >= 200 && im.width < bw { bw = im.width; best = im.url; }
        }
    }
    best
}

fn search() {
    let q = st("q", "");
    if q == "" { status("Type something to search for"); return; }
    let body = http_get("https://api.sketchfab.com/v3/search?type=models&downloadable=true&count=24&q=" + url_encode(q));
    let data = json_parse(body);
    if type_of(data) != "map" || type_of(data.results) != "array" {
        state_set("msg", "Couldn't reach Sketchfab (see the Plugins log).");
        return;
    }
    let list = [];
    for r in data.results {
        let author = if type_of(r.user) == "map" { r.user.displayName } else { "" };
        let lic = if type_of(r.license) == "map" { r.license.label } else { "" };
        list.push(#{ uid: r.uid, name: r.name, author: author, license: lic, thumb: thumb(r), url: r.viewerUrl });
    }
    state_set("results", list);
    state_set("msg", `${list.len()} downloadable models for "${q}"`);
}

// The download links for a model. Personal API tokens use "Token ...",
// OAuth tokens "Bearer ...": the one that worked last time is tried first.
fn download_info(uid, token) {
    let api = "https://api.sketchfab.com/v3/models/" + uid + "/download";
    let schemes = if st("auth", "Token") == "Bearer" { ["Bearer", "Token"] } else { ["Token", "Bearer"] };
    for s in schemes {
        let d = json_parse(http_get(api, #{ Authorization: s + " " + token }));
        if type_of(d) == "map" && (type_of(d.gltf) == "map" || type_of(d.glb) == "map") {
            state_set("auth", s);
            return d;
        }
    }
    ()
}

fn size_of(x) {
    if type_of(x) == "map" && (type_of(x.size) == "i64" || type_of(x.size) == "f64") { x.size } else { -1 }
}

fn start_download(uid, name) {
    let token = secret_get("token");
    if token == "" { state_set("msg", "Save your Sketchfab API token first (top of this window)."); return; }
    for j in st("jobs", []) {
        if !j.done && j.uid == uid { state_set("msg", name + " is already downloading"); return; }
    }
    let d = download_info(uid, token);
    if type_of(d) != "map" {
        state_set("msg", "Sketchfab didn't give a download (wrong token, or the model isn't downloadable).");
        return;
    }
    // Anvil cleans up the file name itself (safe characters, keeps the extension)
    let file = uid + "_" + name;
    // both kinds hold the same model: take the smaller download
    let use_glb = type_of(d.glb) == "map";
    if use_glb && type_of(d.gltf) == "map" {
        let gs = size_of(d.glb);
        let zs = size_of(d.gltf);
        if gs > 0 && zs > 0 && zs < gs { use_glb = false; }
    }
    let job = -1;
    let kind = "";
    if use_glb {
        job = download(d.glb.url, file + ".glb");
        kind = "glb";
    } else if type_of(d.gltf) == "map" {
        job = download(d.gltf.url, file + ".zip");
        kind = "zip";
    } else {
        state_set("msg", "No glTF download for this model.");
        return;
    }
    let jobs = st("jobs", []);
    jobs.push(#{ job: job, kind: kind, name: name, uid: uid, done: false, unzip: -1 });
    state_set("jobs", jobs);
    state_set("msg", "Downloading " + name + "…");
}

// called every frame while the window is open: finish downloads (unzipping
// happens in the background too, so the editor never stalls)
fn poll_jobs() {
    let jobs = st("jobs", []);
    let changed = false;
    for i in 0..jobs.len() {
        let j = jobs[i];
        if j.done { continue; }
        if j.unzip >= 0 {
            let u = job_status(j.unzip);
            if u == "done" {
                let found = find_files(job_file(j.unzip), "gltf");
                if found.len() > 0 && import_model(found[0], j.name) {
                    state_set("msg", "Added " + j.name + " to the model library");
                } else {
                    state_set("msg", "Downloaded " + j.name + " but couldn't find its .gltf");
                }
                j.done = true;
            } else if u.starts_with("error") {
                state_set("msg", j.name + ": " + u);
                j.done = true;
            }
            jobs[i] = j;
            changed = true;
            continue;
        }
        let s = job_status(j.job);
        if s == "done" {
            let file = job_file(j.job);
            if j.kind == "zip" {
                j.unzip = unzip_job(file);
            } else {
                if import_model(file, j.name) {
                    state_set("msg", "Added " + j.name + " to the model library");
                }
                j.done = true;
            }
            jobs[i] = j;
            changed = true;
        } else if s.starts_with("error") {
            state_set("msg", if s.contains("cancelled") { j.name + ": cancelled" } else { j.name + ": " + s });
            j.done = true;
            jobs[i] = j;
            changed = true;
        }
    }
    if changed { state_set("jobs", jobs); }
}

fn panel() {
    poll_jobs();
    let out = [];
    let has_token = secret_get("token") != "";
    if has_token {
        out.push(#{ t: "row", items: [
            #{ t: "label", text: "API token saved (encrypted).", small: true },
            #{ t: "button", id: "forget_token", text: "Forget token" }
        ]});
    } else {
        out.push(#{ t: "label", text: "To download, paste your Sketchfab API token (Settings > Password & API on sketchfab.com):", small: true });
        out.push(#{ t: "row", items: [
            #{ t: "input", id: "token_in", value: st("token_in", ""), hint: "API token", password: true },
            #{ t: "button", id: "save_token", text: "Save token" },
            #{ t: "link", text: "get a token", url: "https://sketchfab.com/settings/password" }
        ]});
    }
    out.push(#{ t: "separator" });
    out.push(#{ t: "row", items: [
        #{ t: "input", id: "q", value: st("q", ""), hint: "search, e.g. barrel, chair, tree" },
        #{ t: "button", id: "search", text: "Search" }
    ]});
    let msg = st("msg", "");
    if msg != "" { out.push(#{ t: "label", text: msg, small: true }); }
    for j in st("jobs", []) {
        if !j.done {
            if j.unzip >= 0 {
                out.push(#{ t: "label", text: "Unpacking " + j.name + "…", small: true });
            } else {
                out.push(#{ t: "row", items: [
                    #{ t: "label", text: "Downloading " + j.name, small: true },
                    #{ t: "progress", value: job_progress(j.job) },
                    #{ t: "button", id: "cancel:" + j.job, text: "Cancel" }
                ] });
            }
        }
    }
    out.push(#{ t: "separator" });
    for r in st("results", []) {
        let info = [ #{ t: "label", text: r.name, strong: true }, #{ t: "label", text: "by " + r.author + (if r.license != "" { " · " + r.license } else { "" }), small: true } ];
        let row = [];
        if r.thumb != "" { row.push(#{ t: "image", url: r.thumb, w: 128.0, h: 72.0 }); }
        row.push(#{ t: "label", text: r.name + "\nby " + r.author + (if r.license != "" { "\n" + r.license } else { "" }) });
        row.push(#{ t: "button", id: "dl:" + r.uid, text: "Download" });
        if r.url != () && r.url != "" { row.push(#{ t: "link", text: "page", url: r.url }); }
        out.push(#{ t: "row", items: row });
    }
    out
}

fn panel_event(id, value) {
    if id == "q" { state_set("q", value); }
    else if id == "token_in" { state_set("token_in", value); }
    else if id == "search" { search(); }
    else if id == "save_token" {
        // note: Rhai's trim() trims the string in place and returns nothing
        let t = st("token_in", "");
        t.trim();
        if t != "" && secret_set("token", t) { state_set("token_in", ""); state_set("msg", "Token saved."); }
    }
    else if id == "forget_token" { secret_set("token", ""); state_set("msg", "Token forgotten."); }
    else if id.starts_with("cancel:") {
        job_cancel(parse_int(id.sub_string(7)));
    }
    else if id.starts_with("dl:") {
        let uid = id.sub_string(3);
        let name = uid;
        for r in st("results", []) { if r.uid == uid { name = r.name; } }
        start_download(uid, name);
    }
}
