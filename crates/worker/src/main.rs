use std::time::Duration;

use sw_engine::netguard::Policy;
use sw_infrastructure::{config::Config, db, shutdown, telemetry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = Config::worker_from_env()?;
    if let Some(dir) = config.database_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let pool = db::open(&config.database_path).await?;
    tracing::info!("worker démarré");
    let settings = sw_worker::Settings {
        owner: format!("worker-{}", uuid::Uuid::new_v4()),
        lease_ms: 30_000,
        backoff_unit: Duration::from_secs(1),
        policy: Policy::parse_allowed(&config.allowed_internal_networks)?,
        max_concurrent_runs: config.max_concurrent_runs,
        retention_days: config.retention_days,
        keyring: config.keyring,
    };
    sw_worker::run(
        pool,
        settings,
        Duration::from_millis(500),
        shutdown::signal(),
    )
    .await;
    Ok(())
}
