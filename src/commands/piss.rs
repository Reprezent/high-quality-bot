use crate::Context;
use crate::iss_telemetry;
use anyhow::Result;

fn fill_status_label(percentage: f64) -> &'static str {
    if percentage >= 90.0 {
        "critically full"
    } else if percentage >= 75.0 {
        "very full"
    } else if percentage >= 50.0 {
        "half full"
    } else if percentage >= 25.0 {
        "filling"
    } else {
        "low"
    }
}

/// Fetch the current ISS urine tank fill level from the public telemetry stream.
#[poise::command(slash_command, rename = "piss")]
pub async fn piss(ctx: Context<'_>) -> Result<()> {
    match iss_telemetry::fetch_iss_urine_telemetry().await {
        Ok(telemetry) => {
            if telemetry.signal_acquired {
                ctx.say(format!(
                    "🧑‍🚀🚽 **ISS Urine Tank**\n\
                    • Fill Level: **{:.2}%** ({})",
                    telemetry.tank_percentage,
                    fill_status_label(telemetry.tank_percentage),
                ))
            .await?;
            } else {
                ctx.say(format!(
                    "🧑‍🚀🚽 **ISS Urine Tank**\n\
                    • Fill Level: ⚠️ lost signal",
                ))
                .await?;
            };


        }
        Err(error) => {
            tracing::warn!(error = ?error, "failed to fetch ISS urine telemetry");
            ctx.say("❌ Failed to fetch ISS urine telemetry right now. Please try again in a bit.")
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fill_status_label;

    #[test]
    fn fill_bands_match_thresholds() {
        assert_eq!(fill_status_label(12.0), "low");
        assert_eq!(fill_status_label(40.0), "filling");
        assert_eq!(fill_status_label(68.0), "half full");
        assert_eq!(fill_status_label(82.0), "very full");
        assert_eq!(fill_status_label(96.0), "critically full");
    }
}