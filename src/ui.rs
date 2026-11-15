//! The web UI, built into the binary from `web/dist` (`npm run build` in
//! `web/` first; a build without it serves no UI). Debug builds embed it
//! too, so a binary built in one place serves it in another.

use axum::Router;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

// Source maps stay out: rolldown orders their asset references
// differently from build to build, so the binary would differ between two
// builds of the same source.
#[derive(Embed)]
#[folder = "web/dist/"]
#[exclude = "*.map"]
#[allow_missing = true]
struct Assets;

/// What the built UI loads, all from its own origin: its scripts, the
/// worker, styles, fonts, the icon, and the API over HTTP and WebSocket.
/// No page may frame it. The WebSocket origin of `host` is named beside
/// `'self'`, which Safari before 15.4 does not take to cover `ws:` and
/// `wss:`. Only a host name or IPv4 address with an optional port, of a
/// DNS name's length at most, is named: CSP source expressions have no
/// form for an IPv6 literal, and anything else could end the directive.
pub fn csp(host: Option<&str>) -> String {
    let ws = host
        .filter(|h| {
            !h.is_empty()
                && h.len() <= MAX_HOST
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
        })
        .map(|h| format!(" ws://{h} wss://{h}"))
        .unwrap_or_default();
    format!(
        "default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self'; \
         font-src 'self'; img-src 'self'; connect-src 'self'{ws}; object-src 'none'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    )
}

/// A DNS name's longest form, plus a port.
const MAX_HOST: usize = 253 + 6;

/// Answered at `/` by a binary built without `web/dist`.
const NO_UI: &str = "This faderz binary was built without its web UI: run `npm run build` \
    in web/ before `cargo build`.\n";

/// Served at `/` on the API listener, beside `/api/v1`.
pub fn router() -> Router {
    Router::new().fallback(serve)
}

async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        p => p,
    };
    let Some(file) = Assets::get(path) else {
        if path == "index.html" {
            return (StatusCode::NOT_FOUND, NO_UI).into_response();
        }
        return StatusCode::NOT_FOUND.into_response();
    };
    // Vite names what it builds under assets/ by content hash.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    // HTTP/2 carries the host in the URI's authority, not in Host.
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()));
    let headers = [
        (header::CONTENT_TYPE, file.metadata.mimetype().to_owned()),
        (header::CACHE_CONTROL, cache.to_owned()),
        (header::CONTENT_SECURITY_POLICY, csp(host)),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (header::X_FRAME_OPTIONS, "DENY".to_owned()),
        // The policy depends on the host it was asked under.
        (header::VARY, "Host".to_owned()),
    ];
    (headers, file.data).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get(path: &str) -> Response {
        let req = Request::get(path).body(Body::empty()).unwrap();
        router().oneshot(req).await.unwrap()
    }

    /// The checks of the built UI need `npm --prefix web run build` before
    /// the build; without it the test checks what such a binary answers.
    #[tokio::test]
    async fn serves_the_built_ui_with_its_types_caching_and_protections() {
        let index = get("/").await;
        if Assets::get("index.html").is_none() {
            eprintln!(
                "web/dist was not built: checking the answer of a binary without the UI only"
            );
            assert_eq!(index.status(), StatusCode::NOT_FOUND);
            let body = to_bytes(index.into_body(), 1 << 20).await.unwrap();
            assert_eq!(body, NO_UI.as_bytes());
            return;
        }
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(index.headers()[header::CONTENT_TYPE], "text/html");
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-cache");
        let protections = |r: &Response| {
            assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
            assert_eq!(r.headers()[header::X_FRAME_OPTIONS], "DENY");
            assert_eq!(r.headers()[header::VARY], "Host");
            let csp = r.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap();
            assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
            assert!(csp.contains("script-src 'self'"), "{csp}");
        };
        protections(&index);
        let html = String::from_utf8(to_bytes(index.into_body(), 1 << 20).await.unwrap().to_vec())
            .unwrap();
        let script = html
            .split("src=\"/")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("index.html loads a script");
        let asset = get(&format!("/{script}")).await;
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(asset.headers()[header::CONTENT_TYPE], "text/javascript");
        assert!(
            asset.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        protections(&asset);
        assert_eq!(get("/nope.js").await.status(), StatusCode::NOT_FOUND);
    }

    /// Without a Host header, as over HTTP/2, the host comes from the
    /// request's authority.
    #[tokio::test]
    async fn the_policy_takes_the_host_from_the_authority_without_a_host_header() {
        if Assets::get("index.html").is_none() {
            eprintln!("web/dist was not built: nothing to serve");
            return;
        }
        let req = Request::get("http://mixer.example:8080/")
            .body(Body::empty())
            .unwrap();
        let r = router().oneshot(req).await.unwrap();
        let csp = r.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("wss://mixer.example:8080"), "{csp}");
    }

    /// Safari before 15.4 does not take 'self' to cover the WebSocket.
    #[test]
    fn the_policy_names_the_websocket_origin_of_the_host() {
        let policy = csp(Some("mixer.example:8443"));
        assert!(
            policy.contains("connect-src 'self' ws://mixer.example:8443 wss://mixer.example:8443;"),
            "{policy}"
        );
        assert!(
            csp(Some("[::1]:8080")).contains("connect-src 'self';"),
            "no IPv6 literal"
        );
        let long = format!("{}.example", "a".repeat(MAX_HOST));
        assert!(csp(Some(&long)).contains("connect-src 'self';"), "too long");
        // Nothing that could end the directive or add one gets in.
        for bad in ["evil.example; script-src *", "a b", ""] {
            assert!(csp(Some(bad)).contains("connect-src 'self';"), "{bad}");
        }
        assert!(csp(None).contains("connect-src 'self';"));
    }
}
