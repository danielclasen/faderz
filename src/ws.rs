//! `/api/v1/ws`: the contract's WebSocket protocol over
//! `samplez_service::ws::serve_client`, which keeps the heartbeats, the
//! read-index hold, the promote floor and the order of answers.

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use futures_util::future::BoxFuture;
use samplez_replica::entry::SessionKey;
use samplez_service::replica::{Ack, Begin, NotApplied, Resolution};
use samplez_service::ws::{
    Admission, Answer, CatchUp, ClientQueue, Encode, Frame, Reply, To, serve_client,
};
use samplez_service::{Connect, Owner, OwnerHandle, Position};
use serde_json::{Value, json};

use crate::app::{App, SELECTIONS, splice};
use crate::contract::{check, contract};
use crate::state::{self, Action, Command, Control};

/// Meter credits a connection holds at most.
const CREDIT_CAP: u64 = 8;

type Answered<'a> = BoxFuture<'a, Option<Message>>;

/// What the connection's hello said.
#[derive(Default)]
struct Hello {
    kind: Option<String>,
    /// Set by a hello until its answer has gone out, which the `replicas`
    /// message goes ahead of.
    greet: bool,
    /// Credits granted while the connection is a standby, kept for its
    /// promote: a standby gets no meter frames.
    standby_credits: u64,
}

/// Serves one WebSocket client until it closes or fails.
pub async fn serve(app: Arc<App>, socket: WebSocket, connect: Connect) {
    let queue = app.hub.subscribe(&connect.owner);
    let _connected = app.connected(SessionKey::from(&connect.owner));
    let hello = Arc::new(Mutex::new(Hello::default()));
    let encoder = Encoder {
        app: app.clone(),
        owner: connect.owner.clone(),
        hello: hello.clone(),
    };
    let ended = serve_client(
        socket,
        &connect,
        &queue,
        &*app.replica,
        encoder,
        |message, admission| on_message(&app, &connect, &queue, &hello, message, admission),
    )
    .await;
    tracing::debug!(connection = %connect.owner.connection, ?ended, "connection ended");
}

struct Encoder {
    app: Arc<App>,
    owner: Owner,
    hello: Arc<Mutex<Hello>>,
}

impl Encoder {
    /// Model `Owner` of this connection.
    fn you(&self) -> Value {
        let kind = self.hello.lock().expect("hello lock poisoned").kind.clone();
        owner_record(&self.owner, kind.as_deref().unwrap_or("ui"))
    }
}

pub fn owner_record(owner: &Owner, kind: &str) -> Value {
    json!({
        "principal": owner.principal,
        "handle": OwnerHandle::of(&owner.session).as_str(),
        "kind": kind,
        "connection": owner.connection,
    })
}

impl Encode for Encoder {
    fn ping(&mut self, t: u64) -> Message {
        text(json!({ "type": "ping", "replica": self.app.replica_ref(), "t": t }).to_string())
    }

