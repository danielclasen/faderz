//! Snapshots: the layout save writes and load reads, and a loaded state
//! fitted to the configured shape.

use std::collections::BTreeSet;
use std::sync::Arc;

use samplez_replica::entry::SessionKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::consoles::Registered;
use super::control::Holder;
use super::paths::{escape, leaves, set_at, tokens};
use super::validate::delayed_element;
use super::{Answer, Mixer, PortRef, Ports};
use crate::contract::{check, contract};
use crate::duck;
use crate::shape::Shape;

#[cfg(doc)]
use samplez_replica::state_machine::Product;

/// The layout [`Product::save`] writes. A snapshot carries it beside
/// the state, which binaries before it read from the top level: they
/// refuse it as missing their fields rather than load part of it. 3
/// adds the IS-05 connections ([`Ports`]), which a binary reading 2
/// would drop.
pub(super) const FORMAT: u32 = 3;

/// The layout before [`Ports`], read as one without connections.
pub(super) const FORMAT_WITHOUT_PORTS: u32 = 2;

/// A snapshot from [`FORMAT`] on.
#[derive(Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub(super) format: u32,
    pub(super) mixer: Saved,
}

/// The state as snapshots carry it.
#[derive(Serialize, Deserialize)]
pub(super) struct Saved {
    pub(super) shape: Shape,
    pub(super) state: Value,
    pub(super) owners: Vec<(String, Holder)>,
    pub(super) seqs: Vec<(SessionKey, Vec<(String, u64)>)>,
    pub(super) answers: Vec<(SessionKey, Vec<(String, Answer)>)>,
    pub(super) scenes: Vec<(String, Value)>,
    pub(super) scenes_created: u64,
    /// Absent in snapshots taken before consoles were kept.
    #[serde(default)]
    pub(super) consoles: Vec<Registered>,
    /// Absent before [`FORMAT`] 3.
    #[serde(default)]
    pub(super) ports: Ports,
    /// [`Mixer::fading`], each fade as the bits of `from`, `start` and
    /// `len`. Absent before recalls crossfaded.
    #[serde(default)]
    pub(super) fading: Vec<Vec<Option<(u32, u64, u32)>>>,
}

