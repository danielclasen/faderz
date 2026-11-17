//! The faderz WebSocket protocol (api/ws/*.schema.json) as a state machine
//! without I/O: one primary connection and, with more than one endpoint, a
//! standby on another replica; heartbeats; failover by promote or by
//! reconnecting; commands with ids and seqs, sent again after a 503 or a
//! failover and dropped 3 s after they were issued; continuous writes
//! coalesced to 50 per second. Deadlines follow the measured round trip
//! as the web client's sync engine sizes them
//! (web/src/lib/sync/engine.ts).
//!
//! The driver feeds it connection events and the time in milliseconds,
//! runs [`Engine::tick`] at [`Engine::next_deadline`], and carries out the
//! [`Action`]s it returns.

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde_json::{Value, json};

use crate::mirror::Mirror;

pub const HEARTBEAT_MS: u64 = 250;
pub const DEAD_MS: u64 = 750;
pub const CONNECT_TIMEOUT_MS: u64 = 2000;
pub const FIRST_CONNECT_TIMEOUT_MS: u64 = 10_000;
pub const CONNECT_ROUND_TRIPS: f64 = 4.0;
pub const CONNECT_TIMEOUT_CAP_MS: u64 = 30_000;
pub const PRIMARY_RETRY_MS: [u64; 3] = [0, 250, 500];
pub const OFFLINE_RETRY_MS: u64 = 2000;
pub const RETRY_CAP_MS: u64 = 15_000;
pub const STANDBY_TRIES: u32 = 4;
pub const STANDBY_RETRY_MS: u64 = 100;
pub const STANDBY_BACKOFF_MS: u64 = 2000;
pub const SYNC_TIMEOUT_MS: u64 = 3000;
pub const READ_INDEX_HOLD_MS: u64 = 2000;
pub const SYNC_TIMEOUT_CAP_MS: u64 = 30_000;
pub const SLACK_CAP_MS: u64 = 5000;
pub const NOT_APPLIED_RETRY_MS: u64 = 250;
/// A command is sent no more than this long after it was issued.
pub const COMMAND_EXPIRY_MS: u64 = 3000;
pub const MAX_COMMANDS_IN_FLIGHT: usize = 64;
pub const OUTBOX_CAPACITY: usize = 1024;
/// Continuous controls are written at most 50 times per second.
pub const WRITE_INTERVAL_MS: u64 = 20;
/// A console's registration goes out at most 10 times per second.
pub const CONSOLE_INTERVAL_MS: u64 = 100;
/// A replica that cannot commit is left only when that holds this long.
pub const COMMIT_SETTLE_MS: u64 = HEARTBEAT_MS;

pub type LinkId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Primary,
    Standby,
}

/// What the driver does for the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Opens a WebSocket for `link` to endpoint `endpoint`, with the
    /// session as `?session=`.
    Connect {
        link: LinkId,
        endpoint: usize,
    },
    Send {
        link: LinkId,
        text: String,
    },
    Close {
        link: LinkId,
    },
}

/// What the surface hears from the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The primary answered with state: on a new connection, after a
    /// promote or a resync. Ownership is to be put right
    /// ([`Engine::reconcile`]).
    Synced,
    /// A command on `path` was refused with 409: `owner` holds it.
    Refused { path: String, owner: Value },
    /// Another session asks for a control this one owns.
    Requested { path: String, from: Value },
    /// A refusal or failure that concerns no control of a strip.
    Error(String),
}

/// Round-trip time estimate, as TCP keeps it (RFC 6298).
#[derive(Debug, Default, Clone)]
pub struct Rtt {
    srtt: Option<f64>,
    rttvar: f64,
}

impl Rtt {
    pub fn sample(&mut self, ms: f64) {
        if !ms.is_finite() || !(0.0..=60_000.0).contains(&ms) {
            return;
        }
        match self.srtt {
            None => {
                self.srtt = Some(ms);
                self.rttvar = ms / 2.0;
            }
            Some(srtt) => {
                self.rttvar = 0.75 * self.rttvar + 0.25 * (srtt - ms).abs();
                self.srtt = Some(0.875 * srtt + 0.125 * ms);
            }
        }
    }

    /// srtt + 4 rttvar, without TCP's 1 s floor.
    pub fn rto(&self) -> Option<f64> {
        self.srtt.map(|s| s + 4.0 * self.rttvar)
    }
}

/// `base` doubled per failure, at most `cap`.
pub fn backoff(base: u64, failures: u32, cap: u64) -> u64 {
    base.saturating_mul(1u64 << failures.min(32)).min(cap)
}

/// Whether writes to `path` are continuous (model `ContinuousPath`).
pub fn continuous(path: &str) -> bool {
    let t: Vec<&str> = path.split('/').skip(1).collect();
    match t.as_slice() {
        ["channels" | "buses" | "auxes" | "mixMinus", _, "fader"] => true,
        ["channels", _, "trim" | "delay"] => true,
        ["channels", _, "legTrim", _] => true,
        ["channels", _, "hpf", "freq"] => true,
        ["channels", _, "pan", "lr" | "fr" | "divergence" | "lfe"] => true,
        ["channels", _, "sends", _, "level" | "pan"] => true,
        [
            "channels" | "buses" | "auxes",
            _,
            "eq",
            "bands",
            _,
            "freq" | "gain" | "q",
        ] => true,
        ["channels" | "buses" | "auxes", _, "gate", p] => {
            matches!(*p, "threshold" | "range" | "attack" | "release")
        }
        ["channels" | "buses" | "auxes", _, "comp", p] => {
            matches!(*p, "threshold" | "ratio" | "attack" | "release" | "makeup")
        }
        ["buses" | "auxes", _, "delay"] => true,
        ["buses" | "auxes", _, "limiter", "ceiling"] => true,
        ["vcas", _, "offset"] => true,
        ["monitor", "level"] => true,
        _ => false,
    }
}

#[derive(Debug)]
struct Link {
    id: LinkId,
    role: Role,
    endpoint: usize,
    opened: bool,
    created_at: u64,
    last_rx: u64,
    /// Until it opens: when it is abandoned.
    connect_by: u64,
    /// t of pings sent and not answered, oldest first.
    pings: VecDeque<u64>,
    replica: Option<String>,
    replicas: Option<Value>,
    /// Standby only: a snapshot or resume of this history arrived.
    synced: bool,
    /// Opened to hand the session over to a new credential.
    handover: bool,
}