    fn answer(&mut self, answer: Answer) -> Vec<Message> {
        let mut out = Vec::new();
        let greet = std::mem::take(&mut self.hello.lock().expect("hello lock poisoned").greet);
        if greet {
            out.push(text(self.app.replicas_message()));
        }
        let replica = self.app.replica_ref();
        let stale = |s: bool| if s { r#","stale":true"# } else { "" };
        let message = match answer {
            Answer::Snapshot { snapshot, stale: s } => splice(
                &format!(
                    r#"{{"type":"snapshot","you":{},"replica":{replica},"epoch":"{}","revision":{}{}"#,
                    self.you(),
                    snapshot.epoch,
                    snapshot.revision,
                    stale(s)
                ),
                &snapshot.text,
            ),
            Answer::Resume {
                epoch,
                revision,
                deltas,
                stale: s,
            } => {
                let deltas: Vec<String> = deltas
                    .iter()
                    .map(|d| self.app.delta_message(d.revision, &d.text))
                    .collect();
                format!(
                    r#"{{"type":"resume","you":{},"replica":{replica},"epoch":"{epoch}","revision":{revision},"deltas":[{}]{}}}"#,
                    self.you(),
                    deltas.join(","),
                    stale(s)
                )
            }
            Answer::Ahead {
                epoch,
                committed,
                last_revision,
            } => json!({
                "type": "ahead",
                "replica": replica,
                "epoch": epoch.to_string(),
                "revision": committed,
                "lastRevision": last_revision,
            })
            .to_string(),
        };
        out.push(text(message));
        out
    }
}

fn text(s: String) -> Message {
    Message::Text(s.into())
}

/// A `type: error` message.
fn error(app: &App, code: u16, message: &str, command: Option<(&str, &str)>) -> Message {
    let mut e =
        json!({ "type": "error", "replica": app.replica_ref(), "code": code, "message": message });
    if let Some((id, path)) = command {
        e["commandId"] = json!(id);
        e["path"] = json!(path);
    }
    text(e.to_string())
}

fn ready<'a>(message: Message) -> Reply<Answered<'a>> {
    Reply::Command(Box::pin(async move { Some(message) }))
}

/// The position a hello, resync or promote names, when it names one.
fn position(message: &Value) -> Option<Position> {
    Some(Position {
        epoch: message["epoch"].as_str()?.parse().ok()?,
        revision: message["lastRevision"].as_u64()?,
    })
}

fn on_message<'a>(
    app: &'a Arc<App>,
    connect: &'a Connect,
    queue: &ClientQueue,
    hello: &Mutex<Hello>,
    message: Message,
    admission: Admission,
) -> Reply<Answered<'a>> {
    let raw = match message {
        Message::Text(t) => t,
        _ => {
            return Reply::Now(Some(error(
                app,
                400,
                "binary messages are not accepted",
                None,
            )));
        }
    };
    let Ok(msg) = serde_json::from_str::<Value>(&raw) else {
        return Reply::Now(Some(error(app, 400, "not JSON", None)));
    };
    let kind = msg["type"].as_str().unwrap_or_default();
    let command = matches!(kind, "set" | "touch" | "release");
    if let Err(problem) = check(&contract().client_message, &msg) {
        let message = format!("invalid message: {problem}");
        let id = msg["id"]
            .as_str()
            .filter(|id| check(&contract().command_id, &json!(id)).is_ok());
        let e = match id {
            Some(id) => error(
                app,
                400,
                &message,
                Some((id, msg["path"].as_str().unwrap_or_default())),
            ),
            None => error(app, 400, &message, None),
        };
        return if command {
            ready(e)
        } else {
            Reply::Now(Some(e))
        };
    }
    if kind == "hello" {
        if msg["session"] != connect.owner.session.as_str() {
            return Reply::Now(Some(error(
                app,
                400,
                "hello session differs from ?session=",
                None,
            )));
        }
        let mut h = hello.lock().expect("hello lock poisoned");
        h.kind = msg["kind"].as_str().map(str::to_owned);
        h.greet = true;
        drop(h);
        // Meter frames and control requests go to primary connections only.
        queue.set_primary(msg["mode"] == "primary");
        // Queued now, sent once the answer to the hello has gone out.
        let to = To::Connection {
            principal: &connect.owner.principal,
            connection: &connect.owner.connection,
        };
        app.hub.send(
            to,
            Frame::Status(SELECTIONS, app.selections_message().into()),
        );
        return Reply::CatchUp(CatchUp {
            since: position(&msg),
            full: false,
        });
    }
    let kind_of_client = hello.lock().expect("hello lock poisoned").kind.clone();
    // Answered at once, a command too: samplez-service sends command
    // answers only after the first catch-up, which is the hello's, and
    // every message before the hello is refused alike, in the order sent.
    let Some(client_kind) = kind_of_client else {
        return Reply::Now(Some(error(app, 400, "hello first", None)));
    };
    match kind {
        "ping" => {
            let pong = json!({ "type": "pong", "replica": app.replica_ref(), "t": msg["t"] });
            Reply::Now(Some(text(pong.to_string())))
        }
        "pong" => msg["t"].as_u64().map_or(Reply::Now(None), Reply::Pong),
        "promote" => {
            queue.set_primary(true);
            let kept =
                std::mem::take(&mut hello.lock().expect("hello lock poisoned").standby_credits);
            grant(queue, kept);
            Reply::Promote(position(&msg))
        }
        "select" => {
            let owner = owner_record(&connect.owner, &client_kind);
            app.select(
                SessionKey::from(&connect.owner),
                owner,
                msg["element"].as_str(),
            );
            Reply::Now(None)
        }
        "request" => {
            let (app, asker) = (app.clone(), owner_record(&connect.owner, &client_kind));
            let (from, path) = (
                SessionKey::from(&connect.owner),
                msg["path"].as_str().unwrap_or_default().to_owned(),
            );
            tokio::spawn(async move { app.request(from, asker, path).await });
            Reply::Now(None)
        }
        "resync" => Reply::CatchUp(CatchUp {
            since: position(&msg),
            full: msg["full"] == true,
        }),
        "credit" => {
            let n = msg["n"].as_u64().unwrap_or(0);
            if queue.is_primary() {
                grant(queue, n);
            } else {
                let mut h = hello.lock().expect("hello lock poisoned");
                h.standby_credits = h.standby_credits.saturating_add(n).min(CREDIT_CAP);
            }
            Reply::Now(None)
        }
        "set" | "touch" | "release" => control(app, connect, &client_kind, &msg, admission),
        // A type the contract has but this server does not handle: refused,
        // never taken for a command.
        _ => Reply::Now(Some(error(
            app,
            400,
            &format!("unhandled message type {kind}"),
            None,
        ))),
    }
}

