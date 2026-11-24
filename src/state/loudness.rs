//! Loudness metering: the leaves a preset or target sets besides, and
//! counted measurement resets.

use samplez_replica::entry::SessionKey;
use samplez_replica::state_machine::Effect;
use serde_json::{Value, json};

use super::control::changed;
use super::paths::escape;
use super::{Answer, Edits, Mixer};

impl Mixer {
    /// Counts one more reset in each of `ids`' measurements
    /// (`/loudness/<id>/resets`), the same on every replica; the engines
    /// restart the measurement at the delta's `atIndex`. An id that is no
    /// output refuses the whole command.
    pub(super) fn reset_loudness(
        &mut self,
        session: &SessionKey,
        id: &str,
        ids: &[String],
    ) -> Effect<Edits> {
        let shape = self.shape();
        if let Some(bad) = ids.iter().find(|i| !shape.is_output_bus(i)) {
            let message = format!("unknown bus {bad}");
            return self.refuse(session, id, Answer::NotFound { message });
        }
        let ids: std::collections::BTreeSet<&String> = ids.iter().collect();
        let written = (ids.into_iter())
            .map(|bus| {
                let n = self.state["loudness"][bus.as_str()]["resets"].as_u64();
                (
                    format!("/loudness/{}/resets", escape(bus)),
                    json!(n.unwrap_or(0) + 1),
                )
            })
            .collect();
        changed(self.write(written))
    }
}

/// `written` and the leaves a loudness preset or target written among
/// them sets besides.
pub(super) fn derived(written: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let mut all = written.clone();
    let preset_written = written
        .iter()
        .any(|(p, _)| p == "/metering/loudness/preset");
    for (path, value) in &written {
        match (path.as_str(), value.as_str()) {
            ("/metering/loudness/preset", Some("ebu")) => {
                all.push(("/metering/loudness/target".into(), json!(-23)));
                all.push(("/metering/loudness/tolerance".into(), json!(1)));
            }
            ("/metering/loudness/preset", Some("atsc")) => {
                all.push(("/metering/loudness/target".into(), json!(-24)));
                all.push(("/metering/loudness/tolerance".into(), json!(2)));
            }
            ("/metering/loudness/target" | "/metering/loudness/tolerance", _)
                if !preset_written =>
            {
                all.push(("/metering/loudness/preset".into(), json!("custom")));
            }
            _ => {}
        }
    }
    all
}
