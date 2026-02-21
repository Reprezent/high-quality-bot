use crate::db;
use crate::Context;
use anyhow::Result;

/// Run a World of Warcraft simulation for the given class/spec with your gear.
///
/// Usage: `/sim <class>:<spec> <json gear payload>`
#[poise::command(slash_command, rename = "sim")]
pub async fn sim(
    ctx: Context<'_>,
    #[description = "Class and spec, e.g. warrior:arms"] class_spec: String,
    #[description = "JSON gear payload"] gear_json: String,
) -> Result<()> {
    // Parse class:spec
    let parts: Vec<&str> = class_spec.splitn(2, ':').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        ctx.say("❌ Please provide class and spec in the format `class:spec`, e.g. `warrior:arms`.")
            .await?;
        return Ok(());
    }
    let class = parts[0].trim().to_lowercase();
    let spec = parts[1].trim().to_lowercase();

    // Parse gear JSON
    let gear_payload: serde_json::Value = match serde_json::from_str(&gear_json) {
        Ok(v) => v,
        Err(_) => {
            ctx.say("❌ The gear payload is not valid JSON. Please check your input.").await?;
            return Ok(());
        }
    };

    let user_id = ctx.author().id.to_string();
    let pool = &ctx.data().db;

    // Save run to database
    let run_id = db::create_simulation_run(pool, &user_id, &class, &spec, &gear_payload).await?;

    // Acknowledge quickly so Discord doesn't time out
    ctx.say(format!(
        "✅ Got your sim request for **{class}/{spec}**!\n\
         Your simulation has been queued.\n\
         **Run ID:** `{run_id}`\n\
         Use `/status {run_id}` to check progress.\n\
         Once complete, your results will be available at: <https://example.com/sim/{run_id}>"
    ))
    .await?;

    Ok(())
}
