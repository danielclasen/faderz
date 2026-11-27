//! The bridge's configuration file (JSON).

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::mcu::{Device, Key};
use crate::rtpmidi::{self, Cidr};
use crate::surface::{DuckKeys, UnitSpec};

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
    /// The X-Touch's duck keys by note; F1 to F8 on the shape's duck
    /// groups in order when absent.
    #[serde(default, deserialize_with = "duck_keys")]
    pub keys: Option<DuckKeys>,
    /// The time of day on the X-Touch's timecode display; dark when absent.
    #[serde(default)]
    pub clock: Option<Clock>,
}

/// The time of day an X-Touch shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Clock {
    /// Minutes east of UTC, written `+01:00` or `-05:30`; UTC when absent.
    /// A fixed offset: daylight saving time is not followed.
    #[serde(default, deserialize_with = "utc_offset")]
    pub utc_offset: i32,
}

/// An offset `+HH:MM` or `-HH:MM`, at most 14 hours, as minutes.
fn utc_offset<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    let text = String::deserialize(d)?;
    parse_offset(&text).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "clock.utcOffset: {text:?} is no offset like +01:00"
        ))
    })
}

fn parse_offset(text: &str) -> Option<i32> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let (h, m) = rest.split_once(':')?;
    // i32::parse takes a sign of its own, so "+-1:00" would pass.
    if h.len() != 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    (m < 60 && h * 60 + m <= 14 * 60).then_some(sign * (h * 60 + m))
}

