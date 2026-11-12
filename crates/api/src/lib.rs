pub mod auth;
pub mod error;
pub mod health;
pub mod state;

use axum::Router;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .with_state(state)
}
