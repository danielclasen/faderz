//! Scenes: storing the part of the state a scene keeps, and recalling it.

use samplez_replica::entry::SessionKey;
use samplez_replica::state_machine::{Apply, Effect};
use serde_json::{Map, Value, json};

use super::control::changed;
use super::paths::{Leaves, leaves, tokens};
use super::validate::delayed_element;
use super::{Answer, Edits, Mixer};
use crate::duck;

impl Mixer {
    fn scene_list(&self) -> Vec<Value> {
        self.state["scenes"].as_array().cloned().unwrap_or_default()
    }

    /// The part of the state a scene stores: everything an operator sets
    /// on channels, buses, auxes, mix-minus outputs and VCAs, the AFV
    /// bindings without the stage tally drives, and the duck groups
    /// without their mode and hold.
    fn scene_data(&self) -> Value {
        let mut data = Map::new();
        for group in [
            "channels", "buses", "auxes", "mixMinus", "vcas", "afv", "ducks",
        ] {
            data.insert(group.into(), self.state[group].clone());
        }
        let drop = |data: &mut Map<String, Value>, group: &str, keys: &[&str]| {
            let elements = data.get_mut(group).and_then(Value::as_object_mut);
            for element in elements.into_iter().flat_map(|e| e.values_mut()) {
                for key in keys {
                    element.as_object_mut().map(|e| e.remove(*key));
                }
            }
        };
        drop(&mut data, "afv", &["open"]);
        drop(&mut data, "ducks", &["mode", "hold"]);
        Value::Object(data)
    }

    pub(super) fn store_scene(
        &mut self,
        session: &SessionKey,
        id: &str,
        scene: Option<&str>,
        name: Option<&str>,
        at: &str,
    ) -> Effect<Edits> {
        let mut list = self.scene_list();
        let (scene, name) = match scene {
            Some(scene) => match self.scene(scene) {
                Some(record) => (
                    scene.to_owned(),
                    name.unwrap_or_else(|| record["name"].as_str().unwrap_or(scene))
                        .to_owned(),
                ),
                None => {
                    let message = format!("unknown scene {scene}");
                    return self.refuse(session, id, Answer::NotFound { message });
                }
            },
            None => {
                let scene = loop {
                    self.scenes_created += 1;
                    let scene = format!("scene-{}", self.scenes_created);
                    if !self.scenes.contains_key(&scene) {
                        break scene;
                    }
                };
                (scene, name.unwrap_or_default().to_owned())
            }
        };
        let record = json!({ "id": scene, "name": name, "storedAt": at });
        match list.iter_mut().find(|s| s["id"] == scene.as_str()) {
            Some(s) => *s = record,
            None => list.push(record),
        }
        self.scenes.insert(scene.clone(), self.scene_data());
        self.record(session, id, Answer::Scene { scene });
        changed(self.write(vec![("/scenes".into(), Value::Array(list))]))
    }

    pub(super) fn delete_scene(
        &mut self,
        session: &SessionKey,
        id: &str,
        scene: &str,
    ) -> Effect<Edits> {
        if self.scenes.remove(scene).is_none() {
            let message = format!("unknown scene {scene}");
            return self.refuse(session, id, Answer::NotFound { message });
        }
        let mut list = self.scene_list();
        list.retain(|s| s["id"] != scene);
        let mut written = vec![("/scenes".to_owned(), Value::Array(list))];
        if self.state["sceneRecalled"]["id"] == scene {
            written.push(("/sceneRecalled".into(), Value::Null));
        }
        changed(self.write(written))
    }

    /// Recalls `scene`: every stored leaf whose recall-safe group its
    /// element does not keep, owned controls included, and
    /// `sceneRecalled`.
    pub(super) fn recall(
        &mut self,
        session: &SessionKey,
        id: &str,
        scene: &str,
        at: &str,
        ctx: &Apply,
    ) -> Effect<Edits> {
        let Some((mut written, delays)) = self.recall_writes(scene) else {
            let message = format!("unknown scene {scene}");
            return self.refuse(session, id, Answer::NotFound { message });
        };
        if let Some(answer) = self.reroute_refusal(ctx, &written) {
            return self.refuse(session, id, answer);
        }
        if !delays.is_empty() {
            self.record(session, id, Answer::Recalled { delays });
        }
        written.push(("/sceneRecalled".into(), json!({ "id": scene, "at": at })));
        changed(self.write(written))
    }

