use crate::db;
use crate::Context;
use anyhow::Result;
use uuid::Uuid;

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

            ctx.say(format!(
                "{status_emoji} **Simulation `{run_id}`**\n\
                 • Class/Spec: **{class}/{spec}**\n\
                 • Status: **{status}**\n\
                 • Submitted: {created_at}\n\
                 • Results: <https://example.com/sim/{run_id}>",
                class = run.class,
                spec = run.spec,
                status = run.status,
                created_at = run.created_at.format("%Y-%m-%d %H:%M UTC"),
            ))
            .await?;
        }
    }

    Ok(())
}