/// A `set`, `touch` or `release`: refused here when no order of commands
/// can make it valid, else ordered and answered once applied.
fn control<'a>(
    app: &'a Arc<App>,
    connect: &'a Connect,
    client_kind: &str,
    msg: &Value,
    admission: Admission,
) -> Reply<Answered<'a>> {
    let id = msg["id"].as_str().unwrap_or_default().to_owned();
    let path = msg["path"].as_str().unwrap_or_default().to_owned();
    let refuse = |code, message: &str| ready(error(app, code, message, Some((&id, &path))));
    if connect.require_operator().is_err() {
        return refuse(400, "operator role required");
    }
    let action = match msg["type"].as_str() {
        Some("set") => Action::Set,
        Some("touch") => Action::Touch,
        _ => Action::Release,
    };
    let checked = app.replica.store().with_state(|sm| match action {
        Action::Set => sm.product.check_write(&path, &msg["value"]),
        _ if sm.product.exists(&path) => Ok(()),
        _ => Err(state::Answer::NotFound {
            message: format!("unknown control {path}"),
        }),
    });
    if let Err(answer) = checked {
        return ready(refusal(app, &id, &path, &answer));
    }
    if admission == Admission::NotApplied {
        return refuse(
            503,
            "not applied: this client holds state the replica set never committed",
        );
    }
    let body = Command::Control(Control {
        id: id.clone(),
        action,
        kind: client_kind.to_owned(),
        seq: msg["seq"].as_u64().unwrap_or_default(),
        path: path.clone(),
        value: (action == Action::Set).then(|| msg["value"].clone()),
    });
    let body = serde_json::to_vec(&body).expect("a command serializes");
    let session = SessionKey::from(&connect.owner);
    match app.commands.begin(&connect.owner, &id) {
        Begin::Duplicate(ack) => {
            let reply = answer(app, &session, &id, &path, Ok(ack));
            Reply::Command(Box::pin(async move { reply }))
        }
        Begin::Busy => refuse(503, "not applied: too many commands in flight"),
        Begin::New(pending) => {
            let submitted = pending.submit(body.into());
            Reply::Command(Box::pin(async move {
                answer(app, &session, &id, &path, submitted.await)
            }))
        }
    }
}

