//! NMOS over samplez-nmos: one IS-04 Node for the whole replica set, a
//! Receiver per channel, the channel's input reader, and a Source, Flow
//! and Sender per output, with the IS-05 Connection API over MXL per
//! AMWA BCP-007-03.
//!
//! Every replica serves the Node and Connection APIs under the same ids,
//! derived from `nmos.seed`. An activation becomes a [`Command::Connect`]
//! in the replicated log, so every replica's engine routes alike and the
//! connections survive restarts in the state; each replica's `/active`
//! follows the state, so it shows what another replica activated and a
//! switch between a channel's inputs a and b. A scheduled activation is a
//! [`Command::Schedule`] when it is staged and a [`Command::Fire`] the
//! leader orders at its time, so it survives a leader change. Only the
//! leader registers the Node, and only while a quorum acknowledges it;
//! the other replicas forward `/staged` and `/bulk` to it, so one replica
//! holds the staged parameters.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use openraft::ServerState;
use samplez_mxl::{Instance, MxlApi};
use samplez_nmos::uuid::Uuid;
use samplez_nmos::{
    ActivationMode, Active, Applied, Asset, BoxFuture, ChannelCount, ChannelDesc, Connections,
    Fail, FlowSeen, NmosConfig, NodeModel, Port, PortKey, ReceiverDesc, Reject, Replicated,
    SenderDesc, Store, SystemTai, Tai, TaiClock,
};
use samplez_replica::Replica;
use samplez_replica::entry::SessionKey;
use samplez_service::replica::Begin;
use samplez_service::{Metrics, Owner, ReplicatedState, Role};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::app::App;
use crate::config::{Config, Mxl};
use crate::shape::Shape;
use crate::state::{Activation, Answer, Command, Connection, Mixer, PortRef, Scheduled, tai_text};

/// The principal and session activations are ordered under: shorter than
/// any client session id, so no client shares it.
const SESSION: &str = "nmos";
/// How often an activation the state refused for being ordered off the
/// generation path is sent again.
const RESENDS: usize = 3;
/// How often a port's change in the state that waited on an activation
/// is looked at again.
const FOLLOW_TICK: Duration = Duration::from_millis(100);
/// How long a scheduled activation's time is looked for while none is
/// due.
const FIRE_TICK: Duration = Duration::from_millis(20);
/// How long a scheduled activation that did not fire waits for the next
/// try.
const FIRE_RETRY: Duration = Duration::from_secs(1);
/// The longest since a quorum acknowledged the leader for it to register
/// the Node: a leader cut off from the others stops within it.
const QUORUM_FRESH: u64 = 1_000;
/// How often a registering leader checks it still leads.
const LEAD_CHECK: Duration = Duration::from_millis(100);
/// How much longer than its own work a forwarded request may take.
const FORWARD_MARGIN: Duration = Duration::from_secs(5);
/// The largest Connection API request body taken.
const MAX_BODY: usize = 1 << 20;
/// The header a forwarded request carries: a replica that does not lead
/// answers it with 503 rather than forward it again or stage it itself.
const FORWARDED: &str = "x-faderz-forwarded";
/// How long before its apply timeout an activation stops waiting for room
/// among the commands in flight.
const BUSY_MARGIN: Duration = Duration::from_secs(1);
/// The file in a domain directory BCP-007-03 names its identity in.
const DOMAIN_DEF: &str = "domain_def.json";

/// The node and what it needs to run.
pub struct Node {
    pub nmos: samplez_nmos::Nmos,
    conns: Arc<Conns>,
    /// Each port in the node's order, senders first.
    ports: Arc<Vec<Model>>,
    replica: Weak<Replica<Mixer>>,
    /// The set's voters; 1 for a set of one.
    voters: usize,
    /// Per ordinal, the replica's peer address and the address its NMOS
    /// listener is reached at; empty for a set of one.
    peers: Vec<(String, String)>,
    /// Each port's place by whether it is a sender and its IS-04 id.
    ids: BTreeMap<(bool, String), usize>,
    /// Whether every version comes from the state, as in a set
    /// (`samplez_nmos::Nmos::with_versions`).
    shared: bool,
    /// With shared versions, the version of each port's change in the
    /// state this node last handed samplez-nmos, TAI nanoseconds, in the
    /// node's order.
    passed: Mutex<Vec<u64>>,
}

/// A port as [`actives`] reads it from the state.
#[derive(Debug, Clone)]
enum Model {
    Sender {
        output: String,
        domain: Uuid,
        flow: Uuid,
    },
    Receiver {
        channel: String,
        domain: Uuid,
    },
}

