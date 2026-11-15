//! The web UI, built into the binary from `web/dist` (`npm run build` in
//! `web/` first; a build without it serves no UI). Debug builds embed it
//! too, so a binary built in one place serves it in another.

use axum::Router;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/dist/"]
#[allow_missing = true]
struct Assets;

/// Served at `/` on the API listener, beside `/api/v1`.
pub fn router() -> Router {
    Router::new().fallback(serve)
}

async fn serve(method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        p => p,
    };
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Vite names what it builds under assets/ by content hash.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (header::CONTENT_TYPE, file.metadata.mimetype().to_owned()),
        (header::CACHE_CONTROL, cache.to_owned()),
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

    /// Needs the built UI: `npm --prefix web ci && npm --prefix web run build`.
    #[tokio::test]
    async fn serves_the_built_ui_with_its_types_and_caching() {
        let index = get("/").await;
        assert_eq!(
            index.status(),
            StatusCode::OK,
            "build the UI first: npm --prefix web run build"
        );
        assert_eq!(index.headers()[header::CONTENT_TYPE], "text/html");
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-cache");
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
        assert_eq!(get("/nope.js").await.status(), StatusCode::NOT_FOUND);
    }
}
