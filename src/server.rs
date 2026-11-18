//! A running faderz: the replica, the engine, the listeners and the tasks
//! beside them.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use openraft::{BasicNode, Membership};

use samplez::rt::ProcessStatus;
use samplez_replica::bootstrap::{Kube, Markers};
use samplez_replica::contact::SystemClocks;
use samplez_replica::entry::NodeId;
use samplez_replica::rollout::{self, Fixed};
use samplez_replica::state_machine::Product as _;
use samplez_replica::{Estimate, Replica};
use samplez_service::replica::ReadIndex;
use samplez_service::{Ops, PeerTls, ReplicatedState, Service, ServiceConfig};
use tokio::task::JoinHandle;

use crate::app::App;
use crate::audio::{self, Audio};
use crate::config::Config;
use crate::shape::Shape;
use crate::state::Mixer;
use crate::{api, replica, ui, ws};

/// How often a set's replica reads its StatefulSet's target revision.
const ROLLOUT_READ: Duration = Duration::from_secs(5);

/// What a test gives a set of more than one replica in place of what
/// [`Server::start`] takes from the pod and the configuration.
pub struct SetParts {
    /// In place of the ConfigMap `replica.marker`.
    pub markers: Arc<dyn Markers>,
    /// The peer listener, bound already.
    pub listener: std::net::TcpListener,
    /// How long a write may take to be ordered and applied.
    pub write_timeout: Duration,
}

pub struct Server {
    pub api: SocketAddr,
    pub ops: SocketAddr,
    pub nmos: SocketAddr,
    pub machine: Option<SocketAddr>,
    /// The peer listener of a set of more than one replica.
    pub peer: Option<SocketAddr>,
    pub app: Arc<App>,
    /// The engine, when the configuration has an `mxl` section.
    pub audio: Option<Arc<Audio>>,
    replica: Arc<Replica<Mixer>>,
    /// Awaits the serving task, which ends only on a failure or a panic,
    /// or stops it on `stop`.
    watching: JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    failure: Arc<Mutex<Option<String>>>,
}

impl Server {
    /// Opens the state, starts the engine with it, binds the listeners
    /// and serves them. `process` is the outcome of
    /// [`samplez::rt::setup_process`] run before any thread was started;
    /// without it, it is run here.
    pub async fn start(
        config: &Config,
        shape: Shape,
        name: String,
        process: Option<ProcessStatus>,
    ) -> Result<Server, String> {
        Server::start_with(config, shape, name, process, None).await
    }