impl Node {
    /// Describes the node of `config` and `shape` and sets each port's
    /// `/active` from the state `replica` holds.
    ///
    /// # Errors
    ///
    /// When a domain has no id or two, or the node cannot be described.
    pub fn new(
        config: &Config,
        shape: &Shape,
        replica: &Arc<Replica<Mixer>>,
        app: Arc<App>,
        listener_port: u16,
        metrics: &Metrics,
    ) -> Result<Node, String> {
        let (Some(mxl), Some(engine)) = (&config.mxl, &config.audio) else {
            return Err("nmos.enabled: needs the mxl section".into());
        };
        let domains = domain_ids(mxl)?;
        let nmos = &config.nmos;
        let seed = match (&nmos.seed, &config.set) {
            (Some(seed), _) => seed.clone(),
            (None, Some(set)) => set.statefulset.clone().ok_or(
                "nmos.seed: required in a set whose replica names do not end in their ordinal",
            )?,
            (None, None) => config
                .replica
                .name
                .clone()
                .unwrap_or_else(crate::config::hostname),
        };
        let domain_of = |name: &str| domains[name];
        let flow = |at: &str, id: &str| {
            Uuid::parse_str(id).map_err(|e| format!("nmos: {at} flow {id:?}: {e}"))
        };
        let (receivers, senders) = crate::app::with_state(replica, |sm| {
            let m = &sm.product;
            // A tone input reads no flow: it has no receiver.
            let receivers = (shape.channels.iter().zip(&engine.inputs))
                .filter(|(_, input)| input.tone.is_none())
                .map(|(c, input)| {
                    let (_, selected) = m.receiver(&c.id).expect("every channel is wired");
                    let legs = mxl.inputs[&c.id].channels.as_deref();
                    let least = legs.and_then(|l| l.iter().max().copied());
                    Ok(ReceiverDesc {
                        key: c.id.clone(),
                        label: c.name.clone(),
                        channels: ChannelCount::AtLeast(least.unwrap_or(c.width as u32)),
                        group: c.id.clone(),
                        domain: domain_of(
                            input
                                .domain
                                .as_deref()
                                .expect("a flow input names a domain"),
                        ),
                        flow: Some(flow(&format!("channel {}", c.id), selected)?),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let senders = (shape.outputs.iter().zip(&engine.outputs))
                .map(|(o, output)| {
                    let given = &mxl.outputs[&o.id];
                    let id = given.merged_flow.as_ref().unwrap_or(&output.flow_id);
                    Ok(SenderDesc {
                        key: o.id.clone(),
                        label: o.name.clone(),
                        channels: symbols(&o.layout, o.width)
                            .into_iter()
                            .map(|s| ChannelDesc {
                                label: s.to_owned(),
                                symbol: Some(s.to_owned()),
                            })
                            .collect(),
                        group: o.id.clone(),
                        domain: domain_of(&output.domain),
                        flow: flow(&format!("output {}", o.id), id)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok::<_, String>((receivers, senders))
        })?;
        let model = NodeModel {
            label: seed.clone(),
            description: "faderz mixer".into(),
            senders,
            receivers,
        };
        let senders = model.senders.iter().map(|s| Model::Sender {
            output: s.key.clone(),
            domain: s.domain,
            flow: s.flow,
        });
        let receivers = model.receivers.iter().map(|r| Model::Receiver {
            channel: r.key.clone(),
            domain: r.domain,
        });
        let ports: Arc<Vec<Model>> = Arc::new(senders.chain(receivers).collect());
        let ids = port_ids(&seed, &ports);
        let api = samplez_mxl::load(&mxl.library_path).map_err(|e| format!("nmos: {e}"))?;
        let paths = mxl.domains.iter().map(|d| (d.name.clone(), d.path.clone()));
        let instance = crate::app::uuid().map_err(|e| format!("nmos: {e}"))?;
        let conns = Arc::new_cyclic(|me| Conns {
            me: me.clone(),
            app,
            ports: ports.clone(),
            resolver: Arc::new(Resolver {
                engine: engine.clone(),
                own: config.own_flows(),
                api,
                paths: paths.collect(),
                instances: Mutex::default(),
            }),
            seen: Mutex::new(vec![0; model.senders.len() + model.receivers.len()]),
            asked: Mutex::new(vec![None; model.senders.len() + model.receivers.len()]),
            instance,
            node: Mutex::default(),
            tasks: Mutex::default(),
        });
        let (shown, activations, versions) = crate::app::with_state(replica, |sm| {
            let m = &sm.product;
            let activations: Vec<_> = (ports.iter())
                .map(|p| connection_of(m, p).and_then(|c| c.activation.as_ref()))
                .map(|a| a.and_then(nmos_activation))
                .collect();
            let versions: Vec<u64> = (ports.iter())
                .map(|p| m.port_change(&p.port_ref()).map_or(0, |c| c.version))
                .collect();
            (actives(m, &ports), activations, versions)
        });
        let keys = || ports.iter().map(Model::key);
        let store = Loaded {
            active: keys().zip(shown).collect(),
            activations: (keys().zip(activations))
                .filter_map(|(k, a)| Some((k, a?)))
                .collect(),
        };
        let cfg = NmosConfig {
            seed: seed.clone(),
            advertise_host: nmos.advertise_host.clone(),
            advertise_port: match nmos.advertise_port {
                0 => listener_port,
                p => p,
            },
            asset: Asset {
                manufacturer: nmos.asset.manufacturer.clone(),
                product: "faderz".into(),
                instance_id: seed,
                functions: vec!["Mixer".into()],
            },
            heartbeat: Duration::from_secs(nmos.heartbeat_s),
            registries: nmos.registry.url.iter().cloned().collect(),
            // A set's leaders take turns under one Node id: a new leader
            // continues the registration its predecessor left, so the
            // Node stays registered through a hand-over.
            continue_held: config.set.is_some(),
            dns_sd_domain: nmos.registry.dns_sd_domain.clone(),
        };
        // A set's replicas serve one Node, so every version comes from
        // the state: each replica serves the same documents.
        let shared = config.set.is_some();
        let node = if shared {
            let tai: BTreeMap<PortKey, Tai> = (keys().zip(&versions))
                .map(|(k, v)| (k, Tai::from_nanos(u128::from(*v))))
                .collect();
            samplez_nmos::Nmos::with_versions(model, cfg, conns.clone(), &store, SystemTai, &tai)
        } else {
            samplez_nmos::Nmos::new(model, cfg, conns.clone(), &store, SystemTai)
        }
        .map_err(|e| e.to_string())?;
        if shared {
            *conns.node.lock().expect("node lock poisoned") = Some(node.clone());
        }
        node.register_metrics(metrics);
        let peers = match &config.set {
            Some(set) => {
                let nmos_peers = config.nmos_peers();
                set.peers.iter().cloned().zip(nmos_peers).collect()
            }
            None => Vec::new(),
        };
        Ok(Node {
            nmos: node,
            conns,
            ports,
            replica: Arc::downgrade(replica),
            voters: config
                .set
                .as_ref()
                .map_or(1, |s| usize::from(s.voters.replicas)),
            peers,
            ids,
            shared,
            passed: Mutex::new(versions),
        })
    }

    /// The Node and Connection APIs, with `/staged` and `/bulk` of the
    /// Connection API forwarded to the leader of a set of more than one.
    pub fn router(self: &Arc<Self>) -> Router {
        self.nmos
            .router()
            .layer(axum::middleware::from_fn_with_state(self.clone(), forward))
    }

    /// Keeps each port's `/active` on the state, fires the scheduled
    /// activations that are due while this replica leads, and keeps the
    /// Node registered while it leads. Never returns while the replica
    /// runs.
    pub async fn run(&self) {
        tokio::join!(self.follow(), self.fire(), self.register());
    }

    /// Deletes the Node from its registry when this replica is a set of
    /// one. A replica of a set of more leaves the Node to the others: the
    /// next leader registers it under the same ids, and a registry drops
    /// it once no replica heartbeats.
    pub async fn unregister(&self) {
        if self.voters == 1 && self.replica.upgrade().is_some_and(|r| leading(&r)) {
            self.nmos.unregister().await;
        }
    }

    /// The port a Connection API path names, by its place among the
    /// ports.
    fn index_of(&self, path: &str) -> Option<usize> {
        let (kind, id) = port_of(path)?;
        self.ids.get(&(kind == "senders", id.to_owned())).copied()
    }

    /// The scheduled activation the state holds for the port at `index`.
    fn scheduled(&self, index: usize) -> Option<Scheduled> {
        let port = self.ports[index].port_ref();
        let replica = self.replica.upgrade()?;
        crate::app::with_state(&replica, |sm| {
            (sm.product.scheduled().iter())
                .find(|s| s.port == port)
                .cloned()
        })
    }

    /// Serves a staging request here, on the leader or the replica of a
    /// set of one. A scheduled activation the state holds and samplez-nmos
    /// does not, taken by another leader or before a restart, shows in
    /// `/staged` and locks it (423) as one samplez-nmos holds would, and a
    /// cancel orders [`Command::Unschedule`]: samplez-nmos hands a cancel
    /// over only for an activation it holds itself.
    async fn stage(&self, req: Request, next: Next) -> Response {
        let path = req.uri().path().trim_end_matches('/').to_owned();
        let method = req.method().clone();
        let (parts, body) = req.into_parts();
        let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
            return refused(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request body is too large",
            );
        };
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let single = self.index_of(&path);
        // Each port the request names, and whether it cancels there.
        let named: Vec<(usize, bool)> = match single {
            Some(index) => vec![(index, cancels(&value))],
            None => {
                let senders = path.ends_with("/senders");
                (value.as_array().into_iter().flatten())
                    .filter_map(|item| {
                        let id = item.get("id")?.as_str()?;
                        let index = *self.ids.get(&(senders, id.to_owned()))?;
                        Some((index, item.get("params").is_some_and(cancels)))
                    })
                    .collect()
            }
        };
        // What a scheduled activation is asked for with, kept for the
        // schedule samplez-nmos hands over while serving the request: a
        // PATCH of `/staged`, a POST of `/bulk`.
        if method == axum::http::Method::PATCH || method == axum::http::Method::POST {
            let params: Vec<(usize, &serde_json::Value)> = match single {
                Some(index) => vec![(index, &value)],
                None => {
                    let senders = path.ends_with("/senders");
                    (value.as_array().into_iter().flatten())
                        .filter_map(|item| {
                            let id = item.get("id")?.as_str()?;
                            Some((
                                *self.ids.get(&(senders, id.to_owned()))?,
                                item.get("params")?,
                            ))
                        })
                        .collect()
                }
            };
            let mut asked = self.conns.asked.lock().expect("asked lock poisoned");
            // An activation of any other mode, a cancel among them,
            // leaves nothing asked; staged params alone change nothing.
            for (index, params) in params {
                if params
                    .get("activation")
                    .and_then(|a| a.get("mode"))
                    .is_some()
                {
                    asked[index] = scheduled_request(params);
                }
            }
        }
        let status = self.nmos.status();
        let adopted = |index: usize| {
            if status.ports[index].pending.is_some() {
                None
            } else {
                self.scheduled(index)
            }
        };
        if method == axum::http::Method::PATCH
            && let Some(index) = single
            && !cancels(&value)
            && adopted(index).is_some()
        {
            return refused(StatusCode::LOCKED, "a scheduled activation is pending");
        }
        let req = Request::from_parts(parts, axum::body::Body::from(body));
        let response = next.run(req).await;
        if method == axum::http::Method::GET
            && let Some(index) = single
            && let Some(s) = adopted(index)
        {
            return with_json(response, |v| {
                let at = tai_text(s.at);
                let (mode, requested) = match &s.active.activation {
                    Some(a) => (a.mode.clone(), a.requested.clone()),
                    None => ("activate_scheduled_absolute".to_owned(), Some(at.clone())),
                };
                v["activation"] = serde_json::json!({
                    "mode": mode,
                    "requested_time": requested,
                    "activation_time": at,
                });
                v["master_enable"] = s.active.master_enable.into();
                let peer = match s.port {
                    PortRef::Sender(_) => "receiver_id",
                    PortRef::Receiver(_) => "sender_id",
                };
                v[peer] = s.active.peer.clone().into();
                if let Some(leg) = v["transport_params"].get_mut(0) {
                    leg["mxl_domain_id"] = s.active.domain.clone().into();
                    leg["mxl_flow_id"] = s.active.flow.clone().into();
                }
            })
            .await;
        }
        if method == axum::http::Method::GET
            || !response.status().is_success()
            || !named.iter().any(|(_, cancel)| *cancel)
        {
            return response;
        }
        if single.is_some() {
            for (index, _) in named.into_iter().filter(|(_, cancel)| *cancel) {
                if let Err(e) = self.cancel(index).await {
                    return refused(StatusCode::SERVICE_UNAVAILABLE, &e);
                }
            }
            return response;
        }
        // `/bulk` answers 200 with a code per item, in the order of the
        // request's items: a cancel samplez-nmos refused cancels nothing,
        // a later item for the same port is what the port is left with,
        // and a cancel the set did not take fails on its own item.
        let (mut parts, body) = response.into_parts();
        let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "an unreadable response");
        };
        let Ok(mut items) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) else {
            return Response::from_parts(parts, axum::body::Body::from(bytes));
        };
        let senders = path.ends_with("/senders");
        let asked: Vec<Option<(&str, usize, bool)>> = (value.as_array().into_iter().flatten())
            .map(|item| {
                let id = item.get("id")?.as_str()?;
                let index = *self.ids.get(&(senders, id.to_owned()))?;
                Some((id, index, item.get("params").is_some_and(cancels)))
            })
            .collect();
        for (n, index) in bulk_cancels(&asked, &items) {
            if let Err(e) = self.cancel(index).await {
                items[n]["code"] = StatusCode::SERVICE_UNAVAILABLE.as_u16().into();
                items[n]["error"] = e.into();
            }
        }
        parts.headers.remove(header::CONTENT_LENGTH);
        Response::from_parts(
            parts,
            axum::body::Body::from(serde_json::to_vec(&items).unwrap_or_default()),
        )
    }

    /// Cancels the scheduled activation the state holds for the port at
    /// `index`, if any: samplez-nmos has cancelled only its own. The error
    /// is the answer to give, as the activation stays in the state and
    /// would fire.
    async fn cancel(&self, index: usize) -> Result<(), String> {
        if self.scheduled(index).is_none() {
            return Ok(());
        }
        self.conns.unschedule(index).await.map_err(|Fail(e)| {
            tracing::warn!(port = index, "nmos: the cancel was not ordered: {e}");
            format!("the cancel was not taken by the set: {e}; send it again")
        })
    }

    /// `/active` shows the activation the state holds for the port, the
    /// replicated entry's, on every replica alike: not the one this
    /// replica's NMOS layer served, which takes its time from this
    /// replica's clock.
    async fn active_view(&self, path: &str, response: Response) -> Response {
        let Some(index) = self.index_of(path) else {
            return response;
        };
        if !response.status().is_success() {
            return response;
        }
        // A scheduled activation whose time has come shows as it fires
        // from that time on, as every replica's state holds it since it
        // was staged; the Fire that records it commits later.
        let port = self.ports[index].port_ref();
        let due = u64::try_from(SystemTai.now().as_nanos()).unwrap_or(u64::MAX);
        let shown = self.replica.upgrade().and_then(|r| {
            crate::app::with_state(&r, |sm| {
                let m = &sm.product;
                let fired = (m.scheduled().iter()).find(|s| s.port == port && s.at <= due);
                (fired.map(Scheduled::fired)).or_else(|| {
                    connection_of(m, &self.ports[index]).and_then(|c| c.activation.clone())
                })
            })
        });
        let Some(a) = shown else {
            return response;
        };
        with_json(response, |v| {
            v["activation"] = serde_json::json!({
                "mode": a.mode,
                "requested_time": a.requested,
                "activation_time": a.time,
            });
        })
        .await
    }

    /// The NMOS address of the set's leader when another replica leads;
    /// `Err` while the set has none.
    fn leader_nmos(&self) -> Result<Option<String>, ()> {
        let Some(replica) = self.replica.upgrade() else {
            return Err(());
        };
        let m = replica.raft().metrics().borrow().clone();
        let Some(leader) = m.current_leader else {
            return Err(());
        };
        if leader == replica.id() {
            return Ok(None);
        }
        let node = m
            .membership_config
            .membership()
            .get_node(&leader)
            .ok_or(())?;
        (self.peers.iter())
            .find(|(peer, _)| *peer == node.addr)
            .map(|(_, nmos)| Some(nmos.clone()))
            .ok_or(())
    }

    /// Orders [`Command::Fire`] for each scheduled activation whose time
    /// has come, while this replica leads; another leader fires those
    /// still pending. One that did not fire is tried again after
    /// [`FIRE_RETRY`] and logged once.
    async fn fire(&self) {
        let mut failed: BTreeMap<String, tokio::time::Instant> = BTreeMap::new();
        loop {
            let Some(replica) = self.replica.upgrade() else {
                return;
            };
            let due: Vec<Scheduled> = if leading(&replica) {
                let now = SystemTai.now().as_nanos();
                crate::app::with_state(&replica, |sm| {
                    (sm.product.scheduled().iter())
                        .filter(|s| u128::from(s.at) <= now)
                        .cloned()
                        .collect()
                })
            } else {
                Vec::new()
            };
            drop(replica);
            failed.retain(|id, _| due.iter().any(|s| s.id == *id));
            for s in due {
                let now = tokio::time::Instant::now();
                if failed.get(&s.id).is_some_and(|at| now < *at + FIRE_RETRY) {
                    continue;
                }
                match self.conns.fire(&s).await {
                    Ok(()) => {
                        failed.remove(&s.id);
                    }
                    Err(Fail(e)) => {
                        if failed.insert(s.id.clone(), now).is_none() {
                            tracing::warn!(
                                schedule = s.id,
                                "nmos: the scheduled activation did not fire: {e}"
                            );
                        }
                    }
                }
            }
            tokio::time::sleep(FIRE_TICK).await;
        }
    }

    /// Keeps each port's `/active` on the state's connection: an
    /// activation another replica took, a switch between a channel's
    /// inputs a and b, an activation of this replica that failed or that
    /// the state superseded or refused. A port is left alone while an
    /// activation of this replica is being applied on it, so an earlier
    /// one committing does not overwrite a later one `/active` shows,
    /// and while the state holds a scheduled activation for it whose time
    /// has come, which this replica's `/active` may show until it fires.
    async fn follow(&self) {
        let ports = &self.ports;
        let mut revision = 0;
        loop {
            let Some(replica) = self.replica.upgrade() else {
                return;
            };
            // Also when nothing is applied: a port left alone is looked
            // at again.
            let _ = tokio::time::timeout(FOLLOW_TICK, replica.applied(revision + 1)).await;
            if self.shared {
                revision = self.replicate(&replica, false);
                continue;
            }
            let (now, scheduled);
            (now, scheduled, revision) = crate::app::with_state(&replica, |sm| {
                // Only a schedule whose time has come holds a port: until
                // then the port shows the state's connection as any other.
                let due = u64::try_from(SystemTai.now().as_nanos()).unwrap_or(u64::MAX);
                let scheduled: Vec<PortRef> = sm
                    .product
                    .scheduled()
                    .iter()
                    .filter(|s| s.at <= due)
                    .map(|s| s.port.clone())
                    .collect();
                (actives(&sm.product, ports), scheduled, sm.revision)
            });
            drop(replica);
            let shown = self.nmos.status();
            for (index, active) in now.iter().enumerate() {
                let port = &shown.ports[index];
                if port.active == *active
                    || port.applied == Applied::Applying
                    || scheduled.contains(&ports[index].port_ref())
                {
                    continue;
                }
                let generation = self.nmos.external_change(port.port, active.clone());
                self.conns.saw(index, generation);
            }
        }
    }

    /// With shared versions, hands samplez-nmos each port's change in the
    /// state since the one it took last, as the replicated change it is,
    /// and at a takeover moves every version up to the state's. Returns
    /// the state's revision.
    fn replicate(&self, replica: &Replica<Mixer>, takeover: bool) -> u64 {
        let mut passed = self.passed.lock().expect("passed lock poisoned");
        let (changed, versions, revision) = crate::app::with_state(replica, |sm| {
            let m = &sm.product;
            let versions: Vec<_> = (self.ports.iter())
                .map(|p| m.port_change(&p.port_ref()).cloned())
                .collect();
            let changed: Vec<_> = (self.ports.iter().enumerate())
                .filter(|(i, _)| {
                    versions[*i]
                        .as_ref()
                        .is_some_and(|c| c.version > passed[*i])
                })
                .map(|(i, p)| {
                    let activation = connection_of(m, p).and_then(|c| c.activation.as_ref());
                    let change = Replicated {
                        active: actives(m, std::slice::from_ref(p)).remove(0),
                        activation: activation.and_then(nmos_activation),
                    };
                    (i, change)
                })
                .collect();
            (changed, versions, sm.revision)
        });
        for (index, change) in changed {
            let Some(made) = &versions[index] else {
                continue;
            };
            let port = self.conns.port(index);
            let version = Tai::from_nanos(u128::from(made.version));
            let served = self.conns.served(index, &made.id);
            let stamped = self.nmos.replicated_change(port, &change, version, served);
            if stamped.version != version {
                tracing::warn!(
                    port = index,
                    "nmos: version {} set where the state holds {version}",
                    stamped.version
                );
            }
            passed[index] = made.version;
            self.conns.saw(index, stamped.generation);
        }
        if takeover {
            let keys = self.ports.iter().map(Model::key);
            let held: BTreeMap<PortKey, Tai> = (keys.zip(&versions))
                .filter_map(|(k, c)| Some((k, Tai::from_nanos(u128::from(c.as_ref()?.version)))))
                .collect();
            self.nmos.set_versions(&held);
        }
        revision
    }

    /// Runs the registration while this replica leads; the new leader
    /// registers the same Node.
    async fn register(&self) {
        let Some(replica) = self.replica.upgrade() else {
            return;
        };
        let mut metrics = replica.raft().metrics();
        let id = replica.id();
        drop(replica);
        let alone = self.voters == 1;
        // A leader cut off from the others keeps its state until it hears
        // of a newer term; the quorum's acknowledgements stop at once.
        let leads = |m: &openraft::RaftMetrics<_, _>| {
            m.state == ServerState::Leader
                && m.current_leader == Some(id)
                && (alone
                    || m.millis_since_quorum_ack
                        .is_some_and(|ms| ms < QUORUM_FRESH))
        };
        loop {
            if !leads(&metrics.borrow_and_update()) {
                let changed = tokio::time::timeout(LEAD_CHECK, metrics.changed()).await;
                if matches!(changed, Ok(Err(_))) {
                    return;
                }
                continue;
            }
            tracing::info!("nmos: leading, registering the node");
            if self.shared
                && let Some(replica) = self.replica.upgrade()
            {
                self.replicate(&replica, true);
            }
            let lost = async {
                loop {
                    let changed = tokio::time::timeout(LEAD_CHECK, metrics.changed()).await;
                    if matches!(changed, Ok(Err(_))) || !leads(&metrics.borrow_and_update()) {
                        return;
                    }
                }
            };
            tokio::select! {
                () = self.nmos.registration() => return,
                () = lost => tracing::info!("nmos: no longer leading, the leader registers the node"),
            }
        }
    }
}

impl Drop for Node {
    /// Ends the cycle between the node and its [`Conns`], and the tasks
    /// that order activations or disable failed ones.
    fn drop(&mut self) {
        *self.conns.node.lock().expect("node lock poisoned") = None;
        self.conns
            .tasks
            .lock()
            .expect("tasks lock poisoned")
            .abort_all();
    }
}

/// Whether `replica` leads its set now.
fn leading(replica: &Replica<Mixer>) -> bool {
    let m = replica.raft().metrics().borrow().clone();
    m.state == ServerState::Leader && m.current_leader == Some(replica.id())
}

impl Model {
    fn key(&self) -> PortKey {
        match self {
            Model::Sender { output, .. } => PortKey::Sender(output.clone()),
            Model::Receiver { channel, .. } => PortKey::Receiver(channel.clone()),
        }
    }

    fn port_ref(&self) -> PortRef {
        match self {
            Model::Sender { output, .. } => PortRef::Sender(output.clone()),
            Model::Receiver { channel, .. } => PortRef::Receiver(channel.clone()),
        }
    }
}

/// Each port's `/active` as the state holds it, in the node's order: the
/// last activation, or for a port never activated, enabled on its own
/// domain and its flow, a receiver's being its channel's selected input.
fn actives(mixer: &Mixer, ports: &[Model]) -> Vec<Active> {
    ports
        .iter()
        .map(|p| match p {
            Model::Sender {
                output,
                domain,
                flow,
            } => match mixer.sender(output) {
                Some(c) => active(c),
                None => Active {
                    master_enable: true,
                    peer: None,
                    domain: Some(*domain),
                    flow: Some(*flow),
                },
            },
            Model::Receiver { channel, domain } => match mixer.receiver(channel) {
                Some((Some(c), _)) => active(c),
                Some((None, selected)) => Active {
                    master_enable: true,
                    peer: None,
                    domain: Some(*domain),
                    flow: Uuid::parse_str(selected).ok(),
                },
                None => Active {
                    master_enable: false,
                    peer: None,
                    domain: Some(*domain),
                    flow: None,
                },
            },
        })
        .collect()
}

fn active(c: &Connection) -> Active {
    let id = |v: &Option<String>| v.as_deref().and_then(|v| Uuid::parse_str(v).ok());
    Active {
        master_enable: c.master_enable,
        peer: id(&c.peer),
        domain: id(&c.domain),
        flow: id(&c.flow),
    }
}

/// The connection the state holds for port `p`, none before its first
/// activation.
fn connection_of<'m>(mixer: &'m Mixer, p: &Model) -> Option<&'m Connection> {
    match p {
        Model::Sender { output, .. } => mixer.sender(output),
        Model::Receiver { channel, .. } => mixer.receiver(channel).and_then(|(c, _)| c),
    }
}

/// A state's activation as samplez-nmos shows it; none for one it cannot
/// read.
fn nmos_activation(a: &Activation) -> Option<samplez_nmos::Activation> {
    let mode = match a.mode.as_str() {
        "activate_immediate" => ActivationMode::Immediate,
        "activate_scheduled_absolute" => ActivationMode::ScheduledAbsolute,
        "activate_scheduled_relative" => ActivationMode::ScheduledRelative,
        _ => return None,
    };
    Some(samplez_nmos::Activation {
        mode,
        requested: a.requested.clone(),
        time: a.time.parse().ok()?,
    })
}

/// An activation of samplez-nmos as the state keeps it.
fn state_activation(a: &samplez_nmos::Activation) -> Activation {
    let mode = match a.mode {
        ActivationMode::Immediate => "activate_immediate",
        ActivationMode::ScheduledAbsolute => "activate_scheduled_absolute",
        ActivationMode::ScheduledRelative => "activate_scheduled_relative",
    };
    Activation {
        mode: mode.into(),
        requested: a.requested.clone(),
        time: a.time.to_string(),
    }
}

fn connection(a: &Active, activation: Option<Activation>) -> Connection {
    let text = |v: Option<Uuid>| v.map(|u| u.to_string());
    Connection {
        master_enable: a.master_enable,
        peer: text(a.peer),
        domain: text(a.domain),
        flow: text(a.flow),
        activation,
    }
}

/// Each port's `/active` at start and the activation it shows, from the
/// state.
struct Loaded {
    active: BTreeMap<PortKey, Active>,
    activations: BTreeMap<PortKey, samplez_nmos::Activation>,
}

impl Store for Loaded {
    fn load(&self) -> BTreeMap<PortKey, Active> {
        self.active.clone()
    }

    fn activations(&self) -> BTreeMap<PortKey, samplez_nmos::Activation> {
        self.activations.clone()
    }
}

/// faderz's side of IS-05: checks a receiver's flow in its domain and
/// orders each activation through the replicated log.
struct Conns {
    /// Itself, for the task an activation is ordered on, which outlives
    /// its apply.
    me: Weak<Conns>,
    app: Arc<App>,
    ports: Arc<Vec<Model>>,
    resolver: Arc<Resolver>,
    /// The latest generation seen per port, by the node's order: an apply
    /// below it is stale.
    seen: Mutex<Vec<u64>>,
    /// Drawn at start: activations' command ids are unique across
    /// restarts.
    instance: String,
    /// Per port, the mode and requested time of the last scheduled
    /// activation staged here, until it is taken into the state with its
    /// schedule ([`Conns::schedule`]).
    asked: Mutex<Vec<Option<Activation>>>,
    /// The node, with shared versions, for the change an apply in flight
    /// makes (`samplez_nmos::Nmos::activation`); none otherwise and once
    /// the [`Node`] is dropped.
    node: Mutex<Option<samplez_nmos::Nmos>>,
    /// The tasks activations are ordered and failed ones disabled on,
    /// aborted when the [`Node`] is dropped.
    tasks: Mutex<tokio::task::JoinSet<()>>,
}

impl Conns {
    fn senders(&self) -> usize {
        (self.ports.iter())
            .filter(|p| matches!(p, Model::Sender { .. }))
            .count()
    }

    fn index(&self, port: Port) -> usize {
        match port {
            Port::Sender(i) => i,
            Port::Receiver(i) => self.senders() + i,
        }
    }

    /// The port at `index`, [`Conns::index`]'s inverse.
    fn port(&self, index: usize) -> Port {
        match index.checked_sub(self.senders()) {
            None => Port::Sender(index),
            Some(i) => Port::Receiver(i),
        }
    }

    /// The generation of this replica's apply that ordered command `id`
    /// on the port at `index`, from the id [`Conns::order`] gives it;
    /// none for another replica's or another command's.
    fn served(&self, index: usize, id: &str) -> Option<u64> {
        let rest = id.strip_prefix(&self.instance)?.strip_prefix('-')?;
        let (port, rest) = rest.split_once('-')?;
        if port.parse::<usize>().ok()? != index {
            return None;
        }
        let (generation, attempt) = rest.split_once('-')?;
        attempt.parse::<usize>().ok()?;
        generation.parse().ok()
    }

    /// Notes generation `g` of the port at `index`; whether it is not
    /// below one seen before.
    fn saw(&self, index: usize, g: u64) -> bool {
        let mut seen = self.seen.lock().expect("seen lock poisoned");
        let current = g >= seen[index];
        seen[index] = seen[index].max(g);
        current
    }
}

/// Reads flow definitions in the receivers' domains.
struct Resolver {
    /// For the check of a flow against the engine's outputs.
    engine: samplez::Config,
    /// Every flow the set's outputs write or merge into, all replicas'.
    own: BTreeSet<String>,
    api: Arc<MxlApi>,
    /// Each domain's directory by name.
    paths: BTreeMap<String, PathBuf>,
    /// An instance per domain, opened on the first check there.
    instances: Mutex<BTreeMap<String, Arc<Instance>>>,
}

impl Resolver {
    fn instance(&self, domain: &str) -> Result<Arc<Instance>, String> {
        let mut instances = self.instances.lock().expect("instances lock poisoned");
        if let Some(i) = instances.get(domain) {
            return Ok(i.clone());
        }
        let path = &self.paths[domain];
        let i = Instance::new(&self.api, path).map_err(|e| format!("domain {domain}: {e}"))?;
        instances.insert(domain.to_owned(), i.clone());
        Ok(i)
    }

    /// What receiver `slot`'s domain holds as `flow`.
    fn seen_flow(&self, slot: usize, flow: &str) -> Result<FlowSeen, Reject> {
        if self.own.contains(flow) {
            return Err(Reject(format!(
                "flow {flow} is written by this mixer's own outputs"
            )));
        }
        self.engine
            .check_input_flow(slot, flow)
            .map_err(|e| Reject(e.0))?;
        let domain = (self.engine.inputs[slot].domain.as_deref())
            .ok_or_else(|| Reject(format!("input {slot} reads no flow")))?;
        let instance = self.instance(domain).map_err(Reject)?;
        let fail = |e: samplez_mxl::Error| Reject(format!("flow {flow}: {e}"));
        if instance.is_flow_active(flow).map_err(fail)?.is_none() {
            return Ok(FlowSeen::Absent);
        }
        let def = instance.flow_def(flow).map_err(fail)?;
        flow_seen(&def).ok_or_else(|| Reject(format!("flow {flow}: no audio flow definition")))
    }
}

impl Conns {
    /// [`submit`] through this replica.
    async fn submit(&self, command: impl Fn(usize) -> Command) -> Result<(), Fail> {
        submit(&self.app, command).await
    }

    /// Orders `active` for the port at `index` with `activation`, an
    /// immediate one now when none, and waits until it is applied.
    async fn order(
        &self,
        index: usize,
        active: &Active,
        activation: Option<Activation>,
        generation: u64,
    ) -> Result<(), Fail> {
        let port = self.ports[index].port_ref();
        let activation = activation.unwrap_or_else(|| {
            let now = u64::try_from(SystemTai.now().as_nanos()).unwrap_or(u64::MAX);
            Activation {
                mode: "activate_immediate".into(),
                requested: None,
                time: tai_text(now),
            }
        });
        self.submit(|attempt| Command::Connect {
            id: format!("{}-{index}-{generation}-{attempt}", self.instance),
            port: port.clone(),
            active: connection(active, Some(activation.clone())),
        })
        .await
    }

    /// After an activation of the port at `index` failed, with shared
    /// versions: orders the port's connection in the state disabled, so
    /// every replica ends on `master_enable` false, as samplez-nmos
    /// settles a failed apply without shared versions, where with them it
    /// records the failure only. Again
    /// after [`FIRE_RETRY`] until the state holds it disabled or another
    /// change of the port came after the failed one, as when the set had
    /// no quorum.
    async fn disable(&self, index: usize, activation: Option<Activation>, generation: u64) {
        let port = self.ports[index].port_ref();
        let before = self
            .app
            .with_state(|sm| sm.product.port_change(&port).cloned());
        {
            let id = format!("{}-{index}-{generation}-off", self.instance);
            let mut logged = false;
            loop {
                let (current, change) = self.app.with_state(|sm| {
                    let current = actives(&sm.product, &self.ports[index..=index]).remove(0);
                    (current, sm.product.port_change(&port).cloned())
                });
                let later = change != before
                    && change.is_some_and(|c| self.served(index, &c.id) != Some(generation));
                if later || !current.master_enable {
                    return;
                }
                let off = Active {
                    master_enable: false,
                    ..current
                };
                let off = connection(&off, activation.clone());
                let ordered = submit(&self.app, |attempt| Command::Connect {
                    id: format!("{id}-{attempt}"),
                    port: port.clone(),
                    active: off.clone(),
                })
                .await;
                match ordered {
                    Ok(()) => return,
                    Err(Fail(e)) if !logged => {
                        logged = true;
                        tracing::warn!(
                            port = index,
                            "nmos: a failed activation is not disabled yet: {e}"
                        );
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(FIRE_RETRY).await;
            }
        }
    }

    /// Takes a scheduled activation for `at` into the state. samplez-nmos
    /// cancels one it applied by handing over, for the same `at`, the
    /// connection the port shows: one that is the state's connection, with
    /// no scheduled request staged here for the port, is a cancel of the
    /// port's scheduled activation when there is one and of nothing
    /// otherwise; any other, the state's connection with a new activation
    /// among them, is a schedule to take ([`scheduling`]).
    async fn schedule(
        &self,
        index: usize,
        active: &Active,
        at: Tai,
        generation: u64,
    ) -> Result<(), Fail> {
        let port = self.ports[index].port_ref();
        let at = u64::try_from(at.as_nanos())
            .map_err(|_| Fail("activation time out of range".into()))?;
        let (current, pending) = self.app.with_state(|sm| {
            let current = actives(&sm.product, &self.ports[index..=index]).remove(0);
            let pending = (sm.product.scheduled().iter())
                .find(|s| s.port == port)
                .map(|s| s.at);
            (current, pending)
        });
        let id = |attempt| format!("{}-{index}-{generation}-{attempt}", self.instance);
        let asked_here = self.asked.lock().expect("asked lock poisoned")[index].is_some();
        match scheduling(active, &current, pending, at, asked_here) {
            Scheduling::Nothing => Ok(()),
            Scheduling::Cancel => {
                self.submit(|attempt| Command::Unschedule {
                    id: id(attempt),
                    port: port.clone(),
                })
                .await
            }
            Scheduling::Take => {
                // The request staged here with it, so a fired one shows
                // the mode and requested time it was asked with. It is
                // kept until the schedule is state: samplez-nmos hands a
                // schedule over again when this fails.
                let staged = self.asked.lock().expect("asked lock poisoned")[index].clone();
                let asked = staged.clone().map(|a| Activation {
                    time: tai_text(at),
                    ..a
                });
                let taken = self
                    .submit(|attempt| Command::Schedule {
                        id: id(attempt),
                        port: port.clone(),
                        active: connection(active, asked.clone()),
                        at,
                    })
                    .await;
                if taken.is_ok() {
                    let mut now = self.asked.lock().expect("asked lock poisoned");
                    if now[index] == staged {
                        now[index] = None;
                    }
                }
                taken
            }
        }
    }

    /// Cancels the port's scheduled activation in the state, for a cancel
    /// that reached the leader: samplez-nmos hands one over only when it
    /// holds the activation itself.
    async fn unschedule(&self, index: usize) -> Result<(), Fail> {
        let port = self.ports[index].port_ref();
        let id = crate::app::uuid().map_err(|e| Fail(e.to_string()))?;
        self.submit(|attempt| Command::Unschedule {
            id: format!("{id}-{attempt}"),
            port: port.clone(),
        })
        .await
    }

    /// Orders the firing of scheduled activation `s`. Each try orders
    /// under ids of its own: under an earlier try's, the command dedup
    /// would answer with that try's refusal for as long as it keeps it,
    /// on whichever replica leads. A second leader's Fire of one already
    /// fired changes nothing.
    async fn fire(&self, s: &Scheduled) -> Result<(), Fail> {
        let id = crate::app::uuid().map_err(|e| Fail(e.to_string()))?;
        self.submit(|attempt| Command::Fire {
            id: format!("fire-{id}-{attempt}"),
            schedule: s.id.clone(),
        })
        .await
    }
}

/// Orders `command(attempt)` and waits until it is applied; a refusal
/// for being ordered off the generation path is sent again under a new
/// id, and while the replica has no room for another command in flight
/// it waits, up to shortly before the apply timeout.
async fn submit(app: &App, command: impl Fn(usize) -> Command) -> Result<(), Fail> {
    let owner = Owner {
        principal: SESSION.into(),
        session: SESSION.into(),
        role: Role::Operator,
        connection: SESSION.into(),
    };
    let session = SessionKey::from(&owner);
    let deadline = tokio::time::Instant::now()
        + samplez_nmos::DEFAULT_APPLY_TIMEOUT.saturating_sub(BUSY_MARGIN);
    for attempt in 0..RESENDS {
        let command = command(attempt);
        let id = command.id().to_owned();
        let body = serde_json::to_vec(&command).expect("a command serializes");
        let ack = loop {
            match app.commands.begin(&owner, &id) {
                Begin::Duplicate(ack) => break ack,
                Begin::Busy if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(FOLLOW_TICK).await;
                }
                Begin::Busy => return Err(Fail("too many commands in flight".into())),
                Begin::New(pending) => {
                    break (pending.submit(body.clone().into()).await)
                        .map_err(|e| Fail(format!("not applied: {e}")))?;
                }
            }
        };
        match app.answer(&session, &ack) {
            Some(Answer::Refused { .. }) if attempt + 1 < RESENDS => {}
            Some(
                Answer::Refused { message }
                | Answer::Invalid { message }
                | Answer::NotFound { message },
            ) => return Err(Fail(message)),
            _ => return Ok(()),
        }
    }
    unreachable!("the last attempt returns")
}

impl Connections for Conns {
    fn resolve(&self, port: Port, flow: Uuid) -> BoxFuture<'_, Result<FlowSeen, Reject>> {
        Box::pin(async move {
            let Port::Receiver(slot) = port else {
                return Ok(FlowSeen::Absent);
            };
            let flow = flow.to_string();
            let resolver = self.resolver.clone();
            // libmxl opens and reads files: off the runtime's threads.
            tokio::task::spawn_blocking(move || resolver.seen_flow(slot, &flow))
                .await
                .map_err(|e| Reject(format!("reading the flow: {e}")))?
        })
    }

    /// A scheduled activation is handed over when it is staged, so the
    /// set takes it into its state at once ([`Conns::schedule`]).
    fn lead(&self) -> Duration {
        Duration::MAX
    }

    fn apply(
        &self,
        port: Port,
        active: Active,
        at: Option<Tai>,
        generation: u64,
    ) -> BoxFuture<'_, Result<(), Fail>> {
        Box::pin(async move {
            let index = self.index(port);
            if !self.saw(index, generation) {
                return Ok(());
            }
            if let Some(at) = at {
                return self.schedule(index, &active, at, generation).await;
            }
            // With shared versions, the activation samplez-nmos shows for
            // this apply, so every replica's `/active` shows the same.
            let node = self.node.lock().expect("node lock poisoned").clone();
            let activation = (node.as_ref())
                .and_then(|n| n.activation(port, generation)?.activation)
                .map(|a| state_activation(&a));
            let Some(me) = self.me.upgrade().filter(|_| node.is_some()) else {
                return self.order(index, &active, None, generation).await;
            };
            // On a task of its own: samplez-nmos drops an apply at its
            // timeout, and a failed one still has to be disabled.
            let (tx, rx) = tokio::sync::oneshot::channel();
            {
                let mut tasks = self.tasks.lock().expect("tasks lock poisoned");
                while tasks.try_join_next().is_some() {}
                tasks.spawn(async move {
                    let ordered = me
                        .order(index, &active, activation.clone(), generation)
                        .await;
                    let failed = ordered.is_err();
                    let _ = tx.send(ordered);
                    if failed {
                        me.disable(index, activation, generation).await;
                    }
                });
            }
            rx.await
                .unwrap_or_else(|_| Err(Fail("the activation's task ended".into())))
        })
    }
}

/// The cancels of a `/bulk` request to take into the state, as (item,
/// port): of `asked`, each request item's id, port and whether it
/// cancels, and `answered`, the response's items in the same order, a
/// cancel answered 2xx whose port no later item names.
fn bulk_cancels(
    asked: &[Option<(&str, usize, bool)>],
    answered: &[serde_json::Value],
) -> Vec<(usize, usize)> {
    let mut taken = Vec::new();
    for (n, (item, asked_n)) in answered.iter().zip(asked).enumerate() {
        let Some((id, index, true)) = *asked_n else {
            continue;
        };
        let served = (item.get("code").and_then(serde_json::Value::as_u64))
            .is_some_and(|code| (200..300).contains(&code));
        let later = asked[n + 1..]
            .iter()
            .any(|a| a.is_some_and(|(_, i, _)| i == index));
        if served && !later && item.get("id").and_then(|v| v.as_str()) == Some(id) {
            taken.push((n, index));
        }
    }
    taken
}

/// What an apply samplez-nmos hands over for a scheduled time means.
#[derive(Debug, PartialEq, Eq)]
enum Scheduling {
    Take,
    Cancel,
    Nothing,
}

/// An apply of `active` for `at`, the port's connection in the state being
/// `current` and its scheduled activation in the state due at `pending`:
/// samplez-nmos cancels one it applied by handing over, for the same
/// time, the connection the port shows. One that carries the state's
/// connection, with no scheduled request staged here for the port
/// (`asked`), is a cancel of the port's scheduled activation for that
/// time when there is one, of nothing otherwise; any other is a
/// scheduled activation to take, the state's connection with a new
/// activation among them.
fn scheduling(
    active: &Active,
    current: &Active,
    pending: Option<u64>,
    at: u64,
    asked: bool,
) -> Scheduling {
    match (active == current && !asked, pending == Some(at)) {
        (true, true) => Scheduling::Cancel,
        (true, false) => Scheduling::Nothing,
        (false, _) => Scheduling::Take,
    }
}

/// Connection API requests on their way to samplez-nmos. `/staged` and
/// `/bulk` are served by the leader, which the other replicas forward them
/// to ([`Node::stage`]); 503 while the set has none, and for a request
/// marked forwarded that reaches a replica not leading. A GET of
/// `/active` of a port the state last changed shows the state's
/// activation ([`Node::active_view`]). Everything else is served where it
/// arrives.
async fn forward(State(node): State<Arc<Node>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().trim_end_matches('/').to_owned();
    let connection = path.starts_with("/x-nmos/connection/");
    let staging = connection && (path.ends_with("/staged") || path.contains("/bulk/"));
    let active = connection && path.ends_with("/active");
    if req.method() == axum::http::Method::OPTIONS || !(staging || active) {
        return next.run(req).await;
    }
    if active {
        let response = next.run(req).await;
        return node.active_view(&path, response).await;
    }
    if node.voters == 1 {
        return node.stage(req, next).await;
    }
    match node.leader_nmos() {
        Ok(None) => node.stage(req, next).await,
        Ok(Some(_)) if req.headers().contains_key(FORWARDED) => refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "forwarded to a replica that does not lead; try again",
        ),
        Ok(Some(leader)) => {
            let (parts, body) = req.into_parts();
            let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
                return refused(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the request body is too large",
                );
            };
            let limit = forward_timeout(&body);
            relay(&leader, parts, body, limit).await
        }
        Err(()) => refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "the set has no leader; try again",
        ),
    }
}

