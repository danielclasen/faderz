//! The configuration file: listeners, auth and API settings of
//! samplez-service, where the replicated state lives, the shape, the
//! declared consoles, and the MXL flows the engine reads and writes. The
//! auth settings in [`SETTINGS`] can also be given as command-line flags
//! or environment variables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use samplez_replica::membership::Voters;
use samplez_service::config::{ApiConfig, AuthConfig, AuthMode, Listener, Listeners, TlsMode};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};
use crate::mix::Wiring;
use crate::shape::{ChannelTone, Shape, loudness_channels};
use crate::state::Console;

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
    /// Consoles known before any registers: in `/consoles` from the start,
    /// named by this list, never removed. `protocol` defaults to `mcu`,
    /// `transport` to `bridge`, `strips` to 8.
    #[serde(default)]
    pub consoles: Vec<Console>,
    /// The MXL flows of the audio. Absent, faderz runs no engine: the
    /// state is served, nothing is mixed, and `/readyz` fails.
    #[serde(default)]
    pub mxl: Option<Mxl>,
    /// samplez's engine timing: `sampleRate`, `blockSize`,
    /// `readerMarginSamples`, `maxCatchupBlocks`, `pruneAfterBlocks`,
    /// `rampSamples`, `delayBudgetSamples`. samplez checks the values and
    /// supplies defaults, but for `readerMarginSamples`, [`READER_MARGIN`]
    /// here, and `delayBudgetSamples`, [`Config::delay_budget`].
    #[serde(default)]
    pub engine: Map<String, Value>,
    /// samplez's RT thread settings (`enabled`, `priority`, `rtCpu`,
    /// `isolateRtCpu`). With the RT thread pinned to `rtCpu`,
    /// `isolateRtCpu` (default true) keeps every thread started after it,
    /// faderz's own included, off that CPU.
    #[serde(default)]
    pub rt: Map<String, Value>,
    /// NMOS registration and IS-05 connections over MXL.
    #[serde(default)]
    pub nmos: Nmos,
    /// The engine configuration [`Config::parse`] built from the above,
    /// with a matrix of zeros: the matrix comes from the live state.
    #[serde(skip)]
    pub audio: Option<samplez::Config>,
    /// Each channel's input a and b flows and their domain, from the
    /// `mxl` section; empty without one.
    #[serde(skip)]
    pub wiring: Vec<Wiring>,
    /// The replica set [`Config::parse`] read from `replica`; none for a
    /// set of one.
    #[serde(skip)]
    pub set: Option<Set>,
    /// What [`Config::parse`] passed over, for the log.
    #[serde(skip)]
    pub notes: Vec<String>,
}

/// The variable naming the marker ConfigMap (`replica.marker`).
pub const MARKER_ENV: &str = "FADERZ_REPLICA_MARKER";
/// The variable carrying the pod's `controller-revision-hash` label, the
/// revision a rollout of a set compares with its StatefulSet's.
pub const REVISION_ENV: &str = "FADERZ_POD_REVISION";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mxl {
    /// The libmxl shared library, loaded at start.
    #[serde(default = "default_library_path")]
    pub library_path: PathBuf,
    pub domains: Vec<Domain>,
    /// Each channel's input, by channel id: the flows it reads, or a tone
    /// generator; every channel has one.
    pub inputs: BTreeMap<String, Input>,
    /// The flows each output of the shape writes, by output id; every
    /// output has them.
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
    /// The domain's MXL id, for NMOS; the domain's own `domain_def.json`
    /// gives it otherwise.
    #[serde(default)]
    pub id: Option<String>,
}

/// NMOS: an IS-04 node registering the channels' receivers and the
/// outputs' senders, and the IS-05 Connection API over MXL.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Nmos {
    #[serde(default)]
    pub enabled: bool,
    /// What every resource id derives from: the replica's name in a set
    /// of one, the StatefulSet's in a set of more, when absent.
    #[serde(default)]
    pub seed: Option<String>,
    /// The host controllers reach the NMOS listener at.
    #[serde(default)]
    pub advertise_host: String,
    /// 0 advertises the NMOS listener's port.
    #[serde(default)]
    pub advertise_port: u16,
    #[serde(default = "default_heartbeat")]
    pub heartbeat_s: u64,
    #[serde(default)]
    pub registry: Registry,
    #[serde(default)]
    pub asset: Asset,
    /// Per ordinal, the `host:port` the other replicas of a set reach
    /// that replica's NMOS listener at, to forward staging to the
    /// leader; by default each `replica.peers` host on
    /// `listeners.nmos.port`.
    #[serde(default)]
    pub peers: Option<Vec<String>>,
}

impl Default for Nmos {
    fn default() -> Nmos {
        Nmos {
            enabled: false,
            seed: None,
            advertise_host: String::new(),
            advertise_port: 0,
            heartbeat_s: default_heartbeat(),
            registry: Registry::default(),
            asset: Asset::default(),
            peers: None,
        }
    }
}

fn default_heartbeat() -> u64 {
    5
}

/// The Registration API: `url`, or DNS-SD in `dnsSdDomain` or the
/// resolver's search domains, then mDNS.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub dns_sd_domain: Option<String>,
}

/// BCP-002-02 asset tags besides product and instance.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    #[serde(default = "default_manufacturer")]
    pub manufacturer: String,
}

impl Default for Asset {
    fn default() -> Asset {
        Asset {
            manufacturer: default_manufacturer(),
        }
    }
}

fn default_manufacturer() -> String {
    "danielclasen".into()
}

/// The most seconds between two heartbeats `nmos.heartbeatS` takes: a
/// registry drops a node after 12 s without one.
const MAX_HEARTBEAT_S: u64 = 10;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    /// The flow of input a; required but for a tone input.
    #[serde(default)]
    pub flow: Option<String>,
    /// Makes the channel's input a line-up tone generator in place of a
    /// flow: it reads none, so it takes no `flow`, `altFlow`, `domain`
    /// or `channels`, and has no NMOS receiver.
    #[serde(default)]
    pub tone: Option<ToneInput>,
    /// The flow of input b; required exactly for a channel with
    /// `altInput`.
    #[serde(default)]
    pub alt_flow: Option<String>,
    /// Required with more than one domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// The 1-based flow channel each leg of the channel reads, in leg
    /// order, as many as the channel's width, each at most
    /// [`MAX_FLOW_CHANNEL`]; a number may repeat, so `[1, 1]` feeds one
    /// flow channel to both legs of a stereo channel. `flow` and `altFlow`
    /// are then read through the same selection and need at least as
    /// many channels as the highest number. Absent, each flow has exactly
    /// the channel's width in channels, read in order. The channel's
    /// `source` label in the shape gains the selection, as in `ch 7-8`.
    #[serde(default)]
    pub channels: Option<Vec<u32>>,
    /// The channel's delay line: the longest delay it takes, in ms at
    /// the shape's sample rate, at most 5 s at 48 kHz
    /// (`samplez_dsp::MAX_DELAY_SAMPLES`). Absent or 0, the channel has
    /// none and takes no delay but 0. Every line is allocated at start
    /// and counts against `engine.delayBudgetSamples`.
    #[serde(default)]
    pub max_delay_ms: Option<f64>,
}

/// `mxl.inputs.<channel>.tone`: the frequency and level the generator
/// starts at, within samplez's ranges (`samplez_dsp::TONE_FREQUENCY_HZ`,
/// `samplez_dsp::TONE_LEVEL_DB`); absent, samplez's 1 kHz at -18 dBFS.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToneInput {
    #[serde(default)]
    pub frequency: Option<u32>,
    #[serde(default)]
    pub level: Option<f64>,
}

/// The highest flow channel `mxl.inputs.<channel>.channels` may name:
/// samplez reads selections from the first 64 channels of a flow.
pub const MAX_FLOW_CHANNEL: u32 = 64;

/// The `source` label part naming the 1-based flow channels `legs`:
/// `ch 7`, a rising run as `ch 7-8`, others listed as `ch 1,4`.
fn source_channels(legs: &[u32]) -> String {
    let run = legs.windows(2).all(|w| w[1] == w[0] + 1);
    match legs {
        [one] => format!("ch {one}"),
        [first, .., last] if run => format!("ch {first}-{last}"),
        _ => {
            let list: Vec<String> = legs.iter().map(u32::to_string).collect();
            format!("ch {}", list.join(","))
        }
    }
}

/// The longest channel `source` label model `ChannelShape` takes.
const MAX_SOURCE: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    /// The flow a set of one replica writes.
    #[serde(default)]
    pub flow: Option<String>,
    /// The member flows of a set of more than one replica, by ordinal:
    /// as many as `replica.replicas`, each a flow no other output or
    /// input names. Each replica writes its own; a set writes no flow
    /// together.
    #[serde(default)]
    pub flows: Option<Vec<String>>,
    /// Required with more than one domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// The flow that merges the member flows of a set of more than one
    /// replica, which its NMOS sender names; required there with NMOS
    /// enabled.
    #[serde(default)]
    pub merged_flow: Option<String>,
}

/// `readerMarginSamples` when the configuration sets none: 32 ms. An
/// input read through a copy of a flow on another node has needed 1303
/// to 1399 samples, which samplez's default of 576 reads almost wholly
/// late; the margin delays every output by as much.
pub const READER_MARGIN: u32 = 1536;

