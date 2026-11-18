//! Runs the bridge: the engine's connections, the surfaces' MIDI ports and
//! the mapping between them, in one task.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::engine::{Action, Engine, LinkId};
use crate::midi::{Link, Port};
use crate::surface::{Surface, UnitSpec};

/// How often the surfaces are brought up to date at least: the motor pace.
const FRAME_MS: u64 = crate::surface::MOTOR_INTERVAL_MS;

/// Where a new connection gets its bearer token.
pub trait Credentials: Send + Sync {
    fn bearer(&self) -> Option<String>;
}

/// One token for every connection, or none.
pub struct Static(pub Option<String>);

impl Credentials for Static {
    fn bearer(&self) -> Option<String> {
        self.0.clone()
    }
}

pub struct Options {
    /// WebSocket endpoints, one per replica.
    pub endpoints: Vec<String>,
    pub session: String,
    pub credentials: Arc<dyn Credentials>,
    pub units: Vec<UnitSpec>,
    pub master: Option<String>,
    /// A message here says the credentials changed: the session moves to
    /// a connection opened with the new ones.
    pub renewed: Option<UnboundedReceiver<()>>,
}

enum Net {
    Opened(LinkId),
    Text(LinkId, String),
    Binary(LinkId),
    Closed(LinkId),
}

/// `endpoint` with `session` as its `?session=` parameter.
pub fn with_session(endpoint: &str, session: &str) -> String {
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{sep}session={session}")
}

fn tls() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for e in &found.errors {
        tracing::warn!("loading the system's CA certificates: {e}");
    }
    let (added, _) = roots.add_parsable_certificates(found.certs);
    tracing::debug!(added, "CA certificates");
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the ring provider supports the default versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(config)
}

/// One WebSocket connection: forwards what arrives, sends what the engine
/// sends, and closes once its sender is dropped.
async fn link(
    id: LinkId,
    url: String,
    bearer: Option<String>,
    tls: Arc<rustls::ClientConfig>,
    mut outgoing: UnboundedReceiver<String>,
    events: UnboundedSender<Net>,
) {
    let request = url.into_client_request().map(|mut r| {
        if let Some(token) = bearer
            && let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            r.headers_mut().insert("authorization", v);
        }
        r
    });
    let connected = match request {
        Ok(request) => {
            tokio_tungstenite::connect_async_tls_with_config(
                request,
                None,
                true,
                Some(tokio_tungstenite::Connector::Rustls(tls)),
            )
            .await
        }
        Err(e) => Err(e),
    };
    let ws = match connected {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::info!(link = id, "connecting: {e}");
            let _ = events.send(Net::Closed(id));
            return;
        }
    };
    let _ = events.send(Net::Opened(id));
    let (mut write, mut read) = ws.split();
    loop {
        tokio::select! {
            out = outgoing.recv() => match out {
                Some(text) => {
                    if write.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = write.close().await;
                    break;
                }
            },
            input = read.next() => match input {
                Some(Ok(Message::Text(t))) => {
                    let _ = events.send(Net::Text(id, t.as_str().to_owned()));
                }
                Some(Ok(Message::Binary(_))) => {
                    let _ = events.send(Net::Binary(id));
                }
                Some(Ok(Message::Close(frame))) => {
                    tracing::info!(link = id, ?frame, "closed by the server");
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    tracing::info!(link = id, "connection failed: {e}");
                    break;
                }
                None => break,
            },
        }
    }
    let _ = events.send(Net::Closed(id));
}

/// Drives the units on `ports` (in the order of `options.units`) until
/// `shutdown` completes.
pub async fn run(options: Options, ports: Vec<Port>, shutdown: impl Future<Output = ()>) {
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let seq_base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut engine = Engine::new(
        options.endpoints.len(),
        options.session.clone(),
        seq_base,
        Box::new(crate::session::uuid),
    );
    let mut surface = Surface::new(options.units, options.master);
    let tls = tls();
    let (net_tx, mut net_rx) = unbounded_channel::<Net>();
    let mut links: HashMap<LinkId, UnboundedSender<String>> = HashMap::new();
    let (midi_tx, mut midi_rx) = unbounded_channel::<(usize, Vec<u8>)>();
    let (link_tx, mut link_rx) = unbounded_channel::<(usize, Link, Link)>();
    let mut outputs = Vec::new();
    let mut kept = Vec::new();
    for (unit, port) in ports.into_iter().enumerate() {
        let mut parts = port.into_parts();
        let tx = midi_tx.clone();
        tokio::spawn(async move {
            while let Some(m) = parts.input.recv().await {
                if tx.send((unit, m)).is_err() {
                    break;
                }
            }
        });
        if let Some(mut link) = parts.link {
            let tx = link_tx.clone();
            tokio::spawn(async move {
                let mut was = *link.borrow_and_update();
                while link.changed().await.is_ok() {
                    let is = *link.borrow_and_update();
                    if tx.send((unit, was, is)).is_err() {
                        break;
                    }
                    was = is;
                }
            });
        }
        outputs.push(parts.output);
        kept.push(parts.open);
    }
    let connect = |id: LinkId, endpoint: usize| {
        let (tx, rx) = unbounded_channel();
        let url = with_session(&options.endpoints[endpoint], &options.session);
        let bearer = options.credentials.bearer();
        tokio::spawn(link(id, url, bearer, tls.clone(), rx, net_tx.clone()));
        tx
    };
    engine.start(now());
    surface.register(&mut engine, now());
    tokio::pin!(shutdown);
    let mut renewed = options.renewed;
    loop {
        // Events can issue commands, and commands are actions.
        loop {
            let t = now();
            for event in engine.take_events() {
                surface.event(&event, &mut engine, t);
            }
            let actions = engine.take_actions();
            if actions.is_empty() {
                break;
            }
            for action in actions {
                match action {
                    Action::Connect { link: id, endpoint } => {
                        links.insert(id, connect(id, endpoint));
                    }
                    Action::Send { link: id, text } => {
                        if let Some(tx) = links.get(&id) {
                            let _ = tx.send(text);
                        }
                    }
                    Action::Close { link: id } => {
                        links.remove(&id);
                    }
                }
            }
        }
        let t = now();
        for (unit, message) in surface.render(&engine, t) {
            let _ = outputs[unit].send(message);
        }
        let wake = engine.next_deadline().min(t + FRAME_MS);
        let sleep = tokio::time::sleep_until(start + Duration::from_millis(wake));
        tokio::select! {
            Some((unit, message)) = midi_rx.recv() => surface.input(unit, &message, &mut engine, now()),
            Some((unit, was, is)) = link_rx.recv() => {
                surface.link_changed(unit, was, is, &mut engine, now());
            }
            Some(n) = net_rx.recv() => match n {
                Net::Opened(id) => engine.opened(id, now()),
                Net::Text(id, text) => engine.received(id, &text, now()),
                Net::Binary(id) => engine.binary(id, now()),
                Net::Closed(id) => {
                    links.remove(&id);
                    engine.closed(id, now());
                }
            },
            Some(()) = async {
                match renewed.as_mut() {
                    Some(r) => r.recv().await,
                    None => std::future::pending().await,
                }
            } => engine.handover(now()),
            () = sleep => {}
            () = &mut shutdown => break,
        }
        let t = now();
        engine.tick(t);
        surface.tick(&mut engine, t);
    }
    drop(kept);
}