impl Link {
    fn commitable(&self) -> Option<bool> {
        let id = self.replica.as_deref()?;
        self.replicas
            .as_ref()?
            .as_array()?
            .iter()
            .find(|r| r["id"] == id)?["commitable"]
            .as_bool()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sync {
    /// No primary connection open.
    Down,
    /// A hello, resync or promote is unanswered.
    Syncing,
    /// The primary answered with state; commands go out.
    Live,
    /// The cluster holds less than this client: taking its state.
    Adopting,
}

#[derive(Debug, Clone)]
struct Entry {
    msg: Value,
    id: String,
    path: String,
    kind: &'static str,
    continuous: bool,
    issued_at: u64,
    sent: bool,
    hold_until: u64,
    /// The count of hello and resync requests when it was sent.
    sent_in: u64,
}

/// A console registration and when it last went out.
#[derive(Debug, Clone)]
struct Console {
    msg: Value,
    sent: Option<Value>,
    last: Option<u64>,
}

pub struct Engine {
    endpoints: usize,
    session: String,
    handle: String,
    links: Vec<Link>,
    next_link: LinkId,
    sync: Sync,
    mirror: Mirror,
    rtt: Rtt,
    opening: Rtt,
    connect_timeouts: [u32; 2],
    sync_timeouts: u32,
    sync_by: Option<u64>,
    sync_requests: u64,
    primary_index: usize,
    primary_attempts: usize,
    offline_rounds: u32,
    retry_at: Option<u64>,
    standby_attempts: u32,
    standby_at: Option<u64>,
    commit_check_at: Option<u64>,
    next_beat: u64,
    /// Handing over to a new credential: the standby opened for it.
    handover: bool,
    seq_base: u64,
    seqs: BTreeMap<String, u64>,
    outbox: Vec<Entry>,
    /// Continuous writes held back by the rate limit: path to value and
    /// when it goes out.
    trailing: BTreeMap<String, (Value, u64)>,
    last_write: BTreeMap<String, u64>,
    /// Values written and not yet answered.
    local: BTreeMap<String, Value>,
    selection: Option<String>,
    consoles: BTreeMap<String, Console>,
    actions: Vec<Action>,
    events: Vec<Event>,
    ids: Box<dyn FnMut() -> String + Send>,
}

impl Engine {
    /// An engine for `endpoints` WebSocket endpoints (one per replica) and
    /// the session `session`. `seq_base` is the Unix time in milliseconds
    /// at start, so seqs stay above those of an earlier run of the same
    /// session; `ids` draws command ids.
    pub fn new(
        endpoints: usize,
        session: String,
        seq_base: u64,
        ids: Box<dyn FnMut() -> String + Send>,
    ) -> Engine {
        assert!(endpoints > 0, "no endpoints");
        Engine {
            endpoints,
            handle: crate::session::handle(&session),
            session,
            links: Vec::new(),
            next_link: 1,
            sync: Sync::Down,
            mirror: Mirror::default(),
            rtt: Rtt::default(),
            opening: Rtt::default(),
            connect_timeouts: [0, 0],
            sync_timeouts: 0,
            sync_by: None,
            sync_requests: 0,
            primary_index: 0,
            primary_attempts: 0,
            offline_rounds: 0,
            retry_at: None,
            standby_attempts: 0,
            standby_at: None,
            commit_check_at: None,
            next_beat: 0,
            handover: false,
            seq_base,
            seqs: BTreeMap::new(),
            outbox: Vec::new(),
            trailing: BTreeMap::new(),
            last_write: BTreeMap::new(),
            local: BTreeMap::new(),
            selection: None,
            consoles: BTreeMap::new(),
            actions: Vec::new(),
            events: Vec::new(),
            ids,
        }
    }

    pub fn start(&mut self, now: u64) {
        self.next_beat = now + HEARTBEAT_MS;
        self.connect(Role::Primary, self.primary_index, now);
    }

    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Whether commands go out.
    pub fn live(&self) -> bool {
        self.sync == Sync::Live
    }

    /// The round-trip estimate, when there is one.
    pub fn rto(&self) -> Option<f64> {
        self.rtt.rto()
    }

    /// The value of a control as this client shows it: the last value it
    /// wrote while that is unanswered, else the applied state's.
    pub fn display(&self, path: &str) -> Option<&Value> {
        self.local.get(path).or_else(|| self.mirror.get(path))
    }

    /// The owner record of `path`, when someone owns it.
    pub fn owner(&self, path: &str) -> Option<&Value> {
        self.mirror.owners.get(path)
    }

    /// Whether `owner` is this session.
    pub fn mine(&self, owner: &Value) -> bool {
        owner["handle"] == self.handle.as_str()
    }

    /// Whether a touch of `path` is waiting for its answer.
    pub fn touch_pending(&self, path: &str) -> bool {
        self.outbox
            .iter()
            .any(|e| e.kind == "touch" && e.path == path)
    }

    // ---- timing ------------------------------------------------------------

    fn round_trip(&self) -> Option<f64> {
        self.rtt
            .rto()
            .or_else(|| self.opening.rto().map(|o| o / 3.0))
    }

    /// How long a new socket may take to open.
    pub fn connect_timeout(&self, role: Role) -> u64 {
        let (rt, open) = (self.rtt.rto(), self.opening.rto());
        let base = if rt.is_none() && open.is_none() {
            FIRST_CONNECT_TIMEOUT_MS
        } else {
            let by_rtt = CONNECT_ROUND_TRIPS * rt.unwrap_or(0.0);
            (CONNECT_TIMEOUT_MS as f64)
                .max(by_rtt)
                .max(open.unwrap_or(0.0)) as u64
        };
        backoff(
            base,
            self.connect_timeouts[role as usize],
            CONNECT_TIMEOUT_CAP_MS,
        )
    }

    /// How long a primary has to answer with state.
    pub fn sync_timeout(&self) -> u64 {
        let by_rtt = READ_INDEX_HOLD_MS as f64 + 2.0 * self.round_trip().unwrap_or(0.0);
        let base = (SYNC_TIMEOUT_MS as f64).max(by_rtt) as u64;
        backoff(base, self.sync_timeouts, SYNC_TIMEOUT_CAP_MS)
    }

    /// What a slow link adds to the heartbeat watch: one round-trip timeout.
    fn slack(&self) -> u64 {
        self.rtt
            .rto()
            .map_or(0, |r| r.ceil() as u64)
            .min(SLACK_CAP_MS)
    }

    pub fn dead_ms(&self) -> u64 {
        DEAD_MS + self.slack()
    }

    /// The earliest time [`Engine::tick`] has something to do.
    pub fn next_deadline(&self) -> u64 {
        let mut at = self.next_beat;
        let mut min = |t: Option<u64>| {
            if let Some(t) = t {
                at = at.min(t);
            }
        };
        min(self.retry_at);
        min(self.standby_at);
        min(self.sync_by);
        min(self.commit_check_at);
        for l in &self.links {
            min(Some(if l.opened {
                l.last_rx + self.dead_ms()
            } else {
                l.connect_by
            }));
        }
        for (_, at) in self.trailing.values() {
            min(Some(*at));
        }
        for e in &self.outbox {
            min(Some(e.issued_at + COMMAND_EXPIRY_MS));
            if !e.sent && e.hold_until > 0 {
                min(Some(e.hold_until));
            }
        }
        for c in self.consoles.values() {
            if c.sent.as_ref() != Some(&c.msg) {
                min(c.last.map(|l| l + CONSOLE_INTERVAL_MS));
            }
        }
        at
    }

    // ---- links -------------------------------------------------------------

    fn link(&self, id: LinkId) -> Option<&Link> {
        self.links.iter().find(|l| l.id == id)
    }

    fn link_mut(&mut self, id: LinkId) -> Option<&mut Link> {
        self.links.iter_mut().find(|l| l.id == id)
    }

    fn primary(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.role == Role::Primary)
    }

    fn standby(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.role == Role::Standby)
    }

