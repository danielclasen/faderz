//! The bridge's configuration file (JSON).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::mcu::Device;
use crate::surface::UnitSpec;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// The faderz WebSocket endpoints, one per replica:
    /// `wss://host/api/v1/ws`.
    pub endpoints: Vec<String>,
    /// Where the session id (and signed-in tokens) are kept.
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default)]
    pub auth: Auth,
    /// The units, left to right; each one is a console on the server.
    pub units: Vec<Unit>,
    /// The control the X-Touch master fader drives; the program bus's
    /// fader when absent.
    #[serde(default)]
    pub master: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields,
    tag = "mode"
)]
pub enum Auth {
    /// No credential: faderz runs with `auth.mode` none.
    #[default]
    None,
    /// A bearer token: faderz's static `auth.token`, or an access token of
    /// the OIDC provider. From `token`, else from the file `tokenFile`.
    Token {
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        token_file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Unit {
    /// The console id on the server (`^[a-z][a-z0-9-]{0,31}$`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model: Model,
    /// A part of the MIDI input port's name.
    pub midi_in: String,
    /// A part of the MIDI output port's name; the input's when absent.
    #[serde(default)]
    pub midi_out: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Model {
    /// An X-Touch in MC mode: eight strips and a master fader.
    #[default]
    XTouch,
    /// An X-Touch Extender in MC mode: eight strips.
    XTouchExtender,
}

fn default_state_dir() -> PathBuf {
    PathBuf::from("/var/lib/faderz-bridge")
}

fn id_ok(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let config: Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if config.endpoints.is_empty() {
            return Err("endpoints: at least one".into());
        }
        for e in &config.endpoints {
            if !(e.starts_with("ws://") || e.starts_with("wss://")) {
                return Err(format!("endpoint {e:?} is not a ws:// or wss:// URL"));
            }
        }
        if config.units.is_empty() {
            return Err("units: at least one".into());
        }
        let mut seen = std::collections::HashSet::new();
        for u in &config.units {
            if !id_ok(&u.id) {
                return Err(format!("unit id {:?} is not a faderz id", u.id));
            }
            if u.name.is_empty() || u.name.len() > 64 {
                return Err(format!("unit {}: name of 1 to 64 characters", u.id));
            }
            if !seen.insert(&u.id) {
                return Err(format!("unit id {} twice", u.id));
            }
        }
        if let Auth::Token {
            token: None,
            token_file: None,
        } = config.auth
        {
            return Err("auth mode token needs token or tokenFile".into());
        }
        Ok(config)
    }

    /// The surface's units, left to right.
    pub fn unit_specs(&self) -> Vec<UnitSpec> {
        self.units
            .iter()
            .map(|u| UnitSpec {
                id: u.id.clone(),
                name: u.name.clone(),
                device: match u.model {
                    Model::XTouch => Device::Main,
                    Model::XTouchExtender => Device::Extender,
                },
            })
            .collect()
    }

    pub fn session_file(&self) -> PathBuf {
        self.state_dir.join("session")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configuration_names_endpoints_units_and_auth() {
        let c = Config::parse(
            r#"{
              "endpoints": ["wss://faderz.example/api/v1/ws", "wss://faderz-2.example/api/v1/ws"],
              "stateDir": "/tmp/b",
              "auth": { "mode": "token", "tokenFile": "/run/secrets/faderz" },
              "units": [
                { "id": "xt", "name": "X-Touch", "midiIn": "X-Touch" },
                { "id": "xt-ext", "name": "X-Touch Ext", "model": "x-touch-extender", "midiIn": "X-Touch-Ext" }
              ],
              "master": "/monitor/level"
            }"#,
        )
        .unwrap();
        assert_eq!(c.endpoints.len(), 2);
        assert_eq!(c.session_file(), PathBuf::from("/tmp/b/session"));
        let specs = c.unit_specs();
        assert_eq!(specs[0].device, Device::Main);
        assert_eq!(specs[1].device, Device::Extender);
        assert_eq!(c.master.as_deref(), Some("/monitor/level"));
    }

    #[test]
    fn mistakes_are_refused() {
        let unit = r#"{ "id": "xt", "name": "X", "midiIn": "X" }"#;
        for (text, why) in [
            (format!(r#"{{ "endpoints": [], "units": [{unit}] }}"#), "endpoints"),
            (format!(r#"{{ "endpoints": ["http://x"], "units": [{unit}] }}"#), "ws://"),
            (r#"{ "endpoints": ["ws://x"], "units": [] }"#.into(), "units"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}, {unit}] }}"#), "twice"),
            (r#"{ "endpoints": ["ws://x"], "units": [{ "id": "X", "name": "X", "midiIn": "X" }] }"#.into(), "faderz id"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "auth": {{ "mode": "token" }} }}"#), "tokenFile"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "extra": 1 }}"#), "unknown field"),
        ] {
            let e = Config::parse(&text).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
    }
}
