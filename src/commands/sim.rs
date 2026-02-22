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
    });

    // Acknowledge quickly so Discord doesn't time out
    ctx.say(format!(
        "✅ Got your sim request for **{class}/{spec}**!\n\
         Your simulation has been queued and started asynchronously.\n\
         **Run ID:** `{run_id}`\n\
         Use `/status {run_id}` to check progress.\n\
         Once complete, your results will be available at: <https://example.com/sim/{run_id}>"
    ))
    .await?;

    Ok(())
}