impl Mixer {
    /// Loads `saved` into this state's configured shape: leaves the shape
    /// has no place for, or whose value it does not take, are dropped;
    /// tally, scenes and the scene recalled are kept as they are.
    pub(super) fn reconcile(&mut self, saved: Saved) {
        let shape = self.shape.clone().expect("configured");
        let mut state = shape.initial_state();
        let mut dropped = Vec::new();
        let mut laid = BTreeSet::new();
        for (path, value) in leaves(&saved.state, "") {
            let group = tokens(&path).into_iter().next().unwrap_or_default();
            if matches!(
                group.as_str(),
                "tally" | "scenes" | "sceneRecalled" | "consoles"
            ) {
                continue;
            }
            let fits = match state.pointer(&path) {
                _ if group == "recallSafe" => {
                    tokens(&path).get(1).is_some_and(|id| shape.recallable(id))
                }
                // The schema types the value by its path, a nullable one
                // included.
                Some(_) => {
                    check(
                        &contract().state_change,
                        &json!({ "path": path, "value": value }),
                    )
                    .is_ok()
                        && self.check_references(&path, &value).is_ok()
                }
                None => false,
            };
            if fits {
                set_at(&mut state, &path, value);
                laid.insert(path);
            } else {
                if let Some(element) = delayed_element(&path) {
                    tracing::warn!(element, %value, "the delay line no longer takes the saved delay: 0");
                }
                dropped.push(path);
            }
        }
        // A seed reaches only the channels the saved state lacks, so with
        // the saved triggers it can pass a group's limit: those seeded
        // triggers go, the last channel in shape order first, until the
        // group keeps the rules a write is held to.
        for d in &shape.ducks {
            for c in shape.channels.iter().rev() {
                if duck::check(&state["ducks"], &d.id).is_ok() {
                    break;
                }
                let path = format!(
                    "/ducks/{}/channels/{}/trigger",
                    escape(&d.id),
                    escape(&c.id)
                );
                if !laid.contains(&path) && state.pointer(&path) == Some(&json!(true)) {
                    tracing::warn!(
                        duck = d.id,
                        channel = c.id,
                        "the loaded state keeps the group's triggers: the seed goes"
                    );
                    set_at(&mut state, &path, json!(false));
                    dropped.push(path);
                }
            }
        }
        for group in ["tally", "scenes", "sceneRecalled"] {
            state[group] = saved.state[group].clone();
        }
        self.state = Arc::new(state);
        self.owners = saved
            .owners
            .into_iter()
            .filter(|(p, _)| self.exists(p))
            .collect();
        self.dropped = dropped;
        self.merge_consoles(saved.consoles);
        // Connections of ports the configuration no longer has go.
        let mut ports = saved.ports;
        ports.receivers.retain(|id, _| shape.is_channel(id));
        ports.legs.retain(|id, _| shape.is_channel(id));
        // Leg flows activated over a configuration that wired the channel
        // otherwise are no longer what the operator configured.
        let wired = |id: &String| {
            let slot = shape.channels.iter().position(|c| c.id == *id)?;
            self.wiring.get(slot).map(|w| (w.a.clone(), w.b.clone()))
        };
        let stale: Vec<String> = (ports.legs.iter())
            .filter(|(id, l)| l.wired.is_some() && l.wired != wired(id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            ports.legs.remove(&id);
            ports.receivers.remove(&id);
        }
        let own_port = |p: &PortRef| match p {
            PortRef::Receiver(c) => shape.is_channel(c),
            PortRef::Sender(o) => shape.outputs.iter().any(|x| x.id == *o),
        };
        ports.scheduled.retain(|s| own_port(&s.port));
        ports
            .senders
            .retain(|id, _| shape.outputs.iter().any(|o| o.id == *id));
        self.ports = ports;
    }
}

/// The snapshot message's `shape`, `state` and `owners`.
pub(super) fn snapshot_text(shape: &Shape, state: &Value, owners: &Value) -> String {
    json!({ "shape": shape, "state": state, "owners": owners }).to_string()
}

/// A snapshot an older binary saved has no surround position in the pan
/// of its channels: the initial one fills it in, as a configured state
/// does ([`Mixer::reconcile`]), so a write can reach it.
pub(super) fn fill_pan(state: &mut Value) {
    let channels = state["channels"].as_object_mut().into_iter().flatten();
    for pan in channels.filter_map(|(_, c)| c.get_mut("pan")?.as_object_mut()) {
        for (leaf, initial) in [("frontRear", -1), ("spread", 1), ("lfeSend", -90)] {
            pan.entry(leaf).or_insert(json!(initial));
        }
    }
}

/// A snapshot an older binary saved has no EQ or dynamics in its
/// mix-minus outputs: the initial ones fill them in, off, as a
/// configured state does ([`Mixer::reconcile`]), so a write can reach
/// them and every output strip has its parameters.
pub(super) fn fill_mix_minus(state: &mut Value) {
    let processing = crate::shape::initial_processing();
    let elements = state["mixMinus"].as_object_mut().into_iter().flatten();
    for m in elements.filter_map(|(_, m)| m.as_object_mut()) {
        for (leaf, initial) in processing.as_object().into_iter().flatten() {
            m.entry(leaf).or_insert(initial.clone());
        }
    }
}

/// A snapshot an older binary saved has no mix in its compressors: they
/// ran the compressor alone, a mix of 100 %.
pub(super) fn fill_comp_mix(state: &mut Value) {
    for group in ["channels", "buses", "auxes", "mixMinus"] {
        let elements = state[group].as_object_mut().into_iter().flatten();
        for comp in elements.filter_map(|(_, e)| e.get_mut("comp")?.as_object_mut()) {
            comp.entry("mix").or_insert(json!(100));
        }
    }
}
