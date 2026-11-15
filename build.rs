// The web UI is embedded from web/dist: build again when it changes or
// appears, so a binary never carries a stale UI or none after a UI build.
fn main() {
    println!("cargo:rerun-if-changed=web/dist");
}
