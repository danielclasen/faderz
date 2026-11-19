//! The REST routes of api/openapi.yaml under `/api/v1`. samplez-service
//! authenticates, checks the role of state-changing requests and their
//! `X-Samplez-Request` and content type before a request gets here.

// A refusal is the response itself, passed up to the handler.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use samplez_replica::entry::SessionKey;
use samplez_service::replica::{Ack, Begin, Resolution};
use samplez_service::{Identity, Owner, Role};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, now_rfc3339, uuid};
use crate::contract::{Contract, check, contract};
use crate::shape::Shape;
use crate::state::{
    Answer, Command, Console, ConsolePut, Write, default_protocol, default_strips,
    default_transport,
};

/// The session REST writes are ordered and deduplicated under, one per
/// principal; shorter than any client session id, so no client shares it.
pub const REST_SESSION: &str = "rest";

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/state", get(get_state).patch(patch_state))
        .route("/replicas", get(replicas))
        .route("/status", get(status))
        .route("/scenes", post(create_scene))
        .route("/scenes/{scene}", put(store_scene).delete(delete_scene))
        .route("/scenes/{scene}/recall", post(recall_scene))
        .route("/afv/tally", post(post_tally).put(put_tally))
        .route("/loudness/reset", post(reset_loudness))
        .route("/consoles", get(get_consoles))
        .route(
            "/consoles/{console}",
            put(put_console).delete(delete_console),
        )
        .with_state(app)
}

/// What samplez-service's auth and browser checks refuse, 401 or 403, in
/// the body api/openapi.yaml defines for a refusal.
pub fn refusal(status: StatusCode, reason: &'static str) -> Response {
    error(status, reason)
}

/// A refused request: model `Error`.
fn error(status: StatusCode, message: impl Into<String>) -> Response {
    let body = json!({ "code": status.as_u16(), "message": message.into() });
    (status, Json(body)).into_response()
}

/// The request body as `T`, refused with 400 when it is not JSON or not
/// of the operation's schema.
fn body<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Response> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| error(StatusCode::BAD_REQUEST, "not JSON"))?;
    serde_json::from_value(value)
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid request: {e}")))
}

fn valid(
    validator: impl Fn(&Contract) -> &jsonschema::Validator,
    what: &str,
    value: &Value,
) -> Result<(), Response> {
    check(validator(contract()), value).map_err(|e| {
        error(
            StatusCode::BAD_REQUEST,
            format!("invalid request: {what}: {e}"),
        )
    })
}

/// Who a REST write is ordered for: the request's principal under the
/// REST session.
fn owner(identity: Option<Identity>) -> Owner {
    let identity = identity.unwrap_or(Identity {
        user: None,
        name: None,
        role: Role::Operator,
    });
    Owner {
        principal: identity.user.unwrap_or_else(|| "anonymous".into()),
        session: REST_SESSION.into(),
        role: identity.role,
        connection: REST_SESSION.into(),
    }
}

/// Orders `command` under its id and waits until it is applied: the
/// ack, or the error response of a refusal or a write not applied.
async fn run(app: &App, owner: &Owner, id: &str, command: Command) -> Result<Ack, Response> {
    let body = serde_json::to_vec(&command).expect("a command serializes");
    let ack = match app.commands.begin(owner, id) {
        Begin::Duplicate(ack) => ack,
        Begin::Busy => {
            return Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "not applied: too many commands in flight",
            ));
        }
        Begin::New(pending) => pending
            .submit(body.into())
            .await
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?,
    };
    match app.answer(&SessionKey::from(owner), &ack) {
        Some(Answer::Conflict { conflicts }) => {
            let body = json!({
                "code": 409,
                "message": "controls are owned by other sessions",
                "conflicts": conflicts,
            });
            Err((StatusCode::CONFLICT, Json(body)).into_response())
        }
        Some(Answer::NotFound { message }) => Err(error(StatusCode::NOT_FOUND, message)),
        Some(Answer::Invalid { message }) => Err(error(StatusCode::BAD_REQUEST, message)),
        Some(Answer::Refused { message }) => Err(error(StatusCode::CONFLICT, message)),
        Some(Answer::Scene { .. } | Answer::Created) | None => Ok(ack),
    }
}

