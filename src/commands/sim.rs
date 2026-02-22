use crate::db;
use crate::Context;
use anyhow::Result;
use serde_json::Value;

fn normalize_class(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("Class")
        .to_lowercase()
        .replace('_', "")
        .replace('-', "")
}

fn extract_class_spec_from_payload(payload: &Value) -> Option<(String, String)> {
    if let (Some(class), Some(spec)) = (
        payload.get("class").and_then(|value| value.as_str()),
        payload.get("spec").and_then(|value| value.as_str()),
    ) {
        return Some((normalize_class(class), spec.trim().to_lowercase()));
    }

    let player = payload.get("player")?.as_object()?;
    let class = player.get("class")?.as_str()?;

    let spec = if player.contains_key("bloodDeathKnight") {
        "blood"
    } else if player.contains_key("frostDeathKnight") {
        "frost"
    } else if player.contains_key("unholyDeathKnight") {
        "unholy"
    } else if player.contains_key("balanceDruid") {
        "balance"
    } else if player.contains_key("feralDruid") {
        "feral"
    } else if player.contains_key("guardianDruid") {
        "guardian"
    } else if player.contains_key("restorationDruid") {
        "restoration"
    } else if player.contains_key("beastMasteryHunter") {
        "beastmastery"
    } else if player.contains_key("marksmanshipHunter") {
        "marksmanship"
    } else if player.contains_key("survivalHunter") {
        "survival"
    } else if player.contains_key("arcaneMage") {
        "arcane"
    } else if player.contains_key("fireMage") {
        "fire"
    } else if player.contains_key("frostMage") {
        "frost"
    } else if player.contains_key("brewmasterMonk") {
        "brewmaster"
    } else if player.contains_key("mistweaverMonk") {
        "mistweaver"
    } else if player.contains_key("windwalkerMonk") {
        "windwalker"
    } else if player.contains_key("holyPaladin") {
        "holy"
    } else if player.contains_key("protectionPaladin") {
        "protection"
    } else if player.contains_key("retributionPaladin") {
        "retribution"
    } else if player.contains_key("disciplinePriest") {
        "discipline"
    } else if player.contains_key("holyPriest") {
        "holy"
    } else if player.contains_key("shadowPriest") {
        "shadow"
    } else if player.contains_key("assassinationRogue") {
        "assassination"
    } else if player.contains_key("combatRogue") {
        "combat"
    } else if player.contains_key("subtletyRogue") {
        "subtlety"
    } else if player.contains_key("elementalShaman") {
        "elemental"
    } else if player.contains_key("enhancementShaman") {
        "enhancement"
    } else if player.contains_key("restorationShaman") {
        "restoration"
    } else if player.contains_key("afflictionWarlock") {
        "affliction"
    } else if player.contains_key("demonologyWarlock") {
        "demonology"
    } else if player.contains_key("destructionWarlock") {
        "destruction"
    } else if player.contains_key("armsWarrior") {
        "arms"
    } else if player.contains_key("furyWarrior") {
        "fury"
    } else if player.contains_key("protectionWarrior") {
        "protection"
    } else {
        return None;
    };

    Some((normalize_class(class), spec.to_string()))
}

fn format_metric(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "n/a".to_string()
    }
}

/// Run a World of Warcraft simulation from a WoWSims JSON payload.
///
/// Usage: `/sim <json payload>`
#[poise::command(slash_command, rename = "sim")]
pub async fn sim(
    ctx: Context<'_>,
    #[description = "WoWSims JSON payload (must include player.class + spec)"] gear_json: String,
) -> Result<()> {
    // Parse gear JSON
    let gear_payload: serde_json::Value = match serde_json::from_str(&gear_json) {
        Ok(v) => v,
        Err(_) => {
            ctx.say("❌ The gear payload is not valid JSON. Please check your input.").await?;
            return Ok(());
        }
    };

    let Some((class, spec)) = extract_class_spec_from_payload(&gear_payload) else {
        ctx.say(
            "❌ Could not determine class/spec from payload. Include `player.class` and a spec section like `frostMage`, `armsWarrior`, etc.",
        )
        .await?;
        return Ok(());
    };

    let user_id = ctx.author().id.to_string();
    let user_id_for_reply = ctx.author().id;
    let channel_id = ctx.channel_id();
    let http = ctx.serenity_context().http.clone();
    let pool = &ctx.data().db;

    // Save run to database
    let run_id = db::create_simulation_run(pool, &user_id, &class, &spec, &gear_payload).await?;

    let pool_for_task = pool.clone();
    let sim_api_base_url = ctx.data().sim_api_base_url.clone();
    tokio::spawn(async move {
        if let Err(err) = crate::sim_runtime::run_async_simulation(pool_for_task.clone(), sim_api_base_url, run_id).await {
            tracing::error!(run_id = %run_id, error = ?err, "async simulation failed");
            let _ = db::update_simulation_run_status(&pool_for_task, run_id, "failed").await;
        }

        let completion_message = match db::get_simulation_run(&pool_for_task, run_id).await {
            Ok(Some(run)) => {
                let mention = format!("<@{}>", user_id_for_reply.get());

                let progress_line = match db::get_latest_simulation_progress_frame(&pool_for_task, run_id).await {
                    Ok(Some(frame)) if frame.total_iterations > 0 => {
                        let dps = format_metric(frame.dps);
                        let hps = format_metric(frame.hps);
                        format!(
                            "• Final Progress: **{}/{} iterations** ({:.1}%) | DPS {} | HPS {}",
                            frame.completed_iterations,
                            frame.total_iterations,
                            (frame.completed_iterations as f64 / frame.total_iterations as f64) * 100.0,
                            dps,
                            hps,
                        )
                    }
                    Ok(Some(frame)) => format!(
                        "• Final Progress: frame #{}, sims {}/{}",
                        frame.frame_index, frame.completed_sims, frame.total_sims
                    ),
                    Ok(None) => "• Final Progress: no frames recorded".to_string(),
                    Err(_) => "• Final Progress: unavailable".to_string(),
                };

                let status_emoji = if run.status == "complete" { "✅" } else { "❌" };
                let status_label = if run.status == "complete" {
                    "Complete"
                } else {
                    "Failed"
                };

                format!(
                    "{status_emoji} {mention} sim **{class}/{spec}**: **{status_label}**.\n\
                     {progress_line}\n\
                     • Run ID: `{run_id}`",
                    class = run.class,
                    spec = run.spec,
                )
            }
            Ok(None) => format!(
                "❌ <@{}> sim `{run_id}` finished, but details were not found.",
                user_id_for_reply.get()
            ),
            Err(_) => format!(
                "❌ <@{}> sim `{run_id}` finished, but I couldn't load final status.",
                user_id_for_reply.get()
            ),
        };

        if let Err(error) = channel_id.say(&http, completion_message).await {
            tracing::warn!(run_id = %run_id, error = ?error, "failed to send simulation completion message");
        }
    });

    // Acknowledge quickly so Discord doesn't time out
    ctx.say(format!(
        "✅ Got your sim request for **{class}/{spec}**!\n\
         • Run ID: `{run_id}`"
    ))
    .await?;

    Ok(())
}
