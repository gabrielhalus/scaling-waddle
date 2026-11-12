use sw_api::{AppState, router};
use sw_infrastructure::{config::Config, db, telemetry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = Config::from_env()?;
    if let Some(dir) = config.database_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let pool = db::open(&config.database_path).await?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "api démarrée");
    axum::serve(listener, router(AppState::new(pool, &config.jwt_secret)))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("arrêt demandé");
}
