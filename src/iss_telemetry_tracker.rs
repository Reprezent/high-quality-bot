use crate::{db, iss_telemetry};
use sqlx::PgPool;
use std::time::Duration;
use tokio::task::JoinHandle;

const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub fn spawn(pool: PgPool) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            interval_secs = POLL_INTERVAL.as_secs(),
            "ISS telemetry tracker started"
        );

        loop {
            match iss_telemetry::fetch_iss_urine_telemetry().await {
                Ok(telemetry) => {
                    if let Err(error) = db::insert_iss_telemetry(&pool, &telemetry).await {
                        tracing::warn!(error = ?error, "failed to persist ISS telemetry sample");
                    } else {
                        tracing::debug!(
                            urine = telemetry.tank_percentage,
                            waste = telemetry.waste_water_percentage,
                            clean = telemetry.clean_water_percentage,
                            processor = %telemetry.processor_status,
                            "recorded ISS telemetry sample"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(error = ?error, "failed to fetch ISS telemetry for tracking");
                }
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}