    fn connect(&mut self, role: Role, endpoint: usize, now: u64) -> LinkId {
        let id = self.next_link;
        self.next_link += 1;
        let connect_by = now + self.connect_timeout(role);
        self.links.push(Link {
            id,
            role,
            endpoint,
            opened: false,
            created_at: now,
            last_rx: now,
            connect_by,
            pings: VecDeque::new(),
            replica: None,
            replicas: None,
            synced: false,
            handover: false,
        });
        self.actions.push(Action::Connect { link: id, endpoint });
        id
    }

    fn send(&mut self, link: LinkId, msg: &Value) {
        let Some(l) = self.link(link) else { return };
        if !l.opened {
            return;
        }
        if l.role == Role::Primary && matches!(msg["type"].as_str(), Some("hello" | "resync")) {
            self.sync_requests += 1;
        }
        self.actions.push(Action::Send {
            link,
            text: msg.to_string(),
        });
    }

    fn detach(&mut self, link: LinkId) {
        if let Some(i) = self.links.iter().position(|l| l.id == link) {
            self.links.remove(i);
            self.actions.push(Action::Close { link });
        }
    }

    fn position(&self) -> Value {
        match &self.mirror.epoch {
            Some(epoch) => json!({ "lastRevision": self.mirror.revision, "epoch": epoch }),
            None => json!({}),
        }
    }

    fn hello(&self, mode: &str) -> Value {
        let mut hello = json!({
            "type": "hello",
            "session": self.session,
            "kind": "console",
            "mode": mode,
        });
        if let (Value::Object(h), Value::Object(p)) = (&mut hello, self.position()) {
            h.extend(p);
        }
        hello
    }

    /// A socket opened.
    pub fn opened(&mut self, link: LinkId, now: u64) {
        let Some(l) = self.link_mut(link) else { return };
        l.opened = true;
        l.last_rx = now;
        let (role, created_at) = (l.role, l.created_at);
        self.opening.sample((now - created_at) as f64);
        self.connect_timeouts[role as usize] = 0;
        match role {
            Role::Primary => {
                self.sync = Sync::Syncing;
                self.sync_by = Some(now + self.sync_timeout());
                let hello = self.hello("primary");
                self.send(link, &hello);
            }
            Role::Standby => {
                let hello = self.hello("standby");
                self.send(link, &hello);
            }
        }
    }

    /// A socket closed, failed or could not be opened.
    pub fn closed(&mut self, link: LinkId, now: u64) {
        let Some(l) = self.link(link) else { return };
        let role = l.role;
        self.detach(link);
        match role {
            Role::Standby => {
                self.handover = false;
                self.schedule_standby(now);
            }
            Role::Primary if self.mirror.ready() => self.failover(now),
            Role::Primary => {
                self.sync = Sync::Down;
                self.sync_by = None;
                self.retry_primary(now);
            }
        }
    }

    fn retry_primary(&mut self, now: u64) {
        self.primary_attempts += 1;
        self.primary_index = (self.primary_index + 1) % self.endpoints;
        let delay = if self.primary_attempts >= PRIMARY_RETRY_MS.len() {
            self.primary_attempts = 0;
            let d = backoff(OFFLINE_RETRY_MS, self.offline_rounds, RETRY_CAP_MS);
            self.offline_rounds += 1;
            d
        } else {
            PRIMARY_RETRY_MS[self.primary_attempts]
        };
        self.retry_at = Some(now + delay);
    }

    /// The primary is gone: promotes a synced standby, else reconnects.
    fn failover(&mut self, now: u64) {
        if let Some(p) = self.primary().map(|p| p.id) {
            self.detach(p);
        }
        self.sync = Sync::Down;
        self.sync_by = None;
        self.commit_check_at = None;
        for e in &mut self.outbox {
            e.sent = false;
        }
        let standby = self
            .standby()
            .filter(|s| s.opened && s.synced)
            .map(|s| (s.id, s.endpoint));
        match standby {
            Some((id, endpoint)) => self.promote(id, endpoint, now),
            None => {
                if let Some(s) = self.standby().map(|s| s.id) {
                    self.detach(s);
                }
                self.handover = false;
                self.retry_primary(now);
            }
        }
    }

    fn promote(&mut self, link: LinkId, endpoint: usize, now: u64) {
        if let Some(l) = self.link_mut(link) {
            l.role = Role::Primary;
            l.handover = false;
        }
        self.handover = false;
        self.primary_index = endpoint;
        self.primary_attempts = 0;
        let promote = json!({
            "type": "promote",
            "lastRevision": self.mirror.revision,
            "epoch": self.mirror.epoch,
        });
        self.send(link, &promote);
        self.resync(false, now);
    }

    fn resync(&mut self, full: bool, now: u64) {
        let Some(p) = self.primary().map(|p| p.id) else {
            return;
        };
        let mut msg = json!({ "type": "resync" });
        if let (Value::Object(m), Value::Object(pos)) = (&mut msg, self.position()) {
            m.extend(pos);
        }
        if full && self.mirror.ready() {
            msg["full"] = json!(true);
        }
        self.send(p, &msg);
        self.sync = Sync::Syncing;
        self.sync_by = Some(now + self.sync_timeout());
    }

    fn schedule_standby(&mut self, now: u64) {
        if self.standby().is_some() || self.standby_at.is_some() {
            return;
        }
        let delay = if self.standby_attempts < STANDBY_TRIES {
            STANDBY_RETRY_MS
        } else {
            backoff(
                STANDBY_BACKOFF_MS,
                self.standby_attempts - STANDBY_TRIES,
                RETRY_CAP_MS,
            )
        };
        self.standby_at = Some(now + delay);
    }

    /// Opens a standby on another endpoint than the primary's; with one
    /// endpoint only for a handover.
    fn start_standby(&mut self, now: u64) {
        if self.standby().is_some() || self.sync != Sync::Live || !self.mirror.ready() {
            return;
        }
        let others: Vec<usize> = (0..self.endpoints)
            .filter(|&i| i != self.primary_index)
            .collect();
        let index = if self.handover {
            self.primary_index
        } else if others.is_empty() {
            return;
        } else {
            others[self.standby_attempts as usize % others.len()]
        };
        self.standby_attempts += 1;
        let id = self.connect(Role::Standby, index, now);
        let handover = self.handover;
        if let Some(l) = self.link_mut(id) {
            l.handover = handover;
        }
    }

    fn standby_suitable(&self, link: &Link) -> bool {
        if link.handover {
            return true;
        }
        let primary = self.primary().and_then(|p| p.replica.as_deref());
        link.replica.is_some() && link.replica.as_deref() != primary
    }

    fn drop_standby(&mut self, link: LinkId, now: u64) {
        self.detach(link);
        self.handover = false;
        self.schedule_standby(now);
    }

