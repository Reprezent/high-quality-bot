use crate::db;
use crate::Context;
use anyhow::Result;
use uuid::Uuid;

fn format_metric(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "n/a".to_string()
    }
}

/// Check the status of a simulation run.
///
/// Usage: `/status <run-id>`
#[poise::command(slash_command, rename = "status")]
pub async fn status(
    ctx: Context<'_>,
    #[description = "The run ID returned by /sim"] run_id: String,
) -> Result<()> {
    let uuid = match Uuid::parse_str(&run_id) {
        Ok(u) => u,
        Err(_) => {
            ctx.say("❌ Invalid run ID format. Please provide the UUID returned by `/sim`.")
                .await?;
            return Ok(());
        }
    };

    let pool = &ctx.data().db;
    match db::get_simulation_run(pool, uuid).await? {
        None => {
            ctx.say(format!("❌ No simulation found with ID `{run_id}`."))
                .await?;
        }
        Some(run) => {
            let status_emoji = match run.status.as_str() {
                "queued" => "⏳",
                "running" => "⚙️",
                "complete" => "✅",
                "failed" => "❌",
                _ => "❓",
            };

            let progress_line = match db::get_latest_simulation_progress_frame(pool, uuid).await? {
                Some(frame) if frame.total_iterations > 0 => {
                    let dps = format_metric(frame.dps);
                    let hps = format_metric(frame.hps);
                    format!(
                        "• Progress: **{}/{} iterations** ({:.1}%) | DPS {} | HPS {}",
                        frame.completed_iterations,
                        frame.total_iterations,
                        (frame.completed_iterations as f64 / frame.total_iterations as f64) * 100.0,
                        dps,
                        hps,
                    )
                }
                Some(frame) => format!(
                    "• Progress frame #{}, sims {}/{}",
                    frame.frame_index, frame.completed_sims, frame.total_sims
                ),
                None => "• Progress: no frames yet".to_string(),
            };

            let raid_members_line = if run.raid_members.is_empty() {
                "• Raid Members: n/a".to_string()
            } else {
                format!("• Raid Members: {}", run.raid_members.join(", "))
            };

            ctx.say(format!(
                "{status_emoji} **Simulation `{run_id}`**\n\
                 • Class/Spec: **{class}/{spec}**\n\
                 • Status: **{status}**\n\
                 {progress_line}\n\
                 {raid_members_line}\n\
                 • Submitted: {created_at}\n\
                 • Results: <https://example.com/sim/{run_id}>",
                class = run.class,
                spec = run.spec,
                status = run.status,
                progress_line = progress_line,
                raid_members_line = raid_members_line,
                created_at = run.created_at.format("%Y-%m-%d %H:%M UTC"),
            ))
            .await?;
        }
    }

    Ok(())
}