/// The `keys` map, its notes written in decimal; a note named twice
/// (`"0"` and `"00"`) is refused rather than one entry dropped.
fn duck_keys<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DuckKeys>, D::Error> {
    struct Keys;
    impl<'de> Visitor<'de> for Keys {
        type Value = DuckKeys;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of MIDI notes to duck keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DuckKeys, A::Error> {
            let mut keys = DuckKeys::new();
            while let Some((note, key)) = map.next_entry::<String, _>()? {
                let n: u8 = note
                    .parse()
                    .map_err(|_| de::Error::custom(format!("keys: {note:?} is not a MIDI note")))?;
                if keys.insert(n, key).is_some() {
                    return Err(de::Error::custom(format!("keys: note {n} twice")));
                }
            }
            Ok(keys)
        }
    }
    d.deserialize_map(Keys).map(Some)
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
    /// Sign-in at the OIDC provider by the device authorization grant
    /// (RFC 8628), as a public client.
    Device {
        /// The provider's issuer, e.g. `https://idp.example/realms/media`.
        issuer: String,
        client_id: String,
        /// Requested in addition to `openid`.
        #[serde(default)]
        scopes: Vec<String>,
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
    #[serde(default)]
    pub transport: Transport,
    /// `midi`: a part of the MIDI input port's name.
    #[serde(default)]
    pub midi_in: Option<String>,
    /// `midi`: a part of the MIDI output port's name; the input's when
    /// absent.
    #[serde(default)]
    pub midi_out: Option<String>,
    /// `rtp-midi`: the session.
    #[serde(default)]
    pub rtp_midi: Option<RtpMidi>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// Local MIDI ports (USB-MIDI, or what a network MIDI daemon offers).
    #[default]
    Midi,
    /// An RTP-MIDI (AppleMIDI) session of the bridge's own.
    RtpMidi,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RtpMidi {
    /// The control port; the data port is the one above it.
    #[serde(default = "default_rtp_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind: IpAddr,
    /// The session name the peer sees; the unit's name when absent.
    #[serde(default)]
    pub session_name: Option<String>,
    /// Addresses or ranges (`198.51.100.0/24`) whose invitations are
    /// accepted.
    #[serde(default)]
    pub allow: Vec<String>,
    /// `host:port`: the control port of a peer the bridge invites.
    #[serde(default)]
    pub invite: Option<String>,
    #[serde(default = "default_rate")]
    pub max_bytes_per_second: u32,
}

fn default_rtp_port() -> u16 {
    5004
}

fn default_bind() -> IpAddr {
    IpAddr::from([0, 0, 0, 0])
}

fn default_rate() -> u32 {
    32_000
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

impl Unit {
    fn check_transport(&self) -> Result<(), String> {
        match (self.transport, &self.midi_in, &self.rtp_midi) {
            (Transport::Midi, Some(_), None) => Ok(()),
            (Transport::Midi, None, _) => Err("midiIn: the MIDI port to open".into()),
            (Transport::Midi, _, Some(_)) => Err("rtpMidi needs transport rtp-midi".into()),
            (Transport::RtpMidi, None, Some(r)) if self.midi_out.is_none() => {
                if r.port == 0 || r.port == u16::MAX {
                    return Err("rtpMidi.port: 1 to 65534".into());
                }
                for a in &r.allow {
                    a.parse::<Cidr>()
                        .map_err(|e| format!("rtpMidi.allow: {e}"))?;
                }
                if let Some(i) = &r.invite {
                    let ok = i.rsplit_once(':').is_some_and(|(host, port)| {
                        !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0)
                    });
                    if !ok {
                        return Err(format!("rtpMidi.invite {i:?} is not host:port"));
                    }
                }
                if r.allow.is_empty() && r.invite.is_none() {
                    return Err("rtpMidi: allow or invite, or nobody can connect".into());
                }
                let name = r.session_name.as_deref().unwrap_or(&self.name);
                if name.is_empty() || name.len() > 64 || name.contains('\0') {
                    return Err("rtpMidi.sessionName: 1 to 64 octets".into());
                }
                if r.max_bytes_per_second < 1000 {
                    return Err("rtpMidi.maxBytesPerSecond: at least 1000".into());
                }
                Ok(())
            }
            (Transport::RtpMidi, _, None) => Err("rtpMidi: the session to open".into()),
            (Transport::RtpMidi, _, Some(_)) => {
                Err("midiIn and midiOut belong to transport midi".into())
            }
        }
    }

    /// The RTP-MIDI session of a `rtp-midi` unit, its peer to invite
    /// resolved.
    pub fn rtp_options(&self) -> Result<Option<rtpmidi::Options>, String> {
        let Some(r) = &self.rtp_midi else {
            return Ok(None);
        };
        let invite = match &r.invite {
            Some(i) => Some(
                i.to_socket_addrs()
                    .map_err(|e| format!("rtpMidi.invite {i}: {e}"))?
                    .min_by_key(SocketAddr::is_ipv6)
                    .ok_or_else(|| format!("rtpMidi.invite {i}: no address"))?,
            ),
            None => None,
        };
        Ok(Some(rtpmidi::Options {
            bind: r.bind,
            port: r.port,
            name: r.session_name.clone().unwrap_or_else(|| self.name.clone()),
            allow: r
                .allow
                .iter()
                .map(|a| a.parse())
                .collect::<Result<_, _>>()?,
            invite,
            max_bytes_per_second: r.max_bytes_per_second,
        }))
    }
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
            u.check_transport()
                .map_err(|e| format!("unit {}: {e}", u.id))?;
        }
        let mut ports = std::collections::HashSet::new();
        for (u, r) in config
            .units
            .iter()
            .filter_map(|u| u.rtp_midi.as_ref().map(|r| (u, r)))
        {
            if !(ports.insert(r.port) && ports.insert(r.port.wrapping_add(1))) {
                return Err(format!(
                    "unit {}: rtpMidi ports taken by another unit",
                    u.id
                ));
            }
        }
        for (note, key) in config.keys.iter().flatten() {
            // REC, the function keys, the V-Pot pushes, ALT and notes the
            // surface does not map. A bound key wins over its mapping.
            let free = matches!(
                Key::from_note(*note),
                Key::Rec(_) | Key::Function(_) | Key::Pot(_) | Key::Alt | Key::Other(_)
            ) && *note < 104;
            if !free {
                return Err(format!(
                    "keys.{note}: not a REC, V-Pot push, function, ALT or other unmapped key below note 104"
                ));
            }
            if !id_ok(&key.duck) {
                return Err(format!(
                    "keys.{note}: duck {:?} is not a faderz id",
                    key.duck
                ));
            }
        }
        if let Auth::Token {
            token: None,
            token_file: None,
        } = config.auth
        {
            return Err("auth mode token needs token or tokenFile".into());
        }
        if let Auth::Device {
            issuer, client_id, ..
        } = &config.auth
        {
            let local = ["http://127.0.0.1", "http://[::1]", "http://localhost"]
                .iter()
                .any(|p| issuer.starts_with(p));
            if !(issuer.starts_with("https://") || local) {
                return Err(format!(
                    "auth.issuer {issuer:?}: https, or http on loopback"
                ));
            }
            if client_id.is_empty() {
                return Err("auth.clientId is empty".into());
            }
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

    /// What the configuration asks for that cannot take effect.
    pub fn warnings(&self) -> Vec<String> {
        let x_touch = self.units.iter().any(|u| u.model == Model::XTouch);
        let mut warnings = Vec::new();
        if self.keys.as_ref().is_some_and(|k| !k.is_empty()) && !x_touch {
            warnings.push("keys: no unit is an x-touch, which has the duck keys".into());
        }
        if self.clock.is_some() && !x_touch {
            warnings.push("clock: no unit is an x-touch, which has the timecode display".into());
        }
        warnings
    }

    pub fn session_file(&self) -> PathBuf {
        self.state_dir.join("session")
    }

    /// Where a device sign-in keeps its tokens.
    pub fn tokens_file(&self) -> PathBuf {
        self.state_dir.join("tokens.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::KeyAction;

    #[test]
    fn a_clock_without_an_x_touch_is_warned_about() {
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"],
                 "units": [{ "id": "xe", "name": "X", "model": "x-touch-extender", "midiIn": "X" }],
                 "clock": { "utcOffset": "+01:00" } }"#,
        )
        .unwrap();
        assert_eq!(c.warnings().len(), 1, "{:?}", c.warnings());
        assert!(c.warnings()[0].starts_with("clock:"));
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"], "units": [{ "id": "xt", "name": "X", "midiIn": "X" }],
                 "clock": { "utcOffset": "+01:00" } }"#,
        )
        .unwrap();
        assert!(c.warnings().is_empty());
    }

    #[test]
    fn the_clock_takes_a_fixed_utc_offset() {
        assert_eq!(parse_offset("+01:00"), Some(60));
        assert_eq!(parse_offset("-05:30"), Some(-330));
        assert_eq!(parse_offset("+14:00"), Some(840));
        for bad in [
            "01:00", "+1:00", "+01", "+15:00", "+01:60", "+aa:00", "+-1:00", "++1:00", "+01:-1",
            "-+1:00", "",
        ] {
            assert_eq!(parse_offset(bad), None, "{bad}");
        }
        let unit = r#"{ "id": "xt", "name": "X", "model": "x-touch", "midiIn": "X" }"#;
        let c = Config::parse(&format!(
            r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "clock": {{ "utcOffset": "+02:00" }} }}"#
        ))
        .unwrap();
        assert_eq!(c.clock, Some(Clock { utc_offset: 120 }));
        let c = Config::parse(&format!(
            r#"{{ "endpoints": ["ws://x"], "units": [{unit}] }}"#
        ))
        .unwrap();
        assert_eq!(c.clock, None, "dark when absent");
        let c = Config::parse(&format!(
            r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "clock": {{}} }}"#
        ))
        .unwrap();
        assert_eq!(c.clock, Some(Clock { utc_offset: 0 }), "UTC by default");
        let err = Config::parse(&format!(
            r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "clock": {{ "utcOffset": "+2" }} }}"#
        ))
        .unwrap_err();
        assert!(err.contains("clock.utcOffset"), "{err}");
    }

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
        assert_eq!(c.units[0].transport, Transport::Midi);
        assert!(c.units[0].rtp_options().unwrap().is_none());
        assert_eq!(c.keys, None);
    }

    #[test]
    fn duck_keys_name_a_note_a_group_and_an_action() {
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"], "units": [{ "id": "xt", "name": "X", "midiIn": "X" }],
                 "keys": { "0": { "duck": "comm-a", "action": "hold" },
                           "54": { "duck": "comm-b" },
                           "55": { "duck": "comm-b", "action": "latch" } } }"#,
        )
        .unwrap();
        let keys = c.keys.as_ref().unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[&0].duck, "comm-a");
        assert_eq!(keys[&0].action, KeyAction::Hold);
        assert_eq!(keys[&54].action, KeyAction::HoldOrLatch);
        assert_eq!(keys[&55].action, KeyAction::Latch);
        assert!(c.warnings().is_empty());
        // An empty map is no duck keys, not the function keys.
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"], "units": [{ "id": "xt", "name": "X", "midiIn": "X" }],
                 "keys": {} }"#,
        )
        .unwrap();
        assert_eq!(c.keys, Some(DuckKeys::new()));
        // Keys on a bridge with Extenders only cannot take effect.
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"],
                 "units": [{ "id": "xe", "name": "X", "model": "x-touch-extender", "midiIn": "X" }],
                 "keys": { "0": { "duck": "comm" } } }"#,
        )
        .unwrap();
        assert_eq!(c.warnings().len(), 1, "{:?}", c.warnings());
        assert!(c.warnings()[0].contains("x-touch"));
        // A V-Pot push and ALT can be duck keys, as before they were
        // mapped.
        let c = Config::parse(
            r#"{ "endpoints": ["ws://x"],
                 "units": [{ "id": "xt", "name": "X", "model": "x-touch", "midiIn": "X" }],
                 "keys": { "32": { "duck": "comm" }, "73": { "duck": "comm-b" } } }"#,
        )
        .unwrap();
        let keys = c.keys.unwrap();
        assert_eq!(keys[&32].duck, "comm");
        assert_eq!(keys[&73].duck, "comm-b");
    }

    #[test]
    fn an_rtp_midi_unit_names_its_ports_peers_and_rate() {
        let c = Config::parse(
            r#"{ "endpoints": ["wss://f.example/api/v1/ws"],
                 "units": [
                   { "id": "xt", "name": "X-Touch", "transport": "rtp-midi",
                     "rtpMidi": { "allow": ["198.51.100.0/24", "192.0.2.101"] } },
                   { "id": "xt-ext", "name": "X-Touch Ext", "model": "x-touch-extender",
                     "transport": "rtp-midi",
                     "rtpMidi": { "port": 5006, "bind": "127.0.0.1", "sessionName": "faderz ext",
                                  "invite": "127.0.0.1:5104", "maxBytesPerSecond": 8000 } }
                 ] }"#,
        )
        .unwrap();
        let a = c.units[0].rtp_options().unwrap().unwrap();
        assert_eq!(a.port, 5004);
        assert_eq!(a.bind, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(a.name, "X-Touch");
        assert!(a.allow[0].contains("198.51.100.7".parse().unwrap()));
        assert!(a.allow[1].contains("192.0.2.101".parse().unwrap()));
        assert_eq!(a.invite, None);
        assert_eq!(a.max_bytes_per_second, 32_000);
        let b = c.units[1].rtp_options().unwrap().unwrap();
        assert_eq!(b.port, 5006);
        assert_eq!(b.name, "faderz ext");
        assert!(b.allow.is_empty());
        assert_eq!(b.invite, Some("127.0.0.1:5104".parse().unwrap()));
        assert_eq!(b.max_bytes_per_second, 8000);
    }

    #[test]
    fn a_device_sign_in_names_the_provider_and_the_client() {
        let c = Config::parse(
            r#"{ "endpoints": ["wss://f.example/api/v1/ws"], "stateDir": "/s",
                 "auth": { "mode": "device", "issuer": "https://idp.example/realms/m",
                           "clientId": "faderz-bridge", "scopes": ["offline_access"] },
                 "units": [{ "id": "xt", "name": "X", "midiIn": "X" }] }"#,
        )
        .unwrap();
        let Auth::Device {
            issuer,
            client_id,
            scopes,
        } = &c.auth
        else {
            panic!("{:?}", c.auth);
        };
        assert_eq!(issuer, "https://idp.example/realms/m");
        assert_eq!(client_id, "faderz-bridge");
        assert_eq!(scopes, &["offline_access"]);
        assert_eq!(c.tokens_file(), PathBuf::from("/s/tokens.json"));
    }

    #[test]
    fn mistakes_are_refused() {
        let unit = r#"{ "id": "xt", "name": "X", "midiIn": "X" }"#;
        let rtp = |fields: &str| {
            format!(
                r#"{{ "endpoints": ["ws://x"], "units": [{{ "id": "xt", "name": "X", {fields} }}] }}"#
            )
        };
        for (text, why) in [
            (format!(r#"{{ "endpoints": [], "units": [{unit}] }}"#), "endpoints"),
            (format!(r#"{{ "endpoints": ["http://x"], "units": [{unit}] }}"#), "ws://"),
            (r#"{ "endpoints": ["ws://x"], "units": [] }"#.into(), "units"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}, {unit}] }}"#), "twice"),
            (r#"{ "endpoints": ["ws://x"], "units": [{ "id": "X", "name": "X", "midiIn": "X" }] }"#.into(), "faderz id"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "auth": {{ "mode": "token" }} }}"#), "tokenFile"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "extra": 1 }}"#), "unknown field"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "auth": {{ "mode": "device", "issuer": "http://idp.example", "clientId": "b" }} }}"#), "loopback"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "auth": {{ "mode": "device", "issuer": "https://idp.example", "clientId": "" }} }}"#), "clientId"),
            (r#"{ "endpoints": ["ws://x"], "units": [{ "id": "xt", "name": "X" }] }"#.into(), "midiIn"),
            (rtp(r#""transport": "rtp-midi""#), "rtpMidi: the session"),
            (rtp(r#""midiIn": "X", "rtpMidi": { "allow": ["192.0.2.1"] }"#), "transport rtp-midi"),
            (rtp(r#""transport": "rtp-midi", "midiIn": "X", "rtpMidi": { "allow": ["192.0.2.1"] }"#), "belong to transport midi"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": {}"#), "allow or invite"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": { "allow": ["192.0.2.0/40"] }"#), "rtpMidi.allow"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": { "invite": "192.0.2.1" }"#), "host:port"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": { "port": 65535, "allow": ["192.0.2.1"] }"#), "1 to 65534"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": { "allow": ["192.0.2.1"], "maxBytesPerSecond": 10 }"#), "at least 1000"),
            (rtp(r#""transport": "rtp-midi", "rtpMidi": { "allow": ["192.0.2.1"], "tls": true }"#), "unknown field"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "16": {{ "duck": "comm" }} }} }}"#), "keys.16: not a REC"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "104": {{ "duck": "comm" }} }} }}"#), "keys.104"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "1": {{ "duck": "Comm" }} }} }}"#), "not a faderz id"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "1": {{ "duck": "comm", "action": "toggle" }} }} }}"#), "unknown variant"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "F1": {{ "duck": "comm" }} }} }}"#), "not a MIDI note"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "256": {{ "duck": "comm" }} }} }}"#), "not a MIDI note"),
            (format!(r#"{{ "endpoints": ["ws://x"], "units": [{unit}], "keys": {{ "0": {{ "duck": "a" }}, "00": {{ "duck": "b" }} }} }}"#), "note 0 twice"),
            (
                r#"{ "endpoints": ["ws://x"], "units": [
                    { "id": "a", "name": "A", "transport": "rtp-midi", "rtpMidi": { "allow": ["192.0.2.1"] } },
                    { "id": "b", "name": "B", "transport": "rtp-midi", "rtpMidi": { "port": 5005, "allow": ["192.0.2.1"] } }
                ] }"#.into(),
                "taken by another unit",
            ),
        ] {
            let e = Config::parse(&text).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
    }
}