    /// Moves the session to a new connection, opened with the credential
    /// the driver hands out from now on, and closes the old one once the
    /// new one is primary. Ownership and the lease stay with the session.
    pub fn handover(&mut self, now: u64) {
        if !self.mirror.ready() || self.sync != Sync::Live {
            return;
        }
        if let Some(s) = self.standby().map(|s| s.id) {
            self.detach(s);
        }
        self.standby_at = None;
        self.handover = true;
        self.start_standby(now);
    }

    // ---- server messages ---------------------------------------------------

    /// A binary message (a meter frame): traffic only.
    pub fn binary(&mut self, link: LinkId, now: u64) {
        if let Some(l) = self.link_mut(link) {
            l.last_rx = now;
        }
    }

    pub fn received(&mut self, link: LinkId, text: &str, now: u64) {
        let Some(l) = self.link_mut(link) else { return };
        l.last_rx = now;
        let Ok(msg) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if let Some(id) = msg["replica"]["id"].as_str() {
            l.replica = Some(id.to_owned());
        }
        let role = l.role;
        match msg["type"].as_str().unwrap_or_default() {
            "ping" => self.send(link, &json!({ "type": "pong", "t": msg["t"] })),
            "pong" => {
                let Some(t) = msg["t"].as_u64() else { return };
                let l = self.link_mut(link).expect("present");
                // A sample only for a ping sent on this link, whose t is
                // this clock's time it went out.
                let Some(i) = l.pings.iter().position(|&p| p == t) else {
                    return;
                };
                l.pings.drain(..=i);
                self.rtt.sample(now.saturating_sub(t) as f64);
            }
            "replicas" => {
                self.link_mut(link).expect("present").replicas = Some(msg["replicas"].clone());
                let l = self.link(link).expect("present");
                if role == Role::Standby && l.synced && !self.standby_suitable(l) {
                    return self.drop_standby(link, now);
                }
                self.check_commitable(now);
            }
            "snapshot" if role == Role::Primary => self.primary_snapshot(&msg, now),
            "snapshot" => {
                if msg["epoch"].as_str() == self.mirror.epoch.as_deref() {
                    self.standby_synced(link, now);
                } else {
                    self.drop_standby(link, now);
                }
            }
            "resume" if role == Role::Primary => self.primary_resume(&msg, now),
            "resume" => {
                if msg["epoch"].as_str() != self.mirror.epoch.as_deref() {
                    return self.drop_standby(link, now);
                }
                for d in msg["deltas"].as_array().into_iter().flatten() {
                    self.delta(d, role, now);
                }
                self.standby_synced(link, now);
            }
            "delta" => self.delta(&msg, role, now),
            "ack" => {
                if let Some(id) = msg["commandId"].as_str() {
                    self.answer(id, msg["result"] == "stale", now);
                }
            }
            "error" => self.error(&msg, now),
            "ahead" if role == Role::Primary => {
                // The cluster lost history this client applied; a console
                // holds no state of its own, so it takes the cluster's.
                self.sync = Sync::Adopting;
                self.send(link, &json!({ "type": "resync" }));
                self.sync_by = Some(now + self.sync_timeout());
            }
            "controlRequest" if role == Role::Primary => {
                if let Some(path) = msg["path"].as_str() {
                    self.events.push(Event::Requested {
                        path: path.to_owned(),
                        from: msg["from"].clone(),
                    });
                }
            }
            _ => {}
        }
    }

    fn primary_snapshot(&mut self, msg: &Value, now: u64) {
        let other_history =
            self.mirror.ready() && msg["epoch"].as_str() != self.mirror.epoch.as_deref();
        let older = self.mirror.ready()
            && !other_history
            && msg["revision"].as_u64().unwrap_or(0) < self.mirror.revision;
        if older && self.sync != Sync::Adopting {
            // Replicas answer only once they applied the revision sent, so
            // this is a guard: ask again.
            return self.resync(false, now);
        }
        if other_history {
            tracing::warn!("the cluster started a new history; taking its state");
        }
        let answering = self.sync == Sync::Syncing;
        self.mirror.snapshot(msg);
        // A snapshot can stand in for the deltas that answered commands
        // sent before the request it answers: those go out again.
        let before = if answering {
            self.sync_requests
        } else {
            u64::MAX
        };
        for e in &mut self.outbox {
            if e.sent && e.sent_in < before {
                e.sent = false;
            }
        }
        self.primary_synced(now);
    }

    fn primary_resume(&mut self, msg: &Value, now: u64) {
        if !self.mirror.ready() || msg["epoch"].as_str() != self.mirror.epoch.as_deref() {
            return self.resync(true, now);
        }
        for d in msg["deltas"].as_array().into_iter().flatten() {
            let revision = d["revision"].as_u64().unwrap_or(0);
            if revision <= self.mirror.revision {
                continue;
            }
            if revision != self.mirror.revision + 1 {
                return self.resync(true, now);
            }
            self.apply(d, now);
        }
        if msg["revision"].as_u64() != Some(self.mirror.revision) {
            return self.resync(true, now);
        }
        self.primary_synced(now);
    }

    fn standby_synced(&mut self, link: LinkId, now: u64) {
        let Some(l) = self.link(link) else { return };
        if !self.standby_suitable(l) {
            return self.drop_standby(link, now);
        }
        let handover = l.handover;
        self.link_mut(link).expect("present").synced = true;
        self.standby_attempts = 0;
        if handover {
            // The new credential's connection takes over at once.
            return self.failover(now);
        }
        self.check_commitable(now);
    }

    fn primary_synced(&mut self, now: u64) {
        self.sync = Sync::Live;
        self.sync_by = None;
        self.primary_attempts = 0;
        self.offline_rounds = 0;
        self.sync_timeouts = 0;
        self.events.push(Event::Synced);
        if let Some(p) = self.primary().map(|p| p.id)
            && let Some(element) = self.selection.clone()
        {
            self.send(p, &json!({ "type": "select", "element": element }));
        }
        for c in self.consoles.values_mut() {
            c.sent = None;
            c.last = None;
        }
        self.flush_consoles(now);
        self.transmit(now);
        if let Some(s) = self.standby() {
            let id = s.id;
            if s.synced && !self.standby_suitable(s) {
                self.drop_standby(id, now);
            }
        }
        self.start_standby(now);
        self.check_commitable(now);
    }

    fn delta(&mut self, d: &Value, role: Role, now: u64) {
        if !self.mirror.ready() || self.sync == Sync::Adopting {
            return;
        }
        let revision = d["revision"].as_u64().unwrap_or(0);
        if revision <= self.mirror.revision {
            return;
        }
        if revision != self.mirror.revision + 1 {
            if role == Role::Primary && self.sync == Sync::Live {
                self.resync(false, now);
            }
            return;
        }
        self.apply(d, now);
    }

