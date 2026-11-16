//! What the REST routes and the WebSocket share: the replica, the
//! commands in flight, the fan-out of deltas, and the parts of server
//! messages every route sends.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use samplez_replica::Replica;
use samplez_replica::entry::SessionKey;
use samplez_service::metrics::Metrics;
use samplez_service::replica::{Ack, Commands, Resolution};
use samplez_service::ws::{Delta, Frame, Hub, Slot, To};
use samplez_service::{Position, ReplicatedState};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;

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
    /// The revision of the last delta handed to the hub.
    published: watch::Sender<u64>,
    /// Each session's selection. Not replicated.
    selections: Mutex<BTreeMap<SessionKey, Selection>>,
    /// The connections each session has open on this replica.
    connections: Mutex<BTreeMap<SessionKey, usize>>,
    /// When each asking session last had a control request for a path
    /// forwarded.
    requests: Mutex<BTreeMap<(SessionKey, String), Instant>>,
}

/// A connection counted by [`App::connected`]; dropping it counts it
/// closed.
pub struct Connected<'a> {
    app: &'a App,
    session: SessionKey,
}

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.app.disconnected(&self.session);
    }
}

/// A session's selection.
struct Selection {
    /// The owner record of the connection that made it.
    owner: Value,
    element: String,
    /// When it was made.
    at: Instant,
    /// Whether its session has held a lease since.
    leased: bool,
}

/// The `replicas` message: the latest goes out on every change.
pub const REPLICAS: Slot = Slot {
    name: "replicas",
    every: Duration::ZERO,
};
/// The `selections` message: at most one per connection every 100 ms.
pub const SELECTIONS: Slot = Slot {
    name: "selections",
    every: Duration::from_millis(100),
};
/// How long a selection of a session that has not held a lease is kept:
/// one lease.
const SELECTION_GRACE: Duration = Duration::from_secs(crate::replica::LEASE / crate::replica::RATE);
/// A session's requests for one control forwarded at most this often.
pub const REQUEST_INTERVAL: Duration = Duration::from_secs(3);

impl App {
    pub fn new(replica: Arc<Replica<Mixer>>, name: String, metrics: &Metrics) -> Arc<App> {
        let published = watch::Sender::new(replica.revision());
        Arc::new(App {
            commands: Commands::new(replica.clone(), COMMANDS_IN_FLIGHT),
            replica,
            name,
            hub: Hub::new(CLIENT_DELTAS, metrics),
            published,
            selections: Mutex::default(),
            connections: Mutex::default(),
            requests: Mutex::default(),
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
                            self.published.send_replace(last);
                        }
                        // Dropped from the deltas held: clients catch up
                        // at the next delta, whose prev they did not see.
                        None => {
                            last = self.replica.revision();
                            self.published.send_replace(last);
                        }
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    let now = self.replicas_message();
                    if now != replicas {
                        self.hub.publish(Frame::Status(REPLICAS, now.clone().into()));
                        replicas = now;
                    }
                    self.lapse_selections();
                }
            }
        }
    }
}

impl App {
    /// The `selections` message.
    pub fn selections_message(&self) -> String {
        let list: Vec<Value> = self
            .selections
            .lock()
            .expect("selections lock poisoned")
            .values()
            .map(|s| json!({ "session": s.owner, "element": s.element }))
            .collect();
        json!({ "type": "selections", "replica": self.replica_ref(), "selections": list })
            .to_string()
    }

    /// Sets or clears the selection of `session`, `owner` its owner
    /// record, and tells every connection when that changed it. An
    /// element the shape has no strip or master for is ignored.
    pub fn select(&self, session: SessionKey, owner: Value, element: Option<&str>) {
        let known = element.is_none_or(|e| {
            self.replica.store().with_state(|sm| {
                let shape = sm.product.shape();
                shape.is_channel(e) || shape.is_output_bus(e)
            })
        });
        if !known {
            return;
        }
        let leased = self
            .replica
            .store()
            .with_state(|sm| sm.leases.contains_key(&session));
        let changed = {
            let mut selections = self.selections.lock().expect("selections lock poisoned");
            match element {
                Some(e) if selections.get(&session).is_none_or(|s| s.element != e) => {
                    let selection = Selection {
                        owner,
                        element: e.to_owned(),
                        at: Instant::now(),
                        leased,
                    };
                    selections.insert(session, selection);
                    true
                }
                None => selections.remove(&session).is_some(),
                Some(_) => false,
            }
        };
        if changed {
            self.hub
                .publish(Frame::Status(SELECTIONS, self.selections_message().into()));
        }
    }