/// The answer to command `id` once it resolved; none when the delta
/// listing it answers it.
fn answer(
    app: &App,
    session: &SessionKey,
    id: &str,
    path: &str,
    result: Result<Ack, NotApplied>,
) -> Option<Message> {
    let ack = match result {
        Ok(ack) => ack,
        Err(e) => return Some(error(app, 503, &e.to_string(), Some((id, path)))),
    };
    if let Some(
        a @ (state::Answer::Conflict { .. }
        | state::Answer::NotFound { .. }
        | state::Answer::Invalid { .. }),
    ) = app.answer(session, &ack)
    {
        return Some(refusal(app, id, path, &a));
    }
    let result = match ack.resolution {
        Resolution::Applied => return None,
        Resolution::Unchanged => "noop",
        Resolution::Duplicate => "duplicate",
        _ => "stale",
    };
    let ack = json!({
        "type": "ack",
        "replica": app.replica_ref(),
        "commandId": ack.command,
        "result": result,
        "revision": ack.revision,
    });
    Some(text(ack.to_string()))
}

/// The error message of a refusal.
fn refusal(app: &App, id: &str, path: &str, answer: &state::Answer) -> Message {
    match answer {
        state::Answer::Conflict { conflicts } => {
            let c = &conflicts[0];
            let mut e = json!({
                "type": "error",
                "replica": app.replica_ref(),
                "code": 409,
                "message": "owned by another session",
                "commandId": id,
                "path": path,
                "current": c["current"],
                "owner": c["owner"],
            });
            if c["path"] != path {
                e["path"] = c["path"].clone();
            }
            text(e.to_string())
        }
        state::Answer::NotFound { message } => error(app, 404, message, Some((id, path))),
        state::Answer::Invalid { message } => error(app, 400, message, Some((id, path))),
        state::Answer::Scene { .. } => error(app, 400, "not a control command", Some((id, path))),
    }
}

/// Grants `n` meter credits, within the cap the connection holds at most.
fn grant(queue: &ClientQueue, n: u64) {
    let room = CREDIT_CAP.saturating_sub(queue.meter_credits());
    queue.grant_meter_credits(n.min(room));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, owner, selected};
    use axum::Router;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::get;
    use futures_util::{SinkExt, StreamExt};
    use samplez_service::ws::Sessions;
    use samplez_service::{Identity, Role};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::Message as Sent;

    /// A serve cancelled mid-connection still counts its connection
    /// closed, so the session's selection goes with its last connection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_serve_counts_its_connection_closed() {
        let (app, dir) = app().await;
        let session = "session-a-0123456789abcdef";
        let sessions = Arc::new(Sessions::new("r1").unwrap());
        let (served, mut serving) = mpsc::unbounded_channel::<JoinHandle<()>>();
        let ws_app = app.clone();
        let router = Router::new().route(
            "/ws",
            get(move |upgrade: WebSocketUpgrade| {
                let (app, sessions, served) = (ws_app.clone(), sessions.clone(), served.clone());
                async move {
                    upgrade.on_upgrade(move |socket| async move {
                        let mut owner = owner(session);
                        owner.connection = crate::app::uuid().unwrap();
                        let connect = Connect {
                            identity: Identity {
                                user: None,
                                role: Role::Operator,
                            },
                            owner,
                            sessions,
                        };
                        let _ = served.send(tokio::spawn(serve(app, socket, connect)));
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        let key = SessionKey::from(&owner(session));
        let hello = json!({ "type": "hello", "session": session, "kind": "ui", "mode": "primary" });

        // Served once it answers: counted by then.
        let (mut first, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let serve_first = serving.recv().await.unwrap();
        first.send(Sent::text(hello.to_string())).await.unwrap();
        first.next().await.unwrap().unwrap();
        serve_first.abort();
        assert!(serve_first.await.unwrap_err().is_cancelled());

        let (mut second, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let serve_second = serving.recv().await.unwrap();
        second.send(Sent::text(hello.to_string())).await.unwrap();
        let select = json!({ "type": "select", "element": "ch1" });
        second.send(Sent::text(select.to_string())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !selected(&app, &key) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("selected");
        drop(second);
        tokio::time::timeout(Duration::from_secs(5), serve_second)
            .await
            .expect("the second connection ends")
            .unwrap();
        assert!(!selected(&app, &key), "gone with the last connection");
        let _ = std::fs::remove_dir_all(dir);
    }
}