/// Model `CommandResult`; `command_id` only for requests that carry one.
async fn result(app: &App, ack: &Ack, command_id: bool) -> Value {
    let (name, at_index) = match ack.resolution {
        Resolution::Applied => ("applied", app.at_index(ack.revision).await),
        Resolution::Duplicate => ("duplicate", None),
        _ => ("noop", None),
    };
    let mut r = json!({ "result": name, "revision": ack.revision });
    if command_id {
        r["commandId"] = json!(ack.command);
    }
    if let Some(i) = at_index {
        r["atIndex"] = json!(i);
    }
    r
}

async fn get_state(State(app): State<Arc<App>>) -> Response {
    // Taken before the state: it waits on the replica's locks.
    let replica = app.replica_ref();
    // Pointers and the owners only under the store lock: the state is
    // serialised after it, as the lock holds off applying entries.
    let (revision, shape, state, owners) = app.with_state(|sm| {
        (
            sm.revision,
            sm.product.shared_shape(),
            sm.product.shared_state(),
            sm.product.owners(),
        )
    });
    /// Model `StateDocument`, serialised from the shared state in place.
    #[derive(Serialize)]
    struct Document<'a> {
        revision: u64,
        replica: Value,
        shape: &'a Shape,
        state: &'a Value,
        owners: Value,
    }
    Json(Document {
        revision,
        replica,
        shape: &shape,
        state: &state,
        owners,
    })
    .into_response()
}

async fn status(State(app): State<Arc<App>>) -> Response {
    Json(app.status()).into_response()
}

async fn replicas(State(app): State<Arc<App>>) -> Response {
    Json(json!({ "replica": app.replica_ref(), "build": crate::app::build(), "replicas": app.replicas() }))
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchBody {
    command_id: String,
    changes: Vec<Write>,
}

async fn patch_state(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: PatchBody = body(&bytes)?;
        valid(|c| &c.command_id, "commandId", &json!(b.command_id))?;
        if b.changes.is_empty() {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid request: no changes",
            ));
        }
        for c in &b.changes {
            valid(
                |c| &c.change,
                "changes",
                &json!({ "path": c.path, "value": c.value }),
            )?;
        }
        let checked = app.with_state(|sm| {
            b.changes
                .iter()
                .try_for_each(|c| sm.product.check_write(&c.path, &c.value))
        });
        match checked {
            Err(Answer::NotFound { message }) => return Err(error(StatusCode::NOT_FOUND, message)),
            Err(Answer::Invalid { message }) => {
                return Err(error(StatusCode::BAD_REQUEST, message));
            }
            _ => {}
        }
        let owner = owner(identity.map(|Extension(i)| i));
        let command = Command::Patch {
            id: b.command_id.clone(),
            changes: b.changes,
        };
        let ack = run(&app, &owner, &b.command_id, command).await?;
        Ok(Json(result(&app, &ack, true).await).into_response())
    };
    run.await.unwrap_or_else(|e| e)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SceneBody {
    command_id: String,
    name: Option<String>,
}

impl SceneBody {
    fn check(&self, name_required: bool) -> Result<(), Response> {
        valid(|c| &c.command_id, "commandId", &json!(self.command_id))?;
        match &self.name {
            None if name_required => Err(error(
                StatusCode::BAD_REQUEST,
                "invalid request: name is required",
            )),
            Some(n) if n.is_empty() || n.chars().count() > 64 => Err(error(
                StatusCode::BAD_REQUEST,
                "invalid request: name takes 1 to 64 characters",
            )),
            _ => Ok(()),
        }
    }
}

