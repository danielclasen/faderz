//! The checks of a write that do not depend on the order of commands,
//! and of the duck groups it sets.

use serde_json::{Value, json};

use super::paths::{set_at, tokens};
use super::{Answer, Mixer};
use crate::contract::{check, contract};
use crate::duck;

impl Mixer {
    /// Whether `path` names a control of this shape: an existing leaf,
    /// or the recall-safe groups of an element.
    pub fn exists(&self, path: &str) -> bool {
        let tokens = tokens(path);
        if tokens.first().map(String::as_str) == Some("recallSafe") {
            return tokens.len() == 2 && self.shape().recallable(&tokens[1]);
        }
        self.state.pointer(path).is_some_and(|v| !v.is_object())
    }

    /// The checks of a write that do not depend on the order of commands:
    /// the path names a control (404), and the value is one its leaf
    /// takes (400): the contract's `Change`, then what it names exists.
    pub fn check_write(&self, path: &str, value: &Value) -> Result<(), Answer> {
        if !self.exists(path) {
            return Err(Answer::NotFound {
                message: format!("unknown control {path}"),
            });
        }
        let invalid = |message: String| Err(Answer::Invalid { message });
        if let Err(e) = check(&contract().change, &json!({ "path": path, "value": value })) {
            return invalid(format!("invalid value for {path}: {e}"));
        }
        self.check_references(path, value).or_else(invalid)
    }

    /// Ids a value names exist in the shape.
    pub(super) fn check_references(&self, path: &str, value: &Value) -> Result<(), String> {
        let shape = self.shape();
        let ids = || {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        };
        if path.ends_with("/exclude") || path.ends_with("/members") {
            if let Some(bad) = ids().find(|&id| !shape.is_channel(id)) {
                return Err(format!("unknown channel {bad}"));
            }
        } else if path == "/tone/targets" {
            if let Some(bad) = ids().find(|&id| !shape.is_output_bus(id)) {
                return Err(format!("unknown output {bad}"));
            }
        } else if path == "/monitor/source" {
            if !value.as_str().is_some_and(|s| shape.is_output_bus(s)) {
                return Err(format!("unknown monitor source {value}"));
            }
        } else if let [group, channel, leaf] = tokens(path).as_slice()
            && (group.as_str(), leaf.as_str()) == ("channels", "input")
            && value == "b"
            && !shape
                .channels
                .iter()
                .any(|c| &c.id == channel && c.alt_input)
        {
            return Err(format!("channel {channel} has no B input"));
        } else if let Some(element) = delayed_element(path) {
            // The engine refuses a delay past the line of the channel's
            // input or of the bus's or aux's outputs.
            let line = shape.delay_line(&element);
            let samples = value.as_u64().unwrap_or(0);
            let kind = match tokens(path)[0].as_str() {
                "channels" => "channel",
                "buses" => "bus",
                _ => "aux",
            };
            match line {
                None if samples > 0 => {
                    return Err(format!("{kind} {element} has no delay line"));
                }
                Some(max) if samples > u64::from(max) => {
                    return Err(format!(
                        "{kind} {element} delays at most {max} samples, its delay line"
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The first duck group whose triggers or targets `written` sets and
    /// that breaks its rules ([`duck::check`]) on the state the writes
    /// leave, with why.
    pub(super) fn broken_duck(&self, written: &[(String, Value)]) -> Option<(String, String)> {
        let mut groups: Vec<&str> = written
            .iter()
            .filter_map(|(p, _)| duck::members_of(p))
            .collect();
        if groups.is_empty() {
            return None;
        }
        groups.sort_unstable();
        groups.dedup();
        let mut after = json!({ "ducks": self.state["ducks"].clone() });
        for (path, value) in written.iter().filter(|(p, _)| p.starts_with("/ducks/")) {
            set_at(&mut after, path, value.clone());
        }
        groups.into_iter().find_map(|g| {
            let e = duck::check(&after["ducks"], g).err()?;
            Some((g.to_owned(), e))
        })
    }
}

/// The channel, bus or aux whose delay `path` is.
pub(super) fn delayed_element(path: &str) -> Option<String> {
    match tokens(path).as_slice() {
        [group, element, leaf]
            if matches!(group.as_str(), "channels" | "buses" | "auxes") && leaf == "delay" =>
        {
            Some(element.clone())
        }
        _ => None,
    }
}