/// How long a forwarded request may take: the leader resolves each flow
/// it stages, each within [`samplez_nmos::RESOLVE_TIMEOUT`], one item of a
/// `/bulk` request after the other.
fn forward_timeout(body: &[u8]) -> Duration {
    let items = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.as_array().map(Vec::len))
        .unwrap_or(1)
        .max(1);
    samplez_nmos::RESOLVE_TIMEOUT * u32::try_from(items).unwrap_or(u32::MAX) + FORWARD_MARGIN
}

/// Sends a request to the leader's NMOS listener at `leader` and passes
/// its answer on: 502 when it cannot be reached, 504 when it does not
/// answer within `limit`.
async fn relay(
    leader: &str,
    parts: axum::http::request::Parts,
    body: axum::body::Bytes,
    limit: Duration,
) -> Response {
    match tokio::time::timeout(limit, proxy(leader, parts, body)).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => refused(
            StatusCode::BAD_GATEWAY,
            &format!("forwarding to the leader at {leader}: {e}"),
        ),
        Err(_) => refused(
            StatusCode::GATEWAY_TIMEOUT,
            &format!("the leader at {leader} did not answer"),
        ),
    }
}

/// A Connection API path's port: its kind and id.
fn port_of(path: &str) -> Option<(&str, &str)> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        [
            "x-nmos",
            "connection",
            _,
            "single",
            kind @ ("senders" | "receivers"),
            id,
            ..,
        ] => Some((kind, id)),
        _ => None,
    }
}

