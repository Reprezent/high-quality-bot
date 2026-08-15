use crate::Context;
use crate::db;
use crate::sim_request_codec::{INPUT_FORMAT_GEAR_JSON, MOP_UPSTREAM_REVISION};
use anyhow::Result;
use serde_json::Value;
use uuid::Uuid;

fn normalize_class(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("Class")
        .to_ascii_lowercase()
        .replace('_', "")
        .replace('-', "")
}

fn extract_class_spec_from_payload(payload: &Value) -> Option<(String, String)> {
    if let (Some(class), Some(spec)) = (
        payload.get("class").and_then(|value| value.as_str()),
        payload.get("spec").and_then(|value| value.as_str()),
    ) {
        return Some((normalize_class(class), spec.trim().to_ascii_lowercase()));
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

/// Run a World of Warcraft simulation from a gear profile.
///
/// Usage: `/sim <gear_json>`
#[poise::command(slash_command, rename = "sim")]
pub async fn sim(
    ctx: Context<'_>,
    #[description = "Gear profile JSON (must include class, spec, and gear.items)"]
    gear_json: String,
) -> Result<()> {
    let source_payload = match serde_json::from_str(&gear_json) {
        Ok(payload) => payload,
        Err(error) => {
            ctx.say(format!("❌ The gear profile is not valid JSON: {}", error))
                .await?;
            return Ok(());
        }
    };

    let Some((class, spec)) = extract_class_spec_from_payload(&source_payload) else {
        ctx.say(
            "❌ Could not determine class/spec. Include top-level `class` and `spec`, or a WoWSims `player` object with its spec section.",
        )
        .await?;
        return Ok(());
    };

    let run_id = Uuid::new_v4();
    let user_id = ctx.author().id.to_string();
    let user_id_for_reply = ctx.author().id;
    let channel_id = ctx.channel_id();
    let http = ctx.serenity_context().http.clone();
    let pool = &ctx.data().db;

    db::create_simulation_run(
        pool,
        &db::NewSimulationRun {
            run_id,
            discord_user_id: &user_id,
            class: &class,
            spec: &spec,
            source_payload: &source_payload,
            input_format: INPUT_FORMAT_GEAR_JSON,
            upstream_revision: Some(MOP_UPSTREAM_REVISION),
            normalized_request: None,
            effective_random_seed: None,
            effective_iterations: None,
        },
    )
    .await?;

    let pool_for_task = pool.clone();
    let sim_api_base_url = ctx.data().sim_api_base_url.clone();
    tokio::spawn(async move {
        if let Err(err) = crate::sim_runtime::run_async_simulation(
            pool_for_task.clone(),
            sim_api_base_url,
            run_id,
        )
        .await
        {
            tracing::error!(run_id = %run_id, error = ?err, "async simulation failed");
            let _ = db::update_simulation_run_status(&pool_for_task, run_id, "failed").await;
        }

        let completion_message = match db::get_simulation_run(&pool_for_task, run_id).await {
            Ok(Some(run)) => {
                let mention = format!("<@{}>", user_id_for_reply.get());

                let progress_line =
                    match db::get_latest_simulation_progress_frame(&pool_for_task, run_id).await {
                        Ok(Some(frame)) if frame.total_iterations > 0 => {
                            let dps = format_metric(frame.dps);
                            let hps = format_metric(frame.hps);
                            format!(
                                "• Final Progress: **{}/{} iterations** ({:.1}%) | DPS {} | HPS {}",
                                frame.completed_iterations,
                                frame.total_iterations,
                                (frame.completed_iterations as f64 / frame.total_iterations as f64)
                                    * 100.0,
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

                let status_emoji = if run.status == "complete" {
                    "✅"
                } else {
                    "❌"
                };
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
        "✅ Got your gear profile for **{class}/{spec}**!\n\
         • Server defaults will supply raid buffs, encounter, sim options, and missing player settings.\n\
         • Run ID: `{run_id}`",
    ))
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_class_and_spec_from_a_flat_gear_profile() {
        let payload = json!({
            "class": "mage",
            "spec": "arcane",
            "gear": { "items": [] },
        });

        assert_eq!(
            extract_class_spec_from_payload(&payload),
            Some(("mage".to_string(), "arcane".to_string()))
        );
    }

    #[test]
    fn reads_class_and_spec_from_a_wowsims_player_payload() {
        let payload = json!({
            "player": {
                "class": "ClassMage",
                "arcaneMage": {},
            },
        });

        assert_eq!(
            extract_class_spec_from_payload(&payload),
            Some(("mage".to_string(), "arcane".to_string()))
        );
    }
}
