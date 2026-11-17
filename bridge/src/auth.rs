//! Sign-in at the OIDC provider faderz trusts, by the OAuth 2.0 device
//! authorization grant (RFC 8628): the bridge shows a URL and a code, the
//! user confirms in any browser, and the bridge receives an access token
//! and a refresh token. It keeps them in a file only its owner can read,
//! refreshes the access token a quarter of its lifetime before it expires
//! (faderz closes a connection at the token's `exp`), and asks the engine
//! to move the session to a connection opened with the new one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::Credentials;

/// The grant type of a device code (RFC 8628 3.4).
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// The polling interval when the provider names none (RFC 8628 3.2).
pub const DEFAULT_INTERVAL: u64 = 5;
/// Added to the interval on `slow_down` (RFC 8628 3.5).
pub const SLOW_DOWN: u64 = 5;
/// A failed refresh is tried again after this long while the token lasts.
pub const RETRY: Duration = Duration::from_secs(5);

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The provider's endpoints, from its discovery document.
#[derive(Debug, Clone)]
pub struct Provider {
    pub device_authorization_endpoint: String,
    pub token_endpoint: String,
}

/// What a sign-in or refresh gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds since the Unix epoch.
    pub issued_at: u64,
    pub expires_at: u64,
}

impl Tokens {
    /// When to refresh: a quarter of the lifetime before it ends.
    pub fn refresh_at(&self) -> u64 {
        let life = self.expires_at.saturating_sub(self.issued_at);
        self.expires_at.saturating_sub(life / 4)
    }
}

/// The device authorization response (RFC 8628 3.2).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub interval: Option<u64>,
}

/// Why a token request failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The provider refused the grant (`invalid_grant`, `access_denied`,
    /// `expired_token`, ...): only a new sign-in helps.
    Refused(String),
    /// The provider could not be reached or answered something else: the
    /// same request may succeed later.
    Transient(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(e) => write!(f, "refused: {e}"),
            Failure::Transient(e) => write!(f, "{e}"),
        }
    }
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// Reads the provider's discovery document.
pub async fn discover(http: &reqwest::Client, issuer: &str) -> Result<Provider, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let doc: Value = get_json(http, &url).await?;
    let endpoint = |name: &str| {
        doc[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{url}: no {name}; is the device grant enabled?"))
    };
    Ok(Provider {
        device_authorization_endpoint: endpoint("device_authorization_endpoint")?,
        token_endpoint: endpoint("token_endpoint")?,
    })
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value, String> {
    let res = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let status = res.status();
    let text = res.text().await.map_err(|e| format!("{url}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{url}: {status}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))
}

/// Posts a form to `url`: the JSON answer, or the OAuth error code of a
/// 4xx answer as [`Failure::Refused`].
async fn post_form(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<Value, Failure> {
    let res = http
        .post(url)
        .form(form)
        .send()
        .await
        .map_err(|e| Failure::Transient(format!("{url}: {e}")))?;
    let status = res.status();
    let text = res
        .text()
        .await
        .map_err(|e| Failure::Transient(format!("{url}: {e}")))?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    match body["error"].as_str() {
        Some(code) if status.is_client_error() => Err(Failure::Refused(code.to_owned())),
        _ => Err(Failure::Transient(format!("{url}: {status}"))),
    }
}

/// Starts a sign-in: the code the user enters and where.
pub async fn start(
    http: &reqwest::Client,
    provider: &Provider,
    client_id: &str,
    scope: &str,
) -> Result<DeviceCode, Failure> {
    let body = post_form(
        http,
        &provider.device_authorization_endpoint,
        &[("client_id", client_id), ("scope", scope)],
    )
    .await?;
    serde_json::from_value(body).map_err(|e| Failure::Transient(e.to_string()))
}

fn tokens(body: &Value, previous_refresh: Option<&str>) -> Result<Tokens, Failure> {
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| Failure::Transient("no access_token in the answer".into()))?;
    let now = unix_now();
    Ok(Tokens {
        access_token: access.to_owned(),
        // A provider that does not rotate refresh tokens sends none back.
        refresh_token: body["refresh_token"]
            .as_str()
            .or(previous_refresh)
            .map(str::to_owned),
        issued_at: now,
        expires_at: now + body["expires_in"].as_u64().unwrap_or(300),
    })
}

/// The interval to poll at after an answer to a token request with
/// `error` (RFC 8628 3.5).
pub fn next_interval(interval: u64, error: &str) -> u64 {
    if error == "slow_down" {
        interval + SLOW_DOWN
    } else {
        interval
    }
}

/// Polls until the user confirmed the sign-in, refused it or the code
/// expired.
pub async fn poll(
    http: &reqwest::Client,
    provider: &Provider,
    client_id: &str,
    code: &DeviceCode,
) -> Result<Tokens, Failure> {
    let mut interval = code.interval.unwrap_or(DEFAULT_INTERVAL);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::Refused("expired_token".into()));
        }
        let form = [
            ("grant_type", DEVICE_GRANT),
            ("device_code", code.device_code.as_str()),
            ("client_id", client_id),
        ];
        match post_form(http, &provider.token_endpoint, &form).await {
            Ok(body) => return tokens(&body, None),
            Err(Failure::Refused(e)) if e == "authorization_pending" || e == "slow_down" => {
                interval = next_interval(interval, &e);
            }
            Err(Failure::Transient(e)) => tracing::warn!("polling for the sign-in: {e}"),
            Err(e) => return Err(e),
        }
    }
}