/// The `engine` keys faderz passes to samplez; the others it sets itself.
const ENGINE_KEYS: [&str; 7] = [
    "sampleRate",
    "blockSize",
    "readerMarginSamples",
    "maxCatchupBlocks",
    "pruneAfterBlocks",
    "rampSamples",
    "delayBudgetSamples",
];

/// `delayBudgetSamples` when the configuration sets none: samplez's
/// default, 2^22. faderz sets it on the engine itself, so the engine and
/// the replica's peer connections, which read a donor's delay lines
/// within it, always take one value.
pub const DELAY_BUDGET: u64 = 1 << 22;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplicaConfig {
    /// The replica's id towards clients (model `ReplicaRef.id`); the host
    /// name when absent. In a set it is left out, so that each pod takes
    /// its pod name and the ordinal that name ends in; one given there
    /// needs `ordinal` too.
    pub name: Option<String>,
    /// Where the replicated log and snapshots live.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// The voters of the replica set, an odd count; 1, the default, is a
    /// set of one, which takes none of the keys below. A set starts on
    /// empty volumes: the data directory of a set of one cannot join a
    /// set, nor one of a set run alone, so changing between the two needs
    /// a new volume and leaves the state behind.
    #[serde(default = "one")]
    pub replicas: u16,
    /// This replica's StatefulSet ordinal; absent, the number after the
    /// last `-` of the replica name, as in a StatefulSet pod's name.
    pub ordinal: Option<u16>,
    /// Every replica's peer address, `host:port`, by ordinal: as many as
    /// `replicas`, each once. The StatefulSet's pods reach each other by
    /// their stable names, `<pod>.<headless service>:<port>`.
    #[serde(default)]
    pub peers: Vec<String>,
    /// The peer listener the other replicas reach this one on, on a port
    /// other than 0: TLS in `files` mode with a `clientCaFile`, whose
    /// certificates every replica's certificate is issued by. Each
    /// replica's certificate names the host of its `peers` address, which
    /// the others check.
    pub peer: Option<Listener>,
    /// The ConfigMap the set starts from, in the pod's namespace: ordinal
    /// 0 creates it, the others read it to join, so the pods' service
    /// account may get and create ConfigMaps there. `FADERZ_REPLICA_MARKER`
    /// sets it when the file does not; a set of one ignores the variable.
    /// Ordinal 0 starts a set only while a majority of the voters answer,
    /// so the StatefulSet starts its pods in parallel
    /// (`podManagementPolicy: Parallel`): one at a time, each waiting to be
    /// ready, the first would wait for the others forever.
    pub marker: Option<String>,
    /// The most a set's apply deadline may lead the engine's processed
    /// index, ms, in [`MAX_APPLY_LEAD_MS`]: samplez-replica's
    /// `maxApplyLeadMs`, read by a set of more than one replica that runs
    /// an engine.
    #[serde(default = "default_max_apply_lead_ms")]
    pub max_apply_lead_ms: u64,
}

/// The values `replica.maxApplyLeadMs` takes.
pub const MAX_APPLY_LEAD_MS: std::ops::RangeInclusive<u64> = 30..=120;

fn default_max_apply_lead_ms() -> u64 {
    50
}

impl Default for ReplicaConfig {
    fn default() -> Self {
        ReplicaConfig {
            name: None,
            data_dir: default_data_dir(),
            replicas: 1,
            ordinal: None,
            peers: Vec::new(),
            peer: None,
            marker: None,
            max_apply_lead_ms: default_max_apply_lead_ms(),
        }
    }
}

fn one() -> u16 {
    1
}

/// A replica set of more than one voter, as [`Config::parse`] checked it.
#[derive(Debug, Clone)]
pub struct Set {
    pub voters: Voters,
    pub ordinal: u16,
    /// By ordinal.
    pub peers: Vec<String>,
    pub peer: Listener,
    pub marker: String,
    /// The StatefulSet the replica's pod belongs to: the replica name
    /// without its `-<ordinal>`, when it ends in one.
    pub statefulset: Option<String>,
    /// The pod's `controller-revision-hash` label, from
    /// [`REVISION_ENV`].
    pub revision: Option<String>,
    /// `replica.maxApplyLeadMs`.
    pub max_apply_lead: std::time::Duration,
}

impl Set {
    /// This replica's own peer address.
    pub fn addr(&self) -> &str {
        &self.peers[usize::from(self.ordinal)]
    }
}

impl ReplicaConfig {
    /// The set these keys describe for the replica named `name`; none for
    /// a set of one, which takes none of the set's keys.
    fn set(
        &self,
        name: &str,
        env_marker: Option<String>,
        revision: Option<String>,
    ) -> Result<Option<Set>, String> {
        if self.replicas == 1 {
            let given = [
                ("ordinal", self.ordinal.is_some()),
                ("peers", !self.peers.is_empty()),
                ("peer", self.peer.is_some()),
                ("marker", self.marker.is_some()),
            ];
            return match given.iter().find(|(_, set)| *set) {
                Some((key, _)) => Err(format!(
                    "replica.{key}: a set of one replica takes none; set replica.replicas"
                )),
                None => Ok(None),
            };
        }
        let voters =
            Voters::new(self.replicas, false).map_err(|e| format!("replica.replicas: {e}"))?;
        let ordinal = match self.ordinal {
            Some(o) => o,
            // Every pod would read the same name, and take the same slot.
            None if self.name.is_some() => {
                return Err(
                    "replica.ordinal: required with replica.name in a set of more than one \
                     replica; leave both out to take them from the pod name"
                        .into(),
                );
            }
            None => name
                .rsplit_once('-')
                .and_then(|(_, n)| n.parse().ok())
                .ok_or(format!(
                    "replica.ordinal: absent, and the replica name {name:?} ends in no -<ordinal>"
                ))?,
        };
        if ordinal >= self.replicas {
            return Err(format!(
                "replica.ordinal {ordinal}: the set has ordinals 0 to {}",
                self.replicas - 1
            ));
        }
        if self.peers.len() != usize::from(self.replicas) {
            return Err(format!(
                "replica.peers: names {} addresses, the set has {} replicas",
                self.peers.len(),
                self.replicas
            ));
        }
        for (i, addr) in self.peers.iter().enumerate() {
            let port = addr
                .rsplit_once(':')
                .map(|(host, port)| (host, port.parse::<u16>()));
            if !matches!(port, Some((host, Ok(port))) if !host.is_empty() && port != 0) {
                return Err(format!("replica.peers[{i}] {addr:?}: not host:port"));
            }
            if self.peers[..i].contains(addr) {
                return Err(format!("replica.peers[{i}] {addr:?}: named twice"));
            }
        }
        let peer = self
            .peer
            .clone()
            .ok_or("replica.peer: required for a set of more than one replica")?;
        if peer.tls.mode != TlsMode::Files || peer.tls.client_ca_file.is_none() {
            return Err("replica.peer.tls: needs files mode and a clientCaFile".into());
        }
        if peer.port == 0 {
            return Err("replica.peer.port: 0 is no address the others can reach".into());
        }
        let marker = self
            .marker
            .clone()
            .or(env_marker)
            .ok_or("replica.marker: required for a set of more than one replica")?;
        let statefulset = (name.rsplit_once('-'))
            .filter(|(set, n)| !set.is_empty() && n.parse() == Ok(ordinal))
            .map(|(set, _)| set.to_owned());
        Ok(Some(Set {
            voters,
            ordinal,
            peers: self.peers.clone(),
            peer,
            marker,
            statefulset,
            revision,
            max_apply_lead: std::time::Duration::from_millis(self.max_apply_lead_ms),
        }))
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
        apply(&mut value, flags, &env)?;
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
        let mut shape = Shape::parse(&config.shape).map_err(|e| format!("shape: {e}"))?;
        for (i, c) in config.consoles.iter().enumerate() {
            check(&contract().console_status, &c.status(None, false, 0))
                .map_err(|e| format!("consoles[{i}]: {e}"))?;
            if config.consoles[..i].iter().any(|d| d.id == c.id) {
                return Err(format!("consoles[{i}].id {:?}: used twice", c.id));
            }
        }
        let mut config = config;
        if !MAX_APPLY_LEAD_MS.contains(&config.replica.max_apply_lead_ms) {
            return Err(format!(
                "replica.maxApplyLeadMs {}: not in {}..={}",
                config.replica.max_apply_lead_ms,
                MAX_APPLY_LEAD_MS.start(),
                MAX_APPLY_LEAD_MS.end()
            ));
        }
        let name = config.replica.name.clone().unwrap_or_else(host);
        check(&contract().id, &json!(name)).map_err(|e| {
            format!("replica.name {name:?}: {e}; name the replica in the configuration")
        })?;
        let env_marker = env(MARKER_ENV).filter(|m| !m.is_empty());
        if config.replica.replicas == 1 && env_marker.is_some() {
            config
                .notes
                .push(format!("{MARKER_ENV} ignored: replica.replicas is 1"));
        }
        let revision = env(REVISION_ENV).filter(|r| !r.is_empty());
        config.set = config.replica.set(&name, env_marker, revision)?;
        config.delay_lines(&mut shape)?;
        config.tone_inputs(&mut shape)?;
        config.audio = config.audio(&shape)?;
        if let (Some(engine), Some(mxl)) = (&config.audio, &config.mxl) {
            config.wiring = (shape.channels.iter().zip(&engine.inputs))
                .map(|(c, input)| match input.tone {
                    // Reads no flow: names none ([`crate::mix::selected`]).
                    Some(_) => Wiring {
                        a: String::new(),
                        b: None,
                        domain: String::new(),
                    },
                    None => Wiring {
                        a: input
                            .flow_id
                            .clone()
                            .expect("every configured flow input names a flow"),
                        b: mxl.inputs[&c.id].alt_flow.clone(),
                        domain: input
                            .domain
                            .clone()
                            .expect("every configured flow input names a domain"),
                    },
                })
                .collect();
        }
        config.label_sources(&mut shape)?;
        config.check_nmos()?;
        Ok((config, shape, name))
    }
}

