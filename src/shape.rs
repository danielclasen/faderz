//! The mixer's shape from the start-up configuration, in the contract's
//! `Shape` form, and the live state an empty start begins with.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shape {
    pub sample_rate: u32,
    pub frame_rate: FrameRate,
    pub channels: Vec<ChannelShape>,
    pub buses: Vec<BusShape>,
    pub auxes: Vec<ElementShape>,
    pub mix_minus: Vec<MixMinusShape>,
    pub vcas: Vec<VcaShape>,
    pub monitor_outputs: Vec<ElementShape>,
    pub outputs: Vec<OutputShape>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelShape {
    pub id: String,
    pub name: String,
    pub width: usize,
    pub layout: String,
    pub alt_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusShape {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub width: usize,
    pub layout: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementShape {
    pub id: String,
    pub name: String,
    pub width: usize,
    pub layout: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixMinusShape {
    pub id: String,
    pub name: String,
    pub bus: String,
    pub width: usize,
    pub layout: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VcaShape {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputShape {
    pub id: String,
    pub name: String,
    pub source: String,
    pub width: usize,
    pub layout: String,
}

fn layout_width(layout: &str) -> usize {
    match layout {
        "mono" => 1,
        "stereo" => 2,
        "5.1" => 6,
        _ => 8,
    }
}

impl Shape {
    /// Checks `value` against the contract's `Shape`, then what the schema
    /// cannot say: element ids are unique among channels, buses, auxes,
    /// mix-minus outputs, VCAs and monitor outputs (recall-safe groups and
    /// output sources name them alone) and among outputs; each width
    /// matches its layout; a mix-minus names a bus; an output names an
    /// element of its width; there is a bus, aux or mix-minus output for
    /// the monitor to listen to.
    pub fn parse(value: &Value) -> Result<Shape, String> {
        check(&contract().shape, value)?;
        let shape: Shape = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let mut ids = HashSet::new();
        let mut widths = Vec::new();
        for (id, width, layout) in shape.elements() {
            if !ids.insert(id) {
                return Err(format!("element id {id} is used twice"));
            }
            if let Some(layout) = layout
                && width != layout_width(layout)
            {
                return Err(format!("{id}: width {width} does not fit layout {layout}"));
            }
            widths.push((id, width));
        }
        for m in &shape.mix_minus {
            if !shape.buses.iter().any(|b| b.id == m.bus) {
                return Err(format!("mix-minus {} names no bus {}", m.id, m.bus));
            }
        }
        let mut outputs = HashSet::new();
        for o in &shape.outputs {
            if !outputs.insert(&o.id) {
                return Err(format!("output id {} is used twice", o.id));
            }
            if o.width != layout_width(&o.layout) {
                return Err(format!(
                    "output {}: width {} does not fit layout {}",
                    o.id, o.width, o.layout
                ));
            }
            match widths.iter().find(|(id, _)| *id == o.source) {
                None => return Err(format!("output {} names no element {}", o.id, o.source)),
                Some((_, w)) if *w != o.width => {
                    return Err(format!(
                        "output {} has width {}, its source {} {w}",
                        o.id, o.width, o.source
                    ));
                }
                Some(_) => {}
            }
        }
        if shape.buses.is_empty() {
            return Err("the shape needs a bus".into());
        }
        Ok(shape)
    }

    /// Id, width and layout of every element but outputs; VCAs carry no
    /// audio, so width 0 and no layout.
    fn elements(&self) -> impl Iterator<Item = (&str, usize, Option<&str>)> {
        let c = self
            .channels
            .iter()
            .map(|e| (e.id.as_str(), e.width, Some(e.layout.as_str())));
        let b = self
            .buses
            .iter()
            .map(|e| (e.id.as_str(), e.width, Some(e.layout.as_str())));
        let a = self
            .auxes
            .iter()
            .chain(&self.monitor_outputs)
            .map(|e| (e.id.as_str(), e.width, Some(e.layout.as_str())));
        let m = self
            .mix_minus
            .iter()
            .map(|e| (e.id.as_str(), e.width, Some(e.layout.as_str())));
        let v = self.vcas.iter().map(|e| (e.id.as_str(), 0, None));
        c.chain(b).chain(a).chain(m).chain(v)
    }

    /// Whether `id` names an element that may carry recall-safe groups.
    pub fn recallable(&self, id: &str) -> bool {
        self.channels.iter().any(|e| e.id == id)
            || self.buses.iter().any(|e| e.id == id)
            || self.auxes.iter().any(|e| e.id == id)
            || self.mix_minus.iter().any(|e| e.id == id)
            || self.vcas.iter().any(|e| e.id == id)
    }

    pub fn is_channel(&self, id: &str) -> bool {
        self.channels.iter().any(|c| c.id == id)
    }

    /// Bus, aux and mix-minus ids: what the monitor listens to and tone
    /// goes to.
    pub fn is_output_bus(&self, id: &str) -> bool {
        self.buses.iter().any(|e| e.id == id)
            || self.auxes.iter().any(|e| e.id == id)
            || self.mix_minus.iter().any(|e| e.id == id)
    }

    /// The live state of an empty start: input channels closed, buses at
    /// 0 dB, nothing muted, every channel routed to every bus, sends off,
    /// processing off, the monitor on the first bus, EBU R 68 and R 128
    /// metering, no scenes and no tally.
    pub fn initial_state(&self) -> Value {
        let eq = || {
            json!({ "enabled": false, "bands": [
                { "type": "lowShelf", "freq": 100, "gain": 0, "q": 0.7 },
                { "type": "peak", "freq": 400, "gain": 0, "q": 1 },
                { "type": "peak", "freq": 2500, "gain": 0, "q": 1 },
                { "type": "highShelf", "freq": 8000, "gain": 0, "q": 0.7 },
            ]})
        };
        let gate =
            json!({ "enabled": false, "threshold": -50, "range": 20, "attack": 1, "release": 200 });
        let comp = json!({ "enabled": false, "threshold": -18, "ratio": 3, "attack": 10, "release": 150, "makeup": 0 });
        let bus = |width: usize| {
            json!({
                "polarity": vec![false; width],
                "eq": eq(),
                "gate": gate,
                "comp": comp,
                "dynamicsBeforeEq": false,
                "limiter": { "enabled": false, "ceiling": -1 },
                "delay": 0,
                "fader": 0,
                "mute": false,
            })
        };
        let by_id =
            |items: Vec<(String, Value)>| Value::Object(items.into_iter().collect::<Map<_, _>>());
        let channels = by_id(
            self.channels
                .iter()
                .map(|c| {
                    let sends = by_id(
                        self.auxes
                            .iter()
                            .map(|a| {
                                (
                                    a.id.clone(),
                                    json!({ "on": false, "level": -90, "pan": 0, "pre": true }),
                                )
                            })
                            .collect(),
                    );
                    let routes = by_id(
                        self.buses
                            .iter()
                            .map(|b| (b.id.clone(), json!(true)))
                            .collect(),
                    );
                    let state = json!({
                        "input": "a",
                        "color": "none",
                        "trim": 0,
                        "legTrim": vec![0; c.width],
                        "polarity": vec![false; c.width],
                        "hpf": { "enabled": false, "freq": 80 },
                        "eq": eq(),
                        "gate": gate,
                        "comp": comp,
                        "dynamicsBeforeEq": false,
                        "delay": 0,
                        "pan": { "lr": 0, "fr": 0, "divergence": 0, "lfe": -90 },
                        "image": "normal",
                        "fader": -90,
                        "mute": false,
                        "solo": "off",
                        "meterTap": "preFader",
                        "directOut": { "tap": "preFader" },
                        "sends": sends,
                        "routes": routes,
                    });
                    (c.id.clone(), state)
                })
                .collect(),
        );
        let speakers = self.monitor_outputs.first().map_or(2, |m| m.width);
        json!({
            "channels": channels,
            "buses": by_id(self.buses.iter().map(|b| (b.id.clone(), bus(b.width))).collect()),
            "auxes": by_id(self.auxes.iter().map(|a| (a.id.clone(), bus(a.width))).collect()),
            "mixMinus": by_id(self.mix_minus.iter().map(|m| (m.id.clone(), json!({
                "exclude": [], "fader": 0, "mute": false,
                "interrupt": { "source": null, "active": false },
            }))).collect()),
            "vcas": by_id(self.vcas.iter().map(|v| (v.id.clone(), json!({
                "offset": 0, "mute": false, "members": [], "solo": "off",
            }))).collect()),
            "monitor": {
                "source": self.buses[0].id,
                "level": -10,
                "dim": false,
                "cut": false,
                "mono": false,
                "talkbackDim": true,
                "dimDepth": 20,
                "monoCompensation": 3,
                "downmix": "loRo",
                "image": "normal",
                "speakerMute": vec![false; speakers],
                "speakerSolo": vec![false; speakers],
            },
            "afv": by_id(self.channels.iter().map(|c| (c.id.clone(), json!({
                "enabled": false, "source": null, "riseMs": 40, "fallMs": 200,
                "holdMs": 0, "offLevel": -90, "open": true,
            }))).collect()),
            "tally": {},
            "recallSafe": {},
            "scenes": [],
            "consoles": [],
            "metering": {
                "zones": { "alignment": -18, "permittedMax": -9, "truePeakMax": -1 },
                "loudness": { "preset": "ebu", "target": -23, "tolerance": 1 },
                "scale": "ebu9",
            },
            "tone": { "on": false, "level": -18, "ident": false, "targets": [] },
            "sceneRecalled": null,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small shape: two mono channels and a stereo one, two buses, an
    /// aux, a mix-minus, a VCA and a monitor output.
    pub fn small() -> Value {
        json!({
            "sampleRate": 48000,
            "frameRate": { "numerator": 25, "denominator": 1 },
            "channels": [
                { "id": "ch1", "name": "Pres 1", "width": 1, "layout": "mono", "altInput": true },
                { "id": "ch2", "name": "Pres 2", "width": 1, "layout": "mono", "altInput": false },
                { "id": "st1", "name": "Music", "width": 2, "layout": "stereo", "altInput": false },
            ],
            "buses": [
                { "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" },
                { "id": "cf", "name": "Clean", "kind": "cleanFeed", "width": 2, "layout": "stereo" },
            ],
            "auxes": [{ "id": "aux1", "name": "IFB 1", "width": 1, "layout": "mono" }],
            "mixMinus": [{ "id": "mm1", "name": "N-1", "bus": "pgm", "width": 2, "layout": "stereo" }],
            "vcas": [{ "id": "vca1", "name": "PRES" }],
            "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
            "outputs": [{ "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" }],
        })
    }

    #[test]
    fn the_initial_state_matches_the_contract() {
        let shape = Shape::parse(&small()).unwrap();
        let state = shape.initial_state();
        check(&contract().state, &state).unwrap();
        assert_eq!(state["channels"]["st1"]["legTrim"], json!([0, 0]));
        assert_eq!(
            state["channels"]["ch1"]["sends"]["aux1"]["on"],
            json!(false)
        );
        assert_eq!(state["monitor"]["source"], json!("pgm"));
    }

    /// Each rule the schema cannot express refuses a shape breaking it.
    #[test]
    fn shapes_breaking_a_rule_are_refused() {
        let broken = |edit: fn(&mut Value), wanted: &str| {
            let mut v = small();
            edit(&mut v);
            let e = Shape::parse(&v).unwrap_err();
            assert!(e.contains(wanted), "{e}");
        };
        broken(|v| v["auxes"][0]["id"] = json!("ch1"), "used twice");
        broken(|v| v["vcas"][0]["id"] = json!("pgm"), "used twice");
        broken(|v| v["channels"][0]["width"] = json!(2), "does not fit");
        broken(|v| v["mixMinus"][0]["bus"] = json!("aux1"), "names no bus");
        broken(
            |v| v["outputs"][0]["source"] = json!("x"),
            "names no element",
        );
        broken(|v| v["outputs"][0]["source"] = json!("aux1"), "has width");
        broken(
            |v| {
                v["buses"] = json!([]);
                v["mixMinus"] = json!([]);
                v["outputs"] = json!([]);
            },
            "needs a bus",
        );
        broken(|v| v["sampleRate"] = json!(44100), "44100");
    }
}
