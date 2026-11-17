//! The configuration file: listeners, auth and API settings of
//! samplez-service, where the replicated state lives, the shape, and the
//! MXL flows the engine reads and writes. The auth settings in
//! [`SETTINGS`] can also be given as command-line flags or environment
//! variables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use samplez_service::config::{ApiConfig, AuthConfig, AuthMode, Listeners};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};
use crate::shape::{Shape, loudness_channels};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub listeners: Listeners,
    /// In `proxy` mode the API listener takes the user and groups the
    /// identity headers name, so it has to be reachable from the
    /// authenticating proxy alone: [`Config::parse`] refuses an API bind
    /// address other than loopback, where the proxy runs beside faderz.
    ///
    /// In `oidc` mode faderz signs users in against an OpenID Connect
    /// provider itself (samplez-service `OidcConfig`); `auth.oidc.operatorRole`
    /// defaults to [`DEFAULT_OPERATOR_ROLE`].
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
    /// `rampSamples`. samplez checks the values and supplies defaults,
    /// but for `readerMarginSamples`, [`READER_MARGIN`] here.
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

/// `readerMarginSamples` when the configuration sets none: 32 ms. An
/// input read through a copy of a flow on another node has needed 1303
/// to 1399 samples, which samplez's default of 576 reads almost wholly
/// late; the margin delays every output by as much.
pub const READER_MARGIN: u32 = 1536;

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

/// The role value `auth.oidc.operatorRole` takes when the file, the flags
/// and the environment leave it out.
pub const DEFAULT_OPERATOR_ROLE: &str = "faderz-operator";

/// How a [`Setting`]'s value is written at its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Text,
    /// A list of strings, separated by commas or spaces.
    List,
}

/// A setting of the configuration file that a command-line flag or an
/// environment variable sets as well. The flag wins over the variable, the
/// variable over the file; an empty variable counts as unset.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub flag: &'static str,
    pub env: &'static str,
    /// Where in the configuration file it goes.
    pub path: &'static [&'static str],
    pub kind: Kind,
    pub help: &'static str,
}

const fn text(
    flag: &'static str,
    env: &'static str,
    path: &'static [&'static str],
    help: &'static str,
) -> Setting {
    Setting {
        flag,
        env,
        path,
        kind: Kind::Text,
        help,
    }
}

pub const SETTINGS: &[Setting] = &[
    text(
        "--auth-mode",
        "FADERZ_AUTH_MODE",
        &["auth", "mode"],
        "none, token, proxy or oidc",
    ),
    text(
        "--oidc-issuer-url",
        "FADERZ_OIDC_ISSUER_URL",
        &["auth", "oidc", "issuerUrl"],
        "the provider's issuer; required in oidc mode",
    ),
    text(
        "--oidc-client-id",
        "FADERZ_OIDC_CLIENT_ID",
        &["auth", "oidc", "clientId"],
        "required in oidc mode",
    ),
    text(
        "--oidc-client-secret-file",
        "FADERZ_OIDC_CLIENT_SECRET_FILE",
        &["auth", "oidc", "clientSecret", "file"],
        "the client secret's file; absent for a public client",
    ),
    text(
        "--oidc-public-url",
        "FADERZ_OIDC_PUBLIC_URL",
        &["auth", "oidc", "publicUrl"],
        "scheme://host[:port] browsers reach faderz under; required in oidc mode",
    ),
    text(
        "--oidc-role-claim",
        "FADERZ_OIDC_ROLE_CLAIM",
        &["auth", "oidc", "roleClaim"],
        "the claim carrying roles or groups; default groups",
    ),
    Setting {
        flag: "--oidc-scopes",
        env: "FADERZ_OIDC_SCOPES",
        path: &["auth", "oidc", "scopes"],
        kind: Kind::List,
        help: "scopes requested besides openid, comma- or space-separated",
    },
    text(
        "--oidc-operator-role",
        "FADERZ_OIDC_OPERATOR_ROLE",
        &["auth", "oidc", "operatorRole"],
        "the claim value granting operator; default faderz-operator",
    ),
    text(
        "--oidc-viewer-role",
        "FADERZ_OIDC_VIEWER_ROLE",
        &["auth", "oidc", "viewerRole"],
        "when set, required to sign in as viewer",
    ),
    Setting {
        flag: "--oidc-allowed-clients",
        env: "FADERZ_OIDC_ALLOWED_CLIENTS",
        path: &["auth", "oidc", "allowedClients"],
        kind: Kind::List,
        help: "clients a bearer access token's azp may name; default the client id",
    },
    text(
        "--oidc-audience",
        "FADERZ_OIDC_AUDIENCE",
        &["auth", "oidc", "audience"],
        "the audience a bearer access token must name; default none, not checked",
    ),
    text(
        "--oidc-session-ttl",
        "FADERZ_OIDC_SESSION_TTL",
        &["auth", "oidc", "sessionTtl"],
        "how long a session lasts: seconds, or with s, m, h or d; default 8h",
    ),
    text(
        "--oidc-session-key-file",
        "FADERZ_OIDC_SESSION_KEY_FILE",
        &["auth", "oidc", "sessionKey", "file"],
        "the session cookie key's file; replicas behind one host share it",
    ),
];

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run {
        config: PathBuf,
        /// `(flag, value)` of each [`SETTINGS`] flag given.
        flags: Vec<(String, String)>,
    },
    Version,
    Help,
}

