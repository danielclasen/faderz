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
//! switch between a channel's inputs a and b. Only the replica leading
//! the set registers the Node. `/staged` and scheduled activations are
//! held by the replica that took the request.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use openraft::ServerState;
use samplez_mxl::{Instance, MxlApi};
use samplez_nmos::uuid::Uuid;
use samplez_nmos::{
    Active, Applied, Asset, BoxFuture, ChannelCount, ChannelDesc, Connections, Fail, FlowSeen,
    NmosConfig, NodeModel, Port, PortKey, ReceiverDesc, Reject, SenderDesc, Store, SystemTai, Tai,
};
use samplez_replica::Replica;
use samplez_replica::entry::SessionKey;
use samplez_service::replica::Begin;
use samplez_service::{Metrics, Owner, ReplicatedState, Role};

use crate::app::App;
use crate::config::{Config, Mxl};
use crate::shape::Shape;
use crate::state::{Answer, Command, Connection, Mixer, PortRef};

/// The principal and session activations are ordered under: shorter than
/// any client session id, so no client shares it.
const SESSION: &str = "nmos";
/// How often an activation the state refused for being ordered off the
/// generation path is sent again.
const RESENDS: usize = 3;
/// How often a port's change in the state that waited on an activation
/// is looked at again.
const FOLLOW_TICK: Duration = Duration::from_millis(100);
/// The file in a domain directory BCP-007-03 names its identity in.
const DOMAIN_DEF: &str = "domain_def.json";

