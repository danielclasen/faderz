//! The engine targets the state derives, the inputs a command re-routes,
//! and the fades of the gains a command changes.

use std::collections::BTreeSet;

use samplez::{Fade, Targets};
use samplez_replica::state_machine::Apply;
use serde_json::{Map, Value};

use super::paths::tokens;
use super::{Action, Answer, Command, Crossfade, Mixer, Transition, crossfade_lead};
use crate::mix::{self, Wiring};

#[cfg(doc)]
use samplez_replica::state_machine::Product;

/// How [`Mixer::fading`] times the gain targets a command changes.
#[derive(Clone, Copy)]
pub(super) enum Timing {
    /// None fades: the engine ramps each over the ramp length.
    Cut,
    /// Every one alike, a recall's crossfade.
    All(Crossfade),
    /// Those of each channel whose AFV stage a tally opens or closes, by
    /// that stage's times ([`Mixer::afv_fades`]); the others none.
    Afv(Transition),
}

impl Mixer {
    /// Derives the engine targets of the whole state again.
    pub(super) fn derive(&mut self) {
        self.derived = match (self.shape.as_deref(), self.wiring.is_empty()) {
            (Some(shape), false) => Some(mix::Derived::new(shape, &self.effective, &self.state)),
            _ => None,
        };
    }

    /// The channel slots whose selected flow `written` changes, applied to
    /// the state as it is, ascending.
    pub(super) fn rerouted(&self, written: &[(String, Value)]) -> Vec<u32> {
        let (Some(shape), false) = (self.shape.as_deref(), self.wiring.is_empty()) else {
            return Vec::new();
        };
        let mut after = Map::new();
        for (path, value) in written {
            if let [group, id, leaf] = tokens(path).as_slice()
                && group == "channels"
                && leaf == "input"
            {
                after.insert(id.clone(), value.clone());
            }
        }
        let flow = |w: &Wiring, input: Option<&Value>| match (input.and_then(Value::as_str), &w.b) {
            (Some("b"), Some(b)) => b.clone(),
            _ => w.a.clone(),
        };
        (shape.channels.iter().zip(&self.effective).enumerate())
            .filter(|(_, (c, w))| {
                after.get(&c.id).is_some_and(|input| {
                    flow(w, Some(input)) != flow(w, self.state["channels"][&c.id].get("input"))
                })
            })
            .map(|(slot, _)| slot as u32)
            .collect()
    }

    /// Why a command writing `written`, ordered as `ctx` says, is refused
    /// for the inputs it re-routes: off the generation path, or across
    /// more than one MXL domain, which one engine generation cannot
    /// switch together. A command the writer did not classify
    /// (`generation` `None`) is taken as it was before.
    pub(super) fn reroute_refusal(
        &self,
        ctx: &Apply,
        written: &[(String, Value)],
    ) -> Option<Answer> {
        let generation = ctx.generation?;
        let slots = self.rerouted(written);
        if slots.is_empty() {
            return None;
        }
        if !generation {
            let message =
                "the command switches inputs and was not ordered to: send it again".into();
            return Some(Answer::Refused { message });
        }
        let domains: BTreeSet<&str> = (slots.iter())
            .map(|&s| self.effective[s as usize].domain.as_str())
            .collect();
        (domains.len() > 1).then(|| Answer::Refused {
            message: format!(
                "the command switches inputs in more than one MXL domain ({})",
                domains.into_iter().collect::<Vec<_>>().join(", ")
            ),
        })
    }

    /// [`Mixer::wiring`] with the leg flows activations named.
    pub(super) fn rewire(&mut self) {
        let Some(shape) = self.shape.clone() else {
            return;
        };
        self.effective = (shape.channels.iter().zip(&self.wiring))
            .map(|(c, w)| {
                let mut w = w.clone();
                if let Some(legs) = self.ports.legs.get(&c.id) {
                    if let Some(a) = &legs.a {
                        w.a.clone_from(a);
                    }
                    if let (Some(b), Some(_)) = (&legs.b, &w.b) {
                        w.b = Some(b.clone());
                    }
                }
                w
            })
            .collect();
    }

    /// Detaches the channels whose receiver is disabled and sets each
    /// output's enable, in `t` as derived from the state.
    pub(super) fn overlay(&self, t: &mut Targets) {
        let shape = self.shape();
        for (flow, c) in t.flows.iter_mut().zip(&shape.channels) {
            if self
                .ports
                .receivers
                .get(&c.id)
                .is_some_and(|r| !r.master_enable)
            {
                *flow = None;
            }
        }
        t.outputs = (shape.outputs.iter())
            .map(|o| {
                self.ports
                    .senders
                    .get(&o.id)
                    .is_none_or(|s| s.master_enable)
            })
            .collect();
    }

