//! The mixer's shape from the start-up configuration, in the contract's
//! `Shape` form, and the live state an empty start begins with.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::contract::{check, contract};
use crate::duck;

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
    /// Absent in configurations without duck groups, and left out of a
    /// serialized shape without them, so a binary that has no duck groups
    /// loads its snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ducks: Vec<DuckShape>,
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
    /// The longest delay the channel's input delay line takes, in
    /// samples, from `mxl.inputs.<id>.maxDelayMs`; none without a line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<u32>,
    /// The line-up tone generator the channel's input is, from
    /// `mxl.inputs.<id>.tone`; none for a channel reading a flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<ChannelTone>,
}

/// A tone channel's generator as configured: the frequency, Hz, and the
/// level, dBFS sample peak, it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelTone {
    pub frequency: u32,
    pub level: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusShape {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub width: usize,
    pub layout: String,
    /// The longest delay the element's output delay line takes, in
    /// samples, from `mxl.outputs.<id>.maxDelayMs` of the outputs
    /// carrying it; none without a line.
    #[serde(default, rename = "maxDelay", skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementShape {
    pub id: String,
    pub name: String,
    pub width: usize,
    pub layout: String,
    /// The longest delay an aux's output delay line takes, in
    /// samples, from `mxl.outputs.<id>.maxDelayMs` of the outputs
    /// carrying it; none without a line.
    #[serde(default, rename = "maxDelay", skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<u32>,
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
pub struct DuckShape {
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

/// The loudness role of each leg of `layout` as samplez names them
/// (BS.1770-5 Table 3 weights); `None` for 7.1, whose rear legs samplez
/// has no names for, so every leg weighs 1.
pub fn loudness_channels(layout: &str) -> Option<&'static [&'static str]> {
    match layout {
        "mono" => Some(&["C"]),
        "stereo" => Some(&["L", "R"]),
        "5.1" => Some(&["L", "R", "C", "LFE", "Ls", "Rs"]),
        _ => None,
    }
}

fn layout_width(layout: &str) -> usize {
    match layout {
        "mono" => 1,
        "stereo" => 2,
        "5.1" => 6,
        _ => 8,
    }
}

/// The EQ, gate and compressor of an empty start, off and flat, and
/// their default order: those of every channel, bus, aux and mix-minus
/// output.
pub fn initial_processing() -> Value {
    json!({
        "eq": { "enabled": false, "bands": [
            { "type": "lowShelf", "freq": 100, "gain": 0, "q": 0.7 },
            { "type": "peak", "freq": 400, "gain": 0, "q": 1 },
            { "type": "peak", "freq": 2500, "gain": 0, "q": 1 },
            { "type": "highShelf", "freq": 8000, "gain": 0, "q": 0.7 },
        ]},
        "gate": { "enabled": false, "threshold": -50, "range": 20, "attack": 1, "release": 200 },
        "comp": { "enabled": false, "threshold": -18, "ratio": 3, "attack": 10, "release": 150, "makeup": 0, "mix": 100 },
        "dynamicsBeforeEq": false,
    })
}

impl Shape {
    /// Checks `value` against the contract's `Shape`, then what the schema
    /// cannot say: element ids are unique among channels, buses, auxes,
    /// mix-minus outputs, VCAs, duck groups and monitor outputs
    /// (recall-safe groups and output sources name them alone) and among outputs; each width
    /// matches its layout; a mix-minus names a bus; an output names an
    /// element at least as wide, which it folds down to its own width; there is a bus, aux or mix-minus output for
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
                Some((_, w)) if o.width > *w => {
                    return Err(format!(
                        "output {} has width {}, wider than its source {} at {w}",
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

    /// Id, width and layout of every element but outputs; VCAs and duck
    /// groups carry no audio, so width 0 and no layout.
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
        let d = self.ducks.iter().map(|e| (e.id.as_str(), 0, None));
        c.chain(b).chain(a).chain(m).chain(v).chain(d)
    }

    /// Id, width and layout of each bus, aux and mix-minus output no
    /// output carries at its full width, then of every monitor output, in
    /// meter frame order: the engine's meters, computed on matrix columns
    /// of their own. An output narrower than its source folds it down and
    /// meters only itself. A monitor output's meter reads it before its
    /// level, dim, cut, mono and speaker mutes ([`crate::mix`]), which an
    /// output carrying it applies.
    pub fn meter_columns(&self) -> Vec<(&str, usize, &str)> {
        let b = self.buses.iter().map(|e| (&e.id, e.width, &e.layout));
        let a = self.auxes.iter().map(|e| (&e.id, e.width, &e.layout));
        let m = self.mix_minus.iter().map(|e| (&e.id, e.width, &e.layout));
        let o = self
            .monitor_outputs
            .iter()
            .map(|e| (&e.id, e.width, &e.layout));
        b.chain(a)
            .chain(m)
            .filter(|(id, w, _)| self.full_output(id, *w).is_none())
            .chain(o)
            .map(|(id, w, l)| (id.as_str(), w, l.as_str()))
            .collect()
    }

    /// The first output carrying element `id`, `width` legs wide, at its
    /// full width: the output whose meters are the element's.
    pub fn full_output(&self, id: &str, width: usize) -> Option<usize> {
        (self.outputs.iter()).position(|o| o.source == id && o.width == width)
    }

    /// Whether `id` names an element that may carry recall-safe groups.
    pub fn recallable(&self, id: &str) -> bool {
        self.channels.iter().any(|e| e.id == id)
            || self.buses.iter().any(|e| e.id == id)
            || self.auxes.iter().any(|e| e.id == id)
            || self.mix_minus.iter().any(|e| e.id == id)
            || self.vcas.iter().any(|e| e.id == id)
            || self.ducks.iter().any(|e| e.id == id)
    }

    pub fn is_channel(&self, id: &str) -> bool {
        self.channels.iter().any(|c| c.id == id)
    }

    /// Whether an output carries channel `id`: its direct out.
    pub fn has_direct_out(&self, id: &str) -> bool {
        self.outputs.iter().any(|o| o.source == id)
    }

    /// Bus, aux and mix-minus ids: what the monitor listens to and tone
    /// goes to.
    pub fn is_output_bus(&self, id: &str) -> bool {
        self.buses.iter().any(|e| e.id == id)
            || self.auxes.iter().any(|e| e.id == id)
            || self.mix_minus.iter().any(|e| e.id == id)
    }

    /// Whether `id` names a bus or an aux, the elements whose state holds a
    /// limiter: an output carrying one has the engine's limiter.
    pub fn has_limiter(&self, id: &str) -> bool {
        self.buses.iter().any(|e| e.id == id) || self.auxes.iter().any(|e| e.id == id)
    }

    /// The delay line of channel, bus or aux `id`, in samples: none for
    /// one without a line or any other element.
    pub fn delay_line(&self, id: &str) -> Option<u32> {
        let channel = self
            .channels
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.max_delay);
        let bus = self.buses.iter().find(|e| e.id == id).map(|e| e.max_delay);
        let aux = self.auxes.iter().find(|e| e.id == id).map(|e| e.max_delay);
        channel.or(bus).or(aux).flatten()
    }

    /// Whether `id` names a bus, an aux or a mix-minus output, the
    /// elements whose state holds EQ and dynamics: an output carrying one
    /// has the engine's output strip.
    pub fn has_strip(&self, id: &str) -> bool {
        self.has_limiter(id) || self.mix_minus.iter().any(|e| e.id == id)
    }

    /// The live state of an empty start: input channels closed, a tone
    /// channel's generator as configured with no ident, buses at 0 dB,
    /// nothing muted, every channel routed to every bus, sends off,
    /// processing off, the monitor on the first bus, EBU R 68 and R 128
    /// metering with every loudness measurement running, duck groups as
    /// [`duck::initial_state`] sets them, no scenes and no tally.
    pub fn initial_state(&self) -> Value {
        let processing = initial_processing();
        let (eq, gate, comp) = (
            || processing["eq"].clone(),
            &processing["gate"],
            &processing["comp"],
        );
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
                    let mut state = json!({
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
                        "pan": {
                            "lr": 0, "fr": 0, "divergence": 0, "lfe": -90,
                            "frontRear": -1, "spread": 1, "lfeSend": -90,
                        },
                        "image": "normal",
                        "fader": -90,
                        "mute": false,
                        "solo": "off",
                        // The meter shows what the fader lets through,
                        // as operators read a channel meter; the direct
                        // out stays pre-fader, so a closed fader does not
                        // silence an ISO record or IFB feed.
                        "meterTap": "postFader",
                        "directOut": { "tap": "preFader" },
                        "sends": sends,
                        "routes": routes,
                    });
                    if let Some(t) = c.tone {
                        state["tone"] =
                            json!({ "frequency": t.frequency, "level": t.level, "ident": false });
                    }
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
                "eq": eq(), "gate": gate, "comp": comp, "dynamicsBeforeEq": false,
            }))).collect()),
            "vcas": by_id(self.vcas.iter().map(|v| (v.id.clone(), json!({
                "offset": 0, "mute": false, "members": [], "solo": "off",
            }))).collect()),
            "ducks": duck::initial_state(self),
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
            "loudness": by_id(
                self.buses.iter().map(|e| &e.id)
                    .chain(self.auxes.iter().map(|e| &e.id))
                    .chain(self.mix_minus.iter().map(|e| &e.id))
                    .map(|id| (id.clone(), json!({ "running": true, "resets": 0 })))
                    .collect(),
            ),
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
            "ducks": [{ "id": "comm", "name": "Commentary" }],
            "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
            "outputs": [{ "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" }],
        })
    }

    /// [`small`] with a 5.1 channel on a 5.1 programme bus, which a 6-leg
    /// output and a stereo one carry.
    pub fn surround() -> Value {
        let mut v = small();
        v["channels"].as_array_mut().unwrap().push(
            json!({ "id": "sur", "name": "Surround", "width": 6, "layout": "5.1", "altInput": false }),
        );
        v["buses"][0] =
            json!({ "id": "pgm", "name": "PGM", "kind": "program", "width": 6, "layout": "5.1" });
        v["mixMinus"] = json!([]);
        v["outputs"] = json!([
            { "id": "out-51", "name": "PGM 5.1", "source": "pgm", "width": 6, "layout": "5.1" },
            { "id": "out-lr", "name": "PGM", "source": "pgm", "width": 2, "layout": "stereo" },
        ]);
        v
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
        // A closed fader closes the channel meter; the direct out does not.
        assert_eq!(state["channels"]["ch1"]["meterTap"], json!("postFader"));
        assert_eq!(state["channels"]["st1"]["meterTap"], json!("postFader"));
        assert_eq!(
            state["channels"]["ch1"]["directOut"]["tap"],
            json!("preFader")
        );
        let measured: Vec<&String> = state["loudness"].as_object().unwrap().keys().collect();
        assert_eq!(
            measured,
            ["aux1", "cf", "mm1", "pgm"],
            "every bus, aux and mix-minus output"
        );
        assert_eq!(
            state["loudness"]["pgm"],
            json!({ "running": true, "resets": 0 })
        );
    }

    /// A duck group starts idle on the programme buses alone: the clean
    /// feed, auxes and mix-minus outputs stay free of it, no channel keys
    /// or is ducked, and the detector is at samplez-dsp's defaults.
    #[test]
    fn a_duck_group_starts_on_the_programme_buses_with_no_members() {
        let state = Shape::parse(&small()).unwrap().initial_state();
        let comm = &state["ducks"]["comm"];
        assert_eq!(
            comm["destinations"],
            json!({ "pgm": true, "cf": false, "aux1": false, "mm1": false })
        );
        for channel in ["ch1", "ch2", "st1"] {
            assert_eq!(
                comm["channels"][channel],
                json!({ "trigger": false, "target": false, "depth": 10 })
            );
        }
        assert_eq!(
            (&comm["mode"], &comm["hold"]),
            (&json!("auto"), &json!(false))
        );
        assert_eq!(comm["keyTap"], "preFaderOpen");
        assert_eq!(
            [
                &comm["threshold"],
                &comm["hysteresis"],
                &comm["attackMs"],
                &comm["holdMs"],
                &comm["releaseMs"]
            ],
            [
                &json!(-30),
                &json!(4),
                &json!(20),
                &json!(500),
                &json!(1000)
            ]
        );
        assert_eq!(comm["keyHpf"], json!({ "enabled": true, "freq": 120 }));
        assert_eq!(comm["keyLpf"], json!({ "enabled": false, "freq": 4000 }));
    }

    /// A configuration without duck groups has none, and its serialized
    /// shape leaves them out.
    #[test]
    fn duck_groups_are_optional() {
        let mut v = small();
        v.as_object_mut().unwrap().remove("ducks");
        let shape = Shape::parse(&v).unwrap();
        assert!(shape.ducks.is_empty());
        let serialized = serde_json::to_value(&shape).unwrap();
        assert!(serialized.get("ducks").is_none(), "{serialized}");
        assert_eq!(shape.initial_state()["ducks"], json!({}));
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
        broken(|v| v["ducks"][0]["id"] = json!("vca1"), "used twice");
        broken(
            |v| {
                let nine = (1..=9).map(|n| json!({ "id": format!("d{n}"), "name": "D" }));
                v["ducks"] = nine.collect();
            },
            "/ducks",
        );
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
