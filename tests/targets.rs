//! The cost of deriving engine entries at the largest shape an engine
//! at a block of 48 samples takes: [`Product::targets`] after a command
//! and the diff to the targets before it, which a set of several replicas
//! runs under its store's lock for every applied entry.

use std::time::{Duration, Instant};

use faderz::mix::Wiring;
use faderz::shape::Shape;
use faderz::state::{Action, Command, Control, Mixer};
use samplez_replica::entry::Origin;
use samplez_replica::state_machine::{Apply, Effect, Product};
use samplez_service::Role;
use serde_json::{Value, json};

/// Coefficients an engine at a block of 48 sets in one block
/// (`samplez::config::COEFFICIENTS_PER_BLOCK_SAMPLE` times 48).
const MAX_AT_48: usize = 150 * 48;

fn stereo(id: String, name: &str) -> Value {
    json!({ "id": id, "name": name, "width": 2, "layout": "stereo" })
}

fn mono(id: String, name: &str) -> Value {
    json!({ "id": id, "name": name, "width": 1, "layout": "mono" })
}

/// 64 stereo channels, the inputs an engine takes, onto 56 columns:
/// 128 by 56 coefficients, the most at a block of 48. Every channel
/// under one of 8 VCAs and in a duck group, sends to every aux.
fn largest() -> Mixer {
    let channels: Vec<Value> = (0..64)
        .map(|i| {
            let mut c = stereo(format!("ch{i}"), "Ch");
            c["altInput"] = json!(true);
            c
        })
        .collect();
    let auxes: Vec<Value> = (0..34).map(|i| mono(format!("aux{i}"), "Aux")).collect();
    let mix_minus: Vec<Value> = (0..8)
        .map(|i| {
            let mut m = stereo(format!("mm{i}"), "N-1");
            m["bus"] = json!("pgm");
            m
        })
        .collect();
    let mut pgm = stereo("pgm".into(), "PGM");
    pgm["kind"] = json!("program");
    let mut cf = stereo("cf".into(), "Clean");
    cf["kind"] = json!("cleanFeed");
    let mut out = stereo("out-pgm".into(), "PGM out");
    out["source"] = json!("pgm");
    let shape = json!({
        "sampleRate": 48000,
        "frameRate": { "numerator": 25, "denominator": 1 },
        "channels": channels,
        "buses": [pgm, cf],
        "auxes": auxes,
        "mixMinus": mix_minus,
        "vcas": (0..8).map(|i| json!({ "id": format!("vca{i}"), "name": "VCA" })).collect::<Vec<_>>(),
        "ducks": [{ "id": "comm", "name": "Commentary" }],
        "monitorOutputs": [stereo("mon".into(), "Monitor")],
        "outputs": [out],
    });
    let shape = Shape::parse(&shape).unwrap();
    let rows: usize = shape.channels.iter().map(|c| c.width).sum();
    let columns: usize = (shape.outputs.iter().map(|o| o.width))
        .chain(shape.meter_columns().iter().map(|m| m.1))
        .sum();
    assert!(rows * columns <= MAX_AT_48, "{rows} by {columns}");
    assert!(
        rows * columns > MAX_AT_48 - columns,
        "{rows} by {columns}: not the largest"
    );
    let wiring = (0..64)
        .map(|i| Wiring {
            a: format!("a{i}"),
            b: Some(format!("b{i}")),
            domain: "d".into(),
        })
        .collect();
    let mut m = Mixer::configured(shape, Vec::new(), wiring);
    for v in 0..8 {
        let members: Vec<String> = (0..64)
            .filter(|i| i % 8 == v)
            .map(|i| format!("ch{i}"))
            .collect();
        apply(&mut m, &format!("/vcas/vca{v}/members"), json!(members));
    }
    for i in 0..64 {
        apply(
            &mut m,
            &format!("/channels/ch{i}/sends/aux{}/on", i % 34),
            json!(true),
        );
    }
    m
}

fn apply(m: &mut Mixer, path: &str, value: Value) -> Effect<faderz::state::Edits> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let command = Command::Control(Control {
        id: format!("c-{path}"),
        action: Action::Set,
        kind: "ui".into(),
        seq: 1,
        path: path.into(),
        value: Some(value),
    });
    let origin = Origin {
        principal: "p".into(),
        session: format!("{path}-{seq}"),
        role: Role::Operator,
        connection: "c".into(),
    };
    m.apply(
        &origin,
        &serde_json::to_vec(&command).unwrap(),
        &Apply::default(),
    )
}

/// The ids a busy console's commands name.
struct Ids {
    channels: Vec<String>,
    vcas: Vec<String>,
    auxes: Vec<String>,
}

impl Ids {
    fn of(shape: &Shape) -> Ids {
        Ids {
            channels: shape.channels.iter().map(|c| c.id.clone()).collect(),
            vcas: shape.vcas.iter().map(|v| v.id.clone()).collect(),
            auxes: shape.auxes.iter().map(|a| a.id.clone()).collect(),
        }
    }

