//! A mock IS-04 Registration API, as far as a node's registration uses
//! it: replica `n` reaches it at `http://<addr>/r<n>`, so it tells the
//! replicas of a set apart.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::routing::any;
use serde_json::{Value, json};

/// A registry drops a Node it heard no heartbeat from for this long.
pub const EXPIRY: Duration = Duration::from_secs(12);

/// One request the mock registry took: from replica `from`, by the path
/// prefix of the registry URL that replica was given.
#[derive(Debug, Clone)]
pub struct Seen {
    pub from: usize,
    pub at: Instant,
    /// The method and what follows `/x-nmos/registration/v1.3/`.
    pub request: String,
    /// A POSTed resource: its type and id.
    pub resource: Option<(String, String)>,
}

/// A Registration API, as far as a node's registration uses it: the
/// resources it holds by id, with the version each was last POSTed at,
/// each Node's last heartbeat, and every request it took.
#[derive(Default)]
pub struct Registry {
    pub resources: BTreeMap<String, String>,
    pub versions: BTreeMap<String, String>,
    pub heartbeats: BTreeMap<String, Instant>,
    pub seen: Vec<Seen>,
}

impl Registry {
    /// Drops every Node, and with it every resource, whose last
    /// heartbeat is older than [`EXPIRY`].
    pub fn expire(&mut self) {
        let now = Instant::now();
        let expired: Vec<String> = (self.heartbeats.iter())
            .filter(|(_, at)| now.duration_since(**at) > EXPIRY)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.heartbeats.remove(&id);
            self.resources.clear();
            self.versions.clear();
        }
    }

    pub fn nodes(&self) -> Vec<String> {
        (self.resources.iter())
            .filter(|(_, kind)| *kind == "node")
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The ids of the resources replica `n` posted at or after `since`.
    pub fn posted_by(&self, n: usize, since: Instant) -> BTreeSet<String> {
        (self.seen.iter())
            .filter(|s| s.from == n && s.at >= since)
            .filter_map(|s| s.resource.as_ref().map(|(_, id)| id.clone()))
            .collect()
    }
}

pub type Shared = Arc<Mutex<Registry>>;

/// Serves the mock registry on a thread and a runtime of its own, until
/// the process ends: replica `n` reaches it at `http://<addr>/r<n>`.
pub fn start() -> (Shared, SocketAddr) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/{*rest}", any(answer))
        .with_state(shared.clone());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, app).await
        })
    });
    (shared, addr)
}

async fn answer(
    State(shared): State<Shared>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> (StatusCode, String) {
    let path = uri.path();
    let Some((from, rest)) = (path.strip_prefix("/r"))
        .and_then(|p| p.split_once("/x-nmos/registration/v1.3/"))
        .and_then(|(n, rest)| Some((n.parse::<usize>().ok()?, rest.to_owned())))
    else {
        return (StatusCode::NOT_FOUND, String::new());
    };
    let mut r = shared.lock().unwrap();
    r.expire();
    let mut seen = Seen {
        from,
        at: Instant::now(),
        request: format!("{method} {rest}"),
        resource: None,
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let reply = match (&method, parts.as_slice()) {
        (&Method::POST, ["resource"]) => {
            let v: Value = serde_json::from_slice(&body).unwrap_or_default();
            let kind = v["type"].as_str().unwrap_or_default().to_owned();
            let id = v["data"]["id"].as_str().unwrap_or_default().to_owned();
            let version = v["data"]["version"].as_str().unwrap_or_default();
            r.versions.insert(id.clone(), version.to_owned());
            seen.resource = Some((kind.clone(), id.clone()));
            if kind == "node" {
                r.heartbeats.insert(id.clone(), Instant::now());
            }
            let held = r.resources.insert(id, kind).is_some();
            let status = if held {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (status, v["data"].to_string())
        }
        (&Method::POST, ["health", "nodes", id]) => match r.heartbeats.get_mut(*id) {
            Some(at) => {
                *at = Instant::now();
                (StatusCode::OK, json!({ "health": "0" }).to_string())
            }
            None => (StatusCode::NOT_FOUND, String::new()),
        },
        (&Method::DELETE, ["resource", "nodes", id]) => {
            if r.heartbeats.remove(*id).is_some() {
                r.resources.clear();
                r.versions.clear();
                (StatusCode::NO_CONTENT, String::new())
            } else {
                (StatusCode::NOT_FOUND, String::new())
            }
        }
        (&Method::DELETE, ["resource", _, id]) => match r.resources.remove(*id) {
            Some(_) => {
                r.versions.remove(*id);
                (StatusCode::NO_CONTENT, String::new())
            }
            None => (StatusCode::NOT_FOUND, String::new()),
        },
        _ => (StatusCode::BAD_REQUEST, String::new()),
    };
    r.seen.push(seen);
    reply
}

#[test]
fn the_mock_registry_expires_a_silent_node() {
    let mut r = Registry::default();
    r.resources.insert("n".into(), "node".into());
    r.resources.insert("d".into(), "device".into());
    r.heartbeats
        .insert("n".into(), Instant::now() - EXPIRY - Duration::from_secs(1));
    r.expire();
    assert!(r.resources.is_empty() && r.heartbeats.is_empty());
}
