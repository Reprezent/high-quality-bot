use crate::{
    Context,
    db::{self, NewWclSubscription},
    warcraft_logs_tracker,
};
use anyhow::Result;
use chrono::{Duration, Utc};
use poise::serenity_prelude as serenity;
use serenity::{ChannelType, GuildChannel, Permissions};

const BASELINE_LOOKBACK_HOURS: i64 = 24;

/// Configure Warcraft Logs report and boss-kill announcements.
#[poise::command(slash_command, guild_only, subcommands("track", "untrack", "status"))]
pub async fn warcraftlogs(_: Context<'_>) -> Result<()> {
    Ok(())
}

/// Track a public Warcraft Logs guild in this Discord server.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn track(
    ctx: Context<'_>,
    #[description = "Warcraft Logs guild name"] guild: String,
    #[description = "Realm name or Warcraft Logs server slug"] server: String,
    #[description = "Warcraft Logs region, such as US or EU"] region: String,
    #[description = "Text channel where announcements should be posted"] channel: GuildChannel,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let Some(client) = ctx.data().wcl_client.as_ref() else {
        ctx.say(
            "Warcraft Logs tracking is not configured. Set the bot's \
             `WARCRAFT_LOGS_CLIENT_ID` and `WARCRAFT_LOGS_CLIENT_SECRET` environment variables.",
        )
        .await?;
        return Ok(());
    };

    if !matches!(channel.kind, ChannelType::Text | ChannelType::News) {
        ctx.say("Choose a standard text or announcement channel.")
            .await?;
        return Ok(());
    }

    let Some(discord_guild_id) = ctx.guild_id() else {
        ctx.say("This command can only be used in a Discord server.")
            .await?;
        return Ok(());
    };
    if channel.guild_id != discord_guild_id {
        ctx.say("The destination channel must belong to this Discord server.")
            .await?;
        return Ok(());
    }

    let bot_user = ctx.http().get_current_user().await?;
    let bot_member = discord_guild_id.member(ctx.http(), bot_user.id).await?;
    let partial_guild = discord_guild_id.to_partial_guild(ctx.http()).await?;
    let permissions = partial_guild.user_permissions_in(&channel, &bot_member);
    let required =
        Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::EMBED_LINKS;
    if !permissions.contains(required) {
        ctx.say(format!(
            "I need View Channel, Send Messages, and Embed Links permissions in <#{}>.",
            channel.id
        ))
        .await?;
        return Ok(());
    }

    let guild_name = guild.trim();
    if guild_name.is_empty() {
        ctx.say("The Warcraft Logs guild name cannot be empty.")
            .await?;
        return Ok(());
    }
    let server_slug = normalize_server_slug(&server);
    if server_slug.is_empty() {
        ctx.say("The realm/server value cannot be empty.").await?;
        return Ok(());
    }
    let Some(region) = normalize_region(&region) else {
        ctx.say("Region must be one of US, EU, KR, TW, or CN.")
            .await?;
        return Ok(());
    };

    let wcl_guild = match client.lookup_guild(guild_name, &server_slug, region).await {
        Ok(Some(guild)) => guild,
        Ok(None) => {
            ctx.say(format!(
                "I could not find a public Warcraft Logs guild named **{}** on **{}-{}**.",
                guild_name, server_slug, region
            ))
            .await?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!(error = ?error, "failed to validate Warcraft Logs guild");
            ctx.say(
                "Warcraft Logs could not validate that guild. Check the guild, realm, and region, \
                 then try again.",
            )
            .await?;
            return Ok(());
        }
    };

    let baseline_time_ms = Utc::now().timestamp_millis();
    let baseline_start_ms =
        (Utc::now() - Duration::hours(BASELINE_LOOKBACK_HOURS)).timestamp_millis();
    let baseline_discovery = match client
        .reports_since(wcl_guild.id, baseline_start_ms, baseline_time_ms)
        .await
    {
        Ok(discovery) => discovery,
        Err(error) => {
            tracing::error!(
                error = ?error,
                wcl_guild_id = wcl_guild.id,
                "failed to establish Warcraft Logs baseline"
            );
            ctx.say(
                "The guild was found, but Warcraft Logs could not establish the current report \
                 baseline. No tracking settings were changed.",
            )
            .await?;
            return Ok(());
        }
    };
    let baseline = baseline_discovery
        .reports
        .iter()
        .filter(|report| report.visibility.eq_ignore_ascii_case("public"))
        .map(warcraft_logs_tracker::report_record_from_api)
        .collect::<Result<Vec<_>>>()?;
    let mut baseline_fights = Vec::new();
    for report in baseline.iter().filter(|report| {
        report
            .end_time_ms
            .is_none_or(|end_time| end_time >= baseline_time_ms - 30 * 60 * 1_000)
    }) {
        let details = match client.report_fights(&report.code).await {
            Ok(details) => details,
            Err(error) => {
                tracing::error!(
                    error = ?error,
                    report_code = %report.code,
                    "failed to establish Warcraft Logs fight baseline"
                );
                ctx.say(
                    "The guild was found, but Warcraft Logs could not establish the current fight \
                     baseline. No tracking settings were changed.",
                )
                .await?;
                return Ok(());
            }
        };
        baseline_fights.push((
            report.code.clone(),
            warcraft_logs_tracker::fight_records_from_api(&details.fights)?,
        ));
    }

    db::replace_wcl_subscription(
        &ctx.data().db,
        NewWclSubscription {
            discord_guild_id: &discord_guild_id.to_string(),
            discord_channel_id: &channel.id.to_string(),
            wcl_guild_id: wcl_guild.id,
            wcl_guild_name: &wcl_guild.name,
            server_slug: &wcl_guild.server.slug,
            server_name: &wcl_guild.server.name,
            region,
            baseline_time_ms,
        },
        &baseline,
        &baseline_fights,
    )
    .await?;

    ctx.say(format!(
        "Now tracking public reports for **{}** on **{}-{}**. New reports and boss kills will be \
         posted in <#{}>; existing reports and kills were recorded without announcements.",
        wcl_guild.name, wcl_guild.server.name, region, channel.id
    ))
    .await?;

    Ok(())
}

