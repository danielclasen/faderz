//! The bridge's session: its id, kept across restarts in a file only its
//! owner can read, as the contract keeps a console's session across bridge
//! restarts (model `SessionId`), and the handle the server publishes for it.

use std::io::Write;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// 32 random bytes in base64url without padding: 43 characters.
pub fn random_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// A random (version 4) UUID, for command ids.
pub fn uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the operating system's random source");
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Model `OwnerHandle`: the first 16 bytes of the SHA-256 of the session
/// id in base64url without padding.
pub fn handle(session: &str) -> String {
    URL_SAFE_NO_PAD.encode(&Sha256::digest(session.as_bytes())[..16])
}

fn valid(id: &str) -> bool {
    (22..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The session id stored at `path`, or a new one stored there with mode
/// 0600.
pub fn load_or_create(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) if valid(text.trim()) => return Ok(text.trim().to_owned()),
        Ok(_) => {
            tracing::warn!(path = %path.display(), "no session id in the file; drawing a new one")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let id = random_id()?;
    write_private(path, &id)?;
    Ok(id)
}

/// Writes `text` to `path` readable by its owner only, replacing it
/// atomically.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_handle_is_the_contracts() {
        assert_eq!(
            handle("session-a-0123456789abcdef"),
            "JF68nDKMSrXn0ouxbNvKIQ"
        );
        assert_eq!(handle("abc"), "ungWv48Bz-pBQUDeXa4iIw");
    }

    #[test]
    fn the_session_survives_a_restart_in_a_private_file() {
        let dir = std::env::temp_dir().join(format!("bridge-session-{}", random_id().unwrap()));
        let path = dir.join("session");
        let first = load_or_create(&path).unwrap();
        assert!(valid(&first) && first.len() == 43);
        assert_eq!(load_or_create(&path).unwrap(), first);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::write(&path, "short").unwrap();
        assert_ne!(load_or_create(&path).unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn command_ids_are_version_4_uuids() {
        let id = uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
