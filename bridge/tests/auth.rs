//! The device sign-in and the refresh against a stand-in provider.

mod idp;

use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::time::Duration;

use faderz_bridge::auth::{self, DeviceCode, Failure, Session, Signed};
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
    let idp = Idp::start(4).await;
    idp.book().pending = Some(0);
    let (s, dir) = session(&idp).await;
    let first = s.sign_in(&|_| {}).await.unwrap();
    let signed = Signed::new(first.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let keeper = signed.clone();
    let s2 = s.clone();
    tokio::spawn(async move { s2.keep_fresh(keeper, tx, &|_| {}).await });
    tokio::time::timeout(Duration::from_secs(4), rx.recv())
        .await
        .expect("renewed in time")
        .unwrap();
    let now = auth::unix_now();
    assert!(
        now < first.expires_at,
        "renewed before the old token expired"
    );
    assert_ne!(signed.bearer(), Some(first.access_token));
    assert_eq!(auth::load(&s.file).unwrap(), signed.tokens());
    std::fs::remove_dir_all(dir).unwrap();
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