/// The mode and requested time a staging request's `params` schedule an
/// activation with; none for an immediate one, a cancel or none at all.
fn scheduled_request(params: &serde_json::Value) -> Option<Activation> {
    let activation = params.get("activation")?;
    let mode = activation.get("mode")?.as_str()?;
    if mode != "activate_scheduled_relative" && mode != "activate_scheduled_absolute" {
        return None;
    }
    Some(Activation {
        mode: mode.to_owned(),
        requested: activation
            .get("requested_time")?
            .as_str()
            .map(str::to_owned),
        time: String::new(),
    })
}

/// Whether a staging request's `activation` asks for a cancel: `mode`
/// given and null.
fn cancels(params: &serde_json::Value) -> bool {
    params
        .get("activation")
        .and_then(|a| a.get("mode"))
        .is_some_and(serde_json::Value::is_null)
}

fn refused(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "code": status.as_u16(), "error": message, "debug": null });
    let mut r = (status, body.to_string()).into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    r
}

/// Sends `req` to the NMOS listener at `addr` over HTTP/1.1 and returns
/// its answer.
async fn proxy(
    addr: &str,
    parts: axum::http::request::Parts,
    body: axum::body::Bytes,
) -> Result<Response, String> {
    let target = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{FORWARDED}: 1\r\n\
         Content-Length: {}\r\n",
        parts.method,
        body.len()
    );
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::ORIGIN] {
        if let Some(v) = parts.headers.get(&name).and_then(|v| v.to_str().ok()) {
            head += &format!("{name}: {v}\r\n");
        }
    }
    head += "\r\n";
    let mut tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| e.to_string())?;
    tcp.write_all(head.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    tcp.write_all(&body).await.map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    tcp.read_to_end(&mut answer)
        .await
        .map_err(|e| e.to_string())?;
    parse_response(&answer)
}