/// Parses the arguments after the program name: `--config <file>` and
/// any [`SETTINGS`] flag, each as `--flag value` or `--flag=value`, or
/// `--version` or `--help` alone.
pub fn command(args: &[String]) -> Result<Command, String> {
    match args {
        [only] if only == "--version" => return Ok(Command::Version),
        [only] if only == "--help" || only == "-h" => return Ok(Command::Help),
        _ => {}
    }
    let mut config = None;
    let mut flags = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (arg.as_str(), None),
        };
        let known = flag == "--config" || SETTINGS.iter().any(|s| s.flag == flag);
        if !known {
            return Err(format!("unknown argument {arg:?}"));
        }
        let value = match inline {
            Some(v) => v,
            None => args.next().ok_or(format!("{flag} needs a value"))?.clone(),
        };
        if flag == "--config" {
            config = Some(PathBuf::from(value));
        } else {
            flags.push((flag.to_owned(), value));
        }
    }
    let config = config.ok_or("--config <file> is required")?;
    Ok(Command::Run { config, flags })
}

/// The usage text with every flag, its variable and its key.
pub fn usage() -> String {
    let mut text = String::from(
        "usage: faderz --config <file> [flags]\n       faderz --version\n\n\
         Each flag sets the configuration key named after it, as does its\n\
         environment variable; the flag wins over the variable, the variable\n\
         over the file. An empty variable counts as unset.\n\n",
    );
    for s in SETTINGS {
        text.push_str(&format!(
            "  {} <value>\n      {}, {}: {}\n",
            s.flag,
            s.env,
            s.path.join("."),
            s.help
        ));
    }
    text
}

/// Where a setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    /// The configuration file, or the default when the file has none.
    File,
}

/// The value of `setting` in `flags`, else in `env`.
fn given(
    setting: &Setting,
    flags: &[(String, String)],
    env: &impl Fn(&str) -> Option<String>,
) -> Option<(String, Source)> {
    flags
        .iter()
        .rev()
        .find(|(f, _)| f == setting.flag)
        .map(|(_, v)| (v.clone(), Source::Flag))
        .or_else(|| {
            env(setting.env)
                .filter(|v| !v.is_empty())
                .map(|v| (v, Source::Env))
        })
}

/// Where `auth.mode` comes from, and how to name it in the log.
pub fn auth_mode_source(
    flags: &[(String, String)],
    env: impl Fn(&str) -> Option<String>,
) -> String {
    let mode = SETTINGS
        .iter()
        .find(|s| s.path == ["auth", "mode"])
        .expect("auth.mode is a setting");
    match given(mode, flags, &env) {
        Some((_, Source::Flag)) => mode.flag.to_owned(),
        Some((_, Source::Env)) => mode.env.to_owned(),
        _ => "the configuration file".to_owned(),
    }
}

