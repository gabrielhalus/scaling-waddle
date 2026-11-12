pub mod auth;
pub mod error;
pub mod health;
pub mod state;
pub mod workflows;

use axum::Router;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .merge(workflows::routes())
        .with_state(state)
}