/// Stores the scene and answers with its record and the command result.
async fn scene_result(
    app: &App,
    owner: &Owner,
    b: SceneBody,
    scene: Option<String>,
    status: StatusCode,
) -> Result<Response, Response> {
    let command = Command::StoreScene {
        id: b.command_id.clone(),
        scene,
        name: b.name,
        at: now_rfc3339(),
    };
    let ack = run(app, owner, &b.command_id, command).await?;
    let record = match app.answer(&SessionKey::from(owner), &ack) {
        Some(Answer::Scene { scene }) => app.with_state(|sm| sm.product.scene(&scene).cloned()),
        _ => None,
    };
    let Some(record) = record else {
        return Err(error(StatusCode::NOT_FOUND, "the scene was deleted since"));
    };
    let body = json!({ "scene": record, "result": result(app, &ack, true).await });
    Ok((status, Json(body)).into_response())
}

async fn create_scene(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: SceneBody = body(&bytes)?;
        b.check(true)?;
        let owner = owner(identity.map(|Extension(i)| i));
        scene_result(&app, &owner, b, None, StatusCode::CREATED).await
    };
    run.await.unwrap_or_else(|e| e)
}

fn known_scene(app: &App, scene: &str) -> Result<(), Response> {
    if app.with_state(|sm| sm.product.has_scene(scene)) {
        Ok(())
    } else {
        Err(error(
            StatusCode::NOT_FOUND,
            format!("unknown scene {scene}"),
        ))
    }
}

async fn store_scene(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    Path(scene): Path<String>,
    bytes: Bytes,
) -> Response {
    let run = async {
        known_scene(&app, &scene)?;
        let b: SceneBody = body(&bytes)?;
        b.check(false)?;
        let owner = owner(identity.map(|Extension(i)| i));
        scene_result(&app, &owner, b, Some(scene), StatusCode::OK).await
    };
    run.await.unwrap_or_else(|e| e)
}

async fn delete_scene(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    Path(scene): Path<String>,
) -> Response {
    let run = async {
        known_scene(&app, &scene)?;
        let owner = owner(identity.map(|Extension(i)| i));
        let id = uuid()
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!("not applied: {e}")))?;
        let ack = run(
            &app,
            &owner,
            &id,
            Command::DeleteScene {
                id: id.clone(),
                scene,
            },
        )
        .await?;
        Ok(Json(result(&app, &ack, false).await).into_response())
    };
    run.await.unwrap_or_else(|e| e)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecallBody {
    command_id: String,
    crossfade_ms: u32,
}

async fn recall_scene(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    Path(scene): Path<String>,
    bytes: Bytes,
) -> Response {
    let run = async {
        known_scene(&app, &scene)?;
        let b: RecallBody = body(&bytes)?;
        valid(|c| &c.command_id, "commandId", &json!(b.command_id))?;
        if b.crossfade_ms > 60_000 {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid request: crossfadeMs above 60000",
            ));
        }
        let owner = owner(identity.map(|Extension(i)| i));
        let command = Command::Recall {
            id: b.command_id.clone(),
            scene,
            at: now_rfc3339(),
        };
        let ack = run(&app, &owner, &b.command_id, command).await?;
        Ok(Json(result(&app, &ack, true).await).into_response())
    };
    run.await.unwrap_or_else(|e| e)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TallyBody {
    source: String,
    state: String,
    // Read once the engine applies tallies in audio (M3c).
    #[allow(dead_code)]
    at_index: Option<u64>,
    #[allow(dead_code)]
    at: Option<String>,
    transition_ms: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TallyStateBody {
    sources: BTreeMap<String, String>,
}

async fn tally(
    app: &App,
    identity: Option<Extension<Identity>>,
    sources: BTreeMap<String, String>,
    replace: bool,
) -> Result<Response, Response> {
    for (source, state) in &sources {
        valid(|c| &c.tally_source, "source", &json!(source))?;
        valid(|c| &c.tally_state, "state", &json!(state))?;
    }
    let owner = owner(identity.map(|Extension(i)| i));
    let id =
        uuid().map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!("not applied: {e}")))?;
    let command = Command::Tally {
        id: id.clone(),
        sources,
        replace,
    };
    let ack = run(app, &owner, &id, command).await?;
    Ok(Json(result(app, &ack, false).await).into_response())
}

