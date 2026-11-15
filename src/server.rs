//! A running faderz: the replica, the listeners and the tasks beside
//! them.

use std::net::SocketAddr;
use std::sync::Arc;

use samplez_replica::{Estimate, Replica};
use samplez_service::{Ops, ReplicatedState, Service, ServiceConfig};
use tokio::task::JoinHandle;

use crate::app::App;
use crate::config::Config;
use crate::shape::Shape;
use crate::state::Mixer;
use crate::{api, replica, ws};

pub struct Server {
    pub api: SocketAddr,
    pub ops: SocketAddr,
    pub nmos: SocketAddr,
    pub machine: Option<SocketAddr>,
    pub app: Arc<App>,
    replica: Arc<Replica<Mixer>>,
    serving: JoinHandle<Result<(), String>>,
}

impl Server {
    /// Opens the state, binds the listeners and serves them.
    pub async fn start(config: &Config, shape: Shape, name: String) -> Result<Server, String> {
        let estimate: Estimate = Arc::new(replica::tai_samples);
        let state =
            Arc::new(replica::start(&config.replica.data_dir, shape, estimate.clone()).await?);
        let app = App::new(state.clone(), name);
        let service_config = ServiceConfig {
            listeners: config.listeners.clone(),
            auth: config.auth.clone(),
            api: config.api.clone(),
        };
        let ws_app = app.clone();
        let service = Service::new(
            service_config,
            Ops::default(),
            state.clone() as Arc<dyn ReplicatedState>,
        )
        .map_err(|e| format!("config: {e}"))?
        .api(api::router(app.clone()))
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
        let (tasks_app, tasks_state) = (app.clone(), state.clone());
        let serving = tokio::spawn(async move {
            tokio::select! {
                r = bound.serve() => r.map_err(|e| format!("serving: {e}")),
                () = tasks_app.publish() => unreachable!("publishing never returns"),
                // Stop-gap until samplez-replica decides lease expiries.
                () = replica::expire_leases(&tasks_state, estimate) => unreachable!("expiring never returns"),
            }
        });
        Ok(Server {
            api,
            ops,
            nmos,
            machine,
            app,
            replica: state,
            serving,
        })
    }

    /// Resolves when serving failed.
    pub async fn failed(&mut self) -> String {
        match (&mut self.serving).await {
            Ok(Ok(())) => "the listeners stopped".into(),
            Ok(Err(e)) => e,
            Err(e) => e.to_string(),
        }
    }

    /// Stops serving and shuts the replica down; what it applied is on
    /// its volume.
    pub async fn stop(self) {
        self.serving.abort();
        let _ = self.serving.await;
        self.replica.shutdown().await;
    }
}