    /// The `n`th command of a busy console, one entry each: faders, VCA
    /// offsets, sends, gates and input switches.
    fn command(&self, n: usize) -> (String, Value) {
        let ch = &self.channels[n % self.channels.len()];
        let db = -((n * 37 % 800) as f64) / 10.0;
        let pick = |ids: &[String]| ids.get(n % ids.len().max(1)).cloned();
        match (n % 5, pick(&self.vcas), pick(&self.auxes)) {
            (1, Some(vca), _) => (format!("/vcas/{vca}/offset"), json!(db / 4.0)),
            (2, _, Some(aux)) => (format!("/channels/{ch}/sends/{aux}/level"), json!(db)),
            (3, ..) => (format!("/channels/{ch}/gate/threshold"), json!(db - 20.0)),
            (4, ..) => (
                format!("/channels/{ch}/input"),
                json!(if n.is_multiple_of(2) { "a" } else { "b" }),
            ),
            _ => (format!("/channels/{ch}/fader"), json!(db)),
        }
    }
}

fn quantile(sorted: &[Duration], q: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// The cost of targets() and the diff after one command, and of 64
/// entries applied and derived in a row, printed for the gate a set's
/// engine hand-off is held to (1 ms at the 99th percentile, 50 ms for 64
/// in a row, in a release build). Asserted are bounds 10 times those in
/// a release build and 100 times in a debug one, which catch a
/// derivation that grew by an order, not the gate.
#[test]
fn targets_and_diff_at_the_largest_shape() {
    measure("largest", largest());
}

/// [`targets_and_diff_at_the_largest_shape`] at the shape in the file
/// `FADERZ_COST_SHAPE` names, each channel wired to flows of its own in
/// one domain; nothing without it.
#[test]
fn targets_and_diff_at_a_given_shape() {
    let Ok(path) = std::env::var("FADERZ_COST_SHAPE") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let shape = Shape::parse(&serde_json::from_str(&text).unwrap()).unwrap();
    let wiring = (shape.channels.iter())
        .map(|c| Wiring {
            a: format!("a-{}", c.id),
            b: c.alt_input.then(|| format!("b-{}", c.id)),
            domain: "d".into(),
        })
        .collect();
    measure(&path, Mixer::configured(shape, Vec::new(), wiring));
}

fn measure(label: &str, mut m: Mixer) {
    let ids = Ids::of(m.shape());
    let command = |n| ids.command(n);
    let mut image = m.targets().unwrap();
    let mut took = Vec::new();
    let mut split = Vec::new();
    let mut entries = 0;
    // A debug build, as CI runs it, measures fewer.
    let (commands, rounds) = if cfg!(debug_assertions) {
        (300, 3)
    } else {
        (2000, 20)
    };
    for n in 0..commands {
        let (path, value) = command(n);
        let Effect::Changed(_) = apply(&mut m, &path, value) else {
            continue;
        };
        let start = Instant::now();
        let next = m.targets().unwrap();
        let mid = Instant::now();
        entries += image.diff(&next).len();
        took.push(start.elapsed());
        split.push((mid - start, mid.elapsed()));
        image = next;
    }
    took.sort();
    let mut t: Vec<_> = split.iter().map(|s| s.0).collect();
    let mut d: Vec<_> = split.iter().map(|s| s.1).collect();
    t.sort();
    d.sort();
    println!(
        "{label}: targets p50 {:?} p99 {:?}; diff p50 {:?} p99 {:?}",
        quantile(&t, 0.5),
        quantile(&t, 0.99),
        quantile(&d, 0.5),
        quantile(&d, 0.99)
    );
    let mut catch_up = Vec::new();
    let mut applying = Vec::new();
    for round in 0..rounds {
        let start = Instant::now();
        for n in 0..64 {
            let (path, value) = command(round * 64 + n + 7);
            let at = Instant::now();
            apply(&mut m, &path, value);
            applying.push(at.elapsed());
            let next = m.targets().unwrap();
            entries += image.diff(&next).len();
            image = next;
        }
        catch_up.push(start.elapsed());
    }
    catch_up.sort();
    applying.sort();
    println!(
        "{label}: apply alone: p50 {:?} p99 {:?}",
        quantile(&applying, 0.5),
        quantile(&applying, 0.99)
    );
    let (p50, p99, max) = (
        quantile(&took, 0.5),
        quantile(&took, 0.99),
        took[took.len() - 1],
    );
    let catch = quantile(&catch_up, 0.99);
    println!(
        "{label}: targets+diff over {} entries: p50 {p50:?} p99 {p99:?} max {max:?}; \
         64 in a row: p99 {catch:?}; {entries} engine entries",
        took.len()
    );
    let slack = if cfg!(debug_assertions) { 100 } else { 10 };
    assert!(p99 < Duration::from_millis(slack), "p99 {p99:?}");
    assert!(
        catch < Duration::from_millis(50 * slack),
        "64 in a row {catch:?}"
    );
}