    /// Each gain's fade ([`Mixer::fading`]) with the gain target it fades
    /// to, shaped as the targets' gains, or empty for none: what an engine
    /// following the state's deltas sends a gain change with.
    pub fn gain_fades(&self) -> Vec<Vec<Option<(f32, Fade)>>> {
        let Some(d) = &self.derived else {
            return Vec::new();
        };
        (self.fading.iter().zip(&d.targets().gains))
            .map(|(fades, gains)| {
                (fades.iter().zip(gains))
                    .map(|(f, &g)| f.map(|f| (g, f)))
                    .collect()
            })
            .collect()
    }

    /// What the engine reads and writes beside the state: the flows with
    /// the activations' leg flows, the detached channels and each
    /// output's enable ([`Product::targets`] without deriving them).
    pub fn engine_view(&self) -> mix::EngineView {
        let mut t = Targets {
            flows: vec![Some(String::new()); self.effective.len()],
            ..Targets::default()
        };
        self.overlay(&mut t);
        mix::EngineView {
            wiring: self.effective.clone(),
            detached: t.flows.iter().map(Option::is_none).collect(),
            outputs: t.outputs,
        }
    }

    /// The leaves a command would write, applied to the state as it is,
    /// for the writer's classification ([`Product::reroutes`]): those of a
    /// set, a patch or a recall, the only commands that write an input
    /// selection.
    pub(super) fn writes_of(&self, command: &Command) -> Vec<(String, Value)> {
        match command {
            Command::Control(c) if c.action == Action::Set => {
                vec![(c.path.clone(), c.value.clone().unwrap_or(Value::Null))]
            }
            Command::Patch { changes, .. } => (changes.iter())
                .map(|c| (c.path.clone(), c.value.clone()))
                .collect(),
            Command::Recall { scene, .. } => (self.recall_writes(scene))
                .map(|(written, _)| written)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Runs `f`, a command or a lease expiry, and keeps
    /// [`Mixer::fading`] for each gain target it changes: with a fade
    /// from `timing`, the gain ramps from the value it has at the fade's
    /// start, its running fade's value there or its old target, to the
    /// new target; without, it loses its fade, and the engine ramps it
    /// over the ramp length from the value it holds when the change
    /// applies. A gain left as it was keeps its fade. Every gain is the
    /// one the engine takes, an invalid one 0 ([`samplez_dsp::valid_gain`]).
    pub(super) fn fading<T>(&mut self, timing: Timing, f: impl FnOnce(&mut Mixer) -> T) -> T {
        // Fades ended by the index the recall was received at, its
        // logged start less the lead, go: no replica applies the recall
        // earlier, so their gains hold the scene's values by then, and a
        // fade still running where the recall applies is not cut short.
        if let Timing::All(c) = timing {
            let received = c
                .start
                .saturating_sub(crossfade_lead(u64::from(self.shape().sample_rate)));
            for cell in self.fading.iter_mut().flatten() {
                if cell.is_some_and(|f| f.start + u64::from(f.len) <= received) {
                    *cell = None;
                }
            }
            if self.fading.iter().flatten().all(Option::is_none) {
                self.fading.clear();
            }
        }
        let opens = matches!(timing, Timing::Afv(_)).then(|| self.afv_opens());
        let before = match &self.derived {
            Some(d) if !matches!(timing, Timing::Cut) || !self.fading.is_empty() => {
                Some(d.targets().gains.clone())
            }
            _ => None,
        };
        let out = f(self);
        let (Some(before), Some(d)) = (before, &self.derived) else {
            return out;
        };
        let after = &d.targets().gains;
        // Each row's fade; none past the end.
        let timed = match (timing, opens) {
            (Timing::Afv(t), Some(opens)) => self.afv_fades(&opens, t),
            (Timing::All(c), _) => vec![Some(c); after.len()],
            _ => Vec::new(),
        };
        if self.fading.is_empty() {
            self.fading = after.iter().map(|row| vec![None; row.len()]).collect();
        }
        let valid = |g: f32| if samplez_dsp::valid_gain(g) { g } else { 0.0 };
        let rows = after.iter().zip(&before).zip(&mut self.fading);
        for (r, ((row, was), fades)) in rows.enumerate() {
            let fade = timed.get(r).copied().flatten();
            for ((&gain, &old), cell) in row.iter().zip(was).zip(fades) {
                if valid(gain).to_bits() == valid(old).to_bits() {
                    continue;
                }
                let old = valid(old);
                *cell = fade.map(|c| Fade {
                    from: cell.map_or(old, |f| f.value_at(old, c.start)),
                    start: c.start,
                    len: c.len,
                });
            }
        }
        if self.fading.iter().flatten().all(Option::is_none) {
            self.fading.clear();
        }
        out
    }
}
