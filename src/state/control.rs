//! Clients' controls: who holds which, the last seq per session and
//! control, the answers kept, and writing leaves with what follows from
//! them.

use std::sync::Arc;

use samplez_replica::entry::{Origin, SessionKey};
use samplez_replica::state_machine::{Apply, Effect};
use samplez_service::OwnerHandle;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::loudness::derived;
use super::paths::set_at;
use super::{Action, Answer, Control, Edits, Mixer, Write};
use crate::duck;

/// The holder of a touched control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Holder {
    principal: String,
    /// Kept in the replicated state, never published: the owner record
    /// carries its handle.
    session: String,
    kind: String,
    connection: String,
}

impl Holder {
    pub(super) fn key(&self) -> SessionKey {
        SessionKey::from_parts(&self.principal, &self.session)
    }

    /// The model's `Owner`.
    pub(super) fn public(&self) -> Value {
        json!({
            "principal": self.principal,
            "handle": OwnerHandle::of(&self.session).as_str(),
            "kind": self.kind,
            "connection": self.connection,
        })
    }
}

/// Answers kept per session, the newest: as many as samplez-replica's
/// dedup table keeps command ids of one session. Only some commands get
/// one, so every command id the table still answers as a duplicate
/// finds its answer; the table also retires ids by age and by its total,
/// which only shortens what it holds.
fn answers_kept() -> usize {
    samplez_replica::dedup::Limits::defaults(crate::replica::RATE).max_session_commands
}

/// The principal of every client while no authentication names one: not
/// an identity two sessions can be told apart by.
const ANONYMOUS: &str = "anonymous";

impl Mixer {
    pub(super) fn record(&mut self, session: &SessionKey, id: &str, answer: Answer) {
        let kept = self.answers.entry(session.clone()).or_default();
        if kept.len() >= answers_kept() {
            kept.pop_front();
        }
        kept.push_back((id.to_owned(), answer));
    }

    pub(super) fn refuse(
        &mut self,
        session: &SessionKey,
        id: &str,
        answer: Answer,
    ) -> Effect<Edits> {
        self.record(session, id, answer);
        Effect::Unchanged
    }

    fn conflict(&self, path: &str, holder: &Holder) -> Value {
        json!({
            "path": path,
            "current": self.state.pointer(path).cloned().unwrap_or(Value::Null),
            "owner": holder.public(),
        })
    }

    /// Writes `written` and what follows from it, and returns every leaf
    /// that changed: loudness target and tolerance from a preset, preset
    /// `custom` from a written target or tolerance, AFV stages from
    /// bindings and tally. The engine targets follow the leaves changed.
    pub(super) fn write(&mut self, written: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let all = derived(written);
        let vcas = (self.derived.is_some() && all.iter().any(|(p, _)| p.starts_with("/vcas")))
            .then(|| self.state["vcas"].clone());
        let afv = all
            .iter()
            .any(|(p, _)| p.starts_with("/afv/") || p.starts_with("/tally/"));
        let mut changed: Vec<(String, Value)> = Vec::new();
        for (path, value) in all {
            if self.state.pointer(&path) != Some(&value) {
                set_at(Arc::make_mut(&mut self.state), &path, value.clone());
                changed.retain(|(p, _)| *p != path);
                changed.push((path, value));
            }
        }
        if afv {
            for (channel, open) in self.afv_open() {
                if self.state["afv"][&channel]["open"] != open {
                    let path = format!("/afv/{channel}/open");
                    set_at(Arc::make_mut(&mut self.state), &path, json!(open));
                    changed.push((path, json!(open)));
                }
            }
        }
        if let (Some(d), Some(shape)) = (&mut self.derived, self.shape.as_deref()) {
            let vcas = vcas.as_ref().unwrap_or(&Value::Null);
            let paths = changed.iter().map(|(p, _)| p.as_str());
            d.update(shape, &self.effective, &self.state, vcas, paths);
        }
        self.follow_switches(&changed);
        changed
    }

    /// Momentary controls among `paths` (IFB interrupt, duck hold) go back
    /// to off.
    fn momentary_off(&self, paths: &[String]) -> Vec<(String, Value)> {
        paths
            .iter()
            .filter(|p| momentary(p))
            .filter(|p| self.state.pointer(p) == Some(&json!(true)))
            .map(|p| (p.clone(), json!(false)))
            .collect()
    }