    fn apply(&mut self, d: &Value, now: u64) {
        self.mirror.apply(d);
        let mine: Vec<String> = d["origins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["owner"]["handle"] == self.handle.as_str())
            .filter_map(|o| o["commandId"].as_str().map(str::to_owned))
            .collect();
        for id in mine {
            self.answer(&id, false, now);
        }
    }

    fn error(&mut self, msg: &Value, now: u64) {
        let id = msg["commandId"].as_str();
        let code = msg["code"].as_u64().unwrap_or(0);
        if let (503, Some(id)) = (code, id) {
            if let Some(e) = self.outbox.iter_mut().find(|e| e.id == id && e.sent) {
                e.sent = false;
                e.hold_until = now + NOT_APPLIED_RETRY_MS;
            }
            return;
        }
        if let Some(id) = id {
            self.answer(id, false, now);
        }
        // Console registrations carry no id: one dropped (429) or not
        // applied (503) goes out again at the next flush.
        if id.is_none() && matches!(code, 429 | 503) {
            for c in self.consoles.values_mut() {
                c.sent = None;
            }
        }
        match (code, msg["path"].as_str()) {
            (409, Some(path)) if id.is_some() => self.events.push(Event::Refused {
                path: path.to_owned(),
                owner: msg["owner"].clone(),
            }),
            _ => {
                let text = msg["message"].as_str().unwrap_or_default();
                self.events
                    .push(Event::Error(format!("{code} {text}").trim().to_owned()));
            }
        }
    }

    fn check_commitable(&mut self, now: u64) {
        let p = self.primary().and_then(Link::commitable);
        let s = self
            .standby()
            .filter(|s| s.synced)
            .and_then(Link::commitable);
        if self.sync == Sync::Live && p == Some(false) && s == Some(true) {
            self.commit_check_at
                .get_or_insert(now + COMMIT_SETTLE_MS + self.slack());
        } else {
            self.commit_check_at = None;
        }
    }

    // ---- commands ------------------------------------------------------------

    fn next_seq(&mut self, path: &str) -> u64 {
        let seq = self.seqs.entry(path.to_owned()).or_insert(self.seq_base);
        *seq += 1;
        *seq
    }

    fn issue(&mut self, kind: &'static str, path: &str, value: Option<Value>, now: u64) {
        let id = (self.ids)();
        let seq = self.next_seq(path);
        let mut msg = json!({ "type": kind, "id": id, "seq": seq, "path": path });
        if let Some(v) = value {
            msg["value"] = v;
        }
        let continuous = kind == "set" && continuous(path);
        if continuous
            && let Some(i) = self
                .outbox
                .iter()
                .rposition(|e| e.path == path && e.continuous && !e.sent && e.kind == "set")
        {
            self.outbox.remove(i);
        }
        if self.outbox.len() >= OUTBOX_CAPACITY {
            self.events.push(Event::Error(format!(
                "too many unanswered commands; {kind} of {path} was not sent"
            )));
            self.settled(kind, path);
            return;
        }
        self.outbox.push(Entry {
            msg,
            id,
            path: path.to_owned(),
            kind,
            continuous,
            issued_at: now,
            sent: false,
            hold_until: 0,
            sent_in: 0,
        });
        self.transmit(now);
    }

    /// Writes `value` to `path`; continuous controls at most 50 times per
    /// second, the latest value winning.
    pub fn set(&mut self, path: &str, value: Value, now: u64) {
        self.local.insert(path.to_owned(), value.clone());
        if !continuous(path) {
            return self.issue("set", path, Some(value), now);
        }
        if let Some(t) = self.trailing.get_mut(path) {
            t.0 = value;
            return;
        }
        match self.last_write.get(path) {
            Some(&last) if now < last + WRITE_INTERVAL_MS => {
                self.trailing
                    .insert(path.to_owned(), (value, last + WRITE_INTERVAL_MS));
            }
            _ => {
                self.last_write.insert(path.to_owned(), now);
                self.issue("set", path, Some(value), now);
            }
        }
    }

    pub fn touch(&mut self, path: &str, now: u64) {
        self.issue("touch", path, None, now);
    }

    pub fn release(&mut self, path: &str, now: u64) {
        self.flush_trailing(path, now);
        self.issue("release", path, None, now);
    }

    /// Sends a held-back write of `path` now.
    fn flush_trailing(&mut self, path: &str, now: u64) {
        if let Some((value, _)) = self.trailing.remove(path) {
            self.last_write.insert(path.to_owned(), now);
            self.issue("set", path, Some(value), now);
        }
    }

    /// Releases every control this session owns that is not in
    /// `touching`, and touches again what is in it but no longer owned.
    pub fn reconcile(&mut self, touching: &HashSet<String>, now: u64) {
        let owned: Vec<String> = self
            .mirror
            .owners
            .iter()
            .filter(|(_, o)| self.mine(o))
            .map(|(p, _)| p.clone())
            .collect();
        let has = |e: &Engine, kind: &str, path: &str| {
            e.outbox.iter().any(|x| x.kind == kind && x.path == path)
        };
        for path in &owned {
            if !touching.contains(path) && !has(self, "release", path) {
                self.issue("release", path, None, now);
            }
        }
        for path in touching {
            if !owned.contains(path) && !has(self, "touch", path) {
                self.issue("touch", path, None, now);
            }
        }
    }

    /// Publishes the element this session has selected.
    pub fn select(&mut self, element: Option<&str>) {
        self.selection = element.map(str::to_owned);
        if self.sync == Sync::Live
            && let Some(p) = self.primary().map(|p| p.id)
        {
            self.send(p, &json!({ "type": "select", "element": element }));
        }
    }

    /// Asks the owner of `path` to give it up.
    pub fn request(&mut self, path: &str) {
        if self.sync == Sync::Live
            && let Some(p) = self.primary().map(|p| p.id)
        {
            self.send(p, &json!({ "type": "request", "path": path }));
        }
    }

    /// Registers a console (a `console` message): sent after every sync
    /// and whenever it changes, at most 10 times per second.
    pub fn console(&mut self, msg: Value, now: u64) {
        let id = msg["id"].as_str().unwrap_or_default().to_owned();
        let c = self.consoles.entry(id).or_insert(Console {
            msg: Value::Null,
            sent: None,
            last: None,
        });
        c.msg = msg;
        self.flush_consoles(now);
    }

    fn flush_consoles(&mut self, now: u64) {
        let Some(p) = self
            .primary()
            .filter(|_| self.sync == Sync::Live)
            .map(|p| p.id)
        else {
            return;
        };
        let due: Vec<String> = self
            .consoles
            .iter()
            .filter(|(_, c)| c.sent.as_ref() != Some(&c.msg))
            .filter(|(_, c)| c.last.is_none_or(|l| now >= l + CONSOLE_INTERVAL_MS))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let msg = self.consoles[&id].msg.clone();
            self.send(p, &msg);
            let c = self.consoles.get_mut(&id).expect("present");
            c.sent = Some(msg);
            c.last = Some(now);
        }
    }