    /// [`Server::start`], a set of more than one replica taking `parts`.
    pub async fn start_with(
        config: &Config,
        shape: Shape,
        name: String,
        process: Option<ProcessStatus>,
        parts: Option<SetParts>,
    ) -> Result<Server, String> {
        let estimate: Estimate = Arc::new(replica::tai_samples);
        let ops_state = Ops::default();
        let dir = &config.replica.data_dir;
        let mixer = Mixer::factory(
            shape.clone(),
            config.consoles.clone(),
            config.wiring.clone(),
        );
        let product = mixer().config_hash();
        let (state, peer, kube, write_timeout) = match &config.set {
            None => {
                let state = replica::start(dir, mixer, estimate).await?;
                (Arc::new(state), None, None, replica::WRITE_TIMEOUT)
            }
            Some(set) => {
                let tls = PeerTls::new(&set.peer.tls, ops_state.metrics())
                    .map_err(|e| format!("replica.peer.tls: {e}"))?;
                let tls = Arc::new(tls);
                let bind = |e: std::io::Error| format!("binding the peer listener: {e}");
                let (markers, tcp, write_timeout, kube) = match parts {
                    Some(p) => {
                        p.listener.set_nonblocking(true).map_err(bind)?;
                        let tcp = tokio::net::TcpListener::from_std(p.listener).map_err(bind)?;
                        (p.markers, tcp, p.write_timeout, None)
                    }
                    None => {
                        let mut kube = Kube::in_cluster(set.marker.clone())
                            .map_err(|e| format!("replica.marker: {e}"))?;
                        // The marker goes with the StatefulSet.
                        if let Some(statefulset) = &set.statefulset {
                            kube = kube.owned_by(statefulset.clone());
                        }
                        let kube = Arc::new(kube);
                        let tcp = tokio::net::TcpListener::bind((set.peer.bind, set.peer.port))
                            .await
                            .map_err(bind)?;
                        (
                            kube.clone() as Arc<dyn Markers>,
                            tcp,
                            replica::WRITE_TIMEOUT,
                            Some(kube),
                        )
                    }
                };
                let state = replica::start_set(
                    dir,
                    mixer,
                    estimate,
                    set,
                    tls.clone(),
                    markers,
                    write_timeout,
                )
                .await?;
                (Arc::new(state), Some((tls, tcp)), kube, write_timeout)
            }
        };
        let peer_addr = match &peer {
            Some((_, tcp)) => Some(tcp.local_addr().map_err(|e| e.to_string())?),
            None => None,
        };
        let app = App::new(
            state.clone(),
            name.clone(),
            config.auth.mode,
            ops_state.metrics(),
        );
        // A set's replica starts contact, and with it its engine, once
        // the set lets it (Contact::begin).
        let mut contact = None;
        let audio = match &config.audio {
            Some(engine) => {
                let process = process.unwrap_or_else(|| samplez::rt::setup_process(&engine.rt));
                let (live, revision) = audio::state_of(&state);
                let audio = match &config.set {
                    None => Audio::start(
                        engine.clone(),
                        process,
                        &shape,
                        config.wiring.clone(),
                        &live,
                        ops_state.clone(),
                    )?,
                    Some(set) => {
                        let started = Audio::start_set(
                            engine.clone(),
                            process,
                            &shape,
                            config.wiring.clone(),
                            &live,
                            ops_state.clone(),
                            set,
                        )?;
                        let bridge = Arc::new(started.bridge);
                        let key = rollout::key(&config.compatibility(product));
                        let fixed = Fixed::new(key, set.revision.clone());
                        let follow = (kube.zip(set.statefulset.clone()))
                            .map(|(kube, statefulset)| (kube, statefulset, fixed.clone()));
                        let voters = usize::from(set.voters.replicas);
                        contact = Some(Contact {
                            timing: started.timing,
                            audio: Some(bridge),
                            rollout: fixed,
                            follow,
                            voters: set.peers.iter().take(voters).cloned().collect(),
                            formed: config.replica.data_dir.join(FORMED),
                        });
                        audio::report_inputs(
                            started.reports,
                            Arc::downgrade(&state),
                            write_timeout,
                        );
                        started.audio
                    }
                };
                Some((Arc::new(audio), live, revision))
            }
            None => None,
        };
        let service_config = ServiceConfig {
            listeners: config.listeners.clone(),
            auth: config.auth.clone(),
            api: config.api.clone(),
        };
        let ws_app = app.clone();
        let service = Service::new(
            service_config,
            ops_state.clone(),
            state.clone() as Arc<dyn ReplicatedState>,
        )
        .map_err(|e| format!("config: {e}"))?
        .api(api::router(app.clone()))
        .refusal(api::refusal)
        .ui(ui::router())
        .ws(move |socket, connect| ws::serve(ws_app.clone(), socket, connect));
        let bound = service
            .bind()
            .await
            .map_err(|e| format!("binding listeners: {e}"))?;
        let addr = |a: std::io::Result<SocketAddr>| a.map_err(|e| e.to_string());
        let (api, ops, nmos) = (
            addr(bound.api_addr())?,
            addr(bound.ops_addr())?,
            addr(bound.nmos_addr())?,
        );
        let machine = bound
            .machine_addr()
            .transpose()
            .map_err(|e| e.to_string())?;
        // A set of one is started once its engine plays the state
        // (Audio::follow), a replica of a set of more with an engine once
        // its contact started (Contact::begin). From then on, and from
        // the start without an engine, it is ready on the replica's terms:
        // joined and caught up and, with an engine, committing its
        // outputs, or none of the set's replicas in contact with it does
        // (samplez_replica's ReplicaInfo::ready).
        ops_state.set_engine_started(config.set.is_some() && contact.is_none());
        let contact_ops = ops_state.clone();
        let contact_replica = Arc::downgrade(&state);
        let replicated = config.set.is_some();
        let engine = audio.as_ref().map(|(a, _, _)| a.clone());
        let (tasks_app, tasks_state) = (app.clone(), state.clone());
        let failure = Arc::new(Mutex::new(None));
        let failed = failure.clone();
        let peer_state = state.clone();
        let serving = tokio::spawn(async move {
            let peers = async {
                match peer {
                    Some((tls, tcp)) => {
                        let served = samplez_replica::peer::serve(
                            tls,
                            tcp,
                            peer_state,
                            std::future::pending(),
                        )
                        .await;
                        match served {
                            Ok(()) => "the peer listener stopped".to_owned(),
                            Err(e) => format!("serving peers: {e}"),
                        }
                    }
                    None => std::future::pending().await,
                }
            };
            let follow = async {
                match audio {
                    Some((audio, live, revision)) => {
                        let meter = audio.meter(&tasks_app.hub, &name);
                        let count = tasks_app.count(&audio);
                        // A set's engine follows the replicated log.
                        let follow = async {
                            if !replicated {
                                audio.follow(&tasks_state, live, revision).await;
                            }
                            std::future::pending::<()>().await
                        };
                        tokio::join!(follow, meter, count);
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                r = bound.serve() => match r {
                    Ok(()) => "the listeners stopped".to_owned(),
                    Err(e) => format!("serving: {e}"),
                },
                e = peers => e,
                e = async {
                    match contact {
                        Some(contact) => match contact.begin(contact_replica).await {
                            Ok(()) => {
                                contact_ops.set_engine_started(true);
                                std::future::pending().await
                            }
                            Err(e) => format!("starting contact: {e}"),
                        },
                        None => std::future::pending().await,
                    }
                } => e,
                () = tasks_app.publish() => unreachable!("publishing never returns"),
                () = follow => unreachable!("following the state never returns"),
            }
        });
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let watching = tokio::spawn(async move {
            let mut serving = serving;
            tokio::select! {
                r = &mut serving => {
                    *failed.lock().expect("failure lock poisoned") = Some(ended(r));
                    // SAFETY: SIGTERM to this process ends the binary's sigwait.
                    unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
                }
                _ = stopped => {
                    serving.abort();
                    let _ = serving.await;
                }
            }
        });
        Ok(Server {
            api,
            ops,
            nmos,
            machine,
            peer: peer_addr,
            app,
            audio: engine,
            replica: state,
            watching,
            stop,
            failure,
        })
    }

    /// Why serving stopped, once it did; it then sends the process
    /// SIGTERM.
    pub fn failure(&self) -> Arc<Mutex<Option<String>>> {
        self.failure.clone()
    }

    /// Stops serving, stops the engine, which deletes its output flows,
    /// and shuts the replica down; what it applied is on its volume. A
    /// replica of a set first stops ordering commands and, leading, waits
    /// up to a write's timeout for every voter to hold what it appended;
    /// its engine stops once the runtime ends the replica's tasks, as the
    /// process exits.
    pub async fn stop(self) {
        if self.peer.is_some() {
            self.replica.quiesce(replica::WRITE_TIMEOUT).await;
        }
        let _ = self.stop.send(());
        let _ = self.watching.await;
        if let Some(audio) = self.audio.and_then(Arc::into_inner) {
            audio.stop();
        }
        self.replica.shutdown().await;
    }
}

/// What a set's replica starts contact with: the timers, its engine's
/// bridge, its rollout, and the StatefulSet whose target revision it
/// follows.
struct Contact {
    timing: samplez_replica::contact::Timing,
    /// The engine's bridge; none for a replica without one.
    audio: Option<Arc<dyn samplez_replica::contact::Audio>>,
    rollout: Arc<Fixed>,
    follow: Option<(Arc<Kube>, String, Arc<Fixed>)>,
    /// The peer addresses of the set's audio replicas.
    voters: Vec<String>,
    /// The file that says this replica saw its set formed ([`FORMED`]).
    formed: PathBuf,
}

impl Contact {
    /// Starts contact, and the replica's engine with it, once [`gate`]
    /// lets it and the replica has applied what the set committed, then
    /// follows the StatefulSet's rollout. A restarted replica catches up
    /// first: entries it applies while it catches up would be late, and
    /// held unresolved by the engine with nothing to decide them. Until
    /// then the engine computes and commits nothing. Returns once contact
    /// runs, or with the replica gone.
    ///
    /// # Errors
    ///
    /// When contact or the rollout does not start, or the formed file
    /// cannot be written.
    async fn begin(self, replica: Weak<Replica<Mixer>>) -> Result<(), String> {
        let formed = self.formed.exists();
        let mut tick = tokio::time::interval(CONTACT_WAIT);
        let mut told: Option<(Wait, tokio::time::Instant)> = None;
        loop {
            tick.tick().await;
            let Some(replica) = replica.upgrade() else {
                return Ok(());
            };
            let membership = replica.raft().metrics().borrow().membership_config.clone();
            let gated = match gate(membership.membership(), replica.id(), formed, &self.voters) {
                Ok(()) => match replica.read_index().await {
                    ReadIndex::Confirmed(revision) if replica.revision() >= revision => Ok(()),
                    _ => Err(Wait::CatchingUp),
                },
                Err(wait) => Err(wait),
            };
            let Err(wait) = gated else {
                break;
            };
            let repeat = told
                .as_ref()
                .is_some_and(|(w, at)| *w == wait && at.elapsed() < WAIT_TOLD);
            if !repeat {
                tracing::warn!("contact waits: {wait}");
                told = Some((wait, tokio::time::Instant::now()));
            }
        }
        if !formed {
            write_formed(&self.formed).map_err(|e| format!("{}: {e}", self.formed.display()))?;
        }
        let Some(replica) = replica.upgrade() else {
            return Ok(());
        };
        replica.contact(self.timing, Arc::new(SystemClocks), self.audio)?;
        replica.rollout(self.rollout)?;
        if let Some((kube, statefulset, into)) = self.follow {
            tokio::spawn(rollout::follow(kube, statefulset, into, ROLLOUT_READ));
        }
        Ok(())
    }
}

/// The file in a set's replica's data directory that says the replica
/// saw its set formed: every audio replica a voter, no learner.
pub const FORMED: &str = "set-formed";

fn write_formed(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::create(path)?.sync_all()?;
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Why a set's replica does not start contact yet.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wait {
    /// A joint configuration: the membership is changing.
    Joint,
    /// Not a voter: joining, or a learner.
    NotVoter,
    /// Before the set formed: these audio replicas are not voters yet.
    Voters(Vec<String>),
    /// Before the set formed: these replicas are learners.
    Learners(Vec<NodeId>),
    /// Applying what the set committed.
    CatchingUp,
}

impl std::fmt::Display for Wait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Wait::Joint => write!(f, "the membership is changing"),
            Wait::NotVoter => write!(f, "this replica is not a voter"),
            Wait::Voters(missing) => write!(f, "the set forms; not voters yet: {missing:?}"),
            Wait::Learners(ids) => write!(f, "the set forms; learners: {ids:?}"),
            Wait::CatchingUp => write!(f, "applying what the set committed"),
        }
    }
}