/// Sets each [`SETTINGS`] entry given in `flags`, else in `env`, at its
/// path in `config`, then, in `oidc` mode, [`DEFAULT_OPERATOR_ROLE`] where
/// `auth.oidc` has no operator role.
pub fn apply(
    config: &mut Value,
    flags: &[(String, String)],
    env: impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    for s in SETTINGS {
        let Some((given, _)) = given(s, flags, &env) else {
            continue;
        };
        let value = match s.kind {
            Kind::Text => json!(given),
            Kind::List => json!(
                given
                    .split([',', ' '])
                    .filter(|v| !v.is_empty())
                    .collect::<Vec<_>>()
            ),
        };
        let (last, parents) = s.path.split_last().expect("a path has a key");
        let mut at = &mut *config;
        for key in parents {
            if !at.is_object() {
                return Err(format!("{}: not an object", s.path.join(".")));
            }
            at = at
                .as_object_mut()
                .expect("checked above")
                .entry(*key)
                .or_insert_with(|| json!({}));
            if at.is_null() {
                *at = json!({});
            }
        }
        match at.as_object_mut() {
            Some(o) => o.insert((*last).to_owned(), value),
            None => return Err(format!("{}: not an object", s.path.join("."))),
        };
    }
    let oidc_mode = config.pointer("/auth/mode").and_then(Value::as_str) == Some("oidc");
    if let Some(oidc) = config
        .pointer_mut("/auth/oidc")
        .and_then(Value::as_object_mut)
        .filter(|_| oidc_mode)
    {
        oidc.entry("operatorRole")
            .or_insert_with(|| json!(DEFAULT_OPERATOR_ROLE));
    }
    Ok(())
}