impl Config {
    /// The engine's `delayBudgetSamples`: the configured one, else
    /// [`DELAY_BUDGET`]. One that is not a number is refused where the
    /// engine's configuration is checked.
    pub fn delay_budget(&self) -> u64 {
        (self.engine.get("delayBudgetSamples"))
            .and_then(Value::as_u64)
            .unwrap_or(DELAY_BUDGET)
    }

    /// Every flow an output names, member and merged flows included: what
    /// no input may read.
    pub fn own_flows(&self) -> std::collections::BTreeSet<String> {
        (self.mxl.iter())
            .flat_map(|m| m.outputs.values())
            .flat_map(|o| {
                (o.flow.iter())
                    .chain(o.flows.iter().flatten())
                    .chain(o.merged_flow.iter())
            })
            .cloned()
            .collect()
    }

    /// Per ordinal, where the replicas of a set reach each other's NMOS
    /// listener ([`Nmos::peers`]); empty for a set of one.
    pub fn nmos_peers(&self) -> Vec<String> {
        let Some(set) = &self.set else {
            return Vec::new();
        };
        match &self.nmos.peers {
            Some(peers) => peers.clone(),
            None => (set.peers.iter())
                .map(|p| {
                    let host = p.rsplit_once(':').map_or(p.as_str(), |(h, _)| h);
                    format!("{host}:{}", self.listeners.nmos.port)
                })
                .collect(),
        }
    }

