use crate::Context;
use anyhow::Result;

const BOT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TIMESTAMP_UTC: &str = match option_env!("BUILD_TIMESTAMP_UTC") {
    Some(value) => value,
    None => "unknown",
};

/// Show the bot build version.
#[poise::command(slash_command, rename = "version")]
pub async fn version(ctx: Context<'_>) -> Result<()> {
    ctx.say(format!(
        "🤖 **high-quality-bot**\n\
         • Version: **{BOT_VERSION}**\n\
         • Built: **{BUILD_TIMESTAMP_UTC}**"
    ))
    .await?;

    Ok(())
}