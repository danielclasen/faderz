//! The committed contract in `api/`, compiled in: what clients send is
//! checked against the same JSON Schemas the web client and its tests
//! read, so the server and the contract cannot drift apart.

use std::sync::OnceLock;

use jsonschema::{Registry, Validator};
use serde_json::{Value, json};

const BASE: &str = "https://github.com/danielclasen/faderz/raw/main/api/ws/";

const SCHEMAS: [(&str, &str); 3] = [
    (
        "model.schema.json",
        include_str!("../api/ws/model.schema.json"),
    ),
    (
        "client-message.schema.json",
        include_str!("../api/ws/client-message.schema.json"),
    ),
    (
        "server-message.schema.json",
        include_str!("../api/ws/server-message.schema.json"),
    ),
];

pub struct Contract {
    /// `client-message.schema.json`.
    pub client_message: Validator,
    /// `server-message.schema.json`.
    pub server_message: Validator,
    /// `model.schema.json#/$defs/Change`: a writable path and a value its
    /// leaf takes.
    pub change: Validator,
    /// `model.schema.json#/$defs/StateChange`, as deltas carry them.
    pub state_change: Validator,
    pub shape: Validator,
    pub state: Validator,
    pub tally_source: Validator,
    pub tally_state: Validator,
    pub command_id: Validator,
    pub id: Validator,
    pub id_set: Validator,
}

/// The compiled contract; built on first use.
pub fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        let mut registry = Registry::new();
        for (name, text) in SCHEMAS {
            let schema: Value = serde_json::from_str(text).expect("a committed schema is JSON");
            registry = registry
                .add(format!("{BASE}{name}"), schema)
                .expect("schema URIs parse");
        }
        let registry = registry.prepare().expect("the committed schemas resolve");
        let build = |pointer: String| {
            jsonschema::options()
                .with_registry(&registry)
                .should_validate_formats(true)
                .build(&json!({ "$ref": pointer }))
                .expect("the committed schemas compile")
        };
        let def = |name: &str| build(format!("{BASE}model.schema.json#/$defs/{name}"));
        Contract {
            client_message: build(format!("{BASE}client-message.schema.json")),
            server_message: build(format!("{BASE}server-message.schema.json")),
            change: def("Change"),
            state_change: def("StateChange"),
            shape: def("Shape"),
            state: def("State"),
            tally_source: def("TallySource"),
            tally_state: def("TallyState"),
            command_id: def("CommandId"),
            id: def("Id"),
            id_set: def("IdSet"),
        }
    })
}

/// `Ok` when `instance` matches, else the first few problems, each with
/// where in the instance it is.
pub fn check(validator: &Validator, instance: &Value) -> Result<(), String> {
    let problems: Vec<String> = validator
        .iter_errors(instance)
        .take(3)
        .map(|e| {
            let at = e.instance_path().to_string();
            format!("{} {e}", if at.is_empty() { "/" } else { &at })
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value rules of `ValueByPath` apply through `Change`: a fader
    /// above +10 dB is refused, one at +10 dB taken.
    #[test]
    fn change_carries_the_value_rules_of_its_path() {
        let c = contract();
        let at = |v: f64| json!({ "path": "/channels/ch1/fader", "value": v });
        assert!(check(&c.change, &at(10.0)).is_ok());
        assert!(check(&c.change, &at(10.5)).is_err());
        assert!(check(&c.change, &json!({ "path": "/nope", "value": 1 })).is_err());
    }

    /// Formats are asserted: a command id that is not a UUID is refused.
    #[test]
    fn command_ids_are_checked_as_uuids() {
        let c = contract();
        assert!(
            check(
                &c.command_id,
                &json!("0b6b2c2e-7d1f-4a3c-9a55-0f5c1d8e2b41")
            )
            .is_ok()
        );
        assert!(check(&c.command_id, &json!("not-a-uuid")).is_err());
    }
}