impl Config {
    /// Reads and checks `path` with the [`SETTINGS`] given in `flags` or
    /// the process environment on top: the shape against the contract and
    /// its own rules ([`Shape::parse`]), the replica name against model
    /// `Id`.
    pub fn load(
        path: &Path,
        flags: &[(String, String)],
    ) -> Result<(Config, Shape, String), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::parse_with(&text, flags, |k| std::env::var(k).ok(), hostname)
    }

    /// [`Config::parse_with`] without flags or environment.
    pub fn parse(text: &str, host: fn() -> String) -> Result<(Config, Shape, String), String> {
        Config::parse_with(text, &[], |_| None, host)
    }

    pub fn parse_with(
        text: &str,
        flags: &[(String, String)],
        env: impl Fn(&str) -> Option<String>,
        host: fn() -> String,
    ) -> Result<(Config, Shape, String), String> {
        let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        apply(&mut value, flags, env)?;
        let config: Config = serde_json::from_value(value).map_err(|e| e.to_string())?;
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
    /// output per output of the shape, and one meter per bus, aux,
    /// mix-minus and monitor output no output carries.
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
                "loudnessChannels": loudness_channels(&c.layout),
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
                "loudnessChannels": loudness_channels(&o.layout),
            }));
        }
        let columns = shape.meter_columns();
        if columns.len() > samplez::config::MAX_METERS {
            return Err(format!(
                "shape: {} buses, auxes, mix-minus and monitor outputs have no output, \
                 the engine meters at most {} of them; add outputs for the others",
                columns.len(),
                samplez::config::MAX_METERS
            ));
        }
        let meters: Vec<Value> = (columns.into_iter())
            .map(|(id, width, layout)| {
                json!({
                    "name": id,
                    "channels": width,
                    "loudnessChannels": loudness_channels(layout),
                })
            })
            .collect();
        if let Some(id) = mxl.inputs.keys().find(|id| !shape.is_channel(id)) {
            return Err(format!("mxl.inputs.{id}: not a channel of the shape"));
        }
        if let Some(id) =
            (mxl.outputs.keys()).find(|id| !shape.outputs.iter().any(|o| &o.id == *id))
        {
            return Err(format!("mxl.outputs.{id}: not an output of the shape"));
        }
        let rows: usize = shape.channels.iter().map(|c| c.width).sum();
        let columns: usize = (shape.outputs.iter().map(|o| o.width))
            .chain(shape.meter_columns().iter().map(|m| m.1))
            .sum();
        let mut engine = self.engine.clone();
        engine
            .entry("readerMarginSamples")
            .or_insert(json!(READER_MARGIN));
        let mut v = Value::Object(engine);
        v["libmxl"] = json!(mxl.library_path);
        v["domains"] = mxl
            .domains
            .iter()
            .map(|d| json!({ "name": d.name, "path": d.path }))
            .collect();
        v["inputs"] = json!(inputs);
        v["outputs"] = json!(outputs);
        v["meters"] = json!(meters);
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

    fn oidc_flags() -> Vec<(String, String)> {
        let args: Vec<String> = [
            "--config",
            "/etc/faderz/config.json",
            "--auth-mode",
            "oidc",
            "--oidc-issuer-url=https://idp.example/realms/media",
            "--oidc-client-id",
            "faderz",
            "--oidc-public-url",
            "https://faderz.example",
            "--oidc-scopes",
            "profile, groups",
        ]
        .map(String::from)
        .into();
        match command(&args).unwrap() {
            Command::Run { config, flags } => {
                assert_eq!(config, PathBuf::from("/etc/faderz/config.json"));
                flags
            }
            other => panic!("{other:?}"),
        }
    }

    /// Every OIDC setting the file takes, the command line and the
    /// environment take as well; the flag wins over the variable, the
    /// variable over the file.
    #[test]
    fn oidc_settings_come_from_file_flags_or_environment() {
        let file = json!({
            "replica": { "name": "r1" },
            "shape": small(),
            "auth": { "oidc": { "clientId": "from-file", "roleClaim": "roles" } },
        })
        .to_string();
        let env = |k: &str| match k {
            "FADERZ_OIDC_CLIENT_ID" => Some("from-env".to_owned()),
            "FADERZ_OIDC_VIEWER_ROLE" => Some("faderz-viewer".to_owned()),
            "FADERZ_OIDC_SESSION_KEY_FILE" => Some("/run/secrets/key".to_owned()),
            "FADERZ_OIDC_AUDIENCE" => Some(String::new()),
            _ => None,
        };
        let (config, _, _) =
            Config::parse_with(&file, &oidc_flags(), env, || unreachable!()).unwrap();
        assert_eq!(config.auth.mode, AuthMode::Oidc);
        let o = config.auth.oidc.unwrap();
        assert_eq!(o.client_id, "faderz", "the flag over the variable");
        assert_eq!(o.issuer_url, "https://idp.example/realms/media");
        assert_eq!(o.public_url, "https://faderz.example");
        assert_eq!(o.scopes, ["profile", "groups"]);
        assert_eq!(o.role_claim, "roles", "the file where nothing else sets it");
        assert_eq!(o.viewer_role.as_deref(), Some("faderz-viewer"));
        assert_eq!(o.audience, None, "an empty variable is unset");
        assert_eq!(
            o.session_key.unwrap().file,
            PathBuf::from("/run/secrets/key")
        );
        assert_eq!(o.operator_role, DEFAULT_OPERATOR_ROLE);

        let flags = [
            ("--auth-mode", "oidc"),
            ("--oidc-client-secret-file", "/run/s"),
            ("--oidc-issuer-url", "https://idp.example"),
            ("--oidc-public-url", "https://faderz.example"),
        ]
        .map(|(f, v)| (f.to_owned(), v.to_owned()));
        let (config, _, _) =
            Config::parse_with(&file, &flags, |_| None, || unreachable!()).unwrap();
        let o = config.auth.oidc.unwrap();
        assert_eq!(o.client_id, "from-file");
        assert_eq!(o.client_secret.unwrap().file, PathBuf::from("/run/s"));

        // Every setting maps onto a key the file takes.
        let all: Vec<(String, String)> = SETTINGS
            .iter()
            .map(|s| (s.flag.to_owned(), "x".to_owned()))
            .collect();
        let mut v = json!({});
        apply(&mut v, &all, |_| None).unwrap();
        v["auth"]["mode"] = json!("oidc");
        serde_json::from_value::<AuthConfig>(v["auth"].clone()).unwrap();
    }

    /// The operator role is defaulted for oidc mode only: a stray
    /// `auth.oidc` section in another mode is not made valid by it.
    #[test]
    fn the_operator_role_defaults_in_oidc_mode_only() {
        let config = |mode: &str| {
            json!({
                "replica": { "name": "r1" },
                "shape": small(),
                "auth": { "mode": mode, "token": { "file": "/run/t" }, "oidc": {
                    "issuerUrl": "https://idp.example", "clientId": "c",
                    "publicUrl": "https://faderz.example",
                } },
            })
            .to_string()
        };
        let (c, _, _) = Config::parse(&config("oidc"), || unreachable!()).unwrap();
        assert_eq!(c.auth.oidc.unwrap().operator_role, DEFAULT_OPERATOR_ROLE);
        let e = Config::parse(&config("token"), || unreachable!()).unwrap_err();
        assert!(e.contains("operatorRole"), "{e}");
        let flags = [("--auth-mode".to_owned(), "oidc".to_owned())];
        let parsed = Config::parse_with(&config("token"), &flags, |_| None, || unreachable!());
        assert!(parsed.is_ok(), "the mode a flag sets counts");
    }

    #[test]
    fn the_auth_mode_names_where_it_came_from() {
        let flag = [("--auth-mode".to_owned(), "none".to_owned())];
        let env = |k: &str| (k == "FADERZ_AUTH_MODE").then(|| "token".to_owned());
        assert_eq!(auth_mode_source(&flag, env), "--auth-mode");
        assert_eq!(auth_mode_source(&[], env), "FADERZ_AUTH_MODE");
        assert_eq!(auth_mode_source(&[], |_| None), "the configuration file");
        let empty = |_: &str| Some(String::new());
        assert_eq!(auth_mode_source(&[], empty), "the configuration file");
    }

    #[test]
    fn the_command_line_is_checked() {
        let args = |a: &[&str]| command(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(args(&["--version"]), Ok(Command::Version));
        assert_eq!(args(&["--help"]), Ok(Command::Help));
        assert!(args(&[]).is_err());
        assert!(
            args(&["--oidc-client-id", "x"])
                .unwrap_err()
                .contains("--config")
        );
        assert!(args(&["--config"]).is_err());
        assert!(
            args(&["--config", "c", "--oidc-nope", "x"])
                .unwrap_err()
                .contains("unknown")
        );
        assert!(args(&["--config", "c", "--oidc-client-id"]).is_err());
        assert!(args(&["--config", "c", "--version"]).is_err());
        let help = usage();
        for s in SETTINGS {
            assert!(help.contains(s.flag) && help.contains(s.env), "{}", s.flag);
        }
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

        // Each bus, aux, mix-minus and monitor output no output carries
        // is an engine meter on matrix columns after the outputs', and
        // each leg is weighted for loudness by its layout.
        let (c, ..) = Config::parse(&ok, || unreachable!()).unwrap();
        let e = c.audio.unwrap();
        let meters: Vec<_> = e
            .meters
            .iter()
            .map(|m| (m.name.as_str(), m.channels))
            .collect();
        assert_eq!(meters, [("cf", 2), ("aux1", 1), ("mm1", 2), ("mon", 2)]);
        assert_eq!((e.columns(), e.matrix[0].len()), (9, 9));
        let stereo = Some(vec!["L".to_owned(), "R".to_owned()]);
        assert_eq!(e.inputs[2].loudness_channels, stereo);
        assert_eq!(e.meters[0].loudness_channels, stereo);
        assert_eq!(e.reader_margin_samples, READER_MARGIN);

        let mut set: Value = serde_json::from_str(&ok).unwrap();
        set["engine"] = json!({ "readerMarginSamples": 960 });
        let (c, ..) = Config::parse(&set.to_string(), || unreachable!()).unwrap();
        assert_eq!(c.audio.unwrap().reader_margin_samples, 960);
    }

    /// More buses, auxes, mix-minus and monitor outputs without an output
    /// than the engine has meters for refuse the configuration, naming
    /// the limit; as many as it has are taken.
    #[test]
    fn more_uncarried_elements_than_engine_meters_are_refused() {
        let id = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
        let config = |auxes: usize| {
            let mut shape = small();
            // cf, mm1 and mon have no output: aux1 and the added auxes
            // make up the rest.
            let aux = |i: usize| json!({ "id": format!("aux{i}"), "name": "a", "width": 1, "layout": "mono" });
            shape["auxes"] = (1..=auxes).map(aux).collect();
            json!({
                "replica": { "name": "r1" },
                "shape": shape,
                "mxl": {
                    "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                    "inputs": {
                        "ch1": { "flow": id(1), "altFlow": id(5) },
                        "ch2": { "flow": id(2) },
                        "st1": { "flow": id(3) },
                    },
                    "outputs": { "out-pgm": { "flow": id(4) } },
                },
            })
            .to_string()
        };
        let max = samplez::config::MAX_METERS;
        Config::parse(&config(max - 3), || unreachable!()).unwrap();
        let e = Config::parse(&config(max - 2), || unreachable!()).unwrap_err();
        assert!(e.contains(&format!("at most {max}")), "{e}");
    }
}