    pub(super) fn control(&mut self, origin: &Origin, c: &Control, ctx: &Apply) -> Effect<Edits> {
        let (id, action, kind, seq, path) = (
            c.id.as_str(),
            c.action,
            c.kind.as_str(),
            c.seq,
            c.path.as_str(),
        );
        let value = c.value.as_ref();
        let session = origin.key();
        let holder = self.owners.get(path).cloned();
        let other = holder.as_ref().filter(|h| h.key() != session);
        // A console takes a control its own principal holds on a screen.
        let takeover = other.is_some_and(|h| {
            kind == "console"
                && h.kind != "console"
                && h.principal == origin.principal
                && origin.principal != ANONYMOUS
        });
        if action != Action::Release
            && !takeover
            && let Some(h) = other
        {
            let conflicts = vec![self.conflict(path, h)];
            return self.refuse(&session, id, Answer::Conflict { conflicts });
        }
        if !self.exists(path) {
            let message = format!("unknown control {path}");
            return self.refuse(&session, id, Answer::NotFound { message });
        }
        let last = self.seqs.get(&session).and_then(|s| s.get(path)).copied();
        if last.is_some_and(|last| seq <= last) {
            return Effect::Stale;
        }
        let me = Holder {
            principal: origin.principal.clone(),
            session: origin.session.clone(),
            kind: kind.to_owned(),
            connection: origin.connection.clone(),
        };
        let origin = Some(json!({ "owner": me.public(), "commandId": id, "seq": seq }));
        let edits = match action {
            Action::Touch => {
                self.seen(&session, path, seq);
                if holder.is_some() && !takeover {
                    return Effect::Unchanged;
                }
                let owners = vec![(path.to_owned(), me.public())];
                self.owners.insert(path.to_owned(), me);
                Edits {
                    owners,
                    origin,
                    ..Edits::default()
                }
            }
            Action::Release => {
                self.seen(&session, path, seq);
                if holder.is_none_or(|h| h.key() != session) {
                    return Effect::Unchanged;
                }
                self.owners.remove(path);
                let off = self.momentary_off(&[path.to_owned()]);
                Edits {
                    values: self.write(off),
                    owners: vec![(path.to_owned(), Value::Null)],
                    origin,
                    ..Edits::default()
                }
            }
            Action::Set => {
                let value = value.cloned().unwrap_or(Value::Null);
                if let Err(answer) = self.check_write(path, &value) {
                    return self.refuse(&session, id, answer);
                }
                // Held by nobody, a hold set true would outlast its client.
                if duck::is_hold(path) && value == true && holder.is_none() {
                    let message = format!("{path} is momentary: touch it before setting it true");
                    return self.refuse(&session, id, Answer::Invalid { message });
                }
                let written = vec![(path.to_owned(), value)];
                if let Some((_, message)) = self.broken_duck(&written) {
                    return self.refuse(&session, id, Answer::Invalid { message });
                }
                if let Some(answer) = self.reroute_refusal(ctx, &written) {
                    return self.refuse(&session, id, answer);
                }
                self.seen(&session, path, seq);
                let values = self.write(written);
                if values.is_empty() {
                    return Effect::Unchanged;
                }
                Edits {
                    values,
                    origin,
                    ..Edits::default()
                }
            }
        };
        Effect::Changed(edits)
    }

    fn seen(&mut self, session: &SessionKey, path: &str, seq: u64) {
        self.seqs
            .entry(session.clone())
            .or_default()
            .insert(path.to_owned(), seq);
    }

    pub(super) fn patch(
        &mut self,
        session: &SessionKey,
        id: &str,
        changes: &[Write],
        ctx: &Apply,
    ) -> Effect<Edits> {
        let conflicts: Vec<Value> = changes
            .iter()
            .filter_map(|c| Some(self.conflict(&c.path, self.owners.get(&c.path)?)))
            .collect();
        if !conflicts.is_empty() {
            return self.refuse(session, id, Answer::Conflict { conflicts });
        }
        for c in changes {
            if let Err(answer) = self.check_write(&c.path, &c.value) {
                return self.refuse(session, id, answer);
            }
            // No session holds a patch's controls to release them.
            if duck::is_hold(&c.path) && c.value == true {
                let message = format!("{} is momentary: set it on the WebSocket", c.path);
                return self.refuse(session, id, Answer::Invalid { message });
            }
        }
        let written: Vec<(String, Value)> = changes
            .iter()
            .map(|c| (c.path.clone(), c.value.clone()))
            .collect();
        if let Some((_, message)) = self.broken_duck(&written) {
            return self.refuse(session, id, Answer::Invalid { message });
        }
        if let Some(answer) = self.reroute_refusal(ctx, &written) {
            return self.refuse(session, id, answer);
        }
        changed(self.write(written))
    }

    /// [`Product::expire`]: releases every control `session` holds and
    /// disconnects its consoles.
    pub(super) fn end_lease(&mut self, session: &SessionKey) -> Effect<Edits> {
        let held: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, h)| h.key() == *session)
            .map(|(p, _)| p.clone())
            .collect();
        let mut consoles = false;
        for r in &mut self.consoles {
            if r.connected && r.session.as_ref() == Some(session) {
                r.connected = false;
                consoles = true;
            }
        }
        if held.is_empty() && !consoles {
            return Effect::Unchanged;
        }
        for path in &held {
            self.owners.remove(path);
        }
        let mut written = self.momentary_off(&held);
        if consoles {
            written.push(("/consoles".into(), self.console_list()));
        }
        Effect::Changed(Edits {
            values: self.write(written),
            owners: held.into_iter().map(|p| (p, Value::Null)).collect(),
            origin: None,
            ..Edits::default()
        })
    }

    /// Puts a request for `path` from `asker` in the log, addressed to the
    /// control's holder: nothing when nobody else holds it. Every replica
    /// applies it, and the one the holder's primary connection is on
    /// delivers it ([`crate::app::App::publish`]).
    pub(super) fn control_request(
        &self,
        asker: &SessionKey,
        path: &str,
        from: &Value,
        at: u64,
    ) -> Effect<Edits> {
        let Some(holder) = self.owner_of(path) else {
            return Effect::Unchanged;
        };
        if holder == *asker {
            return Effect::Unchanged;
        }
        let to = json!({ "principal": holder.principal, "session": holder.session });
        Effect::Changed(Edits {
            requests: vec![json!({ "path": path, "from": from, "to": to, "at": at })],
            ..Edits::default()
        })
    }
}

/// An IFB interrupt or a duck hold, which goes back to off when
/// released.
fn momentary(path: &str) -> bool {
    path.starts_with("/mixMinus/") && path.ends_with("/interrupt/active") || duck::is_hold(path)
}

/// A command's effect from the leaves it changed, none of them owners.
pub(super) fn changed(values: Vec<(String, Value)>) -> Effect<Edits> {
    if values.is_empty() {
        Effect::Unchanged
    } else {
        Effect::Changed(Edits {
            values,
            ..Edits::default()
        })
    }
}