    /// The leaves a recall of `scene` writes, none for an unknown scene,
    /// and the channels whose stored delay their delay line no longer
    /// takes: those recall a delay of 0.
    pub(super) fn recall_writes(&self, scene: &str) -> Option<(Leaves, Vec<String>)> {
        let data = self.scenes.get(scene)?;
        let mut written = Vec::new();
        let mut short = Vec::new();
        for (path, mut v) in self.recalled(data) {
            let Some(current) = self.state.pointer(&path) else {
                continue;
            };
            // A scene keeps the ids and delays of the shape it was stored
            // in.
            if let Err(e) = self.check_references(&path, &v) {
                let Some(element) = delayed_element(&path) else {
                    continue;
                };
                tracing::warn!(scene, element, "recall sets the delay to 0: {e}");
                short.push(element);
                v = json!(0);
            }
            if *current != v {
                written.push((path, v));
            }
        }
        // With a channel added since the scene was stored, a group's
        // triggers can pass the limit: that group keeps its routing, as
        // if it were recall-safe, so its members and destinations stay a
        // pair an operator set.
        while let Some((group, e)) = self.broken_duck(&written) {
            tracing::warn!(
                scene,
                duck = group,
                "recall keeps the routing of the duck group: {e}"
            );
            written.retain(|(p, _)| duck::routing_of(p) != Some(group.as_str()));
        }
        Some((written, short))
    }

    /// The leaves of the scene `data` a recall writes: those of the groups
    /// recall-safe does not keep.
    fn recalled(&self, data: &Value) -> Vec<(String, Value)> {
        let mut written = Vec::new();
        for (group, elements) in data.as_object().into_iter().flatten() {
            for (element, value) in elements.as_object().into_iter().flatten() {
                let safe = &self.state["recallSafe"][element];
                // A scene stored before the compressor's mix ran the
                // compressor alone, and recalls it so.
                let mut value = value.clone();
                if let Some(comp) = value.get_mut("comp").and_then(Value::as_object_mut) {
                    comp.entry("mix").or_insert(json!(100));
                }
                for (path, v) in leaves(&value, &format!("/{group}/{element}")) {
                    let Some(g) = recall_group(&path) else {
                        continue;
                    };
                    if !safe.as_array().is_some_and(|s| s.iter().any(|x| x == g)) {
                        written.push((path, v));
                    }
                }
            }
        }
        written
    }
}

/// The recall-safe group a leaf of a scene belongs to; `None` for leaves
/// a recall never touches.
fn recall_group(path: &str) -> Option<&'static str> {
    let tokens = tokens(path);
    let group = tokens.first()?.as_str();
    let key = tokens.get(2).map(String::as_str);
    match group {
        "afv" => (key != Some("open")).then_some("afv"),
        "ducks" => duck::recall_group(tokens.get(2..)?),
        "mixMinus" => match key? {
            "exclude" => Some("routing"),
            "interrupt" => None,
            "eq" => Some("eq"),
            "gate" | "comp" | "dynamicsBeforeEq" => Some("dynamics"),
            "fader" => Some("fader"),
            "mute" => Some("mute"),
            _ => None,
        },
        "vcas" => match key? {
            "offset" => Some("fader"),
            "members" => Some("routing"),
            "solo" => None,
            _ => Some("mute"),
        },
        _ => match key? {
            "input" | "trim" | "legTrim" | "polarity" | "hpf" | "delay" => Some("input"),
            "eq" => Some("eq"),
            "gate" | "comp" | "dynamicsBeforeEq" | "limiter" => Some("dynamics"),
            "pan" | "image" => Some("pan"),
            "sends" => Some("sends"),
            "routes" | "directOut" => Some("routing"),
            "fader" => Some("fader"),
            "mute" => Some("mute"),
            _ => None,
        },
    }
}
