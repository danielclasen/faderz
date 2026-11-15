//! What the REST routes and the WebSocket share: the replica, the
//! commands in flight, the fan-out of deltas, and the parts of server
//! messages every route sends.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use samplez_replica::Replica;
use samplez_replica::entry::SessionKey;
use samplez_service::replica::{Ack, Commands, Resolution};
use samplez_service::ws::{Delta, Frame, Hub};
use samplez_service::{Position, ReplicatedState};
use serde_json::{Value, json};

use crate::state::{Answer, Mixer};

/// Deltas waiting per client before it is caught up by snapshot instead.
pub const CLIENT_DELTAS: usize = 1024;
/// Commands waiting for their answer on this replica at once, every
/// connection and REST together.
pub const COMMANDS_IN_FLIGHT: usize = 4096;

pub struct App {
    pub replica: Arc<Replica<Mixer>>,
    /// The replica's id in the contract (`ReplicaRef.id`).
    pub name: String,
    pub hub: Hub,
    pub commands: Commands,
}

impl App {
    pub fn new(replica: Arc<Replica<Mixer>>, name: String) -> Arc<App> {
        Arc::new(App {
            commands: Commands::new(replica.clone(), COMMANDS_IN_FLIGHT),
            replica,
            name,
            hub: Hub::new(CLIENT_DELTAS),
        })
    }

    pub fn epoch(&self) -> u64 {
        self.replica.epoch()
    }

    /// Model `ReplicaRef`: the replica's role in the contract is leader or
    /// follower; openraft's other states are not leading.
    pub fn replica_ref(&self) -> Value {
        let role = if self.replica.replica().role == "leader" {
            "leader"
        } else {
            "follower"
        };
        json!({ "id": self.name, "role": role })
    }

    /// Model `ReplicaStatus` of every replica this one knows: itself.
    pub fn replicas(&self) -> Value {
        let info = self.replica.replica();
        let mut status = self.replica_ref();
        status["healthy"] = json!(info.ready);
        status["commitable"] = json!(info.commitable);
        json!([status])
    }

    /// The `replicas` message.
    pub fn replicas_message(&self) -> String {
        json!({ "type": "replicas", "replica": self.replica_ref(), "replicas": self.replicas() })
            .to_string()
    }

    /// The `delta` message at `revision` of a delta the state machine
    /// keeps without them ([`samplez_replica::state_machine::Product::delta`]).
    pub fn delta_message(&self, revision: u64, body: &str) -> String {
        splice(
            &format!(
                r#"{{"type":"delta","replica":{},"revision":{revision}"#,
                self.replica_ref()
            ),
            body,
        )
    }

    /// What a command that resolved as `ack` got beside it, from the
    /// state's table of refusals and stored scenes.
    pub fn answer(&self, session: &SessionKey, ack: &Ack) -> Option<Answer> {
        match ack.resolution {
            Resolution::Applied | Resolution::Unchanged | Resolution::Duplicate => self
                .replica
                .store()
                .with_state(|sm| sm.product.answer(session, &ack.command).cloned()),
            _ => None,
        }
    }

    /// The `atIndex` of the delta at `revision`, while this replica holds
    /// it.
    pub async fn at_index(&self, revision: u64) -> Option<u64> {
        let since = Position {
            epoch: self.epoch(),
            revision: revision.checked_sub(1)?,
        };
        let deltas = self.replica.deltas_since(since).await?;
        let delta = deltas.into_iter().find(|d| d.revision == revision)?;
        serde_json::from_str::<Value>(&delta.text).ok()?["atIndex"].as_u64()
    }

    /// Publishes every delta to the clients once this replica applied
    /// it, with this replica's ref, and a `replicas` message whenever
    /// what it says changes. Never returns.
    pub async fn publish(&self) {
        let mut last = self.replica.revision();
        let mut replicas = self.replicas_message();
        loop {
            tokio::select! {
                () = self.replica.applied(last + 1) => {
                    let since = Position { epoch: self.epoch(), revision: last };
                    match self.replica.deltas_since(since).await {
                        Some(deltas) => {
                            for d in deltas {
                                last = d.revision;
                                let text = self.delta_message(d.revision, &d.text);
                                self.hub.publish(Frame::Delta(Delta { text: text.into(), ..d }));
                            }
                        }
                        // Dropped from the deltas held: clients catch up
                        // at the next delta, whose prev they did not see.
                        None => last = self.replica.revision(),
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    let now = self.replicas_message();
                    if now != replicas {
                        // samplez-service has no frame for product status
                        // messages: it rides the counters slot, which no
                        // counters use while there is no engine.
                        self.hub.publish(Frame::Counters(now.clone().into()));
                        replicas = now;
                    }
                }
            }
        }
    }
}

/// `prefix` (an object's opening and first members) followed by the
/// members of the JSON object `object`.
pub fn splice(prefix: &str, object: &str) -> String {
    let members = object.trim().strip_prefix('{').unwrap_or("}");
    if members.trim_start().starts_with('}') {
        format!("{prefix}}}")
    } else {
        format!("{prefix},{members}")
    }
}

/// The current time as RFC 3339 UTC with milliseconds.
pub fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (H. Hinnant, chrono-compatible).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        now.subsec_millis()
    )
}

/// A random (version 4) UUID, for commands the server issues itself.
pub fn uuid() -> String {
    let mut b = [0u8; 16];
    // SAFETY: `b` is writable for its length.
    unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{check, contract};

    #[test]
    fn splice_appends_the_members_of_an_object() {
        assert_eq!(splice(r#"{"a":1"#, r#"{"b":2}"#), r#"{"a":1,"b":2}"#);
        assert_eq!(splice(r#"{"a":1"#, "{}"), r#"{"a":1}"#);
    }

    #[test]
    fn issued_ids_and_times_match_the_contract() {
        check(&contract().command_id, &json!(uuid())).unwrap();
        let at = now_rfc3339();
        let scene = json!({ "id": "s", "name": "n", "storedAt": at });
        let shape = jsonschema::options()
            .should_validate_formats(true)
            .build(&json!({ "type": "string", "format": "date-time" }))
            .unwrap();
        assert!(shape.is_valid(&scene["storedAt"]), "{at}");
    }
}