/// The node and what it needs to run.
pub struct Node {
    pub nmos: samplez_nmos::Nmos,
    conns: Arc<Conns>,
    /// Each port in the node's order, senders first.
    ports: Arc<Vec<Model>>,
    replica: Weak<Replica<Mixer>>,
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
        let (receivers, senders) = replica.store().with_state(|sm| {
            let m = &sm.product;
            let receivers = (shape.channels.iter().zip(&engine.inputs))
                .map(|(c, input)| {
                    let (_, selected) = m.receiver(&c.id).expect("every channel is wired");
                    let legs = mxl.inputs[&c.id].channels.as_deref();
                    let least = legs.and_then(|l| l.iter().max().copied());
                    Ok(ReceiverDesc {
                        key: c.id.clone(),
                        label: c.name.clone(),
                        channels: ChannelCount::AtLeast(least.unwrap_or(c.width as u32)),
                        group: c.id.clone(),
                        domain: domain_of(&input.domain),
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
        let api = samplez_mxl::load(&mxl.library_path).map_err(|e| format!("nmos: {e}"))?;
        let paths = mxl.domains.iter().map(|d| (d.name.clone(), d.path.clone()));
        let conns = Arc::new(Conns {
            app,
            ports: ports.clone(),
            resolver: Arc::new(Resolver {
                engine: engine.clone(),
                api,
                paths: paths.collect(),
                instances: Mutex::default(),
            }),
            seen: Mutex::new(vec![0; model.senders.len() + model.receivers.len()]),
            instance: crate::app::uuid().map_err(|e| format!("nmos: {e}"))?,
        });
        let shown = replica
            .store()
            .with_state(|sm| actives(&sm.product, &ports));
        let store = Loaded(ports.iter().map(Model::key).zip(shown).collect());
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
            dns_sd_domain: nmos.registry.dns_sd_domain.clone(),
        };
        let node = samplez_nmos::Nmos::new(model, cfg, conns.clone(), &store, SystemTai)
            .map_err(|e| e.to_string())?;
        node.register_metrics(metrics);
        Ok(Node {
            nmos: node,
            conns,
            ports,
            replica: Arc::downgrade(replica),
        })
    }

    /// Keeps each port's `/active` on the state, and the Node registered
    /// while this replica leads the set. Never returns while the replica
    /// runs.
    pub async fn run(&self) {
        tokio::join!(self.follow(), self.register());
    }

    /// Deletes the Node from its registry when this replica leads.
    pub async fn unregister(&self) {
        if self.replica.upgrade().is_some_and(|r| leading(&r)) {
            self.nmos.unregister().await;
        }
    }

    /// Shows in `/active` each change of a port's connection in the
    /// state that `/active` does not show yet: an activation another
    /// replica took, or a switch between a channel's inputs a and b. A
    /// change waits while an activation of this replica is being
    /// applied on the port, so an earlier one of its own committing does
    /// not overwrite a later one `/active` shows; the state's order
    /// decides once it is applied. A port whose activation failed keeps
    /// showing `master_enable` false until the state changes.
    async fn follow(&self) {
        let Some(replica) = self.replica.upgrade() else {
            return;
        };
        let ports = &self.ports;
        let (mut last, mut revision) = replica
            .store()
            .with_state(|sm| (actives(&sm.product, ports), sm.revision));
        drop(replica);
        let mut changed = vec![false; ports.len()];
        loop {
            let Some(replica) = self.replica.upgrade() else {
                return;
            };
            // Also when nothing is applied: a change waiting on an
            // activation shows once that is done.
            let _ = tokio::time::timeout(FOLLOW_TICK, replica.applied(revision + 1)).await;
            let now;
            (now, revision) = replica
                .store()
                .with_state(|sm| (actives(&sm.product, ports), sm.revision));
            drop(replica);
            let shown = self.nmos.status();
            for (index, (active, before)) in now.iter().zip(&last).enumerate() {
                changed[index] |= active != before;
                let port = &shown.ports[index];
                if !changed[index] || port.applied == Applied::Applying {
                    continue;
                }
                changed[index] = false;
                if port.active != *active {
                    let generation = self.nmos.external_change(port.port, active.clone());
                    self.conns.saw(index, generation);
                }
            }
            last = now;
        }
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
        let leads = |m: &openraft::RaftMetrics<_, _>| {
            m.state == ServerState::Leader && m.current_leader == Some(id)
        };
        loop {
            if !leads(&metrics.borrow_and_update()) {
                if metrics.changed().await.is_err() {
                    return;
                }
                continue;
            }
            tracing::info!("nmos: leading, registering the node");
            let lost = async {
                loop {
                    if metrics.changed().await.is_err() || !leads(&metrics.borrow_and_update()) {
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

fn connection(a: &Active) -> Connection {
    let text = |v: Option<Uuid>| v.map(|u| u.to_string());
    Connection {
        master_enable: a.master_enable,
        peer: text(a.peer),
        domain: text(a.domain),
        flow: text(a.flow),
    }
}

/// Each port's `/active` at start, from the state.
struct Loaded(BTreeMap<PortKey, Active>);

impl Store for Loaded {
    fn load(&self) -> BTreeMap<PortKey, Active> {
        self.0.clone()
    }
}

/// faderz's side of IS-05: checks a receiver's flow in its domain and
/// orders each activation through the replicated log.
struct Conns {
    app: Arc<App>,
    ports: Arc<Vec<Model>>,
    resolver: Arc<Resolver>,
    /// The latest generation seen per port, by the node's order: an apply
    /// below it is stale.
    seen: Mutex<Vec<u64>>,
    /// Drawn at start: activations' command ids are unique across
    /// restarts.
    instance: String,
}

impl Conns {
    fn index(&self, port: Port) -> usize {
        let senders = (self.ports.iter())
            .filter(|p| matches!(p, Model::Sender { .. }))
            .count();
        match port {
            Port::Sender(i) => i,
            Port::Receiver(i) => senders + i,
        }
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
        self.engine
            .check_input_flow(slot, flow)
            .map_err(|e| Reject(e.0))?;
        let domain = &self.engine.inputs[slot].domain;
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
    /// Orders `active` for the port at `index` and waits until it is
    /// applied; a refusal for being ordered off the generation path is
    /// sent again under a new id.
    async fn order(&self, index: usize, active: &Active, generation: u64) -> Result<(), Fail> {
        let owner = Owner {
            principal: SESSION.into(),
            session: SESSION.into(),
            role: Role::Operator,
            connection: SESSION.into(),
        };
        let session = SessionKey::from(&owner);
        for attempt in 0..RESENDS {
            let id = format!("{}-{index}-{generation}-{attempt}", self.instance);
            let command = Command::Connect {
                id: id.clone(),
                port: self.ports[index].port_ref(),
                active: connection(active),
            };
            let body = serde_json::to_vec(&command).expect("a command serializes");
            let ack = match self.app.commands.begin(&owner, &id) {
                Begin::Duplicate(ack) => ack,
                Begin::Busy => return Err(Fail("too many commands in flight".into())),
                Begin::New(pending) => (pending.submit(body.into()).await)
                    .map_err(|e| Fail(format!("not applied: {e}")))?,
            };
            match self.app.answer(&session, &ack) {
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

    /// An activation takes effect once ordered and applied: faderz cannot
    /// place it on a sample index ahead of time.
    fn lead(&self) -> Duration {
        Duration::ZERO
    }

    fn apply(
        &self,
        port: Port,
        active: Active,
        _at: Option<Tai>,
        generation: u64,
    ) -> BoxFuture<'_, Result<(), Fail>> {
        Box::pin(async move {
            let index = self.index(port);
            if !self.saw(index, generation) {
                return Ok(());
            }
            self.order(index, &active, generation).await
        })
    }
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
    fn every_layout_names_its_legs() {
        assert_eq!(symbols("mono", 1), ["M1"]);
        assert_eq!(symbols("stereo", 2), ["L", "R"]);
        assert_eq!(symbols("7.1", 8).len(), 8);
    }
}
