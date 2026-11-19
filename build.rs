use std::path::Path;

fn main() {
    // The web UI is embedded from web/dist: build again when it changes or
    // appears, so a binary never carries a stale UI or none after a UI build.
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=Cargo.lock");
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let lock = std::fs::read_to_string(Path::new(&dir).join("Cargo.lock"))
        .expect("Cargo.lock is committed and read at build");
    println!("cargo:rustc-env=FADERZ_SAMPLEZ={}", samplez(&lock));
}

/// The samplez release Cargo.lock pins: the tag of its git source without
/// the `v`, or for a commit no tag names, the package version with the
/// commit's first 7 hex digits as build metadata.
fn samplez(lock: &str) -> String {
    let entry = (lock.split("[[package]]"))
        .find(|p| p.lines().any(|l| l.trim() == r#"name = "samplez""#))
        .expect("Cargo.lock has the samplez package");
    let field = |key: &str| {
        (entry.lines()).find_map(|l| {
            l.trim()
                .strip_prefix(key)?
                .strip_prefix(" = \"")?
                .strip_suffix('"')
        })
    };
    let version = field("version").expect("the samplez entry has a version");
    let Some(source) = field("source") else {
        return version.to_string();
    };
    let (url, commit) = source.split_once('#').unwrap_or((source, ""));
    let tag = (url.split_once('?').map(|(_, q)| q).unwrap_or(""))
        .split('&')
        .find_map(|p| p.strip_prefix("tag="));
    match tag {
        Some(tag) => tag.strip_prefix('v').unwrap_or(tag).to_string(),
        None => format!("{version}+{}", &commit[..commit.len().min(7)]),
    }
}
