//! Consoles: the declared ones and those registered at runtime, as
//! `/consoles` publishes them.

use std::sync::Arc;

use samplez_replica::entry::{Origin, SessionKey};
use samplez_replica::state_machine::Effect;
use samplez_service::OwnerHandle;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::control::changed;
use super::{Answer, Edits, Mixer};

/// A console's descriptor: model `ConsoleStatus` without what the server
/// keeps (`handle`, `connected`, `bankOffset`). Also an entry of the
/// configuration's `consoles`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Console {
    pub id: String,
    pub name: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default = "default_strips")]
    pub strips: u64,
}

pub(crate) fn default_protocol() -> String {
    "mcu".into()
}

pub(crate) fn default_transport() -> String {
    "bridge".into()
}

pub(crate) fn default_strips() -> u64 {
    8
}

impl Console {
    /// Model `ConsoleStatus` of this descriptor.
    pub fn status(&self, session: Option<&SessionKey>, connected: bool, bank_offset: u64) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "handle": session.map(|s| OwnerHandle::of(&s.session).as_str().to_owned()),
            "protocol": self.protocol,
            "transport": self.transport,
            "connected": connected,
            "bankOffset": bank_offset,
            "strips": self.strips,
        })
    }
}

/// What `PUT /consoles/{id}` sets: a field left out keeps its value, or
/// takes its default on a new console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsolePut {
    pub id: String,
    pub name: String,
    pub protocol: Option<String>,
    pub transport: Option<String>,
    pub strips: Option<u64>,
}

/// A console the state holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Registered {
    console: Console,
    bank_offset: u64,
    pub(super) connected: bool,
    /// From the configuration: never removed, named by it.
    declared: bool,
    /// The session that registered it last. Kept in the replicated state,
    /// never published: the status carries its handle.
    pub(super) session: Option<SessionKey>,
}

impl Registered {
    fn public(&self) -> Value {
        self.console
            .status(self.session.as_ref(), self.connected, self.bank_offset)
    }
}

/// Consoles registered at runtime the state holds at most, besides the
/// declared ones.
pub const MAX_CONSOLES: usize = 64;

impl Mixer {
    pub(super) fn declared_entry(console: &Console) -> Registered {
        Registered {
            console: console.clone(),
            bank_offset: 0,
            connected: false,
            declared: true,
            session: None,
        }
    }

    /// Model `ConsoleStatus` of every console, in published order.
    pub(super) fn console_list(&self) -> Value {
        self.consoles.iter().map(Registered::public).collect()
    }

    /// Model `ConsoleStatus` of console `id`.
    pub fn console(&self, id: &str) -> Option<Value> {
        self.consoles
            .iter()
            .find(|r| r.console.id == id)
            .map(Registered::public)
    }

    /// `/consoles` written from the consoles held.
    fn publish_consoles(&mut self) -> Effect<Edits> {
        let list = self.console_list();
        changed(self.write(vec![("/consoles".into(), list)]))
    }

    fn position(&self, console: &str) -> Option<usize> {
        self.consoles.iter().position(|r| r.console.id == console)
    }

    /// Refuses a console beyond [`MAX_CONSOLES`] not declared.
    fn room_for_console(&self) -> Result<(), Answer> {
        if self.consoles.iter().filter(|r| !r.declared).count() >= MAX_CONSOLES {
            return Err(Answer::Invalid {
                message: format!("at most {MAX_CONSOLES} consoles besides the declared ones"),
            });
        }
        Ok(())
    }

