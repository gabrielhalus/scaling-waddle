pub mod health;
pub mod state;

use axum::Router;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new().merge(health::routes()).with_state(state)
}