/// An HTTP/1.1 response read to its end, a chunked body decoded.
fn parse_response(answer: &[u8]) -> Result<Response, String> {
    let split = (answer.windows(4))
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header end")?;
    let head = std::str::from_utf8(&answer[..split]).map_err(|e| e.to_string())?;
    let mut body = answer[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = (lines.next().and_then(|l| l.split(' ').nth(1)))
        .and_then(|c| c.parse::<u16>().ok())
        .and_then(|c| StatusCode::from_u16(c).ok())
        .ok_or("no status line")?;
    let mut response = Response::builder().status(status);
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "connection" | "content-length" | "keep-alive" => {}
            _ => {
                let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                ) else {
                    continue;
                };
                response = response.header(n, v);
            }
        }
    }
    if chunked {
        body = dechunk(&body).ok_or("a malformed chunked body")?;
    }
    response
        .body(axum::body::Body::from(body))
        .map_err(|e| e.to_string())
}

fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&rest[..line]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[line + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

/// The namespace samplez-nmos derives resource ids under (UUIDv5 of the
/// seed, then of `sender/<key>` or `receiver/<key>`); a test checks the
/// ids against the Node API.
const ID_NAMESPACE: Uuid = samplez_nmos::uuid::uuid!("3bd5c2df-88d0-4122-8009-349861a360c8");

/// Each port's place by whether it is a sender and its IS-04 id.
fn port_ids(seed: &str, ports: &[Model]) -> BTreeMap<(bool, String), usize> {
    let base = Uuid::new_v5(&ID_NAMESPACE, seed.as_bytes());
    (ports.iter().enumerate())
        .map(|(index, p)| {
            let (sender, path) = match p {
                Model::Sender { output, .. } => (true, format!("sender/{output}")),
                Model::Receiver { channel, .. } => (false, format!("receiver/{channel}")),
            };
            (
                (sender, Uuid::new_v5(&base, path.as_bytes()).to_string()),
                index,
            )
        })
        .collect()
}

/// `response` with its JSON body changed by `change`; as it was when the
/// body is no JSON.
async fn with_json(response: Response, change: impl FnOnce(&mut serde_json::Value)) -> Response {
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return refused(StatusCode::INTERNAL_SERVER_ERROR, "an unreadable response");
    };
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    };
    change(&mut v);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(v.to_string()))
}