    /// Registers `console` as `origin`'s session's, connected, at
    /// `bank_offset`. Refused while it is connected to another session. A
    /// declared console keeps the name of the configuration.
    pub(super) fn register(
        &mut self,
        origin: &Origin,
        id: &str,
        console: &Console,
        bank_offset: u64,
    ) -> Effect<Edits> {
        let session = origin.key();
        match self.position(&console.id) {
            Some(i) => {
                let r = &self.consoles[i];
                if r.connected && r.session.as_ref().is_some_and(|s| *s != session) {
                    let message = format!("console {} is connected to another session", console.id);
                    return self.refuse(&session, id, Answer::Refused { message });
                }
                let r = &mut self.consoles[i];
                let name = if r.declared {
                    r.console.name.clone()
                } else {
                    console.name.clone()
                };
                r.console = Console {
                    name,
                    ..console.clone()
                };
                r.bank_offset = bank_offset;
                r.connected = true;
                r.session = Some(session);
            }
            None => {
                if let Err(answer) = self.room_for_console() {
                    return self.refuse(&session, id, answer);
                }
                self.consoles.push(Registered {
                    console: console.clone(),
                    bank_offset,
                    connected: true,
                    declared: false,
                    session: Some(session),
                });
            }
        }
        self.publish_consoles()
    }

    /// Creates or updates the descriptor of a console, keeping who it is
    /// connected to and its bank. A declared console keeps the name of the
    /// configuration.
    pub(super) fn put_console(
        &mut self,
        session: &SessionKey,
        id: &str,
        put: &ConsolePut,
    ) -> Effect<Edits> {
        let i = match self.position(&put.id) {
            Some(i) => i,
            None => {
                if let Err(answer) = self.room_for_console() {
                    return self.refuse(session, id, answer);
                }
                self.consoles.push(Registered {
                    console: Console {
                        id: put.id.clone(),
                        name: put.name.clone(),
                        protocol: default_protocol(),
                        transport: default_transport(),
                        strips: default_strips(),
                    },
                    bank_offset: 0,
                    connected: false,
                    declared: false,
                    session: None,
                });
                self.record(session, id, Answer::Created);
                self.consoles.len() - 1
            }
        };
        let c = &mut self.consoles[i];
        if !c.declared {
            c.console.name = put.name.clone();
        }
        if let Some(p) = &put.protocol {
            c.console.protocol = p.clone();
        }
        if let Some(t) = &put.transport {
            c.console.transport = t.clone();
        }
        if let Some(n) = put.strips {
            c.console.strips = n;
        }
        self.publish_consoles()
    }

    pub(super) fn remove_console(
        &mut self,
        session: &SessionKey,
        id: &str,
        console: &str,
    ) -> Effect<Edits> {
        let Some(i) = self.position(console) else {
            let message = format!("unknown console {console}");
            return self.refuse(session, id, Answer::NotFound { message });
        };
        if self.consoles[i].declared {
            let message = format!("console {console} is declared in the configuration");
            return self.refuse(session, id, Answer::Refused { message });
        }
        // Removed now, it would come back at its session's next
        // registration.
        if self.consoles[i].connected {
            let message = format!("console {console} is connected");
            return self.refuse(session, id, Answer::Refused { message });
        }
        self.consoles.remove(i);
        self.publish_consoles()
    }

    /// The consoles of a loaded state merged with the declared ones: every
    /// declared console, in configuration order and named by it, with the
    /// session, connection and bank it had and, once registered, what the
    /// registration said; then the saved consoles that were never
    /// declared. One declared before and no longer is dropped.
    pub(super) fn merge_consoles(&mut self, saved: Vec<Registered>) {
        let mut merged: Vec<Registered> = self
            .declared
            .iter()
            .map(|d| match saved.iter().find(|s| s.console.id == d.id) {
                Some(s) => {
                    let console = if s.session.is_some() {
                        Console {
                            name: d.name.clone(),
                            ..s.console.clone()
                        }
                    } else {
                        d.clone()
                    };
                    Registered {
                        console,
                        declared: true,
                        ..s.clone()
                    }
                }
                None => Mixer::declared_entry(d),
            })
            .collect();
        let runtime = saved
            .into_iter()
            .filter(|s| !s.declared && !self.declared.iter().any(|d| d.id == s.console.id));
        merged.extend(runtime);
        self.consoles = merged;
        Arc::make_mut(&mut self.state)["consoles"] = self.console_list();
    }
}
