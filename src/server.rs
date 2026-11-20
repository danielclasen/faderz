//! A running faderz: the replica, the engine, the listeners and the tasks
//! beside them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use samplez::rt::ProcessStatus;
use samplez_replica::bootstrap::{Kube, Markers};
use samplez_replica::contact::SystemClocks;
use samplez_replica::net::Pool;
use samplez_replica::peer::max_response;
use samplez_replica::rollout::{self, Fixed, Versions};
use samplez_replica::state_machine::Product as _;
use samplez_replica::{Estimate, Replica};
use samplez_service::{Ops, PeerTls, ReplicatedState, Service, ServiceConfig};
use tokio::task::JoinHandle;

use crate::app::App;
use crate::audio::{self, Audio};
use crate::config::Config;
use crate::nmos::Node;
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
    /// The NMOS node, with `nmos.enabled`.
    pub node: Option<Arc<Node>>,
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
            config.own_flows(),
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
                    // Reads a donor's core with its delay lines' history.
                    Pool::new(tls.clone(), max_response(config.delay_budget())),
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
            config.set.as_ref(),
            config.auth.mode,
            ops_state.metrics(),
        );
        // A set's replica starts contact, and with it its engine, once
        // its listeners serve (Contact::begin).
        let mut contact = None;
        let audio = match &config.audio {
            Some(engine) => {
                let process = process.unwrap_or_else(|| samplez::rt::setup_process(&engine.rt));
                let (live, revision) = audio::owned_state_of(&state);
                // The leg flows IS-05 activations named, the detached
                // channels and the outputs' enable, which the state keeps.
                let view = crate::app::with_state(&state, |sm| sm.product.engine_view());
                let audio = match &config.set {
                    None => Audio::start(
                        engine.clone(),
                        process,
                        &shape,
                        view.clone(),
                        &live,
                        ops_state.clone(),
                    )?,
                    Some(set) => {
                        let started = Audio::start_set(
                            engine.clone(),
                            process,
                            &shape,
                            view.clone(),
                            &live,
                            ops_state.clone(),
                            set,
                        )?;
                        let bridge = Arc::new(started.bridge);
                        let key = rollout::key::<Mixer>(&config.compatibility(product));
                        let fixed =
                            Fixed::new(key, Some(Versions::of::<Mixer>()), set.revision.clone());
                        let follow = (kube.zip(set.statefulset.clone()))
                            .map(|(kube, statefulset)| (kube, statefulset, fixed.clone()));
                        contact = Some(Contact {
                            timing: started.timing,
                            audio: Some(bridge),
                            rollout: fixed,
                            follow,
                        });
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
        let node = match config.nmos.enabled {
            true => Some(Arc::new(Node::new(
                config,
                &shape,
                &state,
                app.clone(),
                config.listeners.nmos.port,
                ops_state.metrics(),
            )?)),
            false => None,
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
        let service = match &node {
            Some(node) => service.nmos(node.router()),
            None => service,
        };
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
        let serving_node = node.clone();
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
                        // A set's engine follows the replicated log; only
                        // its meter taps and loudness resets follow the
                        // state here.
                        let follow = async {
                            if replicated {
                                audio.follow_taps(&tasks_state, revision).await;
                            } else {
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
                        Some(contact) => match contact.begin(&contact_replica) {
                            Ok(()) => {
                                contact_ops.set_engine_started(true);
                                std::future::pending().await
                            }
                            Err(e) => format!("starting contact: {e}"),
                        },
                        None => std::future::pending().await,
                    }
                } => e,
                () = async {
                    match serving_node {
                        Some(node) => node.run().await,
                        None => std::future::pending().await,
                    }
                } => "the replica stopped under the nmos node".to_owned(),
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
            node,
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
        // Once nothing posts to the registry any more.
        if let Some(node) = &self.node {
            node.unregister().await;
        }
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
}

impl Contact {
    /// Starts contact, and the replica's engine with it, then follows the
    /// StatefulSet's rollout. A replica starts contact as it starts,
    /// whether a voter, a learner or catching up: samplez promotes a
    /// learner on the reports contact sends, and rebuilds the engine of a
    /// replica that took entries past their deadlines from the state it
    /// applied. Nothing with the replica gone.
    ///
    /// # Errors
    ///
    /// When contact or the rollout does not start.
    fn begin(self, replica: &Weak<Replica<Mixer>>) -> Result<(), String> {
        let Some(replica) = replica.upgrade() else {
            return Ok(());
        };
        // Before contact, so its first report carries the key: a replica
        // reporting none could leave `starting` before its key is known.
        replica.rollout(self.rollout)?;
        replica.contact(self.timing, Arc::new(SystemClocks), self.audio)?;
        if let Some((kube, statefulset, into)) = self.follow {
            tokio::spawn(rollout::follow(kube, statefulset, into, ROLLOUT_READ));
        }
        Ok(())
    }
}

/// Why the serving task ended: its error, or its panic. A panic would
/// otherwise leave the process running with no listeners.
pub fn ended(r: Result<String, tokio::task::JoinError>) -> String {
    r.unwrap_or_else(|e| format!("serving: {e}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
        let mixer = Mixer::factory(
            shape,
            config.consoles.clone(),
            config.wiring.clone(),
            config.own_flows(),
        );
        let replica = replica::start(&dir, mixer, Arc::new(replica::tai_samples))
            .await
            .unwrap();
        (Arc::new(replica), dir)
    }

    fn contact() -> Contact {
        Contact {
            timing: samplez_replica::contact::Timing::defaults(replica::BLOCK),
            audio: None,
            rollout: Fixed::new(
                rollout::key::<Mixer>(b"faderz"),
                Some(Versions::of::<Mixer>()),
                None,
            ),
            follow: None,
        }
    }

    /// Contact starts as soon as it is begun, with no wait on the set, and
    /// with the replica gone begins nothing.
    #[tokio::test]
    async fn contact_starts_with_the_replica() {
        let (replica, dir) = lone("start").await;
        contact().begin(&Arc::downgrade(&replica)).unwrap();
        assert!(replica.contact_metrics().is_some(), "contact runs");
        contact().begin(&Weak::new()).unwrap();
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
        let e = contact().begin(&Arc::downgrade(&replica)).unwrap_err();
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
