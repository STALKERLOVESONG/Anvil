// Embeds the anvil icon (and version info) into anvil.exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/anvil.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/anvil.ico");
        res.set("ProductName", "Anvil");
        res.set("FileDescription", "Anvil level editor");
        if let Err(e) = res.compile() {
            // never fail the build over an icon
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }
}
