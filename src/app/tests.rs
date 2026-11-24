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
    assert!(hears(2, 1, None, W, &m, &mut seen, now));
    assert!(!hears(3, 1, None, W, &m, &mut seen, now));
    m.current_leader = None;
    assert!(!hears(2, 1, None, W, &m, &mut seen, now));
}

/// A replica the contact layer holds silent for a sample stays up, so
/// a hiccup of a few milliseconds does not list it down; one unheard
/// for the whole window is down, and up again once heard.
#[test]
fn a_replica_is_down_only_once_unheard_for_the_window() {
    let mut heard = BTreeMap::new();
    let t0 = Instant::now();
    assert!(!up_within(2, false, &mut heard, t0), "never heard");
    assert!(up_within(2, true, &mut heard, t0));
    let late = t0 + Duration::from_millis(250);
    assert!(up_within(2, false, &mut heard, late), "one silent sample");
    assert!(
        !up_within(2, false, &mut heard, t0 + DOWN_AFTER),
        "unheard for the window"
    );
    assert!(up_within(2, true, &mut heard, t0 + 2 * DOWN_AFTER));
}

/// A replica that loses every other one stops counting a majority at
/// once, so its own commitable and a client's failover do not wait,
/// while the list still shows the others up within the grace.
#[test]
fn a_replica_alone_has_no_quorum_at_once_though_it_lists_the_others_up() {
    let mut heard = BTreeMap::new();
    let t0 = Instant::now();
    let voters = [1, 2, 3];
    let (up, quorum) = listing(&voters, 1, true, &mut heard, t0, |_| true);
    assert!(quorum && up.values().all(|&u| u), "{up:?}");
    let late = t0 + Duration::from_millis(250);
    let (up, quorum) = listing(&voters, 1, true, &mut heard, late, |_| false);
    assert!(!quorum, "a quorum of replicas heard a moment ago");
    assert!(up[&2] && up[&3], "listed down within the grace: {up:?}");
    let (up, quorum) = listing(&voters, 1, true, &mut heard, t0 + DOWN_AFTER, |_| false);
    assert!(!quorum && !up[&2] && !up[&3], "{up:?}");
}

/// The contact window the tests hear with: samplez's default
/// `contactWindowMs`.
const W: Duration = Duration::from_millis(8);

/// A peer as [`contact`] takes it: its node, its report count,
/// whether contact holds it silent, and the age of its last direct
/// report.
type Peer = (NodeId, u64, bool, Option<Duration>);

/// Contact metrics in or out of contact, with `peers`.
fn contact(in_contact: bool, peers: &[Peer]) -> samplez_replica::contact::Metrics {
    let peer =
        |&(node, reports, silent, last_report_age): &Peer| samplez_replica::contact::PeerMetrics {
            node,
            reports,
            delay_p9999: None,
            gap_max: Duration::ZERO,
            gaps_over_window: 0,
            silent_transitions: 0,
            silent_gap_max: Duration::ZERO,
            silent,
            last_report_age,
            offset_ns: None,
            clock_best: None,
        };
    samplez_replica::contact::Metrics {
        protocol_gate_open: true,
        protocol_blocker: None,
        config_hash: None,
        versions: None,
        build_lead: None,
        fence: None,
        in_contact,
        known_committed: 0,
        peers: peers.iter().map(peer).collect(),
        shared_gaps: 0,
        shared_gap_max: Duration::ZERO,
        stagger_max: Duration::ZERO,
        status_lag_p9999: None,
        status_changes_lost: 0,
        entries_refused: 0,
        entries_filtered: 0,
        unresolved: 0,
        unresolved_refused: 0,
        awaiting_overflow: 0,
        scheduled_dropped: 0,
        snapshot_installs: 0,
        purge_points: 0,
        contradicted: 0,
        late_rebuilds: 0,
        estimate: None,
        counted: Vec::new(),
        lead: 0,
        apply_lead_capped: false,
        decision_lead: 0,
        apply_lead_raised_by: None,
        e_spread_p9999: None,
        withholding: Default::default(),
        floor: None,
        hold_floor_unmet: false,
        replica: None,
        resync: Default::default(),
        leader: Default::default(),
        replica_disagreement: false,
        clock: Default::default(),
        clock_verdicts: 0,
        verified: true,
        tai_offset_s: None,
        clock_unverified: Vec::new(),
        replica_clock_unverified: false,
        replica_tai_offset_zero: Vec::new(),
        horizon_held: false,
        clock_holds: 0,
        start_hold: None,
        start_hold_warnings: 0,
    }
}

/// With contact, a peer is heard while its last direct report is
/// within the contact window, whether or not contact holds it silent
/// at that moment, and only while contact holds a majority. A peer
/// that never reported is not heard.
#[test]
fn a_peer_is_heard_while_its_last_report_is_within_the_window() {
    let m = openraft::RaftMetrics::new_initial(1);
    let mut seen = BTreeMap::new();
    let now = Instant::now();
    let ms = Duration::from_millis;
    let mut heard =
        |c: &samplez_replica::contact::Metrics| hears(2, 1, Some(c), W, &m, &mut seen, now);
    assert!(heard(&contact(true, &[(2, 9, false, Some(ms(1)))])));
    assert!(heard(&contact(true, &[(2, 9, false, Some(W))])));
    // Reported since the last tick, which has not cleared the silence.
    assert!(heard(&contact(true, &[(2, 9, true, Some(ms(1)))])));
    assert!(!heard(&contact(true, &[(2, 9, false, Some(W + ms(1)))])));
    assert!(!heard(&contact(true, &[(2, 0, false, None)])));
    assert!(!heard(&contact(true, &[(3, 9, false, Some(ms(1)))])));
    assert!(!heard(&contact(false, &[(2, 9, false, Some(ms(1)))])));
}

