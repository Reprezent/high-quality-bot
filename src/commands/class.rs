use crate::db;
use crate::Context;
use anyhow::Result;

/// Set your default World of Warcraft class and optionally spec.
///
/// Usage: `/class <class>[:<spec>]`
#[poise::command(slash_command, rename = "class")]
pub async fn class(
    ctx: Context<'_>,
    #[description = "Class and optional spec, e.g. `warrior` or `warrior:arms`"] class_spec: String,
) -> Result<()> {
    let parts: Vec<&str> = class_spec.splitn(2, ':').collect();
    let class = parts[0].trim().to_lowercase();
    let spec: Option<String> = parts.get(1).map(|s| s.trim().to_lowercase());

    if class.is_empty() {
        ctx.say("❌ Please provide at least a class name.").await?;
        return Ok(());
    }

    let user_id = ctx.author().id.to_string();
    let pool = &ctx.data().db;

    db::upsert_user_preference(pool, &user_id, &class, spec.as_deref()).await?;

    let spec_display = spec
        .as_deref()
        .map(|s| format!("/**{s}**"))
        .unwrap_or_default();

    ctx.say(format!(
        "✅ Your default class has been set to **{class}**{spec_display}."
    ))
    .await?;

    Ok(())
}