/// Channels, rate and sample format of an MXL flow definition.
fn flow_seen(def: &str) -> Option<FlowSeen> {
    let v: serde_json::Value = serde_json::from_str(def).ok()?;
    let rate = &v["sample_rate"];
    let numerator = rate["numerator"].as_u64()?;
    let denominator = rate["denominator"].as_u64().unwrap_or(1);
    Some(FlowSeen::Present {
        channels: u32::try_from(v["channel_count"].as_u64()?).ok()?,
        rate: u32::try_from(numerator.checked_div(denominator)?).ok()?,
        float32: v["media_type"] == "audio/float32"
            && v["bit_depth"].as_u64().is_none_or(|b| b == 32),
    })
}

/// Each domain's MXL id by name: from its `domain_def.json`, else from
/// `mxl.domains[].id`; both and different, or neither, is an error.
fn domain_ids(mxl: &Mxl) -> Result<BTreeMap<String, Uuid>, String> {
    let parse = |at: &str, v: &str| {
        Uuid::parse_str(v)
            .ok()
            .filter(|u| u.to_string() == v)
            .ok_or(format!("{at}: {v:?} is no lowercase UUID"))
    };
    let mut ids = BTreeMap::new();
    for d in &mxl.domains {
        let file = d.path.join(DOMAIN_DEF);
        let from_file = match std::fs::read(&file) {
            Ok(bytes) => {
                let at = file.display().to_string();
                let v: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| format!("{at}: {e}"))?;
                let id = v["id"].as_str().ok_or(format!("{at}: no id"))?;
                Some(parse(&at, id)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        let at = format!("mxl.domains[{}].id", d.name);
        let configured = d.id.as_deref().map(|v| parse(&at, v)).transpose()?;
        let id = match (from_file, configured) {
            (Some(f), Some(c)) if f != c => {
                return Err(format!(
                    "{at} {c} differs from the id {f} in {}",
                    file.display()
                ));
            }
            (Some(id), _) | (None, Some(id)) => id,
            (None, None) => {
                return Err(format!(
                    "{at}: required with nmos.enabled, as the domain has no {DOMAIN_DEF}"
                ));
            }
        };
        ids.insert(d.name.clone(), id);
    }
    Ok(ids)
}

/// The VSF TR-03 symbol of each leg of `layout`.
fn symbols(layout: &str, width: usize) -> Vec<&'static str> {
    let all: &[&str] = match layout {
        "mono" => &["M1"],
        "stereo" => &["L", "R"],
        "5.1" => &["L", "R", "C", "LFE", "Ls", "Rs"],
        _ => &["L", "R", "C", "LFE", "Ls", "Rs", "Lrs", "Rrs"],
    };
    all.iter().copied().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `/bulk` cancel is taken into the state only where samplez-nmos
    /// answered it 2xx and no later item of the request names its port,
    /// whose schedule the cancel would otherwise remove.
    #[test]
    fn a_bulk_cancel_is_taken_per_item() {
        let asked = [
            Some(("a", 0, true)),
            Some(("b", 1, true)),
            Some(("c", 2, true)),
            Some(("c", 2, false)),
            None,
            Some(("d", 3, false)),
        ];
        let answered = [
            serde_json::json!({ "id": "a", "code": 400 }),
            serde_json::json!({ "id": "b", "code": 200 }),
            serde_json::json!({ "id": "c", "code": 200 }),
            serde_json::json!({ "id": "c", "code": 202 }),
            serde_json::json!({ "id": "x", "code": 404 }),
            serde_json::json!({ "id": "d", "code": 202 }),
        ];
        assert_eq!(bulk_cancels(&asked, &answered), vec![(1, 1)]);
        // A response out of step with the request cancels nothing.
        let other = [serde_json::json!({ "id": "a", "code": 200 })];
        assert_eq!(bulk_cancels(&asked[1..2], &other), vec![]);
    }

    /// A staging request's scheduled activation is kept with the mode
    /// and requested time it was asked with; an immediate one, a cancel
    /// and a request without an activation keep nothing.
    #[test]
    fn a_scheduled_request_keeps_its_mode_and_requested_time() {
        let asked = |mode: serde_json::Value, requested: serde_json::Value| {
            scheduled_request(&serde_json::json!({
                "activation": { "mode": mode, "requested_time": requested }
            }))
        };
        assert_eq!(
            asked("activate_scheduled_relative".into(), "0:200000000".into()),
            Some(Activation {
                mode: "activate_scheduled_relative".into(),
                requested: Some("0:200000000".into()),
                time: String::new(),
            })
        );
        assert_eq!(
            asked("activate_scheduled_absolute".into(), "1700000000:0".into()).map(|a| a.mode),
            Some("activate_scheduled_absolute".into())
        );
        assert_eq!(
            asked("activate_immediate".into(), serde_json::Value::Null),
            None
        );
        assert_eq!(
            asked(serde_json::Value::Null, serde_json::Value::Null),
            None
        );
        assert_eq!(
            scheduled_request(&serde_json::json!({ "master_enable": true })),
            None
        );
    }

    #[test]
    fn a_flow_definition_reads_as_channels_rate_and_format() {
        let def = r#"{"format":"urn:x-nmos:format:audio","media_type":"audio/float32",
            "sample_rate":{"numerator":48000},"channel_count":2,"bit_depth":32}"#;
        let seen = flow_seen(def);
        assert_eq!(
            seen,
            Some(FlowSeen::Present {
                channels: 2,
                rate: 48_000,
                float32: true
            })
        );
        let other = def.replace("audio/float32", "audio/L24");
        assert!(matches!(
            flow_seen(&other),
            Some(FlowSeen::Present { float32: false, .. })
        ));
        assert_eq!(flow_seen(r#"{"format":"urn:x-nmos:format:video"}"#), None);
    }

    fn mxl(domains: serde_json::Value) -> Mxl {
        serde_json::from_value(serde_json::json!({
            "domains": domains, "inputs": {}, "outputs": {}
        }))
        .unwrap()
    }

    #[test]
    fn a_domain_id_comes_from_its_domain_def_else_the_configuration() {
        let dir = std::env::temp_dir().join(format!("faderz-nmos-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10";
        let b = "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c11";
        let d = |id: Option<&str>| serde_json::json!([{ "name": "d", "path": dir, "id": id }]);
        // Neither: refused, no id is guessed.
        assert!(domain_ids(&mxl(d(None))).unwrap_err().contains("required"));
        assert_eq!(domain_ids(&mxl(d(Some(a)))).unwrap()["d"].to_string(), a);
        assert!(domain_ids(&mxl(d(Some(&a.to_uppercase())))).is_err());
        std::fs::write(
            dir.join(DOMAIN_DEF),
            format!(r#"{{"id":"{a}","label":"x"}}"#),
        )
        .unwrap();
        assert_eq!(domain_ids(&mxl(d(None))).unwrap()["d"].to_string(), a);
        assert_eq!(domain_ids(&mxl(d(Some(a)))).unwrap()["d"].to_string(), a);
        assert!(
            domain_ids(&mxl(d(Some(b))))
                .unwrap_err()
                .contains("differs")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_forwarded_answer_is_passed_on_whole() {
        let plain = b"HTTP/1.1 423 Locked\r\ncontent-type: application/json\r\n\
            access-control-allow-origin: *\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        let r = parse_response(plain).unwrap();
        assert_eq!(r.status(), StatusCode::LOCKED);
        assert_eq!(r.headers()["access-control-allow-origin"], "*");
        assert!(r.headers().get("connection").is_none());
        let chunked = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n\
            3\r\n[1,\r\n2;x=y\r\n2]\r\n0\r\n\r\n";
        assert_eq!(
            dechunk(b"3\r\n[1,\r\n2\r\n2]\r\n0\r\n\r\n").unwrap(),
            b"[1,2]"
        );
        assert_eq!(parse_response(chunked).unwrap().status(), StatusCode::OK);
        assert!(dechunk(b"5\r\n[1").is_none());
        assert!(parse_response(b"garbage").is_err());
    }

    struct Nothing;

    impl Connections for Nothing {
        fn resolve(&self, _: Port, _: Uuid) -> BoxFuture<'_, Result<FlowSeen, Reject>> {
            Box::pin(async { Ok(FlowSeen::Absent) })
        }
        fn lead(&self) -> Duration {
            Duration::ZERO
        }
        fn apply(
            &self,
            _: Port,
            _: Active,
            _: Option<Tai>,
            _: u64,
        ) -> BoxFuture<'_, Result<(), Fail>> {
            Box::pin(async { Ok(()) })
        }
    }

    /// The ids staging requests are mapped to ports by are the ones the
    /// Node API serves.
    #[tokio::test]
    async fn port_ids_are_the_node_apis() {
        use tower::ServiceExt;
        let domain = samplez_nmos::uuid::uuid!("6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10");
        let flow = samplez_nmos::uuid::uuid!("0d7f6b8e-4a2c-4e8f-9b1a-2c3d4e5f6a01");
        let model = NodeModel {
            label: "x".into(),
            description: "x".into(),
            senders: vec![SenderDesc {
                key: "out-pgm".into(),
                label: "PGM".into(),
                channels: vec![ChannelDesc {
                    label: "L".into(),
                    symbol: Some("L".into()),
                }],
                group: "out-pgm".into(),
                domain,
                flow,
            }],
            receivers: vec![ReceiverDesc {
                key: "ch1".into(),
                label: "Mic".into(),
                channels: ChannelCount::AtLeast(1),
                group: "ch1".into(),
                domain,
                flow: None,
            }],
        };
        let cfg = NmosConfig {
            seed: "faderz".into(),
            advertise_host: "127.0.0.1".into(),
            advertise_port: 8081,
            asset: Asset {
                manufacturer: "m".into(),
                product: "p".into(),
                instance_id: "i".into(),
                functions: vec!["Mixer".into()],
            },
            heartbeat: Duration::from_secs(5),
            registries: Vec::new(),
            dns_sd_domain: None,
            continue_held: false,
        };
        let nmos = samplez_nmos::Nmos::new(
            model,
            cfg,
            Arc::new(Nothing),
            &samplez_nmos::NoStore,
            SystemTai,
        )
        .unwrap();
        let get = |path: &'static str| {
            let router = nmos.router();
            async move {
                let req = Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap();
                let body = router.oneshot(req).await.unwrap().into_body();
                let bytes = axum::body::to_bytes(body, MAX_BODY).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()[0]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
        };
        let ports = [
            Model::Sender {
                output: "out-pgm".into(),
                domain,
                flow,
            },
            Model::Receiver {
                channel: "ch1".into(),
                domain,
            },
        ];
        let ids = port_ids("faderz", &ports);
        assert_eq!(ids[&(true, get("/x-nmos/node/v1.3/senders/").await)], 0);
        assert_eq!(ids[&(false, get("/x-nmos/node/v1.3/receivers/").await)], 1);
    }

    /// samplez-nmos's cancel, the port's current connection for the
    /// scheduled time, is never taken as a schedule of its own, also
    /// when the state's scheduled activation was cancelled already; and
    /// another connection for the same time is a schedule.
    #[test]
    fn a_cancel_is_read_by_its_connection() {
        let flow = |n: u8| Uuid::parse_str(&format!("0d7f6b8e-4a2c-4e8f-9b1a-2c3d4e5f6a0{n}")).ok();
        let on = |n| Active {
            master_enable: true,
            peer: None,
            domain: None,
            flow: flow(n),
        };
        assert_eq!(
            scheduling(&on(1), &on(1), Some(5), 5, false),
            Scheduling::Cancel
        );
        assert_eq!(
            scheduling(&on(1), &on(1), None, 5, false),
            Scheduling::Nothing
        );
        assert_eq!(
            scheduling(&on(1), &on(1), Some(6), 5, false),
            Scheduling::Nothing
        );
        assert_eq!(
            scheduling(&on(2), &on(1), Some(5), 5, false),
            Scheduling::Take
        );
        assert_eq!(scheduling(&on(2), &on(1), None, 5, false), Scheduling::Take);
        // A scheduled request staged here for the connection as it is, a
        // controller moving the port's activation alone, is taken.
        assert_eq!(scheduling(&on(1), &on(1), None, 5, true), Scheduling::Take);
    }

    #[test]
    fn staging_paths_and_cancels_are_read() {
        assert_eq!(
            port_of("/x-nmos/connection/v1.2/single/receivers/abc/staged"),
            Some(("receivers", "abc"))
        );
        assert_eq!(port_of("/x-nmos/connection/v1.2/bulk/receivers"), None);
        assert!(cancels(
            &serde_json::json!({ "activation": { "mode": null } })
        ));
        assert!(!cancels(
            &serde_json::json!({ "activation": { "mode": "activate_immediate" } })
        ));
        assert!(!cancels(&serde_json::json!({ "master_enable": false })));
        let one = samplez_nmos::RESOLVE_TIMEOUT + FORWARD_MARGIN;
        assert_eq!(forward_timeout(b"{}"), one);
        assert_eq!(
            forward_timeout(b"[{},{},{}]"),
            samplez_nmos::RESOLVE_TIMEOUT * 3 + FORWARD_MARGIN
        );
    }

    fn parts() -> axum::http::request::Parts {
        let req = Request::builder()
            .method("PATCH")
            .uri("/x-nmos/connection/v1.2/single/receivers/x/staged")
            .body(())
            .unwrap();
        req.into_parts().0
    }

    /// A leader that cannot be reached is 502, one that does not answer
    /// in time 504, and its answer otherwise passes through.
    #[tokio::test]
    async fn a_forwarded_request_fails_as_the_leader_does() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = closed.local_addr().unwrap().to_string();
        drop(closed);
        let r = relay(
            &addr,
            parts(),
            axum::body::Bytes::new(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);

        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = silent.local_addr().unwrap().to_string();
        let held = tokio::spawn(async move {
            let (tcp, _) = silent.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(tcp);
        });
        let r = relay(
            &addr,
            parts(),
            axum::body::Bytes::new(),
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(r.status(), StatusCode::GATEWAY_TIMEOUT);
        held.abort();

        let answering = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = answering.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut tcp, _) = answering.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = tcp.read(&mut buf).await.unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(head.contains(&format!("{FORWARDED}: 1")), "{head}");
            tcp.write_all(b"HTTP/1.1 423 Locked\r\ncontent-length: 2\r\n\r\n{}")
                .await
                .unwrap();
        });
        let r = relay(
            &addr,
            parts(),
            axum::body::Bytes::from_static(b"{}"),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(r.status(), StatusCode::LOCKED);
    }

    #[test]
    fn every_layout_names_its_legs() {
        assert_eq!(symbols("mono", 1), ["M1"]);
        assert_eq!(symbols("stereo", 2), ["L", "R"]);
        assert_eq!(symbols("7.1", 8).len(), 8);
    }
}
