//! A running faderz: the replica, the engine, the listeners and the tasks
//! beside them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use samplez::rt::ProcessStatus;
use samplez_replica::{Estimate, Replica};
use samplez_service::{Ops, ReplicatedState, Service, ServiceConfig};
use tokio::task::JoinHandle;

use crate::app::App;
use crate::audio::{self, Audio};
use crate::config::Config;
use crate::shape::Shape;
use crate::state::Mixer;
use crate::{api, replica, ui, ws};

pub struct Server {
    pub api: SocketAddr,
    pub ops: SocketAddr,
    pub nmos: SocketAddr,
    pub machine: Option<SocketAddr>,
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
        let estimate: Estimate = Arc::new(replica::tai_samples);
        let state = Arc::new(
            replica::start(&config.replica.data_dir, shape.clone(), estimate.clone()).await?,
        );
        let ops_state = Ops::default();
        let app = App::new(state.clone(), name.clone(), ops_state.metrics());
        let audio = match (&config.audio, &config.mxl) {
            (Some(engine), Some(mxl)) => {
                let process = process.unwrap_or_else(|| samplez::rt::setup_process(&engine.rt));
                let alt = shape
                    .channels
                    .iter()
                    .map(|c| mxl.inputs[&c.id].alt_flow.clone());
                let (live, revision) = audio::state_of(&state);
                let audio = Audio::start(
                    engine.clone(),
                    process,
                    &shape,
                    alt.collect(),
                    &live,
                    ops_state.clone(),
                )?;
                Some((Arc::new(audio), live, revision))
            }
            _ => None,
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
        ops_state.set_engine_started(audio.is_some());
        let engine = audio.as_ref().map(|(a, _, _)| a.clone());
        let (tasks_app, tasks_state) = (app.clone(), state.clone());
        let failure = Arc::new(Mutex::new(None));
        let failed = failure.clone();
        let serving = tokio::spawn(async move {
            let follow = async {
                match audio {
                    Some((audio, live, revision)) => {
                        let meter = audio.meter(&tasks_app.hub, &name);
                        tokio::join!(audio.follow(&tasks_state, live, revision), meter);
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                r = bound.serve() => match r {
                    Ok(()) => "the listeners stopped".to_owned(),
                    Err(e) => format!("serving: {e}"),
                },
                () = tasks_app.publish() => unreachable!("publishing never returns"),
                // Stop-gap until samplez-replica decides lease expiries.
                () = replica::expire_leases(&tasks_state, estimate) => unreachable!("expiring never returns"),
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
    /// and shuts the replica down; what it applied is on its volume.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.watching.await;
        if let Some(audio) = self.audio.and_then(Arc::into_inner) {
            audio.stop();
        }
        self.replica.shutdown().await;
    }
}

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