    /// The NMOS settings that cannot work: NMOS without an engine, without
    /// an advertised host or port, over TLS, with a registry not on
    /// plain HTTP or a heartbeat outside 1 to [`MAX_HEARTBEAT_S`] s, or
    /// a set's output without its merged flow. A loopback listener with
    /// another advertised host is noted.
    fn check_nmos(&mut self) -> Result<(), String> {
        let nmos = &self.nmos;
        let merged = (self.mxl.iter()).flat_map(|m| m.outputs.iter());
        for (id, o) in merged {
            match (&o.merged_flow, &self.set) {
                (Some(_), None) => {
                    return Err(format!(
                        "mxl.outputs.{id}.mergedFlow: a set of one writes its `flow` itself"
                    ));
                }
                (None, Some(_)) if nmos.enabled => {
                    return Err(format!(
                        "mxl.outputs.{id}.mergedFlow: required with nmos.enabled in a set of \
                         more than one replica, whose replicas write member flows"
                    ));
                }
                _ => {}
            }
        }
        if !nmos.enabled {
            return Ok(());
        }
        let listener = &self.listeners.nmos;
        if self.mxl.is_none() {
            return Err("nmos.enabled: needs the mxl section, whose flows it connects".into());
        }
        if nmos.advertise_host.is_empty() {
            return Err("nmos.advertiseHost: required with nmos.enabled".into());
        }
        if nmos.advertise_port == 0 && listener.port == 0 {
            return Err("nmos.advertisePort: required with listeners.nmos.port 0".into());
        }
        if listener.tls.mode != TlsMode::None {
            return Err(
                "listeners.nmos.tls: the NMOS APIs are advertised as plain HTTP, mode none".into(),
            );
        }
        if let Some(url) = &nmos.registry.url
            && !url.starts_with("http://")
        {
            return Err(format!(
                "nmos.registry.url {url:?}: the registration client speaks plain HTTP only"
            ));
        }
        if let (Some(peers), Some(set)) = (&nmos.peers, &self.set)
            && peers.len() != usize::from(set.voters.replicas)
        {
            return Err(format!(
                "nmos.peers: names {} replicas, the set has {}",
                peers.len(),
                set.voters.replicas
            ));
        }
        if self.set.is_some() && nmos.peers.is_none() && listener.bind.is_loopback() {
            return Err(format!(
                "listeners.nmos.bind {}: the other replicas forward staging to it; \
                 bind it where they reach it or name their addresses in nmos.peers",
                listener.bind
            ));
        }
        if self.set.is_some() && nmos.peers.is_none() && listener.port == 0 {
            return Err("nmos.peers: required with listeners.nmos.port 0 in a set".into());
        }
        if !(1..=MAX_HEARTBEAT_S).contains(&nmos.heartbeat_s) {
            return Err(format!(
                "nmos.heartbeatS {}: not in 1..={MAX_HEARTBEAT_S}",
                nmos.heartbeat_s
            ));
        }
        let loopback = |h: &str| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        };
        if listener.bind.is_loopback() && !loopback(&nmos.advertise_host) {
            let note = format!(
                "nmos.advertiseHost {} is not loopback but listeners.nmos binds {}: \
                 nothing else reaches the advertised APIs",
                nmos.advertise_host, listener.bind
            );
            self.notes.push(note);
        }
        Ok(())
    }

    /// What a set's compatibility key hashes besides the versions
    /// ([`samplez_replica::rollout::key`]): `product`, the mixer's
    /// configuration hash, the `engine` settings and each channel's flow
    /// channel selection, which change the output a replica computes from
    /// the same log. The member flows, which differ by replica, do not.
    pub fn compatibility(&self, product: u64) -> Vec<u8> {
        let channels: BTreeMap<&String, &Option<Vec<u32>>> = (self.mxl.iter())
            .flat_map(|m| m.inputs.iter().map(|(id, i)| (id, &i.channels)))
            .collect();
        json!({ "product": product, "engine": self.engine, "channels": channels })
            .to_string()
            .into_bytes()
    }

    /// Sets each channel's `maxDelay` in `shape` from
    /// `mxl.inputs.<id>.maxDelayMs`, in samples at the shape's rate, and
    /// refuses lines that together hold more history than
    /// `engine.delayBudgetSamples`: a line times the channel's width,
    /// summed over channels, as samplez counts it.
    fn delay_lines(&self, shape: &mut Shape) -> Result<(), String> {
        if let Some(c) = shape.channels.iter().find(|c| c.max_delay.is_some()) {
            return Err(format!(
                "shape: channel {} maxDelay: set by mxl.inputs.{}.maxDelayMs",
                c.id, c.id
            ));
        }
        let Some(mxl) = &self.mxl else {
            return Ok(());
        };
        let rate = f64::from(shape.sample_rate);
        let mut lines = Vec::new();
        for c in &mut shape.channels {
            let Some(ms) = mxl.inputs.get(&c.id).and_then(|i| i.max_delay_ms) else {
                continue;
            };
            let samples = (ms * rate / 1000.0).round();
            if !(0.0..=f64::from(samplez_dsp::MAX_DELAY_SAMPLES)).contains(&samples) {
                return Err(format!(
                    "mxl.inputs.{}.maxDelayMs {ms}: not in 0 to {} samples at {} Hz",
                    c.id,
                    samplez_dsp::MAX_DELAY_SAMPLES,
                    shape.sample_rate
                ));
            }
            if ms > 0.0 && samples == 0.0 {
                return Err(format!(
                    "mxl.inputs.{}.maxDelayMs {ms}: under one sample at {} Hz; 0 for no line",
                    c.id, shape.sample_rate
                ));
            }
            if samples > 0.0 {
                c.max_delay = Some(samples as u32);
                lines.push((c.id.clone(), samples as u64 * c.width as u64));
            }
        }
        let held: u64 = lines.iter().map(|(_, s)| s).sum();
        let budget = self.delay_budget();
        if held > budget {
            let list: Vec<String> = (lines.iter())
                .map(|(id, s)| format!("{id} ({s})"))
                .collect();
            return Err(format!(
                "mxl.inputs: the delay lines of {} hold {held} samples times legs, above \
                 engine.delayBudgetSamples {budget}; shorten maxDelayMs or raise the budget",
                list.join(", ")
            ));
        }
        Ok(())
    }

    /// Sets each tone channel's `tone` in `shape` from
    /// `mxl.inputs.<id>.tone`: the frequency and level its generator
    /// starts at, samplez's where none is given, and refuses one outside
    /// samplez's ranges.
    fn tone_inputs(&self, shape: &mut Shape) -> Result<(), String> {
        if let Some(c) = shape.channels.iter().find(|c| c.tone.is_some()) {
            return Err(format!(
                "shape: channel {} tone: set by mxl.inputs.{}.tone",
                c.id, c.id
            ));
        }
        let Some(mxl) = &self.mxl else {
            return Ok(());
        };
        let start = samplez_dsp::ToneParams::DEFAULT;
        let hz = samplez_dsp::TONE_FREQUENCY_HZ;
        let db = samplez_dsp::TONE_LEVEL_DB;
        let (low, high) = (f64::from(*db.start()), f64::from(*db.end()));
        for c in &mut shape.channels {
            let Some(tone) = mxl.inputs.get(&c.id).and_then(|i| i.tone.as_ref()) else {
                continue;
            };
            let frequency = tone.frequency.unwrap_or(start.frequency_hz);
            if !hz.contains(&frequency) {
                return Err(format!(
                    "mxl.inputs.{}.tone.frequency {frequency}: not in {}..={} Hz",
                    c.id,
                    hz.start(),
                    hz.end()
                ));
            }
            let level = tone.level.unwrap_or(f64::from(start.level_db));
            if !(low..=high).contains(&level) {
                return Err(format!(
                    "mxl.inputs.{}.tone.level {level}: not in {low}..={high} dBFS",
                    c.id
                ));
            }
            c.tone = Some(ChannelTone { frequency, level });
        }
        Ok(())
    }

    /// Appends each channel's flow channel selection to its `source`
    /// label, or makes it the label.
    fn label_sources(&self, shape: &mut Shape) -> Result<(), String> {
        let Some(mxl) = &self.mxl else {
            return Ok(());
        };
        for c in &mut shape.channels {
            let Some(legs) = mxl.inputs.get(&c.id).and_then(|i| i.channels.as_deref()) else {
                continue;
            };
            let part = source_channels(legs);
            let label = match &c.source {
                Some(s) => format!("{s} {part}"),
                None => part,
            };
            if label.chars().count() > MAX_SOURCE {
                return Err(format!(
                    "shape: channel {} source {label:?}: with mxl.inputs.{}.channels \
                     appended it is longer than {MAX_SOURCE} characters",
                    c.id, c.id
                ));
            }
            c.source = Some(label);
        }
        Ok(())
    }

    /// The flows `output` names, by ordinal: one for a set of one, a
    /// member flow per replica for a set of more.
    fn member_flows(&self, at: &str, output: &Output) -> Result<Vec<String>, String> {
        match (&self.set, &output.flow, &output.flows) {
            (None, Some(flow), None) => Ok(vec![flow.clone()]),
            (None, _, Some(_)) => Err(format!(
                "{at}.flows: a set of one replica writes one flow, `flow`"
            )),
            (None, None, None) => Err(format!("{at}.flow: missing")),
            (Some(_), Some(_), _) => Err(format!(
                "{at}.flow: every replica of a set writes its own member flow, \
                 listed by ordinal in `flows`"
            )),
            (Some(_), None, None) => Err(format!("{at}.flows: missing")),
            (Some(set), None, Some(flows)) => {
                if flows.len() != usize::from(set.voters.replicas) {
                    return Err(format!(
                        "{at}.flows: names {} flows, the set has {} replicas",
                        flows.len(),
                        set.voters.replicas
                    ));
                }
                Ok(flows.clone())
            }
        }
    }

    /// The engine configuration of the `mxl` section, with a matrix of
    /// zeros: one input per channel reading its flow of input a, or a tone
    /// generator, then the line-up tone's generator
    /// ([`crate::mix::TONE_INPUT`]), one
    /// output per output of the shape writing this replica's flow
    /// ([`Config::member_flows`]), one meter per bus, aux,
    /// mix-minus and monitor output no full-width output carries, and the columns a
    /// duck group can lower ([`crate::duck::columns`]).
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
            if input.tone.is_some() {
                let given = [
                    ("flow", input.flow.is_some()),
                    ("altFlow", input.alt_flow.is_some()),
                    ("domain", input.domain.is_some()),
                    ("channels", input.channels.is_some()),
                ];
                if let Some((key, _)) = given.iter().find(|(_, given)| *given) {
                    return Err(format!("{at}.{key}: a tone input reads no flow"));
                }
                if c.alt_input {
                    return Err(format!(
                        "{at}.tone: the channel has altInput, a tone input has no input b"
                    ));
                }
                inputs.push(json!({
                    "name": c.id,
                    "tone": {},
                    "channels": c.width,
                    "loudnessChannels": loudness_channels(&c.layout),
                    "maxDelaySamples": c.max_delay.unwrap_or(0),
                }));
                continue;
            }
            let flow = (input.flow.as_ref()).ok_or(format!("{at}.flow: required without tone"))?;
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
            let mut engine_input = json!({
                "name": c.id,
                "domain": domain(at.clone(), &input.domain)?,
                "flowId": flow,
                "channels": c.width,
                "loudnessChannels": loudness_channels(&c.layout),
                "maxDelaySamples": c.max_delay.unwrap_or(0),
            });
            if let Some(legs) = &input.channels {
                if legs.len() != c.width {
                    return Err(format!(
                        "{at}.channels: names {} flow channels, the channel is {} wide",
                        legs.len(),
                        c.width
                    ));
                }
                if let Some(n) = legs.iter().find(|&&n| !(1..=MAX_FLOW_CHANNEL).contains(&n)) {
                    return Err(format!(
                        "{at}.channels: flow channel {n} is not in 1..={MAX_FLOW_CHANNEL}"
                    ));
                }
                engine_input["flowChannels"] = legs.iter().map(|n| n - 1).collect();
            }
            inputs.push(engine_input);
        }
        inputs.push(json!({
            "name": crate::mix::TONE_INPUT,
            "tone": {},
            "channels": crate::mix::tone_width(shape),
        }));
        let mut outputs = Vec::new();
        // Per output, the flow of each ordinal.
        let mut members: Vec<Vec<String>> = Vec::new();
        for o in &shape.outputs {
            let at = format!("mxl.outputs.{}", o.id);
            let output = mxl.outputs.get(&o.id).ok_or(format!("{at}: missing"))?;
            members.push(self.member_flows(&at, output)?);
            outputs.push(json!({
                "name": o.id,
                "domain": domain(at, &output.domain)?,
                "label": o.name,
                "group": o.id,
                "role": "Audio",
                "channels": o.width,
                "loudnessChannels": loudness_channels(&o.layout),
                "limiter": shape.has_limiter(&o.source),
            }));
        }
        // Inputs may share a flow; an output's flow is written by this
        // engine alone.
        let read: std::collections::BTreeSet<&str> = (mxl.inputs.values())
            .flat_map(|i| i.flow.iter().chain(&i.alt_flow))
            .map(String::as_str)
            .collect();
        let mut written = std::collections::BTreeSet::new();
        let merged = mxl.outputs.values().filter_map(|o| o.merged_flow.as_ref());
        for flow in members.iter().flatten().chain(merged) {
            if read.contains(flow.as_str()) || !written.insert(flow.as_str()) {
                return Err(format!(
                    "mxl.outputs: flow {flow} is named by another output, replica or input"
                ));
            }
        }
        let columns = shape.meter_columns();
        if columns.len() > samplez::config::MAX_METERS {
            return Err(format!(
                "shape: {} buses, auxes, mix-minus and monitor outputs have no full-width output, \
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
        let rows: usize =
            (shape.channels.iter().map(|c| c.width).sum::<usize>()) + crate::mix::tone_width(shape);
        let columns: usize = (shape.outputs.iter().map(|o| o.width))
            .chain(shape.meter_columns().iter().map(|m| m.1))
            .sum();
        let mut engine = self.engine.clone();
        engine
            .entry("readerMarginSamples")
            .or_insert(json!(READER_MARGIN));
        engine
            .entry("delayBudgetSamples")
            .or_insert(json!(DELAY_BUDGET));
        let mut v = Value::Object(engine);
        v["libmxl"] = json!(mxl.library_path);
        v["domains"] = mxl
            .domains
            .iter()
            .map(|d| json!({ "name": d.name, "path": d.path }))
            .collect();
        v["inputs"] = json!(inputs);
        v["meters"] = json!(meters);
        v["matrix"] = json!(vec![vec![0.0f32; columns]; rows]);
        v["duckColumns"] = json!(crate::duck::columns(shape));
        v["rt"] = Value::Object(self.rt.clone());
        // samplez checks the flows of each ordinal; this replica runs its
        // own.
        let ordinal = self.set.as_ref().map_or(0, |s| usize::from(s.ordinal));
        let mut engine = None;
        for n in 0..members.first().map_or(1, Vec::len) {
            for ((output, flows), o) in outputs.iter_mut().zip(&members).zip(&shape.outputs) {
                output["flowId"] = json!(flows[n]);
                // A member flow's label names its replica; the output's
                // NMOS sender keeps the output's own name.
                if flows.len() > 1 {
                    output["label"] = json!(format!("{} (replica {n})", o.name));
                }
            }
            v["outputs"] = json!(outputs);
            let parsed = samplez::Config::parse(&v.to_string()).map_err(|e| match &self.set {
                Some(_) => format!("engine of replica ordinal {n}: {}", e.0),
                None => format!("engine: {}", e.0),
            })?;
            if n == ordinal {
                engine = Some(parsed);
            }
        }
        let engine = engine.expect("the ordinal is below the set's replicas");
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

    fn set_of(replica: Value, host: fn() -> String) -> Result<Option<Set>, String> {
        Config::parse(&with(replica), host).map(|(c, _, _)| c.set)
    }

    fn three(extra: Value) -> Value {
        let mut v = json!({
            "replicas": 3,
            "peers": ["f-0.f:7443", "f-1.f:7443", "f-2.f:7443"],
            "peer": { "bind": "0.0.0.0", "port": 7443, "tls": {
                "mode": "files", "certFile": "/t/tls.crt", "keyFile": "/t/tls.key",
                "clientCaFile": "/t/ca.crt" } },
            "marker": "faderz-replica-cluster",
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }

    /// A StatefulSet pod named `<set>-<ordinal>` is that ordinal, and
    /// reaches the others at the peer addresses listed by ordinal.
    #[test]
    fn a_set_takes_its_ordinal_from_the_pod_name() {
        let set = set_of(three(json!({})), || "faderz-2".into())
            .unwrap()
            .unwrap();
        assert_eq!((set.ordinal, set.voters.replicas), (2, 3));
        assert_eq!(set.addr(), "f-2.f:7443");
        let set = set_of(three(json!({ "ordinal": 1 })), || "faderz-2".into())
            .unwrap()
            .unwrap();
        assert_eq!(set.ordinal, 1, "an explicit ordinal wins");
    }

    /// The `replicas` message names every replica as clients see it named:
    /// its pod's name, the StatefulSet's and its ordinal; without a
    /// StatefulSet, the first label of its peer host; this one by its own
    /// name.
    #[test]
    fn every_replica_of_a_set_is_named_as_its_pod_is() {
        use crate::app::member_names;
        let set = set_of(three(json!({})), || "mix-1".into())
            .unwrap()
            .unwrap();
        assert_eq!(member_names(&set, "mix-1"), ["mix-0", "mix-1", "mix-2"]);
        let set = set_of(three(json!({ "name": "desk", "ordinal": 2 })), || {
            "x".into()
        })
        .unwrap()
        .unwrap();
        assert_eq!(set.statefulset, None);
        assert_eq!(
            member_names(&set, "desk"),
            ["f-0.f:7443", "f-1.f:7443", "desk"]
        );
    }

    /// Without a StatefulSet each other replica is its whole peer
    /// address, entry n for `peers[n]`: IP peers and bracketed IPv6 ones
    /// stay distinct and the same on every replica, and a configured name
    /// that is not `<set>-<n>` takes only its own slot.
    #[test]
    fn replica_ids_are_unique_whatever_the_peer_addresses_are() {
        use crate::app::member_names;
        let mut cfg = three(json!({ "name": "desk", "ordinal": 1 }));
        for peers in [
            ["10.0.0.1:80", "10.0.0.2:80", "10.0.0.3:80"],
            ["[::1]:80", "[::2]:80", "[::3]:80"],
            ["a.x:80", "b.x:80", "a.y:80"],
            ["h:80", "h:81", "h:82"],
        ] {
            cfg["peers"] = json!(peers);
            let set = set_of(cfg.clone(), || "x".into()).unwrap().unwrap();
            let names = member_names(&set, "desk");
            assert_eq!(names.len(), 3);
            for (n, name) in names.iter().enumerate() {
                let expect = if n == 1 { "desk" } else { peers[n] };
                assert_eq!(name, expect, "ordinal {n} is peers[{n}]");
            }
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), 3, "{names:?}");
        }
    }

    /// One replica is a set of one, as before; it takes none of a set's
    /// keys, so a half-configured set does not start as one alone.
    #[test]
    fn one_replica_is_a_set_of_one_and_takes_no_set_keys() {
        assert!(set_of(json!({}), || "faderz-0".into()).unwrap().is_none());
        let e = set_of(json!({ "marker": "m" }), || "x".into()).unwrap_err();
        assert!(e.contains("replica.marker"), "{e}");
    }

    #[test]
    fn a_set_that_cannot_work_is_refused() {
        let host = || "faderz-0".to_owned();
        let refused = |v: Value, key: &str| {
            let e = set_of(v, host).unwrap_err();
            assert!(e.contains(key), "{key}: {e}");
        };
        let mut even = three(json!({ "replicas": 2 }));
        even["peers"] = json!(["a:1", "b:1"]);
        refused(even, "replica.replicas");
        refused(three(json!({ "peers": ["a:1", "b:1"] })), "replica.peers");
        refused(three(json!({ "ordinal": 3 })), "replica.ordinal");
        let e = set_of(three(json!({})), || "faderz".into()).unwrap_err();
        assert!(e.contains("replica.ordinal"), "{e}");
        // A name every pod would read alike: the ordinal is not taken
        // from it.
        refused(three(json!({ "name": "faderz-1" })), "replica.ordinal");
        let named = set_of(three(json!({ "name": "faderz-1", "ordinal": 1 })), host);
        assert_eq!(named.unwrap().unwrap().ordinal, 1);
        for (i, bad) in [
            (1, "f-1.f"),
            (1, ":7443"),
            (1, "f-1.f:0"),
            (2, "f-0.f:7443"),
        ] {
            let mut peers = three(json!({}));
            peers["peers"][i] = json!(bad);
            refused(peers, &format!("replica.peers[{i}]"));
        }
        let mut port0 = three(json!({}));
        port0["peer"]["port"] = json!(0);
        refused(port0, "replica.peer.port");
        let mut plain = three(json!({}));
        plain["peer"]["tls"] = json!({ "mode": "none" });
        refused(plain, "replica.peer.tls");
        let mut no_ca = three(json!({}));
        no_ca["peer"]["tls"]["clientCaFile"] = Value::Null;
        refused(no_ca, "replica.peer.tls");
        let mut no_marker = three(json!({}));
        no_marker.as_object_mut().unwrap().remove("marker");
        refused(no_marker, "replica.marker");
        let mut no_peer = three(json!({}));
        no_peer.as_object_mut().unwrap().remove("peer");
        refused(no_peer, "replica.peer");
    }

    /// A set's configuration with an `mxl` section whose output names
    /// `output`.
    fn set_with_mxl(output: Value) -> String {
        let mut v: Value = serde_json::from_str(&with(three(json!({})))).unwrap();
        v["mxl"] = json!({
            "domains": [{ "name": "d", "path": "/dev/shm/d" }],
            "inputs": { "ch1": { "flow": id(1), "altFlow": id(5) }, "ch2": { "flow": id(2) },
                        "st1": { "flow": id(3) } },
            "outputs": { "out-pgm": output },
        });
        v.to_string()
    }

    fn id(n: u8) -> String {
        format!("00000000-0000-4000-8000-00000000000{n}")
    }

    /// Each replica of a set runs an engine writing the member flow its
    /// ordinal names, under the cap its configuration sets, and knows
    /// its StatefulSet from its pod name and its revision from the
    /// environment.
    #[test]
    fn each_replica_of_a_set_writes_its_own_member_flow() {
        let text = set_with_mxl(json!({ "flows": [id(4), id(6), id(7)] }));
        let env = |k: &str| (k == REVISION_ENV).then(|| "faderz-5d8f7c".to_owned());
        let hosts: [(fn() -> String, String); 3] = [
            (|| "faderz-0".into(), id(4)),
            (|| "faderz-1".into(), id(6)),
            (|| "faderz-2".into(), id(7)),
        ];
        for (n, (host, flow)) in hosts.into_iter().enumerate() {
            let (config, _, _) = Config::parse_with(&text, &[], env, host).unwrap();
            let engine = config.audio.as_ref().expect("an engine");
            assert_eq!(engine.outputs[0].flow_id, flow);
            // The member flow says whose it is.
            assert_eq!(engine.outputs[0].label, format!("PGM out (replica {n})"));
            let set = config.set.unwrap();
            assert_eq!(set.statefulset.as_deref(), Some("faderz"));
            assert_eq!(set.revision.as_deref(), Some("faderz-5d8f7c"));
            assert_eq!(set.max_apply_lead, std::time::Duration::from_millis(50));
        }
        let named = |name: &str, ordinal: u16| {
            let mut v: Value = serde_json::from_str(&text).unwrap();
            v["replica"]["name"] = json!(name);
            v["replica"]["ordinal"] = json!(ordinal);
            let (config, _, _) = Config::parse(&v.to_string(), || unreachable!()).unwrap();
            config.set.unwrap().statefulset
        };
        assert_eq!(named("mixer-a-2", 2).as_deref(), Some("mixer-a"));
        assert_eq!(named("mixer-2", 1), None, "a name of another ordinal");
        assert_eq!(named("mixer", 1), None);
    }

    /// Every replica of a set has the same compatibility key, though
    /// each writes its own member flows; an engine setting or a flow
    /// channel selection changes it.
    #[test]
    fn replicas_of_a_set_share_a_compatibility_key() {
        let text = set_with_mxl(json!({ "flows": [id(4), id(6), id(7)] }));
        let key = |text: &str, host: fn() -> String| {
            let (config, _, _) = Config::parse(text, host).unwrap();
            config.compatibility(7)
        };
        let zero = key(&text, || "faderz-0".into());
        assert_eq!(zero, key(&text, || "faderz-2".into()));
        let (config, _, _) = Config::parse(&text, || "faderz-0".into()).unwrap();
        assert_ne!(
            zero,
            config.compatibility(8),
            "another product configuration"
        );
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["engine"] = json!({ "rampSamples": 960 });
        assert_ne!(zero, key(&v.to_string(), || "faderz-0".into()));
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["mxl"]["inputs"]["st1"]["flow"] = json!(id(8));
        v["mxl"]["inputs"]["st1"]["channels"] = json!([2, 1]);
        assert_ne!(zero, key(&v.to_string(), || "faderz-0".into()));
    }

    /// A set of one writes `flow`, a set of more one member flow per
    /// replica in `flows`, no two outputs or replicas the same one and
    /// none an input reads; samplez checks the flows of every ordinal,
    /// not only this replica's.
    #[test]
    fn member_flows_are_checked() {
        let refused = |output: Value, key: &str| {
            let e = Config::parse(&set_with_mxl(output), || "faderz-0".into()).unwrap_err();
            assert!(e.starts_with(key), "{key}: {e}");
        };
        refused(json!({ "flow": id(4) }), "mxl.outputs.out-pgm.flow");
        refused(json!({}), "mxl.outputs.out-pgm.flows");
        refused(
            json!({ "flows": [id(4), id(6)] }),
            "mxl.outputs.out-pgm.flows",
        );
        refused(json!({ "flows": [id(4), id(6), id(4)] }), "mxl.outputs:");
        refused(json!({ "flows": [id(4), id(6), id(1)] }), "mxl.outputs:");
        refused(json!({ "flows": [id(4), id(6), id(5)] }), "mxl.outputs:");
        refused(
            json!({ "flows": [id(4), id(6), "x"] }),
            "engine of replica ordinal 2",
        );
        let mut one: Value = serde_json::from_str(&set_with_mxl(json!({}))).unwrap();
        one["replica"] = json!({ "name": "r1" });
        one["mxl"]["outputs"]["out-pgm"] = json!({ "flows": [id(4)] });
        let e = Config::parse(&one.to_string(), || unreachable!()).unwrap_err();
        assert!(e.starts_with("mxl.outputs.out-pgm.flows"), "{e}");
        one["mxl"]["outputs"]["out-pgm"] = json!({ "flow": id(4) });
        let (config, _, _) = Config::parse(&one.to_string(), || unreachable!()).unwrap();
        let audio = config.audio.unwrap();
        let output = &audio.outputs[0];
        assert_eq!(output.flow_id, id(4));
        assert_eq!(
            output.label, "PGM out",
            "a set of one writes the merged flow"
        );
    }

    /// The marker's name can come from the environment.
    #[test]
    fn the_marker_comes_from_the_environment() {
        let env =
            |k: &str| (k == "FADERZ_REPLICA_MARKER").then(|| "faderz-replica-cluster".to_owned());
        let mut v = three(json!({}));
        v.as_object_mut().unwrap().remove("marker");
        let text = with(v);
        let (config, _, _) = Config::parse_with(&text, &[], env, || "faderz-0".into()).unwrap();
        assert_eq!(config.set.unwrap().marker, "faderz-replica-cluster");
        // A set of one passes over it, and says so.
        let text = with(json!({}));
        let (config, _, _) = Config::parse_with(&text, &[], env, || "faderz-0".into()).unwrap();
        assert!(config.set.is_none());
        assert_eq!(
            config.notes,
            ["FADERZ_REPLICA_MARKER ignored: replica.replicas is 1"]
        );
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

    /// A channel's input configured `tone` is a samplez tone generator:
    /// no flow, no domain, even beside two domains, and wired to no flow;
    /// the shape carries the frequency and level it starts at, samplez's
    /// 1 kHz at -18 dBFS unless given, and the initial state its tone. A
    /// flow, input b, a domain or a flow channel selection beside it is
    /// refused, as are start values outside samplez's ranges, a channel
    /// with altInput, a channel with neither flow nor tone, and a shape
    /// that sets `tone` itself.
    #[test]
    fn a_tone_input_reads_no_flow() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = |st1: Value, domains: Value| {
            json!({
                "replica": { "name": "r1" },
                "shape": small(),
                "mxl": {
                    "domains": domains,
                    "inputs": {
                        "ch1": { "flow": id(1), "altFlow": id(5), "domain": "d" },
                        "ch2": { "flow": id(2), "domain": "d" },
                        "st1": st1,
                    },
                    "outputs": { "out-pgm": { "flow": id(4), "domain": "d" } },
                },
            })
            .to_string()
        };
        let two = json!([
            { "name": "d", "path": "/dev/shm/d" },
            { "name": "e", "path": "/dev/shm/e" },
        ]);
        let parse = |st1: Value| Config::parse(&config(st1, two.clone()), || unreachable!());

        let (c, shape, _) = parse(json!({ "tone": {} })).unwrap();
        let engine = c.audio.as_ref().unwrap();
        assert!(engine.inputs[2].tone.is_some());
        assert_eq!(
            (&engine.inputs[2].flow_id, &engine.inputs[2].domain),
            (&None, &None)
        );
        assert!(engine.inputs[..2].iter().all(|i| i.tone.is_none()));
        assert_eq!(
            c.wiring[2],
            Wiring {
                a: String::new(),
                b: None,
                domain: String::new(),
            }
        );
        let start = samplez_dsp::ToneParams::DEFAULT;
        assert_eq!(
            shape.channels[2].tone,
            Some(ChannelTone {
                frequency: start.frequency_hz,
                level: f64::from(start.level_db),
            })
        );
        assert_eq!(shape.channels[0].tone, None);
        let state = shape.initial_state();
        assert_eq!(
            state["channels"]["st1"]["tone"],
            json!({ "frequency": 1000, "level": -18.0, "ident": false })
        );
        assert!(state["channels"]["ch1"].get("tone").is_none());

        let (_, shape, _) = parse(json!({ "tone": { "frequency": 440, "level": -20.5 } })).unwrap();
        assert_eq!(
            shape.channels[2].tone,
            Some(ChannelTone {
                frequency: 440,
                level: -20.5,
            })
        );
        // The ends of samplez's ranges are taken.
        for (frequency, level) in [(20, -60.0), (20_000, 0.0)] {
            let tone = json!({ "tone": { "frequency": frequency, "level": level } });
            parse(tone).unwrap();
        }

        for key in ["flow", "altFlow", "domain", "channels"] {
            let value = if key == "channels" {
                json!([1, 2])
            } else if key == "domain" {
                json!("d")
            } else {
                json!(id(6))
            };
            let mut st1 = json!({ "tone": {} });
            st1[key] = value;
            let e = parse(st1).unwrap_err();
            assert_eq!(
                e,
                format!("mxl.inputs.st1.{key}: a tone input reads no flow")
            );
        }
        for (tone, refused) in [
            (json!({ "frequency": 19 }), "frequency 19"),
            (json!({ "frequency": 20_001 }), "frequency 20001"),
            (json!({ "level": 0.001 }), "level 0.001"),
            (json!({ "level": -60.001 }), "level -60.001"),
        ] {
            let e = parse(json!({ "tone": tone })).unwrap_err();
            assert!(
                e.starts_with(&format!("mxl.inputs.st1.tone.{refused}: not in")),
                "{e}"
            );
        }
        assert!(parse(json!({ "tone": { "ident": true } })).is_err());
        let e = parse(json!({})).unwrap_err();
        assert!(
            e.contains("mxl.inputs.st1.flow: required without tone"),
            "{e}"
        );

        let mut alt = json!({
            "replica": { "name": "r1" },
            "shape": small(),
            "mxl": {
                "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                "inputs": {
                    "ch1": { "tone": {} },
                    "ch2": { "flow": id(2) },
                    "st1": { "flow": id(3) },
                },
                "outputs": { "out-pgm": { "flow": id(4) } },
            },
        });
        let e = Config::parse(&alt.to_string(), || unreachable!()).unwrap_err();
        assert!(
            e.contains("mxl.inputs.ch1.tone: the channel has altInput"),
            "{e}"
        );
        alt["shape"]["channels"][0]["altInput"] = json!(false);
        Config::parse(&alt.to_string(), || unreachable!()).unwrap();
        alt["shape"]["channels"][1]["tone"] = json!({ "frequency": 1000, "level": -18 });
        let e = Config::parse(&alt.to_string(), || unreachable!()).unwrap_err();
        assert!(
            e.contains("channel ch2 tone: set by mxl.inputs.ch2.tone"),
            "{e}"
        );
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

        // Each bus, aux, mix-minus and monitor output no full-width output carries
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
        // The shape has a duck group: every column but a direct out's
        // may be ducked.
        assert_eq!(e.duck_columns, (0..9).collect::<Vec<_>>());
        let stereo = Some(vec!["L".to_owned(), "R".to_owned()]);
        assert_eq!(e.inputs[2].loudness_channels, stereo);
        assert_eq!(e.meters[0].loudness_channels, stereo);
        assert_eq!(e.reader_margin_samples, READER_MARGIN);

        let mut set: Value = serde_json::from_str(&ok).unwrap();
        set["engine"] = json!({ "readerMarginSamples": 960 });
        let (c, ..) = Config::parse(&set.to_string(), || unreachable!()).unwrap();
        assert_eq!(c.audio.unwrap().reader_margin_samples, 960);

        // The engine and the replica's peer connections take one delay
        // budget: the configured one, else DELAY_BUDGET.
        let (c, ..) = Config::parse(&ok, || unreachable!()).unwrap();
        assert_eq!(c.delay_budget(), DELAY_BUDGET);
        assert_eq!(c.audio.unwrap().delay_budget_samples, DELAY_BUDGET);
        set["engine"] = json!({ "delayBudgetSamples": 480_000 });
        let (c, ..) = Config::parse(&set.to_string(), || unreachable!()).unwrap();
        assert_eq!(c.delay_budget(), 480_000);
        assert_eq!(c.audio.unwrap().delay_budget_samples, 480_000);
    }

    /// A channel's delay line is opt-in, given in ms and taken in samples
    /// at the shape's rate, by the shape (`maxDelay`) and the engine
    /// (`maxDelaySamples`) alike. Lines that together hold more than the
    /// budget refuse the start, naming each channel's share; one past
    /// what the engine takes, or one written into the shape, too.
    #[test]
    fn delay_lines_are_opt_in_and_within_the_budget() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = |ch1: Value, st1: Value, engine: Value| {
            json!({
                "replica": { "name": "r1" },
                "shape": small(),
                "engine": engine,
                "mxl": {
                    "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                    "inputs": {
                        "ch1": { "flow": id(1), "altFlow": id(5), "maxDelayMs": ch1 },
                        "ch2": { "flow": id(2) },
                        "st1": { "flow": id(3), "maxDelayMs": st1 },
                    },
                    "outputs": { "out-pgm": { "flow": id(4) } },
                },
            })
            .to_string()
        };
        let (c, shape, _) = Config::parse(
            &config(json!(20), json!(5000), json!({})),
            || unreachable!(),
        )
        .unwrap();
        let lines: Vec<Option<u32>> = shape.channels.iter().map(|c| c.max_delay).collect();
        assert_eq!(lines, [Some(960), None, Some(240_000)]);
        let engine: Vec<u32> = (c.audio.unwrap().inputs.iter())
            .map(|i| i.max_delay_samples)
            .collect();
        // The line-up tone's input last, which has no line.
        assert_eq!(engine, [960, 0, 240_000, 0]);
        // 0 is no line.
        let (_, shape, _) =
            Config::parse(&config(json!(0), json!(0), json!({})), || unreachable!()).unwrap();
        assert!(shape.channels.iter().all(|c| c.max_delay.is_none()));

        // The stereo line counts twice.
        let held = 960 + 2 * 240_000;
        let fits = json!({ "delayBudgetSamples": held });
        Config::parse(&config(json!(20), json!(5000), fits), || unreachable!()).unwrap();
        let short = json!({ "delayBudgetSamples": held - 1 });
        let e =
            Config::parse(&config(json!(20), json!(5000), short), || unreachable!()).unwrap_err();
        assert!(e.contains("ch1 (960), st1 (480000)"), "{e}");
        assert!(
            e.contains(&format!("delayBudgetSamples {}", held - 1)),
            "{e}"
        );

        let e = Config::parse(
            &config(json!(20), json!(5000.1), json!({})),
            || unreachable!(),
        )
        .unwrap_err();
        assert!(e.contains("mxl.inputs.st1.maxDelayMs 5000.1"), "{e}");
        let e =
            Config::parse(&config(json!(-1), json!(0), json!({})), || unreachable!()).unwrap_err();
        assert!(e.contains("mxl.inputs.ch1.maxDelayMs -1"), "{e}");
        // A line too short for one sample is refused, not taken as none.
        let e = Config::parse(&config(json!(0.01), json!(0), json!({})), || unreachable!())
            .unwrap_err();
        assert!(
            e.contains("mxl.inputs.ch1.maxDelayMs 0.01: under one sample"),
            "{e}"
        );

        let mut set: Value = serde_json::from_str(&config(json!(0), json!(0), json!({}))).unwrap();
        set["shape"]["channels"][0]["maxDelay"] = json!(960);
        let e = Config::parse(&set.to_string(), || unreachable!()).unwrap_err();
        assert!(e.contains("set by mxl.inputs.ch1.maxDelayMs"), "{e}");
    }

    /// Declared consoles take defaults; a console the contract refuses or
    /// an id used twice refuses the configuration.
    #[test]
    fn consoles_are_checked_and_take_defaults() {
        let config = |consoles: Value| {
            json!({ "replica": { "name": "r1" }, "shape": small(), "consoles": consoles })
                .to_string()
        };
        let (c, ..) = Config::parse(
            &config(json!([
                { "id": "xt-1", "name": "X-Touch" },
                { "id": "osc-1", "name": "Tablet", "protocol": "osc", "transport": "osc-udp", "strips": 16 },
            ])),
            || unreachable!(),
        )
        .unwrap();
        assert_eq!(
            (
                c.consoles[0].protocol.as_str(),
                c.consoles[0].transport.as_str(),
                c.consoles[0].strips
            ),
            ("mcu", "bridge", 8)
        );
        assert_eq!(c.consoles[1].strips, 16);
        let (c, ..) = Config::parse(&with(json!({ "name": "r1" })), || unreachable!()).unwrap();
        assert!(c.consoles.is_empty());
        let refused =
            |consoles: Value| Config::parse(&config(consoles), || unreachable!()).unwrap_err();
        let e = refused(json!([{ "id": "a", "name": "A" }, { "id": "a", "name": "B" }]));
        assert!(e.contains("consoles[1].id \"a\": used twice"), "{e}");
        let e = refused(json!([{ "id": "Bad", "name": "A" }]));
        assert!(e.contains("consoles[0]"), "{e}");
        let e = refused(json!([{ "id": "a", "name": "" }]));
        assert!(e.contains("consoles[0]"), "{e}");
        let e = refused(json!([{ "id": "a", "name": "A", "strips": 0 }]));
        assert!(e.contains("consoles[0]"), "{e}");
        let e = refused(json!([{ "id": "a", "name": "A", "protocol": "midi" }]));
        assert!(e.contains("consoles[0]"), "{e}");
        let e = refused(json!([{ "id": "a", "name": "A", "bankOffset": 0 }]));
        assert!(e.contains("bankOffset"), "{e}");
    }

    /// A channel's 1-based flow channels reach the engine 0-based, one
    /// per leg, and label the channel's source; a selection of the wrong
    /// length or out of range is refused.
    #[test]
    fn a_channel_selects_its_legs_from_the_flow() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = |st1: Value, source: Option<&str>| {
            let mut shape = small();
            if let Some(s) = source {
                shape["channels"][2]["source"] = json!(s);
            }
            json!({
                "replica": { "name": "r1" },
                "shape": shape,
                "mxl": {
                    "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                    "inputs": {
                        "ch1": { "flow": id(1), "altFlow": id(5), "channels": [16] },
                        "ch2": { "flow": id(2) },
                        "st1": st1,
                    },
                    "outputs": { "out-pgm": { "flow": id(4) } },
                },
            })
            .to_string()
        };
        assert_eq!(MAX_FLOW_CHANNEL, samplez_mxl::input::MAX_FLOW_CHANNELS);
        let parse = |st1: Value, source| Config::parse(&config(st1, source), || unreachable!());
        let (c, shape, _) = parse(json!({ "flow": id(3), "channels": [7, 8] }), None).unwrap();
        let e = c.audio.unwrap();
        assert_eq!(e.inputs[0].flow_channels, Some(vec![15]));
        assert_eq!(e.inputs[1].flow_channels, None);
        assert_eq!(e.inputs[2].flow_channels, Some(vec![6, 7]));
        assert_eq!(e.inputs[2].channels, 2);
        let sources: Vec<_> = shape.channels.iter().map(|c| c.source.as_deref()).collect();
        assert_eq!(sources, [Some("ch 16"), None, Some("ch 7-8")]);
        check(&contract().shape, &serde_json::to_value(&shape).unwrap()).unwrap();

        let (_, shape, _) = parse(
            json!({ "flow": id(3), "channels": [2, 1] }),
            Some("Stagebox"),
        )
        .unwrap();
        assert_eq!(shape.channels[2].source.as_deref(), Some("Stagebox ch 2,1"));
        let (_, shape, _) = parse(json!({ "flow": id(3), "channels": [3, 3] }), None).unwrap();
        assert_eq!(shape.channels[2].source.as_deref(), Some("ch 3,3"));

        for (bad, why) in [
            (json!([7]), "names 1 flow channels, the channel is 2 wide"),
            (json!([7, 8, 9]), "names 3 flow channels"),
            (json!([0, 1]), "flow channel 0 is not in 1..=64"),
            (json!([1, 65]), "flow channel 65 is not in 1..=64"),
        ] {
            let e = parse(json!({ "flow": id(3), "channels": bad }), None).unwrap_err();
            assert!(
                e.contains("mxl.inputs.st1.channels: ") && e.contains(why),
                "{e}"
            );
        }
        let long = "x".repeat(MAX_SOURCE - 6);
        assert!(parse(json!({ "flow": id(3), "channels": [7, 8] }), Some(&long)).is_err());
        assert!(
            parse(
                json!({ "flow": id(3), "channels": [7, 8] }),
                Some(&long[1..])
            )
            .is_ok()
        );
    }

    /// More buses, auxes, mix-minus and monitor outputs without an output
    /// than the engine has meters for refuse the configuration, naming
    /// the limit; as many as it has are taken.
    #[test]
    fn more_uncarried_elements_than_engine_meters_are_refused() {
        let id = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
        let config = |auxes: usize| {
            let mut shape = small();
            // cf, mm1 and mon have no full-width output: aux1 and the added auxes
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

    /// NMOS settings that cannot work refuse the configuration: NMOS
    /// without an engine, an advertised address, plain HTTP or a merged
    /// flow where a set of more needs one.
    #[test]
    fn nmos_settings_that_cannot_work_are_refused() {
        let id = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
        let config = |nmos: Value, out: Value, replica: Value| {
            json!({
                "replica": replica,
                "shape": small(),
                "listeners": { "nmos": { "bind": "0.0.0.0", "port": 8081 } },
                "mxl": {
                    "domains": [{ "name": "d", "path": "/dev/shm/d", "id": id(9) }],
                    "inputs": {
                        "ch1": { "flow": id(1), "altFlow": id(5) },
                        "ch2": { "flow": id(2) },
                        "st1": { "flow": id(3) },
                    },
                    "outputs": { "out-pgm": out },
                },
                "nmos": nmos,
            })
            .to_string()
        };
        let one = || json!({ "name": "r1" });
        let flow = || json!({ "flow": id(4) });
        let on = |extra: Value| {
            let mut v = json!({ "enabled": true, "advertiseHost": "10.0.0.1" });
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let host = || "faderz-0".to_owned();
        let (c, ..) = Config::parse(&config(on(json!({})), flow(), one()), host).unwrap();
        assert_eq!(
            (c.nmos.heartbeat_s, c.nmos.asset.manufacturer.as_str()),
            (5, "danielclasen")
        );
        let refused = |text: String, key: &str| {
            let e = Config::parse(&text, host).unwrap_err();
            assert!(e.contains(key), "{key}: {e}");
        };
        let no_mxl = json!({ "replica": one(), "shape": small(), "nmos": on(json!({})) });
        refused(no_mxl.to_string(), "nmos.enabled");
        refused(
            config(json!({ "enabled": true }), flow(), one()),
            "nmos.advertiseHost",
        );
        let https = on(json!({ "registry": { "url": "https://registry:443" } }));
        refused(config(https, flow(), one()), "nmos.registry.url");
        refused(
            config(on(json!({ "heartbeatS": 11 })), flow(), one()),
            "nmos.heartbeatS",
        );
        let merged = json!({ "flow": id(4), "mergedFlow": id(6) });
        refused(config(json!({}), merged, one()), "mergedFlow");
        let set = || {
            json!({
                "replicas": 3,
                "peers": ["f-0.f:7443", "f-1.f:7443", "f-2.f:7443"],
                "peer": { "bind": "0.0.0.0", "port": 7443, "tls": {
                    "mode": "files", "certFile": "/t/tls.crt", "keyFile": "/t/tls.key",
                    "clientCaFile": "/t/ca.crt" } },
                "marker": "m",
            })
        };
        let members = || json!({ "flows": [id(4), id(7), id(8)] });
        refused(config(on(json!({})), members(), set()), "mergedFlow");
        let mut merged = members();
        merged["mergedFlow"] = json!(id(6));
        Config::parse(&config(on(json!({})), merged, set()), host).unwrap();
        // Without NMOS a set needs none.
        Config::parse(&config(json!({}), members(), set()), host).unwrap();
        // A merged flow no input reads: a receiver would feed it back.
        let mut fed = members();
        fed["mergedFlow"] = json!(id(1));
        refused(config(on(json!({})), fed, set()), "mxl.outputs");
        // The NMOS addresses of the others, one per replica.
        let mut merged = members();
        merged["mergedFlow"] = json!(id(6));
        let two = on(json!({ "peers": ["a:1", "b:1"] }));
        refused(config(two, merged.clone(), set()), "nmos.peers");
        let (c, ..) = Config::parse(&config(on(json!({})), merged.clone(), set()), host).unwrap();
        assert_eq!(c.nmos_peers(), ["f-0.f:8081", "f-1.f:8081", "f-2.f:8081"]);
        let mut port0: Value = serde_json::from_str(&config(on(json!({})), merged, set())).unwrap();
        let mut lo = port0.clone();
        lo["listeners"]["nmos"]["bind"] = json!("127.0.0.1");
        refused(lo.to_string(), "listeners.nmos.bind");
        port0["listeners"]["nmos"]["port"] = json!(0);
        port0["nmos"]["advertisePort"] = json!(8081);
        refused(port0.to_string(), "nmos.peers");
        // Every flow an output names is the mixer's own.
        let mut own = members();
        own["mergedFlow"] = json!(id(6));
        let (c, ..) = Config::parse(&config(json!({}), own, set()), host).unwrap();
        let want: std::collections::BTreeSet<String> = [4, 6, 7, 8].map(id).into();
        assert_eq!(c.own_flows(), want);
        // A loopback listener with another advertised host is noted.
        let mut loopback: Value =
            serde_json::from_str(&config(on(json!({})), flow(), one())).unwrap();
        loopback["listeners"]["nmos"]["bind"] = json!("127.0.0.1");
        let (c, ..) = Config::parse(&loopback.to_string(), host).unwrap();
        assert!(c.notes.iter().any(|n| n.contains("nmos.advertiseHost")));
    }

    /// Each channel's wiring names its input a and b flows and the domain
    /// the engine reads them in: the given one, or the only one there is.
    #[test]
    fn the_wiring_names_each_channels_flows_and_domain() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = |domains: Value, st1: Value| {
            json!({
                "replica": { "name": "r1" },
                "shape": small(),
                "mxl": {
                    "domains": domains,
                    "inputs": { "ch1": { "flow": id(1), "altFlow": id(5), "domain": "d" },
                                "ch2": { "flow": id(2), "domain": "d" }, "st1": st1 },
                    "outputs": { "out-pgm": { "flow": id(4), "domain": "d" } },
                },
            })
            .to_string()
        };
        let d = json!({ "name": "d", "path": "/dev/shm/d" });
        let e = json!({ "name": "e", "path": "/dev/shm/e" });
        let two = config(json!([d, e]), json!({ "flow": id(3), "domain": "e" }));
        let (c, ..) = Config::parse(&two, || unreachable!()).unwrap();
        let wiring = |a: u8, b: Option<u8>, domain: &str| Wiring {
            a: id(a),
            b: b.map(id),
            domain: domain.into(),
        };
        assert_eq!(
            c.wiring,
            [
                wiring(1, Some(5), "d"),
                wiring(2, None, "d"),
                wiring(3, None, "e")
            ]
        );
        let one = config(json!([d]), json!({ "flow": id(3) }));
        let (c, ..) = Config::parse(&one, || unreachable!()).unwrap();
        assert_eq!(c.wiring[2], wiring(3, None, "d"));
        // Without an engine there is nothing to wire.
        let none = json!({ "replica": { "name": "r1" }, "shape": small() }).to_string();
        assert!(
            Config::parse(&none, || unreachable!())
                .unwrap()
                .0
                .wiring
                .is_empty()
        );
    }

    /// The line-up tone's generator is the engine's last input, after the
    /// channels', as wide as the widest bus, aux or mix-minus column,
    /// with no flow; the matrix has its rows. It is no channel, so it is
    /// wired to nothing.
    #[test]
    fn the_line_up_tone_is_the_engines_last_input() {
        let id = |n: u8| format!("00000000-0000-4000-8000-00000000000{n}");
        let config = json!({
            "replica": { "name": "r1" },
            "shape": small(),
            "mxl": {
                "domains": [{ "name": "d", "path": "/dev/shm/d" }],
                "inputs": { "ch1": { "flow": id(1), "altFlow": id(5) },
                            "ch2": { "flow": id(2) }, "st1": { "flow": id(3) } },
                "outputs": { "out-pgm": { "flow": id(4) } },
            },
        });
        let (c, shape, _) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
        let engine = c.audio.as_ref().unwrap();
        let names: Vec<&str> = engine.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["ch1", "ch2", "st1", crate::mix::TONE_INPUT]);
        let tone = &engine.inputs[3];
        assert!(tone.tone.is_some() && tone.flow_id.is_none() && tone.domain.is_none());
        assert_eq!(tone.channels, 2, "pgm, cf and mm1 are stereo");
        assert_eq!(
            engine.matrix.len(),
            4 + 2,
            "the channels' legs, then the tone's"
        );
        assert_eq!(c.wiring.len(), shape.channels.len());
    }

    /// `replica.maxApplyLeadMs` is 50 unless set, and is taken from 30 to
    /// 120 alone.
    #[test]
    fn the_apply_lead_cap_is_50_ms_and_bounded() {
        let config = |replica: Value| json!({ "replica": replica, "shape": small() }).to_string();
        let lead = |replica: Value| {
            Config::parse(&config(replica), || unreachable!())
                .map(|c| c.0.replica.max_apply_lead_ms)
        };
        assert_eq!(lead(json!({ "name": "r1" })), Ok(50));
        for ms in [30, 80, 120] {
            assert_eq!(lead(json!({ "name": "r1", "maxApplyLeadMs": ms })), Ok(ms));
        }
        for ms in [29, 121] {
            let e = lead(json!({ "name": "r1", "maxApplyLeadMs": ms })).unwrap_err();
            assert!(e.starts_with("replica.maxApplyLeadMs"), "{e}");
        }
    }
}
