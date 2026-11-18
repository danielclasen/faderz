//! The device sign-in and the refresh against a stand-in provider.

mod idp;

use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use faderz_bridge::auth::{self, DeviceCode, Failure, Session, Signed, Tokens};
use faderz_bridge::bridge::Credentials;
use idp::{CLIENT, Idp, USER_CODE};

async fn session(idp: &Idp) -> (Session, std::path::PathBuf) {
    let http = auth::client().unwrap();
    let provider = auth::discover(&http, &idp.issuer).await.unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bridge-auth-{}",
        faderz_bridge::session::random_id().unwrap()
    ));
    let session = Session {
        http,
        provider,
        client_id: CLIENT.into(),
        scope: "openid offline_access".into(),
        file: dir.join("tokens.json"),
        retry: Duration::from_millis(50),
        writer: auth::store,
    };
    (session, dir)
}

#[tokio::test]
async fn a_device_sign_in_shows_the_code_polls_and_stores_the_tokens_privately() {
    let idp = Idp::start(300).await;
    let (s, dir) = session(&idp).await;
    let shown = Mutex::new(Vec::<String>::new());
    let show = |c: &DeviceCode| shown.lock().unwrap().push(c.user_code.clone());
    let tokens = s.sign_in(&show).await.unwrap();
    assert_eq!(*shown.lock().unwrap(), vec![USER_CODE.to_owned()]);
    // One authorization_pending, then the tokens.
    assert_eq!(idp.book().polls, 2);
    assert_eq!(
        idp.book().device_requests[0]["scope"],
        "openid offline_access"
    );
    assert_eq!(tokens.expires_at - tokens.issued_at, 300);
    let stored = auth::load(&s.file).unwrap();
    assert_eq!(stored, tokens);
    let mode = std::fs::metadata(&s.file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // A restart refreshes with the stored token instead of signing in.
    let again = s.sign_in(&show).await.unwrap();
    assert_eq!(shown.lock().unwrap().len(), 1);
    assert_eq!(idp.book().refreshes, 1);
    assert_ne!(again.access_token, tokens.access_token);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_refused_sign_in_fails_and_a_revoked_refresh_token_signs_in_again() {
    let idp = Idp::start(300).await;
    idp.book().pending = None;
    let (s, dir) = session(&idp).await;
    let e = s.sign_in(&|_| {}).await.unwrap_err();
    assert!(e.contains("access_denied"), "{e}");

    idp.book().pending = Some(0);
    s.sign_in(&|_| {}).await.unwrap();
    idp.book().revoke();
    let shown = Mutex::new(0);
    s.sign_in(&|_| *shown.lock().unwrap() += 1).await.unwrap();
    assert_eq!(*shown.lock().unwrap(), 1, "signed in again");
    let r = auth::refresh(&s.http, &s.provider, CLIENT, "r-unknown").await;
    assert_eq!(r.unwrap_err(), Failure::Refused("invalid_grant".into()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn the_access_token_is_refreshed_before_it_expires_and_the_bridge_told() {
    // Timestamps are whole seconds and keep_fresh sleeps whole seconds from a
    // truncated clock, so the refresh lands up to a second after refresh_at.
    // A life of 8 s refreshes 2 s before expiry, which that second does not
    // eat.
    let idp = Idp::start(8).await;
    idp.book().pending = Some(0);
    let (mut s, dir) = session(&idp).await;
    // No disk in the timed part: a stalled fsync is not what is measured.
    s.writer = |_, _| Ok(());
    let first = s.sign_in(&|_| {}).await.unwrap();
    assert_eq!(first.expires_at - first.refresh_at(), 2);
    let signed = Signed::new(first.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let keeper = signed.clone();
    let s2 = s.clone();
    tokio::spawn(async move { s2.keep_fresh(keeper, tx, &|_| {}).await });
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("renewed in time")
        .unwrap();
    let now = auth::unix_now();
    assert!(
        now < first.expires_at,
        "the bridge was told before the old token expired"
    );
    let renewed = signed.tokens();
    assert!(
        renewed.issued_at < first.expires_at,
        "renewed before the old token expired"
    );
    assert_ne!(signed.bearer(), Some(first.access_token));
    // Nothing was written, so there may be no directory.
    let _ = std::fs::remove_dir_all(dir);
}

/// The tokens a fake disk was given, and how many writes it has seen.
static WRITES: Mutex<Vec<Tokens>> = Mutex::new(Vec::new());
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// A disk that stalls the first write for 3 s and then takes it. Writes
/// go to memory so that the host's own disk cannot add to the timing.
fn stalling_writer(_: &std::path::Path, t: &Tokens) -> std::io::Result<()> {
    if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_secs(3));
    }
    WRITES.lock().unwrap().push(t.clone());
    Ok(())
}

#[tokio::test]
async fn a_stalled_store_does_not_hold_back_the_new_token() {
    let idp = Idp::start(8).await;
    idp.book().pending = Some(0);
    let (mut s, dir) = session(&idp).await;
    s.writer = |_, _| Ok(());
    let first = s.sign_in(&|_| {}).await.unwrap();
    s.writer = stalling_writer;
    let signed = Signed::new(first.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let keeper = signed.clone();
    let s2 = s.clone();
    tokio::spawn(async move { s2.keep_fresh(keeper, tx, &|_| {}).await });
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("renewed")
        .unwrap();
    // The write stalls for longer than the old token has left, and the
    // bridge is told and holds the new token before that.
    assert!(
        auth::unix_now() < first.expires_at,
        "the bridge was told before the old token expired"
    );
    assert!(
        WRITES.lock().unwrap().is_empty(),
        "the write is still stalled"
    );
    let renewed = signed.tokens();
    assert_ne!(signed.bearer(), Some(first.access_token));
    // Then it is written, and what is written is that renewal.
    for _ in 0..600 {
        if !WRITES.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(WRITES.lock().unwrap()[0], renewed);
    let _ = std::fs::remove_dir_all(dir);
}

static LOG: Mutex<Vec<Tokens>> = Mutex::new(Vec::new());
static REFUSALS: AtomicUsize = AtomicUsize::new(0);

/// A disk that refuses the first write and then takes what it is given.
fn refusing_writer(_: &std::path::Path, t: &Tokens) -> std::io::Result<()> {
    if REFUSALS.fetch_add(1, Ordering::SeqCst) == 0 {
        return Err(std::io::Error::other("disk refused"));
    }
    LOG.lock().unwrap().push(t.clone());
    Ok(())
}

#[tokio::test]
async fn a_refresh_during_a_retry_backoff_is_what_gets_written() {
    let idp = Idp::start(4).await;
    idp.book().pending = Some(0);
    let (mut s, dir) = session(&idp).await;
    s.writer = |_, _| Ok(());
    let first = s.sign_in(&|_| {}).await.unwrap();
    s.writer = refusing_writer;
    // The first renewal's write is refused and tried again after 4 s; the
    // next renewal comes about 3 s after the first, inside that wait.
    s.retry = Duration::from_secs(4);
    let signed = Signed::new(first);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let keeper = signed.clone();
    let s2 = s.clone();
    tokio::spawn(async move { s2.keep_fresh(keeper, tx, &|_| {}).await });
    let wait = Duration::from_secs(30);
    tokio::time::timeout(wait, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let older = signed.tokens();
    tokio::time::timeout(wait, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let newest = signed.tokens();
    assert_ne!(older, newest);
    for _ in 0..600 {
        if !LOG.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let log = LOG.lock().unwrap().clone();
    assert_eq!(log.first(), Some(&newest), "the newest tokens are written");
    assert!(!log.contains(&older), "the older tokens are never written");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_provider_failing_at_start_is_waited_for_not_signed_in_again() {
    let idp = Idp::start(300).await;
    idp.book().pending = Some(0);
    let (s, dir) = session(&idp).await;
    s.sign_in(&|_| {}).await.unwrap();
    // The token endpoint answers 503 three times, then works.
    idp.book().failing = 3;
    let shown = Mutex::new(0);
    let tokens = s.sign_in(&|_| *shown.lock().unwrap() += 1).await.unwrap();
    assert_eq!(*shown.lock().unwrap(), 0, "no new sign-in");
    assert_eq!(idp.book().failing, 0);
    assert_eq!(idp.book().refreshes, 1);
    assert_eq!(auth::load(&s.file).unwrap(), tokens);
    // A 401 without an OAuth error body is a refusal, not an outage.
    idp.book().unauthorized = true;
    let r = auth::refresh(
        &s.http,
        &s.provider,
        CLIENT,
        tokens.refresh_token.as_deref().unwrap(),
    )
    .await;
    assert_eq!(r.unwrap_err(), Failure::Refused("unauthorized".into()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_provider_enforcing_pkce_signs_the_bridge_in_and_refuses_a_wrong_verifier() {
    let idp = Idp::start(300).await;
    idp.book().require_pkce = true;
    let (s, dir) = session(&idp).await;
    s.sign_in(&|_| {}).await.unwrap();
    let sent = idp.book().device_requests[0].clone();
    assert_eq!(sent["code_challenge_method"], "S256");
    assert_eq!(sent["code_challenge"].as_str().unwrap().len(), 43);
    // Refresh requests carry no verifier: the stored token is accepted.
    s.sign_in(&|_| {}).await.unwrap();
    assert_eq!(idp.book().refreshes, 1);

    // A verifier that does not match the challenge is refused.
    let mut code = auth::start(&s.http, &s.provider, CLIENT, "openid")
        .await
        .unwrap();
    assert!((43..=128).contains(&code.code_verifier.len()));
    code.code_verifier = "x".repeat(43);
    code.interval = Some(0);
    let r = auth::poll(&s.http, &s.provider, CLIENT, &code).await;
    assert_eq!(r.unwrap_err(), Failure::Refused("invalid_grant".into()));
    std::fs::remove_dir_all(dir).unwrap();
}