    fn transmit(&mut self, now: u64) {
        if self.sync != Sync::Live {
            return;
        }
        let Some(p) = self.primary().filter(|p| p.opened).map(|p| p.id) else {
            return;
        };
        let mut in_flight = self.outbox.iter().filter(|e| e.sent).count();
        let mut out = Vec::new();
        for e in &mut self.outbox {
            if in_flight >= MAX_COMMANDS_IN_FLIGHT {
                break;
            }
            if e.sent || e.hold_until > now || e.issued_at + COMMAND_EXPIRY_MS <= now {
                continue;
            }
            e.sent = true;
            e.sent_in = self.sync_requests;
            in_flight += 1;
            out.push(e.msg.clone());
        }
        for m in out {
            self.send(p, &m);
        }
    }

    fn answer(&mut self, id: &str, stale: bool, now: u64) {
        let Some(i) = self.outbox.iter().position(|e| e.id == id) else {
            return;
        };
        let e = self.outbox.remove(i);
        self.settled(e.kind, &e.path);
        if stale && e.kind != "set" {
            tracing::debug!(path = e.path, kind = e.kind, "stale ownership command");
        }
        self.transmit(now);
    }

    /// A set left the outbox: with nothing pending for its control, the
    /// applied value is shown again.
    fn settled(&mut self, kind: &str, path: &str) {
        if kind != "set" || self.trailing.contains_key(path) {
            return;
        }
        if !self
            .outbox
            .iter()
            .any(|e| e.kind == "set" && e.path == path)
        {
            self.local.remove(path);
        }
    }

    // ---- time --------------------------------------------------------------

    /// Runs whatever is due at `now`.
    pub fn tick(&mut self, now: u64) {
        if now >= self.next_beat {
            self.next_beat = now + HEARTBEAT_MS;
            let opened: Vec<LinkId> = self
                .links
                .iter()
                .filter(|l| l.opened)
                .map(|l| l.id)
                .collect();
            for id in opened {
                self.send(id, &json!({ "type": "ping", "t": now }));
                let l = self.link_mut(id).expect("present");
                l.pings.push_back(now);
                if l.pings.len() > 64 {
                    l.pings.pop_front();
                }
            }
        }
        let dead = self.dead_ms();
        let gone: Vec<(LinkId, Role)> = self
            .links
            .iter()
            .filter(|l| {
                if l.opened {
                    now >= l.last_rx + dead
                } else {
                    now >= l.connect_by
                }
            })
            .map(|l| (l.id, l.role))
            .collect();
        for (id, role) in gone {
            if !self.link(id).is_some_and(|l| l.opened) {
                self.connect_timeouts[role as usize] += 1;
            }
            self.closed(id, now);
        }
        if self.sync_by.is_some_and(|t| now >= t) {
            self.sync_by = None;
            self.sync_timeouts += 1;
            if let Some(p) = self.primary().map(|p| p.id) {
                self.closed(p, now);
            }
        }
        if self.retry_at.is_some_and(|t| now >= t) {
            self.retry_at = None;
            if self.primary().is_none() {
                self.connect(Role::Primary, self.primary_index, now);
            }
        }
        if self.standby_at.is_some_and(|t| now >= t) {
            self.standby_at = None;
            self.start_standby(now);
        }
        if self.commit_check_at.is_some_and(|t| now >= t) {
            self.commit_check_at = None;
            let p = self.primary().and_then(Link::commitable);
            let s = self
                .standby()
                .filter(|s| s.synced)
                .and_then(Link::commitable);
            if self.sync == Sync::Live && p == Some(false) && s == Some(true) {
                self.failover(now);
            }
        }
        let due: Vec<String> = self
            .trailing
            .iter()
            .filter(|(_, (_, at))| now >= *at)
            .map(|(p, _)| p.clone())
            .collect();
        for path in due {
            self.flush_trailing(&path, now);
        }
        let expired: Vec<Entry> = {
            let (gone, kept) = std::mem::take(&mut self.outbox)
                .into_iter()
                .partition(|e| e.issued_at + COMMAND_EXPIRY_MS <= now);
            self.outbox = kept;
            gone
        };
        for e in &expired {
            tracing::debug!(
                path = e.path,
                kind = e.kind,
                "command unanswered after 3 s, dropped"
            );
            self.settled(e.kind, &e.path);
        }
        self.flush_consoles(now);
        self.transmit(now);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SESSION: &str = "c2Vzc2lvbi1vZi10aGUtYnJpZGdlLXRlc3Qtc3VpdGU";

    pub(crate) fn engine(endpoints: usize) -> Engine {
        let mut n = 0;
        Engine::new(
            endpoints,
            SESSION.into(),
            1_000,
            Box::new(move || {
                n += 1;
                format!("00000000-0000-4000-8000-{n:012}")
            }),
        )
    }

    pub(crate) fn sent(actions: &[Action], link: LinkId) -> Vec<Value> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send { link: l, text } if *l == link => serde_json::from_str(text).ok(),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn snapshot(replica: &str, revision: u64) -> String {
        json!({
            "type": "snapshot", "replica": { "id": replica, "role": "leader" },
            "you": {}, "epoch": "1", "revision": revision,
            "shape": { "channels": [{ "id": "ch1", "name": "Mic 1" }], "buses": [] },
            "state": { "channels": { "ch1": { "fader": -90, "mute": false, "solo": "off" } }, "consoles": [] },
            "owners": {},
        })
        .to_string()
    }

    fn replicas(replica: &str, commitable: bool) -> String {
        json!({ "type": "replicas", "replica": { "id": replica, "role": "leader" },
            "replicas": [{ "id": replica, "role": "leader", "healthy": true, "commitable": commitable }] })
        .to_string()
    }

    /// An engine with a live primary on link 1 at t = 0.
    pub(crate) fn live(endpoints: usize) -> Engine {
        let mut e = engine(endpoints);
        e.start(0);
        e.opened(1, 0);
        e.received(1, &snapshot("r1", 5), 0);
        e.take_actions();
        e.take_events();
        e
    }

    #[test]
    fn the_hello_names_a_console_and_its_position_once_it_has_state() {
        let mut e = engine(1);
        e.start(0);
        assert_eq!(
            e.take_actions(),
            vec![Action::Connect {
                link: 1,
                endpoint: 0
            }]
        );
        e.opened(1, 10);
        let hello = &sent(&e.take_actions(), 1)[0];
        assert_eq!(hello["kind"], "console");
        assert_eq!(hello["mode"], "primary");
        assert_eq!(hello["session"], SESSION);
        assert!(hello.get("lastRevision").is_none());
        e.received(1, &snapshot("r1", 5), 20);
        assert_eq!(e.take_events(), vec![Event::Synced]);
        // Lost and reconnected: the hello carries where it was.
        e.closed(1, 30);
        e.tick(30 + PRIMARY_RETRY_MS[1]);
        let Some(Action::Connect { link, .. }) = e
            .take_actions()
            .into_iter()
            .find(|a| matches!(a, Action::Connect { .. }))
        else {
            panic!("reconnects");
        };
        e.opened(link, 400);
        let hello = &sent(&e.take_actions(), link)[0];
        assert_eq!(hello["lastRevision"], 5);
        assert_eq!(hello["epoch"], "1");
    }

    #[test]
    fn continuous_writes_are_coalesced_to_fifty_per_second_latest_wins() {
        let mut e = live(1);
        let path = "/channels/ch1/fader";
        e.set(path, json!(-10), 100);
        e.set(path, json!(-9), 105);
        e.set(path, json!(-8), 110);
        let first = sent(&e.take_actions(), 1);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["value"], -10);
        assert_eq!(e.display(path), Some(&json!(-8)));
        assert_eq!(e.next_deadline(), 120);
        e.tick(120);
        let second = sent(&e.take_actions(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0]["value"], -8);
        assert!(second[0]["seq"].as_u64() > first[0]["seq"].as_u64());
        // Discrete controls go out one by one.
        e.set("/channels/ch1/mute", json!(true), 121);
        e.set("/channels/ch1/mute", json!(false), 122);
        assert_eq!(sent(&e.take_actions(), 1).len(), 2);
    }