/// A peer that stopped stays down once listed down, though for one
/// contact window after this replica regains contact the contact
/// layer holds no peer silent: its last direct report is still as
/// old as it is. Listing it up there would show a stopped replica
/// healthy and commitable for [`DOWN_AFTER`] more.
#[test]
fn a_stopped_peer_stays_down_when_contact_is_regained() {
    let m = openraft::RaftMetrics::new_initial(1);
    let mut seen = BTreeMap::new();
    let mut heard = BTreeMap::new();
    let voters = [1, 2, 3];
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut list = |at: Instant, c: samplez_replica::contact::Metrics| {
        let (up, quorum) = listing(&voters, 1, true, &mut heard, at, |id| {
            hears(id, 1, Some(&c), W, &m, &mut seen, at)
        });
        (up[&2], quorum)
    };
    // 3 reports throughout; 2 stops after its report at t0.
    let live = (3, 1000, false, Some(ms(1)));
    assert_eq!(
        list(t0, contact(true, &[(2, 500, false, Some(ms(1))), live])),
        (true, true),
        "reported 1 ms ago"
    );
    assert_eq!(
        list(
            t0 + ms(500),
            contact(true, &[(2, 500, true, Some(ms(500))), live])
        ),
        (true, true),
        "listed up within the grace"
    );
    let down = t0 + ms(1100);
    assert_eq!(
        list(down, contact(true, &[(2, 500, true, Some(ms(1100))), live])),
        (false, true),
        "unheard for DOWN_AFTER"
    );
    // This replica lost and regained contact: no peer silent, 2's
    // report count unchanged and its last report 2 s old.
    let regained = t0 + ms(2000);
    assert_eq!(
        list(
            regained,
            contact(true, &[(2, 500, false, Some(ms(2000))), live])
        ),
        (false, true),
        "listed up after contact was regained"
    );
    let later = regained + ms(500);
    assert_eq!(
        list(
            later,
            contact(true, &[(2, 500, true, Some(ms(2500))), live])
        ),
        (false, true)
    );
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
    assert!(hears(2, 1, None, W, &m, &mut seen, t0));
    assert!(!hears(3, 1, None, W, &m, &mut seen, t0), "far behind");
    m.last_log_index = Some(105);
    m.replication = Some(BTreeMap::from([(2, Some(at(100)))]));
    assert!(hears(
        2,
        1,
        None,
        W,
        &m,
        &mut seen,
        t0 + Duration::from_secs(1)
    ));
    assert!(!hears(2, 1, None, W, &m, &mut seen, t0 + CAUGHT_UP_WINDOW));
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
/// A test replica's data directory, removed when dropped: on tmpfs it
/// would otherwise hold memory for good, a failed test's included.
pub(crate) struct TestDir(PathBuf);

impl std::ops::Deref for TestDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) async fn app() -> (Arc<App>, TestDir) {
    // On tmpfs: on the disk the builds write to, one fsync of the log
    // can outlast the 2 s a write waits to commit, and a test's write
    // is refused under load.
    let shm = std::path::Path::new("/dev/shm");
    let base = if shm.is_dir() {
        shm.to_path_buf()
    } else {
        let exe = std::env::current_exe().unwrap();
        exe.parent().unwrap().to_path_buf()
    };
    let dir = base.join(format!(
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
        Duration::from_millis(8),
        AuthMode::None,
        &Metrics::default(),
    );
    (app, TestDir(dir))
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
    app.selections
        .lock()
        .expect("selections lock poisoned")
        .contains_key(session)
}

/// A session that has not held a lease keeps its selection for one
/// lease from when it made it, and not longer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_without_a_lease_lapses_after_one_lease() {
    let (app, _dir) = app().await;
    let s = SessionKey::from(&owner("s-a"));
    app.select(s.clone(), json!({}), Some("ch1"));
    app.lapse_selections();
    assert!(selected(&app, &s), "kept within the lease");
    {
        let mut selections = app.selections.lock().expect("selections lock poisoned");
        let at = &mut selections.get_mut(&s).unwrap().at;
        *at = at.checked_sub(SELECTION_GRACE).unwrap();
    }
    app.lapse_selections();
    assert!(!selected(&app, &s), "lapsed after one lease");
}

/// A selection made before the session's first lease lapses as soon
/// as the lease that arrived after it ends, however recent it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_made_before_the_lease_lapses_when_it_ends() {
    let (app, _dir) = app().await;
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
}

/// A session's selection lapses as soon as a lease it held ends,
/// however recent the selection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_lapses_when_its_lease_ends() {
    let (app, _dir) = app().await;
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
}

/// A control request is ordered into the log after the touch that
/// made its holder the owner, and handed to the holder's primary
/// connection once the replica applied it, never ahead of the
/// ownership it is about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_control_request_reaches_the_holder_after_the_owning_delta() {
    let (app, _dir) = app().await;
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
        .expect("the touch commits");
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
}

/// A call into the replica inside [`App::with_state`] holds off
/// applying entries for as long as the replica's locks take; debug
/// builds catch one.
#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "holds off applying entries")]
async fn a_replica_call_inside_with_state_is_caught() {
    let (app, _dir) = app().await;
    app.with_state(|_| app.replica_ref());
}

/// `GET /state` reads the replica's ref beside the state: it takes
/// it before the store lock, as every route does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_state_route_calls_no_replica_inside_with_state() {
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
    let (app, _dir) = app().await;
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