/// Stop Warcraft Logs tracking in this Discord server.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn untrack(ctx: Context<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("This command can only be used in a Discord server.")
            .await?;
        return Ok(());
    };

    if db::remove_wcl_subscription(&ctx.data().db, &guild_id.to_string()).await? {
        ctx.say("Warcraft Logs tracking has been disabled for this Discord server.")
            .await?;
    } else {
        ctx.say("This Discord server is not currently tracking a Warcraft Logs guild.")
            .await?;
    }

    Ok(())
}

/// Show Warcraft Logs tracking status for this Discord server.
#[poise::command(slash_command, guild_only)]
pub async fn status(ctx: Context<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("This command can only be used in a Discord server.")
            .await?;
        return Ok(());
    };

    let Some(subscription) =
        db::get_wcl_subscription(&ctx.data().db, &guild_id.to_string()).await?
    else {
        ctx.say("This Discord server is not currently tracking a Warcraft Logs guild.")
            .await?;
        return Ok(());
    };

    let last_poll = subscription
        .last_polled_at
        .map(|timestamp| format!("<t:{}:R>", timestamp.timestamp()))
        .unwrap_or_else(|| "Waiting for the first poll".to_owned());
    let health = if subscription.last_error.is_some() {
        "⚠️ The latest polling attempt failed and will be retried."
    } else {
        "✅ Healthy"
    };

    ctx.say(format!(
        "**Warcraft Logs Tracking**\n\
         • Guild: **{}**\n\
         • Realm: **{}-{}**\n\
         • Destination: <#{}>\n\
         • Last poll: {}\n\
         • Status: {}",
        subscription.wcl_guild_name,
        subscription.server_name,
        subscription.region,
        subscription.discord_channel_id,
        last_poll,
        health
    ))
    .await?;

    Ok(())
}

fn normalize_region(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_uppercase().as_str() {
        "US" => Some("US"),
        "EU" => Some("EU"),
        "KR" => Some("KR"),
        "TW" => Some("TW"),
        "CN" => Some("CN"),
        _ => None,
    }
}

fn normalize_server_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;

    for character in value.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_separator = false;
        } else if character == '\'' || character == '’' {
            continue;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('-');
            previous_separator = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::{normalize_region, normalize_server_slug};

    #[test]
    fn normalizes_region_values() {
        assert_eq!(normalize_region(" us "), Some("US"));
        assert_eq!(normalize_region("EU"), Some("EU"));
        assert_eq!(normalize_region("moon"), None);
    }

    #[test]
    fn normalizes_server_names_to_slugs() {
        assert_eq!(normalize_server_slug("Area 52"), "area-52");
        assert_eq!(normalize_server_slug("Zul'jin"), "zuljin");
        assert_eq!(normalize_server_slug("  Tarren  Mill  "), "tarren-mill");
    }
}