    /// Counts a connection of `session` opened on this replica until the
    /// guard returned is dropped, as it also is by a panic or a cancelled
    /// serve.
    pub fn connected(&self, session: SessionKey) -> Connected<'_> {
        let mut connections = self.connections.lock().expect("connections lock poisoned");
        *connections.entry(session.clone()).or_default() += 1;
        Connected { app: self, session }
    }

    /// Counts a connection of `session` closed, and drops the session's
    /// selection with its last one.
    fn disconnected(&self, session: &SessionKey) {
        let last = {
            let mut connections = self.connections.lock().expect("connections lock poisoned");
            let open = connections.get_mut(session).expect("counted when opened");
            *open -= 1;
            *open == 0 && connections.remove(session).is_some()
        };
        if last {
            self.select(session.clone(), Value::Null, None);
        }
    }

    /// Drops the selections of sessions whose lease has ended. A session
    /// gets its first lease at samplez's next renewal of the live sessions,
    /// about a second after it connected, so one that has not held a lease
    /// yet keeps its selection for one lease from when it made it.
    fn lapse_selections(&self) {
        let leases = self.replica.store().with_state(|sm| sm.leases.clone());
        let lapsed = {
            let mut selections = self.selections.lock().expect("selections lock poisoned");
            let before = selections.len();
            selections.retain(|session, s| {
                if leases.contains_key(session) {
                    s.leased = true;
                    return true;
                }
                !s.leased && s.at.elapsed() < SELECTION_GRACE
            });
            selections.len() != before
        };
        if lapsed {
            self.hub
                .publish(Frame::Status(SELECTIONS, self.selections_message().into()));
        }
    }

    /// Hands `from`'s request for `path` to the primary connections of
    /// the session that owns it, once the delta that made it the owner
    /// is published, so the request never reaches a client ahead of the
    /// ownership it is about. Nothing when nobody else owns it, or when
    /// `from` asked for it within [`REQUEST_INTERVAL`].
    pub async fn request(&self, from: SessionKey, asker: Value, path: String) {
        let Some((owner, revision)) = self
            .replica
            .store()
            .with_state(|sm| sm.product.owner_of(&path).map(|owner| (owner, sm.revision)))
        else {
            return;
        };
        if owner == from {
            return;
        }
        {
            let mut requests = self.requests.lock().expect("requests lock poisoned");
            let now = Instant::now();
            requests.retain(|_, at| now.duration_since(*at) < REQUEST_INTERVAL);
            if requests.insert((from, path.clone()), now).is_some() {
                return;
            }
        }
        let mut published = self.published.subscribe();
        if published.wait_for(|p| *p >= revision).await.is_err() {
            return;
        }
        let text = json!({
            "type": "controlRequest",
            "replica": self.replica_ref(),
            "path": path,
            "from": asker,
        })
        .to_string();
        let to = To::Primary {
            principal: &owner.principal,
            session: &owner.session,
        };
        self.hub.send(to, Frame::Text(text.into()));
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
///
/// # Errors
///
/// When the operating system's random source fails.
pub fn uuid() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&crate::replica::random()?.to_le_bytes());
    b[8..].copy_from_slice(&crate::replica::random()?.to_le_bytes());
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::contract::{check, contract};
    use crate::replica;
    use crate::shape::Shape;
    use crate::state::{Action, Command, Control};
    use samplez_replica::entry::Body;
    use samplez_service::replica::Begin;
    use samplez_service::ws::Popped;
    use samplez_service::{Owner, Role};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIRS: AtomicU64 = AtomicU64::new(0);

    /// An App over a fresh replica of two channels, its [`App::publish`]
    /// loop not running, and the data directory to remove after.
    pub(crate) async fn app() -> (Arc<App>, PathBuf) {
        let deps = std::env::current_exe().unwrap();
        let dir = deps.parent().unwrap().join(format!(
            "faderz-app-{}-{}",
            std::process::id(),
            DIRS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let shape = Shape::parse(&json!({
            "sampleRate": 48000,
            "frameRate": { "numerator": 25, "denominator": 1 },
            "channels": [
                { "id": "ch1", "name": "1", "width": 1, "layout": "mono", "altInput": false },
                { "id": "ch2", "name": "2", "width": 1, "layout": "mono", "altInput": false },
            ],
            "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
            "auxes": [],
            "mixMinus": [],
            "vcas": [],
            "monitorOutputs": [],
            "outputs": [],
        }))
        .unwrap();
        let replica = replica::start(&dir, shape, Arc::new(replica::tai_samples))
            .await
            .unwrap();
        let app = App::new(Arc::new(replica), "r1".into(), &Metrics::default());
        (app, dir)
    }

    pub(crate) fn owner(session: &str) -> Owner {
        Owner {
            principal: "anonymous".into(),
            session: session.into(),
            role: Role::Operator,
            connection: format!("{session}-connection"),
        }
    }

    /// Waits until `leased` holds of the lease of `session`.
    async fn until_lease(app: &App, session: &SessionKey, leased: bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while app
                .replica
                .store()
                .with_state(|sm| sm.leases.contains_key(session))
                != leased
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the lease entry applied");
    }

    pub(crate) fn selected(app: &App, session: &SessionKey) -> bool {
        app.selections.lock().unwrap().contains_key(session)
    }

    /// A session that has not held a lease keeps its selection for one
    /// lease from when it made it, and not longer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_selection_without_a_lease_lapses_after_one_lease() {
        let (app, dir) = app().await;
        let s = SessionKey::from(&owner("s-a"));
        app.select(s.clone(), json!({}), Some("ch1"));
        app.lapse_selections();
        assert!(selected(&app, &s), "kept within the lease");
        {
            let mut selections = app.selections.lock().unwrap();
            let at = &mut selections.get_mut(&s).unwrap().at;
            *at = at.checked_sub(SELECTION_GRACE).unwrap();
        }
        app.lapse_selections();
        assert!(!selected(&app, &s), "lapsed after one lease");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A selection made before the session's first lease lapses as soon
    /// as the lease that arrived after it ends, however recent it is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_selection_made_before_the_lease_lapses_when_it_ends() {
        let (app, dir) = app().await;
        let o = owner("s-a");
        let s = SessionKey::from(&o);
        app.select(s.clone(), json!({}), Some("ch1"));
        app.replica.renew_sessions(std::slice::from_ref(&o));
        until_lease(&app, &s, true).await;
        app.lapse_selections();
        assert!(selected(&app, &s), "kept while leased");
        let renewed = app.replica.store().with_state(|sm| sm.leases[&s]);
        app.replica.append(Body::LeaseExpiry {
            session: s.clone(),
            renewed,
        });
        until_lease(&app, &s, false).await;
        app.lapse_selections();
        assert!(!selected(&app, &s), "lapsed with the lease");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A session's selection lapses as soon as a lease it held ends,
    /// however recent the selection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_selection_lapses_when_its_lease_ends() {
        let (app, dir) = app().await;
        let o = owner("s-a");
        let s = SessionKey::from(&o);
        app.replica.renew_sessions(std::slice::from_ref(&o));
        until_lease(&app, &s, true).await;
        app.select(s.clone(), json!({}), Some("ch1"));
        app.lapse_selections();
        assert!(selected(&app, &s), "kept while leased");
        let renewed = app.replica.store().with_state(|sm| sm.leases[&s]);
        app.replica.append(Body::LeaseExpiry {
            session: s.clone(),
            renewed,
        });
        until_lease(&app, &s, false).await;
        app.lapse_selections();
        assert!(!selected(&app, &s), "lapsed with the lease");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A control request goes out only once the delta that made its
    /// holder the owner is published, never ahead of it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_control_request_waits_for_the_owning_delta() {
        let (app, dir) = app().await;
        let holder = owner("s-a");
        let path = "/channels/ch1/fader";
        let id = uuid().unwrap();
        let touch = Command::Control(Control {
            id: id.clone(),
            action: Action::Touch,
            kind: "ui".into(),
            seq: 1,
            path: path.into(),
            value: None,
        });
        // The lone voter leads before it counts itself: a write before that
        // is refused not applied.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !app.replica.replica().commitable {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("commitable");
        let Begin::New(pending) = app.commands.begin(&holder, &id) else {
            panic!("a new command");
        };
        pending
            .submit(serde_json::to_vec(&touch).unwrap().into())
            .await
            .unwrap();
        let revision = app.replica.revision();
        assert!(*app.published.borrow() < revision);
        let queue = app.hub.subscribe(&holder);
        queue.set_primary(true);
        let asker = app.clone();
        tokio::spawn(async move {
            let from = SessionKey::from(&owner("s-b"));
            asker.request(from, json!({}), path.into()).await;
        });
        let early = tokio::time::timeout(Duration::from_millis(300), queue.pop()).await;
        assert!(early.is_err(), "nothing before the delta is published");
        app.published.send_replace(revision);
        let popped = tokio::time::timeout(Duration::from_secs(5), queue.pop())
            .await
            .expect("the request once the delta is published");
        let Popped::Frame(Frame::Text(text)) = popped else {
            panic!("a text frame");
        };
        assert!(
            text.as_str().contains(r#""type":"controlRequest""#),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn splice_appends_the_members_of_an_object() {
        assert_eq!(splice(r#"{"a":1"#, r#"{"b":2}"#), r#"{"a":1,"b":2}"#);
        assert_eq!(splice(r#"{"a":1"#, "{}"), r#"{"a":1}"#);
    }

    #[test]
    fn issued_ids_and_times_match_the_contract() {
        check(&contract().command_id, &json!(uuid().unwrap())).unwrap();
        let at = now_rfc3339();
        let scene = json!({ "id": "s", "name": "n", "storedAt": at });
        let shape = jsonschema::options()
            .should_validate_formats(true)
            .build(&json!({ "type": "string", "format": "date-time" }))
            .unwrap();
        assert!(shape.is_valid(&scene["storedAt"]), "{at}");
    }
}
