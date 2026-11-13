pub mod approvals;
pub mod audit;
pub mod auth;
pub mod connectors;
pub mod error;
pub mod health;
pub mod integrations;
pub mod ratelimit;
pub mod runs;
pub mod secrets;
pub mod signature;
pub mod state;
#[cfg(test)]
mod testing;
pub mod triggers;
pub mod web;
pub mod workflows;

use axum::Router;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .merge(workflows::routes())
        .merge(triggers::routes())
        .merge(connectors::routes())
        .merge(runs::routes())
        .merge(integrations::routes())
        .merge(approvals::routes())
        .merge(audit::routes())
        .merge(secrets::routes())
        .with_state(state)
}
