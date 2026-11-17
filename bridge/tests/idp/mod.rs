//! A stand-in OpenID Connect provider for the tests: discovery, an ES256
//! key set, the device authorization grant (RFC 8628, with PKCE enforced on request) for the client
//! `faderz-bridge`, and refresh tokens that rotate. Access tokens name
//! the user `alice` with the group `faderz-operator`, the claims faderz in
//! oidc mode reads.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CLIENT: &str = "faderz-bridge";
pub const USER_CODE: &str = "WDJB-MJHT";

pub struct Idp {
    pub issuer: String,
    pub state: Arc<Mutex<Book>>,
}

/// What the provider has handed out and been asked.
pub struct Book {
    key: EcdsaKeyPair,
    /// Seconds an access token lasts.
    pub ttl: u64,
    /// Polls answered `authorization_pending` before the sign-in is
    /// confirmed; `None` refuses it (`access_denied`).
    pub pending: Option<u32>,
    pub polls: u32,
    pub interval: u64,
    /// Valid refresh tokens.
    refresh: HashMap<String, ()>,
    issued: u64,
    pub refreshes: u32,
    /// Token requests answered 503 before the next is served.
    pub failing: u32,
    /// Token requests answered 401 without a body.
    pub unauthorized: bool,
    pub device_requests: Vec<Value>,
    /// Refuses a device authorization request without an S256 challenge
    /// and a device token request whose verifier does not match it, as a
    /// provider enforcing PKCE does.
    pub require_pkce: bool,
    challenge: Option<String>,
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl Book {
    fn access_token(&self, issuer: &str) -> String {
        self.access_token_for(issuer, self.ttl)
    }

    /// An access token of `ttl` seconds, as a screen of alice would hold.
    pub fn access_token_for(&self, issuer: &str, ttl: u64) -> String {
        let iat = now();
        let header = json!({ "alg": "ES256", "typ": "JWT", "kid": "k1" });
        let claims = json!({
            "iss": issuer, "azp": CLIENT, "aud": "account", "sub": "u-alice",
            "preferred_username": "alice", "groups": ["faderz-operator"],
            "typ": "Bearer", "iat": iat, "exp": iat + ttl,
        });
        let input = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(claims.to_string().as_bytes())
        );
        let sig = self
            .key
            .sign(&SystemRandom::new(), input.as_bytes())
            .unwrap();
        format!("{input}.{}", b64(sig.as_ref()))
    }

    fn tokens(&mut self, issuer: &str) -> Value {
        self.issued += 1;
        let refresh = format!("r{}", self.issued);
        self.refresh.insert(refresh.clone(), ());
        json!({
            "access_token": self.access_token(issuer),
            "refresh_token": refresh,
            "token_type": "Bearer",
            "expires_in": self.ttl,
        })
    }

    /// Makes every refresh token invalid, as a revoked session does.
    pub fn revoke(&mut self) {
        self.refresh.clear();
    }
}

fn error(code: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
}

type Shared = (String, Arc<Mutex<Book>>);

async fn discovery(State((issuer, _)): State<Shared>) -> Json<Value> {
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/auth"),
        "token_endpoint": format!("{issuer}/token"),
        "device_authorization_endpoint": format!("{issuer}/device"),
        "jwks_uri": format!("{issuer}/certs"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
    }))
}

async fn certs(State((_, book)): State<Shared>) -> Json<Value> {
    let book = book.lock().unwrap();
    let point = book.key.public_key().as_ref();
    Json(json!({ "keys": [{
        "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
        "x": b64(&point[1..33]), "y": b64(&point[33..65]),
    }] }))
}

async fn device(
    State((issuer, book)): State<Shared>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut book = book.lock().unwrap();
    book.device_requests.push(json!(form));
    if form.get("client_id").map(String::as_str) != Some(CLIENT) {
        return error("invalid_client");
    }
    if book.require_pkce {
        let challenge = form.get("code_challenge").filter(|c| !c.is_empty());
        if form.get("code_challenge_method").map(String::as_str) != Some("S256")
            || challenge.is_none()
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_request",
                    "error_description": "Missing parameter: code_challenge_method",
                })),
            )
                .into_response();
        }
        book.challenge = challenge.cloned();
    }
    Json(json!({
        "device_code": "dc1", "user_code": USER_CODE,
        "verification_uri": format!("{issuer}/device/verify"),
        "verification_uri_complete": format!("{issuer}/device/verify?user_code={USER_CODE}"),
        "expires_in": 60, "interval": book.interval,
    }))
    .into_response()
}

async fn token(
    State((issuer, book)): State<Shared>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut book = book.lock().unwrap();
    if book.unauthorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if book.failing > 0 {
        book.failing -= 1;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if form.get("client_id").map(String::as_str) != Some(CLIENT) {
        return error("invalid_client");
    }
    match form.get("grant_type").map(String::as_str) {
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            if form.get("device_code").map(String::as_str) != Some("dc1") {
                return error("invalid_grant");
            }
            if book.require_pkce {
                let verifier = form.get("code_verifier").map(String::as_str);
                let ok = verifier.is_some_and(|v| {
                    (43..=128).contains(&v.len())
                        && book.challenge.as_deref()
                            == Some(&b64(Sha256::digest(v.as_bytes()).as_ref()))
                });
                if !ok {
                    return error("invalid_grant");
                }
            }
            book.polls += 1;
            match book.pending {
                None => error("access_denied"),
                Some(n) if book.polls <= n => error("authorization_pending"),
                Some(_) => Json(book.tokens(&issuer)).into_response(),
            }
        }
        Some("refresh_token") => {
            let r = form.get("refresh_token").cloned().unwrap_or_default();
            if book.refresh.remove(&r).is_none() {
                return error("invalid_grant");
            }
            book.refreshes += 1;
            Json(book.tokens(&issuer)).into_response()
        }
        _ => error("unsupported_grant_type"),
    }
}

impl Idp {
    pub async fn start(ttl: u64) -> Idp {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}/realms/test", listener.local_addr().unwrap());
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let state = Arc::new(Mutex::new(Book {
            key,
            ttl,
            pending: Some(1),
            polls: 0,
            interval: 1,
            refresh: HashMap::new(),
            issued: 0,
            refreshes: 0,
            failing: 0,
            unauthorized: false,
            device_requests: Vec::new(),
            require_pkce: false,
            challenge: None,
        }));
        let router = Router::new()
            .route(
                "/realms/test/.well-known/openid-configuration",
                get(discovery),
            )
            .route("/realms/test/certs", get(certs))
            .route("/realms/test/device", post(device))
            .route("/realms/test/token", post(token))
            .with_state((issuer.clone(), state.clone()));
        tokio::spawn(async move { axum::serve(listener, router).await });
        Idp { issuer, state }
    }

    pub fn book(&self) -> std::sync::MutexGuard<'_, Book> {
        self.state.lock().unwrap()
    }
}
