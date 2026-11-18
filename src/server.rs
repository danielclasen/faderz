//! A running faderz: the replica, the engine, the listeners and the tasks
//! beside them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};

use samplez::rt::ProcessStatus;
use samplez_replica::bootstrap::{Kube, Markers};
use samplez_replica::bridge::EngineAudio;
use samplez_replica::contact::SystemClocks;
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
const ROLLOUT_READ: std::time::Duration = std::time::Duration::from_secs(5);

/// What a test gives a set of more than one replica in place of what
/// [`Server::start`] takes from the pod and the configuration.
pub struct SetParts {
    /// In place of the ConfigMap `replica.marker`.
    pub markers: Arc<dyn Markers>,
    /// The peer listener, bound already.
    pub listener: std::net::TcpListener,
    /// How long a write may take to be ordered and applied.
    pub write_timeout: std::time::Duration,
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
        let (state, peer, kube) = match &config.set {
            None => {
                let state = replica::start(dir, mixer, estimate).await?;
                (Arc::new(state), None, None)
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
                (Arc::new(state), Some((tls, tcp)), kube)
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
                        let contact = Contact {
                            timing: started.timing,
                            bridge,
                            rollout: fixed,
                            follow,
                            voters: usize::from(set.voters.replicas),
                        };
                        tokio::spawn(contact.begin(Arc::downgrade(&state)));
                        audio::report_inputs(started.reports, Arc::downgrade(&state));
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
        // (Audio::follow). A replica of a set of more is ready on the
        // replica's terms: joined and caught up and, with an engine,
        // committing its outputs, or none of the set's replicas in contact
        // with it does (samplez_replica's ReplicaInfo::ready).
        ops_state.set_engine_started(config.set.is_some());
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
    bridge: Arc<EngineAudio>,
    rollout: Arc<Fixed>,
    follow: Option<(Arc<Kube>, String, Arc<Fixed>)>,
    /// The set's voters.
    voters: usize,
}

impl Contact {
    /// Starts contact, and the replica's engine with it, once the replica
    /// is a voter of a membership of all the set's voters and has applied
    /// what the set committed. A leader running contact promotes a
    /// learner only on the learner's own reports, which a learner does not
    /// send, so a set forms before contact starts; a restarted replica
    /// catches up first. Until then the engine computes and commits
    /// nothing.
    async fn begin(self, replica: Weak<Replica<Mixer>>) {
        let mut tick = tokio::time::interval(CONTACT_WAIT);
        loop {
            tick.tick().await;
            let Some(replica) = replica.upgrade() else {
                return;
            };
            let metrics = replica.raft().metrics().borrow().clone();
            let membership = metrics.membership_config.membership();
            let voters: Vec<_> = membership.voter_ids().collect();
            if voters.len() < self.voters
                || !voters.contains(&replica.id())
                || membership.learner_ids().next().is_some()
                || membership.get_joint_config().len() > 1
            {
                continue;
            }
            // Entries it applies while it catches up would be late, and
            // held unresolved by the engine with nothing to decide them.
            let caught_up = match replica.read_index().await {
                ReadIndex::Confirmed(revision) => replica.revision() >= revision,
                _ => false,
            };
            if !caught_up {
                continue;
            }
            let started = replica
                .contact(self.timing, Arc::new(SystemClocks), Some(self.bridge))
                .and_then(|()| replica.rollout(self.rollout));
            if let Err(e) = started {
                tracing::error!("contact did not start: {e}");
            }
            if let Some((kube, statefulset, into)) = self.follow {
                tokio::spawn(rollout::follow(kube, statefulset, into, ROLLOUT_READ));
            }
            return;
        }
    }
}

/// How often a set's replica checks whether it may start contact.
const CONTACT_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// Why the serving task ended: its error, or its panic. A panic would
/// otherwise leave the process running with no listeners.
pub fn ended(r: Result<String, tokio::task::JoinError>) -> String {
    r.unwrap_or_else(|e| format!("serving: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panicking_server_task_is_a_failure() {
        let task = tokio::spawn(async { panic!("follow broke") });
        let e = ended(task.await);
        assert!(e.contains("panicked"), "{e}");
        let task = tokio::spawn(async { "the listeners stopped".to_owned() });
        assert_eq!(ended(task.await), "the listeners stopped");
    }
}