/// Whether replica `me` may start contact in `membership`: as a voter of
/// a single configuration and, until the set formed, of one whose voters
/// are every audio replica, `voters` by peer address, and with no
/// learner. A leader running contact promotes a learner only on the
/// learner's own reports, which a learner does not send, so a set forms
/// before contact starts. Once it formed, another replica a learner or
/// down holds no voter back: a voter restarting, as a rollout restarts
/// each, starts again.
fn gate(
    membership: &Membership<NodeId, BasicNode>,
    me: NodeId,
    formed: bool,
    voters: &[String],
) -> Result<(), Wait> {
    if membership.get_joint_config().len() > 1 {
        return Err(Wait::Joint);
    }
    if !membership.voter_ids().any(|id| id == me) {
        return Err(Wait::NotVoter);
    }
    if formed {
        return Ok(());
    }
    let addrs: Vec<&str> = (membership.voter_ids())
        .filter_map(|id| membership.get_node(&id))
        .map(|node| node.addr.as_str())
        .collect();
    let missing: Vec<String> = (voters.iter())
        .filter(|addr| !addrs.contains(&addr.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(Wait::Voters(missing));
    }
    let learners: Vec<NodeId> = membership.learner_ids().collect();
    if !learners.is_empty() {
        return Err(Wait::Learners(learners));
    }
    Ok(())
}

/// How often a set's replica checks whether it may start contact.
const CONTACT_WAIT: Duration = Duration::from_millis(100);
/// How often a replica waiting for the same reason says so again.
const WAIT_TOLD: Duration = Duration::from_secs(30);

/// Why the serving task ended: its error, or its panic. A panic would
/// otherwise leave the process running with no listeners.
pub fn ended(r: Result<String, tokio::task::JoinError>) -> String {
    r.unwrap_or_else(|e| format!("serving: {e}"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    const A: &str = "faderz-0:7400";
    const B: &str = "faderz-1:7400";
    const C: &str = "faderz-2:7400";

    /// Voters 1, 2, 3 at A, B, C, `learners` beside them, and a second
    /// configuration while `joint`.
    fn membership(voters: &[u64], learners: &[u64], joint: bool) -> Membership<NodeId, BasicNode> {
        let addr = |id: u64| [A, B, C, "faderz-3:7400"][id as usize - 1];
        let nodes: BTreeMap<NodeId, BasicNode> = (voters.iter().chain(learners))
            .map(|&id| (id, BasicNode::new(addr(id))))
            .collect();
        let mut configs = vec![voters.iter().copied().collect::<BTreeSet<_>>()];
        if joint {
            configs.push(voters.iter().copied().take(1).collect());
        }
        Membership::new(configs, nodes)
    }

    /// Before the set formed, a voter waits for every audio replica to be
    /// a voter and for no learner; a learner and a replica of a changing
    /// membership wait always.
    #[test]
    fn at_formation_contact_waits_for_every_voter() {
        let all = [A.to_owned(), B.to_owned(), C.to_owned()];
        assert_eq!(
            gate(&membership(&[1, 2, 3], &[], false), 1, false, &all),
            Ok(())
        );
        assert_eq!(
            gate(&membership(&[1, 2], &[], false), 1, false, &all),
            Err(Wait::Voters(vec![C.to_owned()]))
        );
        assert_eq!(
            gate(&membership(&[1, 2, 3], &[4], false), 1, false, &all),
            Err(Wait::Learners(vec![4]))
        );
        assert_eq!(
            gate(&membership(&[1, 2], &[3], false), 3, false, &all),
            Err(Wait::NotVoter)
        );
        assert_eq!(
            gate(&membership(&[1, 2, 3], &[], true), 1, false, &all),
            Err(Wait::Joint)
        );
    }

    /// Once the set formed, a voter starts with another replica down or a
    /// learner, as a rollout restarts each voter in turn; a learner and a
    /// replica of a changing membership still wait.
    #[test]
    fn once_formed_contact_needs_only_a_voter() {
        let all = [A.to_owned(), B.to_owned(), C.to_owned()];
        assert_eq!(
            gate(&membership(&[1, 2], &[], false), 1, true, &all),
            Ok(())
        );
        assert_eq!(
            gate(&membership(&[1, 2], &[3], false), 1, true, &all),
            Ok(())
        );
        assert_eq!(
            gate(&membership(&[1, 2], &[3], false), 3, true, &all),
            Err(Wait::NotVoter)
        );
        assert_eq!(
            gate(&membership(&[1, 2, 3], &[], true), 1, true, &all),
            Err(Wait::Joint)
        );
    }

    /// A replica of a set of one, voter at `local`, in a directory removed
    /// afterwards.
    async fn lone(name: &str) -> (Arc<Replica<Mixer>>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("faderz-contact-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = serde_json::json!({ "bind": "127.0.0.1", "port": 0 });
        let config = serde_json::json!({
            "listeners": { "api": listener, "ops": listener, "nmos": listener },
            "auth": { "mode": "none" },
            "api": { "allowedOrigins": ["http://faderz.test"] },
            "replica": { "name": "faderz-0", "dataDir": dir },
            "shape": {
                "sampleRate": 48000,
                "frameRate": { "numerator": 25, "denominator": 1 },
                "channels": [],
                "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
                "auxes": [], "mixMinus": [], "vcas": [],
                "monitorOutputs": [], "outputs": [],
            },
        });
        let (config, shape, _) = Config::parse(&config.to_string(), || unreachable!()).unwrap();
        let mixer = Mixer::factory(shape, config.consoles.clone(), config.wiring.clone());
        let replica = replica::start(&dir, mixer, Arc::new(replica::tai_samples))
            .await
            .unwrap();
        (Arc::new(replica), dir)
    }

    fn contact(dir: &std::path::Path, voters: &[&str]) -> Contact {
        Contact {
            timing: samplez_replica::contact::Timing::defaults(replica::BLOCK),
            audio: None,
            rollout: Fixed::new(rollout::key(b"faderz"), None),
            follow: None,
            voters: voters.iter().map(|&v| v.to_owned()).collect(),
            formed: dir.join(FORMED),
        }
    }

    /// A replica whose set has not formed does not start contact, so its
    /// engine is not started and it is not ready; once it formed, it is
    /// written to the volume.
    #[tokio::test]
    async fn contact_waits_for_the_set_and_notes_it_formed() {
        let (replica, dir) = lone("wait").await;
        let waiting = contact(&dir, &["local", B]).begin(Arc::downgrade(&replica));
        let waited = tokio::time::timeout(Duration::from_secs(1), waiting).await;
        assert!(waited.is_err(), "contact started: {waited:?}");
        assert!(replica.contact_metrics().is_none());
        assert!(!dir.join(FORMED).exists());
        contact(&dir, &["local"])
            .begin(Arc::downgrade(&replica))
            .await
            .unwrap();
        assert!(replica.contact_metrics().is_some());
        assert!(dir.join(FORMED).exists());
        replica.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Contact that does not start is an error, which ends serving and
    /// with it the process, so the pod restarts.
    #[tokio::test]
    async fn contact_that_does_not_start_is_an_error() {
        let (replica, dir) = lone("fail").await;
        let timing = samplez_replica::contact::Timing::defaults(replica::BLOCK);
        replica
            .contact(timing, Arc::new(SystemClocks), None)
            .unwrap();
        let e = contact(&dir, &["local"])
            .begin(Arc::downgrade(&replica))
            .await
            .unwrap_err();
        assert!(e.contains("contact runs already"), "{e}");
        replica.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_panicking_server_task_is_a_failure() {
        let task = tokio::spawn(async { panic!("follow broke") });
        let e = ended(task.await);
        assert!(e.contains("panicked"), "{e}");
        let task = tokio::spawn(async { "the listeners stopped".to_owned() });
        assert_eq!(ended(task.await), "the listeners stopped");
    }
}
