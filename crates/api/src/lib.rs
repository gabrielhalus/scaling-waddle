pub mod auth;
pub mod error;
pub mod health;
pub mod integrations;
pub mod runs;
pub mod state;
#[cfg(test)]
mod testing;
pub mod triggers;
pub mod workflows;

use axum::Router;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .merge(workflows::routes())
        .merge(triggers::routes())
        .merge(runs::routes())
        .merge(integrations::routes())
        .with_state(state)
}
