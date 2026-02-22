use crate::Context;
use anyhow::Result;

#[poise::command(slash_command, rename = "health")]
pub async fn health(ctx: Context<'_>) -> Result<()> {
    let db_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&ctx.data().db)
        .await
        .is_ok();

    let client = reqwest::Client::new();
    let sim_url = format!("{}/version", ctx.data().sim_api_base_url.trim_end_matches('/'));
    let sim_ok = match client.get(&sim_url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    };

    let overall = if db_ok && sim_ok { "✅" } else { "⚠️" };
    let db_status = if db_ok { "✅ healthy" } else { "❌ unreachable" };
    let sim_status = if sim_ok { "✅ healthy" } else { "❌ unreachable" };

    ctx.say(format!(
        "{overall} **Health Check**\n\
         • Database: {db_status}\n\
         • Sim API: {sim_status}\n\
         • Sim API URL: {url}",
        url = ctx.data().sim_api_base_url,
    ))
    .await?;

    Ok(())
}