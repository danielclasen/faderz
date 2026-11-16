//! The configuration file: listeners, auth and API settings of
//! samplez-service, where the replicated state lives, the shape, and the
//! MXL flows the engine reads and writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use samplez_service::config::{ApiConfig, AuthConfig, AuthMode, Listeners};
use serde::Deserialize;
use serde_json::{Map, Value, json};

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
    /// The MXL flows of the audio. Absent, faderz runs no engine: the
    /// state is served, nothing is mixed, and `/readyz` fails.
    #[serde(default)]
    pub mxl: Option<Mxl>,
    /// samplez's engine timing: `sampleRate`, `blockSize`,
    /// `readerMarginSamples`, `maxCatchupBlocks`, `pruneAfterBlocks`,
    /// `rampSamples`. samplez checks the values and supplies defaults.
    #[serde(default)]
    pub engine: Map<String, Value>,
    /// samplez's RT thread settings (`enabled`, `priority`, `rtCpu`).
    #[serde(default)]
    pub rt: Map<String, Value>,
    /// The engine configuration [`Config::parse`] built from the above,
    /// with a matrix of zeros: the matrix comes from the live state.
    #[serde(skip)]
    pub audio: Option<samplez::Config>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mxl {
    /// The libmxl shared library, loaded at start.
    #[serde(default = "default_library_path")]
    pub library_path: PathBuf,
    pub domains: Vec<Domain>,
    /// The flows each channel reads, by channel id; every channel has one.
    pub inputs: BTreeMap<String, Input>,
    /// The flow each output of the shape writes, by output id; every
    /// output has one.
    pub outputs: BTreeMap<String, Output>,
}

fn default_library_path() -> PathBuf {
    PathBuf::from("/opt/mxl/lib/libmxl.so")
}

/// A mounted MXL domain.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Domain {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    /// The flow of input a.
    pub flow: String,
    /// The flow of input b; required exactly for a channel with
    /// `altInput`.
    #[serde(default)]
    pub alt_flow: Option<String>,
    /// Required with more than one domain.
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    pub flow: String,
    /// Required with more than one domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// The `engine` keys faderz passes to samplez; the others it sets itself.
const ENGINE_KEYS: [&str; 6] = [
    "sampleRate",
    "blockSize",
    "readerMarginSamples",
    "maxCatchupBlocks",
    "pruneAfterBlocks",
    "rampSamples",
];

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
        let mut config = config;
        config.audio = config.audio(&shape)?;
        let name = config.replica.name.clone().unwrap_or_else(host);
        check(&contract().id, &json!(name)).map_err(|e| {
            format!("replica.name {name:?}: {e}; name the replica in the configuration")
        })?;
        Ok((config, shape, name))
    }
}

impl Config {
    /// The engine configuration of the `mxl` section, with a matrix of
    /// zeros: one input per channel reading its flow of input a, one
    /// output per output of the shape.
    fn audio(&self, shape: &Shape) -> Result<Option<samplez::Config>, String> {
        let Some(mxl) = &self.mxl else {
            return Ok(None);
        };
        if let Some(k) = self
            .engine
            .keys()
            .find(|k| !ENGINE_KEYS.contains(&k.as_str()))
        {
            return Err(format!("engine.{k}: not a key faderz passes to samplez"));
        }
        let only = (mxl.domains.len() == 1).then(|| mxl.domains[0].name.clone());
        let domain = |at: String, given: &Option<String>| {
            given
                .clone()
                .or_else(|| only.clone())
                .ok_or(format!("{at}.domain: required with more than one domain"))
        };
        let mut inputs = Vec::new();
        for c in &shape.channels {
            let at = format!("mxl.inputs.{}", c.id);
            let input = mxl.inputs.get(&c.id).ok_or(format!("{at}: missing"))?;
            match (&input.alt_flow, c.alt_input) {
                (Some(_), false) => {
                    return Err(format!("{at}.altFlow: the channel has no altInput"));
                }
                // The state takes input b for it, which would read nothing.
                (None, true) => {
                    return Err(format!("{at}.altFlow: required, the channel has altInput"));
                }
                _ => {}
            }
            inputs.push(json!({
                "name": c.id,
                "domain": domain(at, &input.domain)?,
                "flowId": input.flow,
                "channels": c.width,
            }));
        }
        let mut outputs = Vec::new();
        for o in &shape.outputs {
            let at = format!("mxl.outputs.{}", o.id);
            let output = mxl.outputs.get(&o.id).ok_or(format!("{at}: missing"))?;
            outputs.push(json!({
                "name": o.id,
                "domain": domain(at, &output.domain)?,
                "flowId": output.flow,
                "label": o.name,
                "group": o.id,
                "role": "Audio",
                "channels": o.width,
            }));
        }
        if let Some(id) = mxl.inputs.keys().find(|id| !shape.is_channel(id)) {
            return Err(format!("mxl.inputs.{id}: not a channel of the shape"));
        }
        if let Some(id) =
            (mxl.outputs.keys()).find(|id| !shape.outputs.iter().any(|o| &o.id == *id))
        {
            return Err(format!("mxl.outputs.{id}: not an output of the shape"));
        }
        let rows: usize = shape.channels.iter().map(|c| c.width).sum();
        let columns: usize = shape.outputs.iter().map(|o| o.width).sum();
        let mut v = Value::Object(self.engine.clone());
        v["libmxl"] = json!(mxl.library_path);
        v["domains"] = mxl
            .domains
            .iter()
            .map(|d| json!({ "name": d.name, "path": d.path }))
            .collect();
        v["inputs"] = json!(inputs);
        v["outputs"] = json!(outputs);
        v["matrix"] = json!(vec![vec![0.0f32; columns]; rows]);
        v["rt"] = Value::Object(self.rt.clone());
        let engine =
            samplez::Config::parse(&v.to_string()).map_err(|e| format!("engine: {}", e.0))?;
        if engine.sample_rate != shape.sample_rate {
            return Err(format!(
                "engine.sampleRate {} differs from the shape's {}",
                engine.sample_rate, shape.sample_rate
            ));
        }
        for (slot, c) in shape.channels.iter().enumerate() {
            if let Some(b) = &mxl.inputs[&c.id].alt_flow {
                engine
                    .check_input_flow(slot, b)
                    .map_err(|e| format!("mxl.inputs.{}.altFlow: {}", c.id, e.0))?;
            }
        }
        Ok(Some(engine))
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

    /// Every channel the state lets select input b has a flow for it.
    #[test]
    fn an_alt_input_needs_an_alt_flow() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = |ch1: Value| {
            json!({
                "replica": { "name": "r1" },
                "shape": small(),
                "mxl": {
                    "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                    "inputs": { "ch1": ch1, "ch2": { "flow": id(2) }, "st1": { "flow": id(3) } },
                    "outputs": { "out-pgm": { "flow": id(4) } },
                },
            })
            .to_string()
        };
        let e = Config::parse(&config(json!({ "flow": id(1) })), || unreachable!()).unwrap_err();
        assert!(e.contains("mxl.inputs.ch1.altFlow: required"), "{e}");
        let ok = config(json!({ "flow": id(1), "altFlow": id(5) }));
        Config::parse(&ok, || unreachable!()).unwrap();
    }
}
