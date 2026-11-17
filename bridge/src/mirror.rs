//! The faderz state as this client last applied it: shape, live state,
//! owners, and where it is in the cluster history.

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Default, Clone)]
pub struct Mirror {
    pub epoch: Option<String>,
    pub revision: u64,
    /// Model `Shape`; null before the first snapshot.
    pub shape: Value,
    /// Model `State`; null before the first snapshot.
    pub state: Value,
    /// Model `Owners`: control path to owner record.
    pub owners: BTreeMap<String, Value>,
}

impl Mirror {
    /// Whether a snapshot has been applied.
    pub fn ready(&self) -> bool {
        self.epoch.is_some()
    }

    /// Takes a snapshot message's state.
    pub fn snapshot(&mut self, msg: &Value) {
        self.epoch = msg["epoch"].as_str().map(str::to_owned);
        self.revision = msg["revision"].as_u64().unwrap_or(0);
        self.shape = msg["shape"].clone();
        self.state = msg["state"].clone();
        self.owners = msg["owners"]
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
    }

    /// Applies a delta's changes and owners and takes its revision.
    pub fn apply(&mut self, delta: &Value) {
        for change in delta["changes"].as_array().into_iter().flatten() {
            if let Some(path) = change["path"].as_str() {
                set_at(&mut self.state, path, change["value"].clone());
            }
        }
        for owner in delta["owners"].as_array().into_iter().flatten() {
            let Some(path) = owner["path"].as_str() else {
                continue;
            };
            if owner["owner"].is_null() {
                self.owners.remove(path);
            } else {
                self.owners.insert(path.to_owned(), owner["owner"].clone());
            }
        }
        if let Some(revision) = delta["revision"].as_u64() {
            self.revision = revision;
        }
    }

    /// The value at a JSON pointer of the state.
    pub fn get(&self, path: &str) -> Option<&Value> {
        self.state.pointer(path)
    }
}

/// Sets the value at JSON pointer `path` of `root`, creating objects on the
/// way; an array index past the end appends.
pub fn set_at(root: &mut Value, path: &str, value: Value) {
    let tokens: Vec<String> = path
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some((last, parents)) = tokens.split_last() else {
        *root = value;
        return;
    };
    let mut at = root;
    for token in parents {
        if !at.is_object() && !at.is_array() {
            *at = Value::Object(Default::default());
        }
        at = match at {
            Value::Array(items) => match token.parse::<usize>() {
                Ok(i) if i < items.len() => &mut items[i],
                _ => return,
            },
            Value::Object(map) => map.entry(token.clone()).or_insert(Value::Null),
            _ => return,
        };
    }
    match at {
        Value::Array(items) => {
            if let Ok(i) = last.parse::<usize>() {
                if i < items.len() {
                    items[i] = value;
                } else if i == items.len() {
                    items.push(value);
                }
            }
        }
        Value::Object(map) => {
            map.insert(last.clone(), value);
        }
        other => {
            let mut map = serde_json::Map::new();
            map.insert(last.clone(), value);
            *other = Value::Object(map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_delta_sets_leaves_whole_arrays_and_owners() {
        let mut m = Mirror::default();
        m.snapshot(&json!({
            "epoch": "7", "revision": 3,
            "shape": {}, "state": { "channels": { "ch1": { "fader": -90, "legTrim": [0, 0] } }, "consoles": [] },
            "owners": { "/channels/ch1/mute": { "principal": "a" } },
        }));
        assert!(m.ready());
        m.apply(&json!({
            "revision": 4,
            "changes": [
                { "path": "/channels/ch1/fader", "value": -3 },
                { "path": "/channels/ch1/legTrim/1", "value": 2 },
                { "path": "/consoles", "value": [{ "id": "x" }] },
                { "path": "/tally/cam 1~12", "value": "program" },
            ],
            "owners": [
                { "path": "/channels/ch1/mute", "owner": null },
                { "path": "/channels/ch1/fader", "owner": { "principal": "b" } },
            ],
        }));
        assert_eq!(m.revision, 4);
        assert_eq!(m.get("/channels/ch1/fader"), Some(&json!(-3)));
        assert_eq!(m.get("/channels/ch1/legTrim"), Some(&json!([0, 2])));
        assert_eq!(m.get("/consoles/0/id"), Some(&json!("x")));
        assert_eq!(m.state["tally"]["cam 1/2"], json!("program"));
        assert!(!m.owners.contains_key("/channels/ch1/mute"));
        assert_eq!(m.owners["/channels/ch1/fader"]["principal"], "b");
    }
}
