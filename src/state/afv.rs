//! Audio follows video: tally, each channel's AFV stage, and the fades a
//! tally's transition gives the stages it opens or closes.

use std::collections::BTreeMap;

use samplez_replica::entry::SessionKey;
use samplez_replica::state_machine::Effect;
use serde_json::{Map, Value};

use super::control::changed;
use super::paths::escape;
use super::{Answer, Crossfade, Edits, Mixer, Transition};

/// Tally sources the state holds at most: a source once named stays,
/// `off` after a `PUT` that leaves it out.
pub const MAX_TALLY_SOURCES: usize = 256;

impl Mixer {
    /// Each channel's AFV stage as bindings and tally set it: open when
    /// disabled, else open on `program` of its source, else as it was.
    pub(super) fn afv_open(&self) -> BTreeMap<String, bool> {
        let afv = self.state["afv"].as_object().into_iter().flatten();
        afv.map(|(channel, b)| {
            let source = b["source"]
                .as_str()
                .and_then(|s| self.state["tally"][s].as_str());
            let open = if b["enabled"] != true {
                true
            } else {
                source.map_or(b["open"] == true, |t| t == "program")
            };
            (channel.clone(), open)
        })
        .collect()
    }

    /// Each channel's AFV stage open, by slot.
    pub(super) fn afv_opens(&self) -> Vec<bool> {
        (self.shape().channels.iter())
            .map(|c| self.state["afv"][&c.id]["open"] == true)
            .collect()
    }

    /// The fade of each row of the gains for the tally `t`, the stages
    /// having been open as `opens` says before it: per model
    /// `AfvBinding`, a stage the tally opens rises from `t.start` over its
    /// `riseMs`, one it closes waits its `holdMs` from there and falls
    /// over its `fallMs`, `t.transitionMs` in place of both times. The
    /// rows of a channel whose stage stays, and the line-up tone's, get
    /// none.
    pub(super) fn afv_fades(&self, opens: &[bool], t: Transition) -> Vec<Option<Crossfade>> {
        let rate = u64::from(self.shape().sample_rate);
        let samples = |ms: u64| ms * rate / 1000;
        let mut rows = Vec::new();
        for (c, &was) in self.shape().channels.iter().zip(opens) {
            let b = &self.state["afv"][&c.id];
            let open = b["open"] == true;
            let ms = |key: &str| b[key].as_f64().unwrap_or(0.0) as u64;
            let fade = (open != was).then(|| {
                let (hold, len) = if open {
                    (0, ms("riseMs"))
                } else {
                    (ms("holdMs"), ms("fallMs"))
                };
                let len = t.transition_ms.map_or(len, u64::from);
                Crossfade {
                    start: t.start + samples(hold),
                    len: samples(len) as u32,
                }
            });
            rows.extend(std::iter::repeat_n(fade, c.width));
        }
        rows
    }

    pub(super) fn tally(
        &mut self,
        session: &SessionKey,
        id: &str,
        sources: &BTreeMap<String, String>,
        replace: bool,
    ) -> Effect<Edits> {
        let known = self.state["tally"].as_object().map_or(0, Map::len);
        let new = sources
            .keys()
            .filter(|s| self.state["tally"].get(s.as_str()).is_none());
        if known + new.count() > MAX_TALLY_SOURCES {
            let message = format!("at most {MAX_TALLY_SOURCES} tally sources");
            return self.refuse(session, id, Answer::Invalid { message });
        }
        let mut all = sources.clone();
        if replace {
            for name in self.state["tally"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
            {
                all.entry(name.clone()).or_insert_with(|| "off".into());
            }
        }
        let written = all
            .into_iter()
            .map(|(name, state)| (format!("/tally/{}", escape(&name)), Value::String(state)))
            .collect();
        changed(self.write(written))
    }
}