/// `atIndex`, `at` and `transitionMs` time the transition in audio; the
/// state takes the tally at once.
async fn post_tally(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: TallyBody = body(&bytes)?;
        if b.transition_ms.is_some_and(|t| t > 10_000) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid request: transitionMs above 10000",
            ));
        }
        tally(&app, identity, BTreeMap::from([(b.source, b.state)]), false).await
    };
    run.await.unwrap_or_else(|e| e)
}

async fn put_tally(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: TallyStateBody = body(&bytes)?;
        tally(&app, identity, b.sources, true).await
    };
    run.await.unwrap_or_else(|e| e)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetBody {
    ids: Vec<String>,
}

/// Counts a reset of each output's measurement in the replicated state;
/// every replica's engine restarts it at the delta's `atIndex`.
async fn reset_loudness(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: ResetBody = body(&bytes)?;
        valid(|c| &c.id_set, "ids", &json!(b.ids))?;
        let unknown = app.with_state(|sm| {
            b.ids
                .iter()
                .find(|id| !sm.product.shape().is_output_bus(id))
                .cloned()
        });
        if let Some(id) = unknown {
            return Err(error(StatusCode::NOT_FOUND, format!("unknown bus {id}")));
        }
        let owner = owner(identity.map(|Extension(i)| i));
        let id = uuid()
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!("not applied: {e}")))?;
        run(
            &app,
            &owner,
            &id,
            Command::ResetLoudness {
                id: id.clone(),
                ids: b.ids,
            },
        )
        .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|e| e)
}

async fn get_consoles(State(app): State<Arc<App>>) -> Response {
    let consoles = app.with_state(|sm| sm.product.state()["consoles"].clone());
    Json(consoles).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleBody {
    name: String,
    protocol: Option<String>,
    transport: Option<String>,
    strips: Option<u64>,
}

/// Creates console `console` (201) or updates its descriptor (200), and
/// answers with its status. A field left out keeps its value, or takes its
/// default on a new console; a declared console keeps the name of the
/// configuration.
async fn put_console(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    Path(console): Path<String>,
    bytes: Bytes,
) -> Response {
    let run = async {
        let b: ConsoleBody = body(&bytes)?;
        let checked = Console {
            id: console.clone(),
            name: b.name.clone(),
            protocol: b.protocol.clone().unwrap_or_else(default_protocol),
            transport: b.transport.clone().unwrap_or_else(default_transport),
            strips: b.strips.unwrap_or_else(default_strips),
        };
        valid(
            |c| &c.console_status,
            "console",
            &checked.status(None, false, 0),
        )?;
        let owner = owner(identity.map(|Extension(i)| i));
        let id = uuid()
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!("not applied: {e}")))?;
        let command = Command::PutConsole {
            id: id.clone(),
            console: ConsolePut {
                id: console.clone(),
                name: b.name,
                protocol: b.protocol,
                transport: b.transport,
                strips: b.strips,
            },
        };
        let ack = run(&app, &owner, &id, command).await?;
        let created = matches!(
            app.answer(&SessionKey::from(&owner), &ack),
            Some(Answer::Created)
        );
        let status = app.with_state(|sm| sm.product.console(&console));
        let Some(status) = status else {
            return Err(error(
                StatusCode::NOT_FOUND,
                "the console was removed since",
            ));
        };
        let code = if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        Ok((code, Json(status)).into_response())
    };
    run.await.unwrap_or_else(|e| e)
}

async fn delete_console(
    State(app): State<Arc<App>>,
    identity: Option<Extension<Identity>>,
    Path(console): Path<String>,
) -> Response {
    let run = async {
        let owner = owner(identity.map(|Extension(i)| i));
        let id = uuid()
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!("not applied: {e}")))?;
        let command = Command::RemoveConsole {
            id: id.clone(),
            console,
        };
        run(&app, &owner, &id, command).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(|e| e)
}
