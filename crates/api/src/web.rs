use std::path::Path;

use axum::Router;
use axum::routing::any;
use tower_http::services::{ServeDir, ServeFile};

use crate::error::ApiError;

// Sert l'application web ; toute route inconnue hors /api retombe sur index.html (routage côté client).
pub fn serve(app: Router, dir: &Path) -> Router {
    let files = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    app.route("/api/{*rest}", any(api_not_found))
        .fallback_service(files)
}

async fn api_not_found() -> ApiError {
    ApiError::NotFound
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::testing::{harness, send_raw};

    fn site() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>app</html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log(1)").unwrap();
        dir
    }

    async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
        let (status, _, body) = send_raw(app, "GET", uri, &[], None).await;
        (status, body)
    }

    #[tokio::test]
    async fn static_files_and_the_spa_fallback_are_served() {
        let h = harness().await;
        let dir = site();
        let app = serve(crate::router(h.state.clone()), dir.path());
        assert_eq!(
            get(&app, "/").await,
            (StatusCode::OK, "<html>app</html>".into())
        );
        assert_eq!(
            get(&app, "/runs/abc").await,
            (StatusCode::OK, "<html>app</html>".into())
        );
        assert_eq!(
            get(&app, "/assets/app.js").await,
            (StatusCode::OK, "console.log(1)".into())
        );
    }

    #[tokio::test]
    async fn api_and_health_routes_are_not_shadowed() {
        let h = harness().await;
        let dir = site();
        let app = serve(crate::router(h.state.clone()), dir.path());
        let (status, body) = get(&app, "/health/live").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ok"));
        let (status, body) = get(&app, "/api/v1/auth/setup").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("required"));
    }

    #[tokio::test]
    async fn unknown_api_paths_are_a_json_404_not_the_app() {
        let h = harness().await;
        let dir = site();
        let app = serve(crate::router(h.state.clone()), dir.path());
        let (status, body) = get(&app, "/api/v1/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!body.contains("<html>"));
        let (status, _) = get(&app, "/api/v1/workflows").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
