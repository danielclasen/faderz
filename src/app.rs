//! What the REST routes and the WebSocket share: the replica, the
//! commands in flight, the fan-out of deltas, and the parts of server
//! messages every route sends.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use samplez_replica::Replica;
use samplez_replica::entry::{NodeId, SessionKey};
use samplez_replica::membership;
use samplez_replica::state_machine::StateMachine;
use samplez_service::config::AuthMode;
use samplez_service::metrics::Metrics;
use samplez_service::replica::{Ack, Begin, Commands, ReplicaInfo, Resolution};
use samplez_service::ws::{Delta, Frame, Hub, Slot, To};
use samplez_service::{Identity, Owner, Position, ReplicatedState};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use crate::audio::Audio;
use crate::config::Set;
use crate::state::{Answer, Command, Mixer};

thread_local! {
    /// Whether this thread is inside [`with_state`], holding the log
    /// store's lock.
    static IN_STATE: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` on the replicated state of `replica`, under its log store's
/// lock. samplez takes its contact lock and then the store's, and until
/// the replica is ready its own view of itself takes the store's again,
/// so `f` must not call into the replica: [`App::info`] and
/// [`App::contact_metrics`] would deadlock. Debug builds assert that.
pub fn with_state<T>(replica: &Replica<Mixer>, f: impl FnOnce(&StateMachine<Mixer>) -> T) -> T {
    struct Leave(bool);
    impl Drop for Leave {
        fn drop(&mut self) {
            IN_STATE.set(self.0);
        }
    }
    let _leave = Leave(IN_STATE.replace(true));
    replica.store().with_state(f)
}

/// Asserts in debug builds that this thread is not inside
/// [`with_state`].
fn outside_state() {
    debug_assert!(
        !IN_STATE.get(),
        "a call into the replica inside App::with_state: the contact lock under the store lock"
    );
}

/// Deltas waiting per client before it is caught up by snapshot instead.
pub const CLIENT_DELTAS: usize = 1024;
/// Commands waiting for their answer on this replica at once, every
/// connection and REST together.
pub const COMMANDS_IN_FLIGHT: usize = 4096;

pub struct App {
    pub replica: Arc<Replica<Mixer>>,
    /// The replica's id in the contract (`ReplicaRef.id`).
    pub name: String,
    /// The id of each replica of the set by ordinal ([`member_names`]);
    /// empty for a set of one.
    members: Vec<String>,
    pub hub: Hub,
    pub commands: Commands,
    /// The revision of the last delta handed to the hub.
    published: watch::Sender<u64>,
    /// Each session's selection. Not replicated.
    selections: Mutex<BTreeMap<SessionKey, Selection>>,
    /// The sessions with a connection open on this replica.
    sessions: Mutex<BTreeMap<SessionKey, Present>>,
    /// What `presence` names a session without a display name.
    unnamed: &'static str,
    /// When each asking session last had a control request for a path
    /// forwarded.
    requests: Mutex<BTreeMap<(SessionKey, String), Instant>>,
    /// The engine's counters last read ([`App::count`]); none without an
    /// engine.
    counters: Mutex<Option<Value>>,
    /// The engine's inputs and outputs last read ([`App::count`]); none
    /// without an engine.
    engine: Mutex<Option<Value>>,
    /// When each follower last matched all of this leader's log, as
    /// [`replicas`](App::replicas) saw it; read without an engine only.
    caught_up: Mutex<BTreeMap<NodeId, Instant>>,
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

/// A session with a connection open on this replica.
struct Present {
    /// Its connections open on this replica.
    open: usize,
    /// Its presence record without the times, from the latest hello; none
    /// before the first.
    record: Option<Value>,
    /// When its first open connection opened.
    since: SystemTime,
    /// When it last sent a command, a selection or a control request, or
    /// opened its first connection.
    active: SystemTime,
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
/// The `presence` message: at most one per connection every 100 ms.
pub const PRESENCE: Slot = Slot {
    name: "presence",
    every: Duration::from_millis(100),
};
/// How often [`App::count`] reads and sends the engine's counters.
pub const COUNTERS_PERIOD: Duration = Duration::from_secs(1);
/// How long a selection of a session that has not held a lease is kept:
/// one lease.
const SELECTION_GRACE: Duration = samplez_replica::session::LEASE;
/// A session's requests for one control forwarded at most this often.
pub const REQUEST_INTERVAL: Duration = Duration::from_secs(3);
/// How long after the asker's replica took a control request another
/// replica still hands it to the holder.
const REQUEST_LATE: Duration = Duration::from_secs(10);

/// The id of each replica of `set` by ordinal: `name` for this one, and
/// for the others the StatefulSet's name and their ordinal, as its pods
/// are named, or, without a StatefulSet, their peer address as listed
/// (`host:port`, unique by validation). Entry n is the replica of
/// ordinal n, as `peers[n]` is its address.
pub fn member_names(set: &Set, name: &str) -> Vec<String> {
    (set.peers.iter().enumerate())
        .map(|(n, addr)| match &set.statefulset {
            _ if n == usize::from(set.ordinal) => name.to_owned(),
            Some(statefulset) => format!("{statefulset}-{n}"),
            None => addr.clone(),
        })
        .collect()
}

/// How many log entries a follower may trail the leader's log by and
/// still count as heard: replication runs in batches, so an exact match
/// would flap during a burst of writes.
const REPLICATION_LAG: u64 = 16;

/// How long ago a follower may last have matched the leader's whole log
/// and still count as heard: a live one does between batches of writes,
/// a stopped one never does again once the log moves on.
const CAUGHT_UP_WINDOW: Duration = Duration::from_secs(3);

/// Whether a follower that has matched `matched` of a leader whose log
/// ends at `last` is within [`REPLICATION_LAG`] of it.
fn within_lag(matched: Option<u64>, last: Option<u64>) -> bool {
    match (matched, last) {
        (Some(matched), Some(last)) => matched.saturating_add(REPLICATION_LAG) >= last,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The ordinal of an audio replica's node id; a witness sorts last.
fn ordinal_of(id: NodeId) -> u16 {
    match membership::slot_of(id) {
        membership::Slot::Audio(ordinal) => ordinal,
        membership::Slot::Witness => u16::MAX,
    }
}

/// Whether replica `me` hears replica `id` of its set. With contact
/// running: contact holds a majority and has had a report from `id`
/// within its window. Without (a set without an engine), what raft
/// shows: a follower hears the leader it follows, for as long as raft
/// names one (it drops the name when the election timeout passes without
/// a heartbeat); the leader hears a follower whose matched log is within
/// [`REPLICATION_LAG`] entries of its own and that has matched all of it
/// within [`CAUGHT_UP_WINDOW`] (`caught_up` holds when it last did).
fn hears(
    id: NodeId,
    me: NodeId,
    contact: Option<&samplez_replica::contact::Metrics>,
    metrics: &openraft::RaftMetrics<NodeId, openraft::BasicNode>,
    caught_up: &mut BTreeMap<NodeId, Instant>,
    now: Instant,
) -> bool {
    if let Some(contact) = contact {
        return contact.in_contact
            && (contact.peers.iter()).any(|p| p.node == id && p.reports > 0 && !p.silent);
    }
    if metrics.current_leader == Some(me) {
        let matched = (metrics.replication.as_ref())
            .and_then(|r| r.get(&id).copied().flatten())
            .map(|log| log.index);
        if matched.is_some() && matched == metrics.last_log_index {
            caught_up.insert(id, now);
        }
        return within_lag(matched, metrics.last_log_index)
            && (caught_up.get(&id)).is_some_and(|&t| now.duration_since(t) < CAUGHT_UP_WINDOW);
    }
    metrics.current_leader == Some(id)
}

impl App {
    pub fn new(
        replica: Arc<Replica<Mixer>>,
        name: String,
        set: Option<&Set>,
        auth: AuthMode,
        metrics: &Metrics,
    ) -> Arc<App> {
        let published = watch::Sender::new(replica.revision());
        Arc::new(App {
            commands: Commands::new(replica.clone(), COMMANDS_IN_FLIGHT),
            replica,
            members: set.map_or_else(Vec::new, |set| member_names(set, &name)),
            name,
            hub: Hub::new(CLIENT_DELTAS, metrics),
            published,
            selections: Mutex::default(),
            sessions: Mutex::default(),
            unnamed: if auth == AuthMode::Token {
                "token"
            } else {
                "anonymous"
            },
            requests: Mutex::default(),
            counters: Mutex::default(),
            engine: Mutex::default(),
            caught_up: Mutex::default(),
        })
    }

    pub fn epoch(&self) -> u64 {
        self.replica.epoch()
    }

    /// [`with_state`] on this app's replica.
    pub fn with_state<T>(&self, f: impl FnOnce(&StateMachine<Mixer>) -> T) -> T {
        with_state(&self.replica, f)
    }

    /// The replica's own view of itself; takes samplez's contact lock.
    fn info(&self) -> ReplicaInfo {
        outside_state();
        self.replica.replica()
    }

    /// The audio replica's contact metrics; takes samplez's contact lock.
    fn contact_metrics(&self) -> Option<samplez_replica::contact::Metrics> {
        outside_state();
        self.replica.contact_metrics()
    }

    /// Model `ReplicaRef`: the replica's role in the contract is leader or
    /// follower; openraft's other states are not leading.
    pub fn replica_ref(&self) -> Value {
        let role = if self.info().role == "leader" {
            "leader"
        } else {
            "follower"
        };
        json!({ "id": self.name, "role": role })
    }

    /// Model `ReplicaStatus` of every replica of the set as this one sees
    /// it, by ordinal: itself as [`ReplicatedState::replica`] reports it,
    /// and each other voter that serves clients (not a witness) by what
    /// this replica hears of it ([`hears`]). The leader is the one
    /// raft follows. Another replica can commit, as far as this one can
    /// tell, while it is heard and a majority of the voters is: this
    /// replica counting itself when it is ready. Every entry, this
    /// replica's own included, is commitable by that one rule.
    pub fn replicas(&self) -> Value {
        let info = self.info();
        let mut own = self.replica_ref();
        own["healthy"] = json!(info.ready);
        own["commitable"] = json!(info.commitable);
        if self.members.is_empty() {
            return json!([own]);
        }
        let metrics = self.replica.raft().metrics().borrow().clone();
        let contact = self.contact_metrics();
        let me = self.replica.id();
        let voters: Vec<NodeId> = metrics.membership_config.membership().voter_ids().collect();
        let now = Instant::now();
        let mut caught_up = self.caught_up.lock().expect("caught_up lock poisoned");
        let up: BTreeMap<NodeId, bool> = (voters.iter())
            .map(|&id| {
                let up = if id == me {
                    info.ready
                } else {
                    hears(id, me, contact.as_ref(), &metrics, &mut caught_up, now)
                };
                (id, up)
            })
            .collect();
        let quorum = 2 * up.values().filter(|&&up| up).count() > voters.len();
        let mut listed: Vec<(u16, Value)> = Vec::new();
        for &id in &voters {
            let membership::Slot::Audio(ordinal) = membership::slot_of(id) else {
                continue;
            };
            let healthy = up[&id];
            let status = if id == me {
                own["commitable"] = json!(healthy && quorum);
                own.clone()
            } else {
                let Some(name) = self.members.get(usize::from(ordinal)) else {
                    continue;
                };
                let role = if metrics.current_leader == Some(id) {
                    "leader"
                } else {
                    "follower"
                };
                json!({ "id": name, "role": role, "healthy": healthy, "commitable": healthy && quorum })
            };
            listed.push((ordinal, status));
        }
        // Not yet a voter itself, as while it joins: listed all the same.
        if !voters.contains(&me) {
            listed.push((ordinal_of(me), own));
        }
        listed.sort_by_key(|(ordinal, _)| *ordinal);
        Value::Array(listed.into_iter().map(|(_, s)| s).collect())
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
            Resolution::Applied | Resolution::Unchanged | Resolution::Duplicate => {
                self.with_state(|sm| sm.product.answer(session, &ack.command).cloned())
            }
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

    /// The `counters` message of the engine's counters last read; none
    /// before the first read or without an engine.
    pub fn counters_message(&self) -> Option<String> {
        let values = self
            .counters
            .lock()
            .expect("counters lock poisoned")
            .clone()?;
        let m = json!({ "type": "counters", "replica": self.replica_ref(), "values": values });
        Some(m.to_string())
    }

    /// The model `Status` of `GET /status`: this replica as the replicated
    /// state sees it, its audio replica's state in contact, and the
    /// engine's inputs and outputs as last read.
    pub fn status(&self) -> Value {
        use samplez_replica::state::ReplicaState as S;
        let info = self.info();
        let contact = self.contact_metrics();
        let in_contact = contact.as_ref().map(|m| m.in_contact);
        // None for a replica without an engine, and until the first step.
        let state = (contact.and_then(|m| m.replica?.state)).map(|s| match s {
            S::Starting => "starting",
            S::Committing => "committing",
            S::Withdrawn => "withdrawn",
            S::Diverged => "diverged",
            S::VersionMismatch => "versionMismatch",
            S::Resync => "resync",
        });
        json!({
            "replica": {
                "id": self.name,
                "node": self.replica.id().to_string(),
                "role": info.role,
                "leader": info.leader,
                "inSync": info.in_sync,
                "ready": info.ready,
                "commitable": info.commitable,
                "inContact": in_contact,
                "state": state,
            },
            "engine": self.engine.lock().expect("engine lock poisoned").clone(),
        })
    }

    /// Reads the engine's counters every [`COUNTERS_PERIOD`] and sends
    /// them to every connection, standby ones included, so a promoted
    /// standby has them at once; a connection not sent the last yet gets
    /// only the newest. Never returns.
    pub async fn count(&self, audio: &Audio) {
        let mut ticks = tokio::time::interval(COUNTERS_PERIOD);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let (counters, engine) = audio.readings();
            *self.counters.lock().expect("counters lock poisoned") = Some(counters);
            *self.engine.lock().expect("engine lock poisoned") = Some(engine);
            if let Some(m) = self.counters_message() {
                self.hub.publish(Frame::Counters(m.into()));
            }
        }
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
                                let body = self
                                    .deliver_requests(&d.text)
                                    .unwrap_or_else(|| d.text.to_string());
                                let text = self.delta_message(d.revision, &body);
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
            self.with_state(|sm| {
                let shape = sm.product.shape();
                shape.is_channel(e) || shape.is_output_bus(e)
            })
        });
        if !known {
            return;
        }
        let leased = self.with_state(|sm| sm.leases.contains_key(&session));
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
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        let now = SystemTime::now();
        sessions
            .entry(session.clone())
            .or_insert(Present {
                open: 0,
                record: None,
                since: now,
                active: now,
            })
            .open += 1;
        Connected { app: self, session }
    }

    /// Counts a connection of `session` closed, and drops the session's
    /// selection and presence with its last one.
    fn disconnected(&self, session: &SessionKey) {
        let gone = {
            let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
            let present = sessions.get_mut(session).expect("counted when opened");
            present.open -= 1;
            if present.open == 0 {
                sessions.remove(session)
            } else {
                None
            }
        };
        if let Some(gone) = gone {
            self.select(session.clone(), Value::Null, None);
            if gone.record.is_some() {
                self.publish_presence();
            }
        }
    }

    /// Records what the hello of a connection of `session` said: its
    /// owner record `owner` and `identity`. The session is listed in
    /// `presence` from its first hello on.
    pub fn greeted(&self, session: &SessionKey, owner: &Value, identity: &Identity) {
        let record = json!({
            "handle": owner["handle"],
            "kind": owner["kind"],
            "user": identity.name.as_deref().unwrap_or(self.unnamed),
            "role": identity.role,
        });
        let changed = {
            let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
            let Some(present) = sessions.get_mut(session) else {
                return;
            };
            present.record.as_ref() != Some(&record) && {
                present.record = Some(record);
                true
            }
        };
        if changed {
            self.publish_presence();
        }
    }

    /// Records activity of `session`: a command, a selection or a control
    /// request. `presence` carries it to the second, so it goes out again
    /// at most once a second per session.
    pub fn active(&self, session: &SessionKey) {
        let changed = {
            let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
            let Some(present) = sessions.get_mut(session) else {
                return;
            };
            let now = SystemTime::now();
            let second = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            let changed = second(now) != second(present.active);
            present.active = now;
            changed && present.record.is_some()
        };
        if changed {
            self.publish_presence();
        }
    }

    /// The `presence` message: the sessions connected to this replica that
    /// have sent a hello, in the order they connected.
    pub fn presence_message(&self) -> String {
        let mut list: Vec<(SystemTime, Value)> = self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .values()
            .filter_map(|p| {
                let mut record = p.record.clone()?;
                record["since"] = json!(rfc3339(p.since));
                record["active"] = json!(rfc3339(p.active));
                Some((p.since, record))
            })
            .collect();
        list.sort_by_key(|(since, _)| *since);
        let sessions: Vec<Value> = list.into_iter().map(|(_, r)| r).collect();
        json!({ "type": "presence", "replica": self.replica_ref(), "sessions": sessions })
            .to_string()
    }

    fn publish_presence(&self) {
        self.hub
            .publish(Frame::Status(PRESENCE, self.presence_message().into()));
    }

    /// Drops the selections of sessions whose lease has ended. A session
    /// gets its first lease at samplez's next renewal of the live sessions,
    /// about a second after it connected, so one that has not held a lease
    /// yet keeps its selection for one lease from when it made it.
    fn lapse_selections(&self) {
        let leases = self.with_state(|sm| sm.leases.clone());
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

    /// Orders `from`'s request for `path` into the replicated log, where
    /// every replica applies it after the delta that made the holder the
    /// owner, so the request never reaches a client ahead of the
    /// ownership it is about, and the replica the holder's primary
    /// connection is on hands it to it ([`App::publish`]), whichever
    /// replica the asker is on. Nothing when nobody else owns it, or
    /// when `from` asked for it within [`REQUEST_INTERVAL`].
    pub async fn request(&self, from: Owner, asker: Value, path: String) {
        let key = SessionKey::from(&from);
        let held = self.with_state(|sm| sm.product.owner_of(&path));
        if held.is_none_or(|holder| holder == key) {
            return;
        }
        {
            let mut requests = self.requests.lock().expect("requests lock poisoned");
            let now = Instant::now();
            requests.retain(|_, at| now.duration_since(*at) < REQUEST_INTERVAL);
            if requests.insert((key, path.clone()), now).is_some() {
                return;
            }
        }
        let Ok(id) = uuid() else { return };
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        let command = Command::ControlRequest {
            id: id.clone(),
            path,
            from: asker,
            at,
        };
        if let Begin::New(pending) = self.commands.begin(&from, &id) {
            let body = serde_json::to_vec(&command).expect("a command serializes");
            let _ = pending.submit(body.into()).await;
        }
    }

    /// Hands the control requests a delta carries to the primary
    /// connections of their holders on this replica, and returns the
    /// delta the clients see, without them.
    fn deliver_requests(&self, text: &str) -> Option<String> {
        if !text.contains(r#""requests""#) {
            return None;
        }
        let mut delta: Value = serde_json::from_str(text).ok()?;
        let requests = delta.as_object_mut()?.remove("requests")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        for r in requests.as_array().into_iter().flatten() {
            // One a lagging replica applies late is not handed on: the
            // holder is asked for a control it may have let go long since.
            let asked = r["at"].as_u64().unwrap_or(0);
            if now.saturating_sub(asked) > u64::try_from(REQUEST_LATE.as_millis()).unwrap_or(0) {
                continue;
            }
            let (Some(principal), Some(session)) =
                (r["to"]["principal"].as_str(), r["to"]["session"].as_str())
            else {
                continue;
            };
            let message = json!({
                "type": "controlRequest",
                "replica": self.replica_ref(),
                "path": r["path"],
                "from": r["from"],
            })
            .to_string();
            let to = To::Primary { principal, session };
            self.hub.send(to, Frame::Text(message.into()));
        }
        Some(delta.to_string())
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
    rfc3339(SystemTime::now())
}

/// `at` as RFC 3339 UTC with milliseconds.
pub fn rfc3339(at: SystemTime) -> String {
    let now = at.duration_since(UNIX_EPOCH).unwrap_or_default();
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

    /// A follower that trails the leader's log by a batch is still
    /// heard; one that has fallen further behind, or has matched nothing,
    /// is not. An exact match would list it down on every write burst.
    #[test]
    fn a_follower_within_the_lag_window_is_heard() {
        assert!(within_lag(Some(100), Some(100)));
        assert!(within_lag(Some(100 - REPLICATION_LAG), Some(100)));
        assert!(!within_lag(Some(100 - REPLICATION_LAG - 1), Some(100)));
        assert!(!within_lag(None, Some(100)));
        assert!(within_lag(Some(0), None));
    }

    /// A follower hears the leader raft names, and nothing once raft has
    /// dropped the name (the leader went silent).
    #[test]
    fn the_leader_is_heard_while_raft_names_it() {
        let mut m = openraft::RaftMetrics::new_initial(1);
        let mut seen = BTreeMap::new();
        let now = Instant::now();
        m.current_leader = Some(2);
        assert!(hears(2, 1, None, &m, &mut seen, now));
        assert!(!hears(3, 1, None, &m, &mut seen, now));
        m.current_leader = None;
        assert!(!hears(2, 1, None, &m, &mut seen, now));
    }

    /// The leader hears a follower that trails by a batch while it keeps
    /// catching up, and stops hearing one that has not matched the whole
    /// log for the window, though it still trails by little.
    #[test]
    fn a_follower_that_stops_catching_up_is_not_heard() {
        let mut m = openraft::RaftMetrics::new_initial(1);
        let mut seen = BTreeMap::new();
        let at = |index| openraft::LogId::new(openraft::CommittedLeaderId::new(1, 1), index);
        let t0 = Instant::now();
        m.current_leader = Some(1);
        m.last_log_index = Some(100);
        m.replication = Some(BTreeMap::from([(2, Some(at(100))), (3, Some(at(10)))]));
        assert!(hears(2, 1, None, &m, &mut seen, t0));
        assert!(!hears(3, 1, None, &m, &mut seen, t0), "far behind");
        m.last_log_index = Some(105);
        m.replication = Some(BTreeMap::from([(2, Some(at(100)))]));
        assert!(hears(
            2,
            1,
            None,
            &m,
            &mut seen,
            t0 + Duration::from_secs(1)
        ));
        assert!(!hears(2, 1, None, &m, &mut seen, t0 + CAUGHT_UP_WINDOW));
    }
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
        let mixer = Mixer::factory(shape, Vec::new(), Vec::new(), Default::default());
        let replica = replica::start(&dir, mixer, Arc::new(replica::tai_samples))
            .await
            .unwrap();
        let app = App::new(
            Arc::new(replica),
            "r1".into(),
            None,
            AuthMode::None,
            &Metrics::default(),
        );
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
            while app.with_state(|sm| sm.leases.contains_key(session)) != leased {
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
        let renewed = app.with_state(|sm| sm.leases[&s]);
        app.replica
            .append(Body::LeaseExpiry {
                session: s.clone(),
                renewed,
                at_index: None,
            })
            .unwrap();
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
        let renewed = app.with_state(|sm| sm.leases[&s]);
        app.replica
            .append(Body::LeaseExpiry {
                session: s.clone(),
                renewed,
                at_index: None,
            })
            .unwrap();
        until_lease(&app, &s, false).await;
        app.lapse_selections();
        assert!(!selected(&app, &s), "lapsed with the lease");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A control request is ordered into the log after the touch that
    /// made its holder the owner, and handed to the holder's primary
    /// connection once the replica applied it, never ahead of the
    /// ownership it is about.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_control_request_reaches_the_holder_after_the_owning_delta() {
        let (app, dir) = app().await;
        let app = app.clone();
        let publishing = app.clone();
        tokio::spawn(async move { publishing.publish().await });
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
            while !app.info().commitable {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("commitable");
        let queue = app.hub.subscribe(&holder);
        queue.set_primary(true);
        let Begin::New(pending) = app.commands.begin(&holder, &id) else {
            panic!("a new command");
        };
        pending
            .submit(serde_json::to_vec(&touch).unwrap().into())
            .await
            .unwrap();
        app.request(owner("s-b"), json!({ "name": "b" }), path.into())
            .await;
        // The holder gets the touch's delta, then the request.
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let Popped::Frame(Frame::Text(text)) = queue.pop().await else {
                    continue;
                };
                if text.as_str().contains(r#""type":"controlRequest""#) {
                    return text;
                }
                assert!(
                    !text.as_str().contains(r#""requests""#),
                    "the request is not shown to the clients: {text}"
                );
            }
        })
        .await
        .expect("the request reaches its holder");
        let message: Value = serde_json::from_str(request.as_str()).unwrap();
        assert_eq!(message["path"], path);
        assert_eq!(message["from"]["name"], "b");
        // A request applied long after it was asked is not handed on, and
        // the clients' delta comes without it either way.
        let stale = json!({
            "requests": [{
                "path": path,
                "from": {},
                "to": { "principal": holder.principal, "session": holder.session },
                "at": 1,
            }],
        })
        .to_string();
        let shown = app
            .deliver_requests(&stale)
            .expect("the requests are taken out");
        assert!(!shown.contains("requests"), "{shown}");
        let drained = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                if let Popped::Frame(Frame::Text(text)) = queue.pop().await {
                    assert!(
                        !text.as_str().contains("controlRequest"),
                        "a request that came late is handed on: {text}"
                    );
                }
            }
        })
        .await;
        assert!(drained.is_err());
        // The holder asking itself, or asking again within the interval, orders nothing.
        let revision = app.replica.revision();
        app.request(holder.clone(), json!({}), path.into()).await;
        app.request(owner("s-b"), json!({}), path.into()).await;
        assert_eq!(app.replica.revision(), revision);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// samplez takes its contact lock and then the log store's, so a call
    /// that takes the contact lock inside [`App::with_state`] deadlocks
    /// against it; debug builds catch one before it can.
    #[cfg(debug_assertions)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[should_panic(expected = "the contact lock under the store lock")]
    async fn a_contact_call_under_the_store_lock_is_caught() {
        let (app, _dir) = app().await;
        app.with_state(|_| app.replica_ref());
    }

    /// `GET /state` reads the replica's ref beside the state: it takes
    /// it before the store lock, as every route does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_state_route_takes_no_contact_lock_under_the_store_lock() {
        let (app, _dir) = app().await;
        let response = tower::ServiceExt::oneshot(
            crate::api::router(app.clone()),
            axum::http::Request::get("/state")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    /// A session is listed once from its first hello, whatever number of
    /// connections it holds, with the display name of its identity only,
    /// and leaves with its last connection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn presence_lists_a_session_once_by_its_display_name() {
        let (app, dir) = app().await;
        let o = owner("s-a");
        let s = SessionKey::from(&o);
        let listed = |app: &App| -> Vec<Value> {
            let m: Value = serde_json::from_str(&app.presence_message()).unwrap();
            check(&contract().server_message, &m).unwrap();
            m["sessions"].as_array().unwrap().clone()
        };
        let first = app.connected(s.clone());
        let second = app.connected(s.clone());
        assert!(listed(&app).is_empty(), "nothing before a hello");
        let alice = Identity {
            user: Some("alice@example.org".into()),
            name: Some("alice".into()),
            role: Role::Viewer,
        };
        let record = crate::ws::owner_record(&o, "ui");
        app.greeted(&s, &record, &alice);
        app.greeted(&s, &record, &alice);
        let sessions = listed(&app);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        assert_eq!(sessions[0]["user"], "alice");
        assert_eq!(sessions[0]["role"], "viewer");
        assert_eq!(sessions[0]["handle"], record["handle"]);
        assert!(!sessions[0].to_string().contains("example.org"));
        drop(first);
        assert_eq!(listed(&app).len(), 1, "kept by its other connection");
        drop(second);
        assert!(listed(&app).is_empty(), "gone with its last connection");
        let anonymous = Identity {
            user: None,
            name: None,
            role: Role::Operator,
        };
        let _again = app.connected(s.clone());
        app.greeted(&s, &record, &anonymous);
        assert_eq!(listed(&app)[0]["user"], "anonymous");
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
