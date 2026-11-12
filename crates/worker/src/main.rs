use std::time::Duration;

use sw_infrastructure::{config::Config, db, telemetry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init();
    let config = Config::from_env()?;
    if let Some(dir) = config.database_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let pool = db::open(&config.database_path).await?;
    tracing::info!("worker démarré");
    sw_worker::run(pool, Duration::from_millis(500), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    Ok(())
}
