//! JSON pointer paths into the state, and the leaves under a value.

use serde_json::{Value, json};

/// Leaves to write, as paths and values.
pub(super) type Leaves = Vec<(String, Value)>;

/// The reference tokens of a JSON pointer, unescaped.
pub fn tokens(path: &str) -> Vec<String> {
    path.split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

pub(super) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Sets the leaf at `path`, creating its last key in an object.
pub(crate) fn set_at(root: &mut Value, path: &str, value: Value) {
    let tokens = tokens(path);
    let Some((last, parents)) = tokens.split_last() else {
        *root = value;
        return;
    };
    let mut at = root;
    for t in parents {
        at = match at {
            Value::Object(m) => m.entry(t.clone()).or_insert_with(|| json!({})),
            Value::Array(a) => match t.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    match at {
        Value::Object(m) => {
            m.insert(last.clone(), value);
        }
        Value::Array(a) => {
            if let Some(slot) = last.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                *slot = value;
            }
        }
        _ => {}
    }
}

/// The leaves under `value`: an array of strings (an id set or recall
/// groups) is one leaf, other arrays split into one leaf per leg.
pub(super) fn leaves(value: &Value, base: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    collect(value, base, &mut out);
    out
}

fn collect(value: &Value, base: &str, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Array(a) if a.iter().all(Value::is_string) => {
            out.push((base.to_owned(), value.clone()))
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                collect(v, &format!("{base}/{i}"), out);
            }
        }
        Value::Object(m) => {
            for (k, v) in m {
                collect(v, &format!("{base}/{}", escape(k)), out);
            }
        }
        _ => out.push((base.to_owned(), value.clone())),
    }
}