pub async fn refresh(
    http: &reqwest::Client,
    provider: &Provider,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens, Failure> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    let body = post_form(http, &provider.token_endpoint, &form).await?;
    tokens(&body, Some(refresh_token))
}

/// The tokens stored at `path`, when it holds any.
pub fn load(path: &Path) -> Option<Tokens> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Stores `tokens` at `path` readable by its owner only.
pub fn store(path: &Path, tokens: &Tokens) -> std::io::Result<()> {
    let text = serde_json::to_string(tokens).map_err(std::io::Error::other)?;
    crate::session::write_private(path, &text)
}

/// The current access token, shared with the connections.
pub struct Signed {
    tokens: RwLock<Tokens>,
}

impl Signed {
    pub fn new(tokens: Tokens) -> Arc<Signed> {
        Arc::new(Signed {
            tokens: RwLock::new(tokens),
        })
    }

    pub fn tokens(&self) -> Tokens {
        self.tokens.read().expect("tokens lock poisoned").clone()
    }

    fn set(&self, tokens: Tokens) {
        *self.tokens.write().expect("tokens lock poisoned") = tokens;
    }
}

impl Credentials for Signed {
    fn bearer(&self) -> Option<String> {
        Some(self.tokens().access_token)
    }
}

/// How to reach the provider and where the tokens live.
#[derive(Clone)]
pub struct Session {
    pub http: reqwest::Client,
    pub provider: Provider,
    pub client_id: String,
    pub scope: String,
    pub file: PathBuf,
}

impl Session {
    /// Tokens to start with: the stored refresh token's, else a new
    /// sign-in, shown through `show`.
    pub async fn sign_in(&self, show: &(dyn Fn(&DeviceCode) + Sync)) -> Result<Tokens, String> {
        if let Some(refresh_token) = load(&self.file).and_then(|t| t.refresh_token) {
            match refresh(&self.http, &self.provider, &self.client_id, &refresh_token).await {
                Ok(tokens) => {
                    self.keep(&tokens);
                    return Ok(tokens);
                }
                Err(e) => {
                    tracing::info!("the stored sign-in is no longer valid ({e}); signing in again")
                }
            }
        }
        self.device(show).await
    }

    async fn device(&self, show: &(dyn Fn(&DeviceCode) + Sync)) -> Result<Tokens, String> {
        let code = start(&self.http, &self.provider, &self.client_id, &self.scope)
            .await
            .map_err(|e| format!("starting the sign-in: {e}"))?;
        show(&code);
        let tokens = poll(&self.http, &self.provider, &self.client_id, &code)
            .await
            .map_err(|e| format!("signing in: {e}"))?;
        self.keep(&tokens);
        Ok(tokens)
    }

    fn keep(&self, tokens: &Tokens) {
        if let Err(e) = store(&self.file, tokens) {
            tracing::warn!("storing the tokens at {}: {e}", self.file.display());
        }
    }

    /// Refreshes the access token in `signed` ahead of its expiry, for
    /// ever, and sends `renewed` each time a new one is in place. A
    /// refused refresh signs in again, shown through `show`.
    pub async fn keep_fresh(
        &self,
        signed: Arc<Signed>,
        renewed: UnboundedSender<()>,
        show: &(dyn Fn(&DeviceCode) + Sync),
    ) {
        loop {
            let current = signed.tokens();
            let wait = current.refresh_at().saturating_sub(unix_now());
            tokio::time::sleep(Duration::from_secs(wait)).await;
            let next = match &current.refresh_token {
                Some(r) => refresh(&self.http, &self.provider, &self.client_id, r).await,
                None => Err(Failure::Refused("no refresh token".into())),
            };
            let tokens = match next {
                Ok(t) => t,
                Err(Failure::Transient(e)) => {
                    tracing::warn!("refreshing the access token: {e}");
                    tokio::time::sleep(RETRY).await;
                    continue;
                }
                Err(Failure::Refused(e)) => {
                    tracing::warn!("the provider refused the refresh ({e}); sign in again");
                    match self.device(show).await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::error!("{e}");
                            tokio::time::sleep(RETRY).await;
                            continue;
                        }
                    }
                }
            };
            self.keep(&tokens);
            signed.set(tokens);
            if renewed.send(()).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_refreshed_a_quarter_of_its_lifetime_before_it_ends() {
        let t = Tokens {
            access_token: "a".into(),
            refresh_token: None,
            issued_at: 1000,
            expires_at: 1300,
        };
        assert_eq!(t.refresh_at(), 1225);
    }

    #[test]
    fn slow_down_adds_five_seconds_and_pending_keeps_the_interval() {
        assert_eq!(next_interval(5, "slow_down"), 10);
        assert_eq!(next_interval(5, "authorization_pending"), 5);
    }

    #[test]
    fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() {
        let body = serde_json::json!({ "access_token": "a2", "expires_in": 60 });
        let t = tokens(&body, Some("r1")).unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("r1"));
        assert_eq!(t.expires_at - t.issued_at, 60);
        let body =
            serde_json::json!({ "access_token": "a3", "refresh_token": "r2", "expires_in": 60 });
        assert_eq!(
            tokens(&body, Some("r1")).unwrap().refresh_token.as_deref(),
            Some("r2")
        );
    }
}
