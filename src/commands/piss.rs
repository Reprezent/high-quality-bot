use crate::Context;
use crate::iss_telemetry;
use anyhow::Result;

const BAR_WIDTH: usize = 20;

fn status_bar(percentage: f64) -> String {
    let clamped = percentage.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * BAR_WIDTH as f64).round() as usize;
    let empty = BAR_WIDTH - filled;
    format!("[{}{}] {:.1}%", "█".repeat(filled), "░".repeat(empty), clamped)
}

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
                    "🧑‍🚀🚽 **ISS Water & Waste Status**\n\
                    ```\n\
                    Urine Tank:  {urine_bar} ({urine_label})\n\
                    Waste Water: {waste_bar} ({waste_label})\n\
                    Clean Water: {clean_bar} ({clean_label})\n\
                    Processor:   [{processor}]\n\
                    ```",
                    urine_bar = status_bar(telemetry.tank_percentage),
                    urine_label = fill_status_label(telemetry.tank_percentage),
                    waste_bar = status_bar(telemetry.waste_water_percentage),
                    waste_label = fill_status_label(telemetry.waste_water_percentage),
                    clean_bar = status_bar(telemetry.clean_water_percentage),
                    clean_label = fill_status_label(telemetry.clean_water_percentage),
                    processor = telemetry.processor_status,
                ))
                .await?;
            } else {
                ctx.say(
                    "🧑‍🚀🚽 **ISS Water & Waste Status**\n\
                    ⚠️ Signal lost — telemetry unavailable",
                )
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
    use super::{fill_status_label, status_bar};

    #[test]
    fn fill_bands_match_thresholds() {
        assert_eq!(fill_status_label(12.0), "low");
        assert_eq!(fill_status_label(40.0), "filling");
        assert_eq!(fill_status_label(68.0), "half full");
        assert_eq!(fill_status_label(82.0), "very full");
        assert_eq!(fill_status_label(96.0), "critically full");
    }

    #[test]
    fn status_bar_boundaries() {
        assert!(status_bar(0.0).starts_with("[░░░░░░░░░░░░░░░░░░░░]"));
        assert!(status_bar(100.0).starts_with("[████████████████████]"));
        assert!(status_bar(50.0).contains("50.0%"));
    }
}