    #[test]
    fn seqs_start_above_the_start_time_and_grow_per_control() {
        let mut e = live(1);
        e.touch("/channels/ch1/fader", 1);
        e.release("/channels/ch1/fader", 2);
        e.touch("/channels/ch1/mute", 3);
        let s: Vec<u64> = sent(&e.take_actions(), 1)
            .iter()
            .map(|m| m["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(s, vec![1001, 1002, 1001]);
    }

    #[test]
    fn a_503_sends_the_command_again_with_its_id_and_a_409_is_refused() {
        let mut e = live(1);
        e.touch("/channels/ch1/fader", 0);
        let touch = sent(&e.take_actions(), 1).remove(0);
        let error = json!({ "type": "error", "replica": { "id": "r1", "role": "leader" }, "code": 503,
            "message": "not applied", "commandId": touch["id"], "path": touch["path"] });
        e.received(1, &error.to_string(), 10);
        e.tick(10 + NOT_APPLIED_RETRY_MS - 1);
        assert!(
            sent(&e.take_actions(), 1)
                .iter()
                .all(|m| m["type"] == "ping")
        );
        e.tick(10 + NOT_APPLIED_RETRY_MS);
        let again: Vec<Value> = sent(&e.take_actions(), 1)
            .into_iter()
            .filter(|m| m["type"] == "touch")
            .collect();
        assert_eq!(again, vec![touch.clone()]);
        let owner = json!({ "principal": "ana", "handle": "aaaaaaaaaaaaaaaaaaaaaa", "kind": "ui", "connection": "c" });
        let conflict = json!({ "type": "error", "replica": { "id": "r1", "role": "leader" }, "code": 409,
            "message": "owned by another session", "commandId": touch["id"], "path": touch["path"],
            "current": -90, "owner": owner });
        e.received(1, &conflict.to_string(), 300);
        assert_eq!(
            e.take_events(),
            vec![Event::Refused {
                path: "/channels/ch1/fader".into(),
                owner
            }]
        );
        assert!(!e.touch_pending("/channels/ch1/fader"));
    }

    #[test]
    fn a_delta_listing_a_command_answers_it_and_the_applied_value_shows() {
        let mut e = live(1);
        let path = "/channels/ch1/fader";
        e.set(path, json!(-5), 0);
        let set = sent(&e.take_actions(), 1).remove(0);
        assert_eq!(e.display(path), Some(&json!(-5)));
        let delta = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" }, "revision": 6,
            "atIndex": 1, "changes": [{ "path": path, "value": -5 }],
            "origins": [{ "owner": { "handle": e.handle() }, "commandId": set["id"], "seq": set["seq"] }] });
        e.received(1, &delta.to_string(), 10);
        assert_eq!(e.mirror().revision, 6);
        assert!(e.local.is_empty());
        assert!(e.outbox.is_empty());
    }

    #[test]
    fn a_command_unanswered_for_3_s_is_dropped_and_its_value_reverts() {
        let mut e = live(1);
        e.set("/channels/ch1/mute", json!(true), 0);
        assert_eq!(e.display("/channels/ch1/mute"), Some(&json!(true)));
        e.tick(COMMAND_EXPIRY_MS);
        assert!(e.outbox.is_empty());
        assert_eq!(e.display("/channels/ch1/mute"), Some(&json!(false)));
    }

    #[test]
    fn a_revision_gap_asks_for_a_resync() {
        let mut e = live(1);
        let delta = json!({ "type": "delta", "replica": { "id": "r1", "role": "leader" }, "revision": 7,
            "owners": [{ "path": "/channels/ch1/fader", "owner": null }] });
        e.received(1, &delta.to_string(), 0);
        let resync = sent(&e.take_actions(), 1).remove(0);
        assert_eq!(
            resync,
            json!({ "type": "resync", "lastRevision": 5, "epoch": "1" })
        );
    }

    #[test]
    fn a_silent_primary_is_dead_after_750_ms_and_the_synced_standby_is_promoted() {
        let mut e = live(2);
        // The standby opens on the other endpoint once the primary is live.
        assert_eq!(e.standby().map(|s| s.endpoint), Some(1));
        let sb = e.standby().unwrap().id;
        e.take_actions();
        e.opened(sb, 5);
        let hello = sent(&e.take_actions(), sb).remove(0);
        assert_eq!(hello["mode"], "standby");
        assert_eq!(hello["lastRevision"], 5);
        e.received(
            sb,
            &json!({ "type": "resume", "replica": { "id": "r2", "role": "follower" },
            "you": {}, "epoch": "1", "revision": 5, "deltas": [] })
            .to_string(),
            6,
        );
        e.touch("/channels/ch1/fader", 7);
        let touch = sent(&e.take_actions(), 1).remove(0);
        // Both links keep beating; only the primary goes silent.
        let mut t = 0;
        while t < DEAD_MS {
            t += 50;
            e.received(
                sb,
                &json!({ "type": "ping", "replica": { "id": "r2", "role": "follower" }, "t": t })
                    .to_string(),
                t,
            );
            e.tick(t);
        }
        let actions = e.take_actions();
        assert!(actions.contains(&Action::Close { link: 1 }));
        let on_standby = sent(&actions, sb);
        let types: Vec<&str> = on_standby
            .iter()
            .filter_map(|m| m["type"].as_str())
            .filter(|t| *t != "pong" && *t != "ping")
            .collect();
        assert_eq!(types, vec!["promote", "resync"]);
        assert_eq!(e.primary().map(|p| p.id), Some(sb));
        // The touch goes out again with its id once the promoted standby answers.
        e.received(
            sb,
            &json!({ "type": "resume", "replica": { "id": "r2", "role": "leader" },
            "you": {}, "epoch": "1", "revision": 5, "deltas": [] })
            .to_string(),
            t + 1,
        );
        let resent: Vec<Value> = sent(&e.take_actions(), sb)
            .into_iter()
            .filter(|m| m["type"] == "touch")
            .collect();
        assert_eq!(resent, vec![touch]);
        assert_eq!(e.take_events().last(), Some(&Event::Synced));
    }

