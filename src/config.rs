//! The configuration file: listeners, auth and API settings of
//! samplez-service, where the replicated state lives, and the shape.

use std::path::{Path, PathBuf};

use samplez_service::config::{ApiConfig, AuthConfig, AuthMode, Listeners};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::contract::{check, contract};
use crate::shape::Shape;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub listeners: Listeners,
    /// In `proxy` mode the API listener takes the user and groups the
    /// identity headers name, so it has to be reachable from the
    /// authenticating proxy alone: [`Config::parse`] refuses an API bind
    /// address other than loopback, where the proxy runs beside faderz.
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub replica: ReplicaConfig,
    /// Model `Shape`.
    pub shape: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplicaConfig {
    /// The replica's id towards clients (model `ReplicaRef.id`); the host
    /// name when absent.
    pub name: Option<String>,
    /// Where the replicated log and snapshots live.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

impl Default for ReplicaConfig {
    fn default() -> Self {
        ReplicaConfig {
            name: None,
            data_dir: default_data_dir(),
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/faderz")
}

impl Config {
    /// Reads and checks `path`: the shape against the contract and its
    /// own rules ([`Shape::parse`]), the replica name against model `Id`.
    pub fn load(path: &Path) -> Result<(Config, Shape, String), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::parse(&text, hostname)
    }

    pub fn parse(text: &str, host: fn() -> String) -> Result<(Config, Shape, String), String> {
        let config: Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
        // In proxy mode the API takes who the identity headers name, so
        // only the proxy may reach it: the proxy runs beside faderz, as a
        // sidecar in its pod, and nothing else can connect to loopback.
        if config.auth.mode == AuthMode::Proxy && !config.listeners.api.bind.is_loopback() {
            return Err(format!(
                "auth.mode proxy needs listeners.api.bind on loopback, not {}: \
                 identity headers from anything but the proxy would be taken",
                config.listeners.api.bind
            ));
        }
        let shape = Shape::parse(&config.shape).map_err(|e| format!("shape: {e}"))?;
        let name = config.replica.name.clone().unwrap_or_else(host);
        check(&contract().id, &json!(name)).map_err(|e| {
            format!("replica.name {name:?}: {e}; name the replica in the configuration")
        })?;
        Ok((config, shape, name))
    }
}

/// The host name, lowercased.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for its length.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    match std::str::from_utf8(&buf[..end]) {
        Ok(name) if rc == 0 => name.to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::small;

    fn with(replica: Value) -> String {
        json!({ "replica": replica, "shape": small() }).to_string()
    }

    #[test]
    fn the_replica_is_named_after_the_host_unless_configured() {
        let (_, _, name) = Config::parse(&with(json!({})), || "faderz-0".into()).unwrap();
        assert_eq!(name, "faderz-0");
        let (_, _, name) = Config::parse(&with(json!({ "name": "r1" })), || "x".into()).unwrap();
        assert_eq!(name, "r1");
    }

    /// Proxy mode serves the API to the proxy beside it alone.
    #[test]
    fn proxy_mode_binds_the_api_to_loopback_only() {
        let config = |bind: &str| {
            json!({
                "listeners": { "api": { "bind": bind, "port": 8080 } },
                "auth": { "mode": "proxy" },
                "replica": { "name": "r1" },
                "shape": small(),
            })
            .to_string()
        };
        let e = Config::parse(&config("0.0.0.0"), || unreachable!()).unwrap_err();
        assert!(e.contains("loopback"), "{e}");
        assert!(Config::parse(&config("10.0.0.5"), || unreachable!()).is_err());
        assert!(Config::parse(&config("127.0.0.1"), || unreachable!()).is_ok());
        assert!(Config::parse(&config("::1"), || unreachable!()).is_ok());
    }

    /// A name clients could not be told (`ReplicaRef.id`) refuses the
    /// start rather than every message later.
    #[test]
    fn a_name_that_is_not_an_id_is_refused() {
        let e = Config::parse(&with(json!({})), || "Host.example".into()).unwrap_err();
        assert!(e.contains("replica.name"), "{e}");
    }
}