    #[test]
    fn deadlines_grow_with_the_measured_round_trip() {
        let mut e = live(1);
        assert_eq!(e.dead_ms(), DEAD_MS);
        assert_eq!(e.sync_timeout(), SYNC_TIMEOUT_MS);
        e.tick(HEARTBEAT_MS);
        let ping = sent(&e.take_actions(), 1).remove(0);
        let t = ping["t"].as_u64().unwrap();
        // A 400 ms round trip: rto = 400 + 4 * 200.
        e.received(
            1,
            &json!({ "type": "pong", "replica": { "id": "r1", "role": "leader" }, "t": t })
                .to_string(),
            t + 400,
        );
        assert_eq!(e.rto(), Some(1200.0));
        assert_eq!(e.dead_ms(), DEAD_MS + 1200);
        assert_eq!(e.sync_timeout(), READ_INDEX_HOLD_MS + 2400);
        assert_eq!(e.connect_timeout(Role::Primary), 4800);
    }

    #[test]
    fn a_primary_that_cannot_commit_is_left_for_a_standby_that_can() {
        let mut e = live(2);
        let sb = e.standby().unwrap().id;
        e.opened(sb, 1);
        e.received(sb, &snapshot("r2", 5), 2);
        e.received(sb, &replicas("r2", true), 3);
        e.received(1, &replicas("r1", false), 4);
        e.take_actions();
        e.tick(4 + COMMIT_SETTLE_MS);
        assert_eq!(e.primary().map(|p| p.id), Some(sb));
        assert!(e.take_actions().contains(&Action::Close { link: 1 }));
    }

    #[test]
    fn without_a_standby_the_primary_is_retried_on_the_next_endpoint_then_backs_off() {
        let mut e = engine(2);
        e.start(0);
        e.closed(1, 0);
        e.tick(PRIMARY_RETRY_MS[1]);
        assert!(e.take_actions().contains(&Action::Connect {
            link: 2,
            endpoint: 1
        }));
        e.closed(2, 300);
        e.tick(300 + PRIMARY_RETRY_MS[2]);
        assert!(e.take_actions().contains(&Action::Connect {
            link: 3,
            endpoint: 0
        }));
        e.closed(3, 900);
        e.tick(900 + OFFLINE_RETRY_MS - 1);
        assert!(
            !e.take_actions()
                .iter()
                .any(|a| matches!(a, Action::Connect { .. }))
        );
        e.tick(900 + OFFLINE_RETRY_MS);
        assert!(e.take_actions().contains(&Action::Connect {
            link: 4,
            endpoint: 1
        }));
    }

    #[test]
    fn a_handover_moves_the_session_to_a_new_connection_on_the_same_replica() {
        let mut e = live(1);
        e.handover(10);
        let Some(Action::Connect { link, endpoint: 0 }) = e.take_actions().pop() else {
            panic!("a new connection");
        };
        e.opened(link, 20);
        assert_eq!(sent(&e.take_actions(), link)[0]["mode"], "standby");
        e.received(
            link,
            &json!({ "type": "resume", "replica": { "id": "r1", "role": "leader" },
            "you": {}, "epoch": "1", "revision": 5, "deltas": [] })
            .to_string(),
            30,
        );
        let actions = e.take_actions();
        assert!(actions.contains(&Action::Close { link: 1 }));
        let types: Vec<Value> = sent(&actions, link)
            .into_iter()
            .map(|m| m["type"].clone())
            .collect();
        assert_eq!(types, vec![json!("promote"), json!("resync")]);
        assert_eq!(e.primary().map(|p| p.id), Some(link));
    }

    #[test]
    fn consoles_and_the_selection_go_out_after_every_sync_and_on_change() {
        let mut e = live(1);
        let console = |bank: u64| {
            json!({ "type": "console", "id": "xt", "name": "X-Touch",
            "protocol": "mcu", "transport": "bridge", "strips": 8, "bankOffset": bank })
        };
        e.console(console(0), 0);
        e.select(Some("ch1"));
        assert_eq!(sent(&e.take_actions(), 1).len(), 2);
        // Bank changes within 100 ms go out once, the latest.
        e.console(console(8), 10);
        e.console(console(16), 20);
        assert!(sent(&e.take_actions(), 1).is_empty());
        e.tick(100);
        let out: Vec<Value> = sent(&e.take_actions(), 1)
            .into_iter()
            .filter(|m| m["type"] == "console")
            .collect();
        assert_eq!(out, vec![console(16)]);
        // A new connection gets both again.
        e.closed(1, 200);
        e.tick(1000);
        let link = e.primary().unwrap().id;
        e.opened(link, 1000);
        e.received(link, &snapshot("r1", 9), 1001);
        let types: Vec<Value> = sent(&e.take_actions(), link)
            .into_iter()
            .map(|m| m["type"].clone())
            .collect();
        assert_eq!(
            types,
            vec![json!("hello"), json!("select"), json!("console")]
        );
    }

    #[test]
    fn a_console_registration_not_applied_goes_out_again() {
        let mut e = live(1);
        let console = json!({ "type": "console", "id": "xt", "name": "X-Touch",
            "protocol": "mcu", "transport": "bridge", "strips": 8, "bankOffset": 0 });
        e.console(console.clone(), 0);
        e.take_actions();
        let error = json!({ "type": "error", "replica": { "id": "r1", "role": "leader" }, "code": 503, "message": "not applied" });
        e.received(1, &error.to_string(), 10);
        assert_eq!(
            e.take_events(),
            vec![Event::Error("503 not applied".into())]
        );
        e.tick(CONSOLE_INTERVAL_MS);
        let out: Vec<Value> = sent(&e.take_actions(), 1)
            .into_iter()
            .filter(|m| m["type"] == "console")
            .collect();
        assert_eq!(out, vec![console]);
    }

    #[test]
    fn reconcile_releases_what_is_not_touched_and_touches_what_is() {
        let mut e = live(1);
        let me =
            json!({ "principal": "p", "handle": e.handle(), "kind": "console", "connection": "c" });
        e.mirror.owners.insert("/channels/ch1/mute".into(), me);
        let touching: HashSet<String> = ["/channels/ch1/fader".to_owned()].into();
        e.reconcile(&touching, 0);
        let out: Vec<(Value, Value)> = sent(&e.take_actions(), 1)
            .into_iter()
            .map(|m| (m["type"].clone(), m["path"].clone()))
            .collect();
        assert_eq!(
            out,
            vec![
                (json!("release"), json!("/channels/ch1/mute")),
                (json!("touch"), json!("/channels/ch1/fader")),
            ]
        );
    }

    #[test]
    fn continuous_paths_follow_the_contract() {
        for p in [
            "/channels/ch1/fader",
            "/buses/pgm/fader",
            "/monitor/level",
            "/vcas/v1/offset",
            "/channels/c/sends/a1/level",
            "/auxes/a/limiter/ceiling",
            "/channels/c/eq/bands/2/gain",
        ] {
            assert!(continuous(p), "{p}");
        }
        for p in [
            "/channels/ch1/mute",
            "/channels/ch1/solo",
            "/monitor/dim",
            "/channels/c/sends/a1/on",
        ] {
            assert!(!continuous(p), "{p}");
        }
    }
}
