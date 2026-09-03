use crate::{
    Context,
    db::{self, NewWclSubscription, WclFightRecord, WclPendingFight},
    warcraft_logs::{WarcraftLogsReport, WarcraftLogsSite},
    warcraft_logs_discord, warcraft_logs_tracker,
};
use anyhow::{Context as _, Result, bail};
use chrono::{Duration, Utc};
use percent_encoding::percent_decode_str;
use poise::serenity_prelude as serenity;
use reqwest::Url;
use serenity::{ChannelType, GuildChannel, Permissions};

const BASELINE_LOOKBACK_HOURS: i64 = 24;

/// Configure Warcraft Logs report and boss-kill announcements.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("track", "untrack", "status", "history", "summary")
)]
pub async fn warcraftlogs(_: Context<'_>) -> Result<()> {
    Ok(())
}

#[derive(Clone, Copy, Debug, poise::ChoiceParameter)]
pub enum WarcraftLogsSection {
    #[name = "Classic"]
    Classic,
    #[name = "Retail"]
    Retail,
}

impl From<WarcraftLogsSection> for WarcraftLogsSite {
    fn from(value: WarcraftLogsSection) -> Self {
        match value {
            WarcraftLogsSection::Classic => Self::Classic,
            WarcraftLogsSection::Retail => Self::Retail,
        }
    }
}

#[derive(Debug, PartialEq)]
enum GuildLocator {
    Id {
        site: WarcraftLogsSite,
        guild_id: i64,
    },
    Identity {
        site: WarcraftLogsSite,
        name: String,
        server_slug: String,
        region: String,
    },
}

impl GuildLocator {
    fn site(&self) -> WarcraftLogsSite {
        match self {
            Self::Id { site, .. } | Self::Identity { site, .. } => *site,
        }
    }

    fn lookup_kind(&self) -> &'static str {
        match self {
            Self::Id { .. } => "guild_id",
            Self::Identity { .. } => "guild_identity",
        }
    }
}

#[derive(Debug, PartialEq)]
struct ReportLocator {
    site: WarcraftLogsSite,
    code: String,
    fight_id: Option<i32>,
}

/// Track a public Warcraft Logs guild in this Discord server.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn track(
    ctx: Context<'_>,
    #[description = "Text channel where announcements should be posted"] channel: GuildChannel,
    #[description = "Copied Warcraft Logs guild URL (replaces guild/server/region)"]
    guild_link: Option<String>,
    #[description = "Guild name when not using guild_link"] guild: Option<String>,
    #[description = "Realm name or server slug when not using guild_link"] server: Option<String>,
    #[description = "Region such as US or EU when not using guild_link"] region: Option<String>,
    #[description = "Warcraft Logs section; defaults to Classic for manual lookup"] section: Option<
        WarcraftLogsSection,
    >,
) -> Result<()> {
    ctx.defer().await?;

    let Some(discord_guild_id) = ctx.guild_id() else {
        tracing::warn!(
            user_id = %ctx.author().id,
            "Warcraft Logs track command was invoked outside a Discord guild"
        );
        ctx.say("This command can only be used in a Discord server.")
            .await?;
        return Ok(());
    };
    tracing::info!(
        discord_guild_id = discord_guild_id.get(),
        discord_channel_id = channel.id.get(),
        user_id = ctx.author().id.get(),
        has_guild_link = guild_link.is_some(),
        requested_section = ?section,
        "Starting Warcraft Logs tracker configuration"
    );

    let Some(client) = ctx.data().wcl_client.as_ref() else {
        tracing::warn!(
            discord_guild_id = discord_guild_id.get(),
            "Warcraft Logs tracker configuration rejected because API credentials are unavailable"
        );
        ctx.say(
            "Warcraft Logs tracking is not configured. Set the bot's \
             `WARCRAFT_LOGS_CLIENT_ID` and `WARCRAFT_LOGS_CLIENT_SECRET` environment variables.",
        )
        .await?;
        return Ok(());
    };

    if !matches!(channel.kind, ChannelType::Text | ChannelType::News) {
        tracing::warn!(
            discord_guild_id = discord_guild_id.get(),
            discord_channel_id = channel.id.get(),
            channel_type = ?channel.kind,
            "Warcraft Logs tracker destination is not a supported channel type"
        );
        ctx.say("Choose a standard text or announcement channel.")
            .await?;
        return Ok(());
    }

    if channel.guild_id != discord_guild_id {
        tracing::warn!(
            discord_guild_id = discord_guild_id.get(),
            discord_channel_id = channel.id.get(),
            channel_guild_id = channel.guild_id.get(),
            "Warcraft Logs tracker destination belongs to a different Discord guild"
        );
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
        tracing::warn!(
            discord_guild_id = discord_guild_id.get(),
            discord_channel_id = channel.id.get(),
            bot_permissions = ?permissions,
            "Warcraft Logs tracker destination is missing bot permissions"
        );
        ctx.say(format!(
            "I need View Channel, Send Messages, and Embed Links permissions in <#{}>.",
            channel.id
        ))
        .await?;
        return Ok(());
    }
    tracing::debug!(
        discord_guild_id = discord_guild_id.get(),
        discord_channel_id = channel.id.get(),
        "Validated Warcraft Logs tracker destination permissions"
    );

    let locator = match build_guild_locator(guild_link, guild, server, region, section) {
        Ok(locator) => locator,
        Err(error) => {
            tracing::warn!(
                error = %error,
                discord_guild_id = discord_guild_id.get(),
                "Warcraft Logs tracker guild input is invalid"
            );
            ctx.say(format!("I could not use that guild information: {error}"))
                .await?;
            return Ok(());
        }
    };
    let site = locator.site();
    tracing::info!(
        discord_guild_id = discord_guild_id.get(),
        wcl_site = site.slug(),
        lookup_kind = locator.lookup_kind(),
        "Looking up Warcraft Logs guild"
    );
    let lookup_result = match &locator {
        GuildLocator::Id { guild_id, .. } => client.lookup_guild_by_id(site, *guild_id).await,
        GuildLocator::Identity {
            name,
            server_slug,
            region,
            ..
        } => client.lookup_guild(site, name, server_slug, region).await,
    };
    let wcl_guild = match lookup_result {
        Ok(Some(guild)) => guild,
        Ok(None) => {
            tracing::warn!(
                discord_guild_id = discord_guild_id.get(),
                wcl_site = site.slug(),
                lookup_kind = locator.lookup_kind(),
                "Warcraft Logs guild lookup returned no guild"
            );
            ctx.say(format!(
                "I could not find that public guild on Warcraft Logs {}.",
                site.display_name()
            ))
            .await?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!(
                error = ?error,
                discord_guild_id = discord_guild_id.get(),
                wcl_site = site.slug(),
                lookup_kind = locator.lookup_kind(),
                "Failed to validate Warcraft Logs guild"
            );
            ctx.say(
                "Warcraft Logs could not validate that guild. Check the link or manual guild \
                 details, then try again.",
            )
            .await?;
            return Ok(());
        }
    };
    let resolved_region = wcl_guild.server.region.compact_name.to_ascii_uppercase();
    tracing::info!(
        discord_guild_id = discord_guild_id.get(),
        wcl_site = site.slug(),
        wcl_guild_id = wcl_guild.id,
        wcl_guild_name = %wcl_guild.name,
        server_slug = %wcl_guild.server.slug,
        region = %resolved_region,
        "Validated Warcraft Logs guild"
    );

    let baseline_time_ms = Utc::now().timestamp_millis();
    let baseline_start_ms =
        (Utc::now() - Duration::hours(BASELINE_LOOKBACK_HOURS)).timestamp_millis();
    let baseline_discovery = match client
        .reports_since(site, wcl_guild.id, baseline_start_ms, baseline_time_ms)
        .await
    {
        Ok(discovery) => discovery,
        Err(error) => {
            tracing::error!(
                error = ?error,
                discord_guild_id = discord_guild_id.get(),
                wcl_site = site.slug(),
                wcl_guild_id = wcl_guild.id,
                "Failed to establish Warcraft Logs report baseline"
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
    tracing::info!(
        discord_guild_id = discord_guild_id.get(),
        wcl_site = site.slug(),
        wcl_guild_id = wcl_guild.id,
        baseline_report_count = baseline.len(),
        "Loaded Warcraft Logs report baseline"
    );
    let mut baseline_fights = Vec::new();
    for report in baseline.iter().filter(|report| {
        report
            .end_time_ms
            .is_none_or(|end_time| end_time >= baseline_time_ms - 30 * 60 * 1_000)
    }) {
        let details = match client.report_fights(site, &report.code).await {
            Ok(details) => details,
            Err(error) => {
                tracing::error!(
                    error = ?error,
                    discord_guild_id = discord_guild_id.get(),
                    wcl_site = site.slug(),
                    wcl_guild_id = wcl_guild.id,
                    report_code = %report.code,
                    "Failed to establish Warcraft Logs fight baseline"
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

    let subscription_id = match db::replace_wcl_subscription(
        &ctx.data().db,
        NewWclSubscription {
            discord_guild_id: &discord_guild_id.to_string(),
            discord_channel_id: &channel.id.to_string(),
            wcl_guild_id: wcl_guild.id,
            wcl_site: site,
            wcl_guild_name: &wcl_guild.name,
            server_slug: &wcl_guild.server.slug,
            server_name: &wcl_guild.server.name,
            region: &resolved_region,
            baseline_time_ms,
        },
        &baseline,
        &baseline_fights,
    )
    .await
    {
        Ok(subscription_id) => subscription_id,
        Err(error) => {
            tracing::error!(
                error = ?error,
                discord_guild_id = discord_guild_id.get(),
                discord_channel_id = channel.id.get(),
                wcl_site = site.slug(),
                wcl_guild_id = wcl_guild.id,
                "Failed to persist Warcraft Logs tracker configuration"
            );
            return Err(error);
        }
    };
    tracing::info!(
        subscription_id,
        discord_guild_id = discord_guild_id.get(),
        discord_channel_id = channel.id.get(),
        wcl_site = site.slug(),
        wcl_guild_id = wcl_guild.id,
        baseline_report_count = baseline.len(),
        baseline_fight_count = baseline_fights
            .iter()
            .map(|(_, fights)| fights.len())
            .sum::<usize>(),
        "Saved Warcraft Logs tracker configuration"
    );

    ctx.say(format!(
        "Now tracking **{}** public reports for **{}** on **{}-{}**. New reports and boss kills \
         will be posted in <#{}>; existing reports and kills were recorded without announcements.",
        site.display_name(),
        wcl_guild.name,
        wcl_guild.server.name,
        resolved_region,
        channel.id
    ))
    .await?;

    Ok(())
}

/// Stop Warcraft Logs tracking in this Discord server.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn untrack(ctx: Context<'_>) -> Result<()> {
    ctx.defer().await?;
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
    ctx.defer().await?;
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
         • Section: **{}**\n\
         • Realm: **{}-{}**\n\
         • Destination: <#{}>\n\
         • Last poll: {}\n\
         • Status: {}",
        subscription.wcl_guild_name,
        subscription.wcl_site.display_name(),
        subscription.server_name,
        subscription.region,
        subscription.discord_channel_id,
        last_poll,
        health
    ))
    .await?;

    Ok(())
}

/// Show the tracked guild's three most recent public reports.
#[poise::command(slash_command, guild_only)]
pub async fn history(ctx: Context<'_>) -> Result<()> {
    ctx.defer().await?;
    let Some(client) = ctx.data().wcl_client.as_ref() else {
        ctx.say(
            "Warcraft Logs tracking is not configured. Set the bot's \
             `WARCRAFT_LOGS_CLIENT_ID` and `WARCRAFT_LOGS_CLIENT_SECRET` environment variables.",
        )
        .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("This command can only be used in a Discord server.")
            .await?;
        return Ok(());
    };
    let Some(subscription) =
        db::get_wcl_subscription(&ctx.data().db, &guild_id.to_string()).await?
    else {
        ctx.say(
            "This Discord server is not tracking a Warcraft Logs guild yet. \
             Run `/warcraftlogs track` first.",
        )
        .await?;
        return Ok(());
    };

    let discovery = match client
        .recent_reports(subscription.wcl_site, subscription.wcl_guild_id, 3)
        .await
    {
        Ok(discovery) => discovery,
        Err(error) => {
            tracing::error!(
                error = ?error,
                wcl_guild_id = subscription.wcl_guild_id,
                "failed to fetch recent Warcraft Logs reports"
            );
            ctx.say(
                "Warcraft Logs could not load recent reports right now. Please try again later.",
            )
            .await?;
            return Ok(());
        }
    };
    let reports = discovery
        .reports
        .iter()
        .filter(|report| report.visibility.eq_ignore_ascii_case("public"))
        .take(3)
        .collect::<Vec<_>>();

    ctx.say(format_recent_reports(
        subscription.wcl_site,
        &subscription.wcl_guild_name,
        &reports,
    ))
    .await?;
    Ok(())
}

/// Preview the boss-kill summary embed for a Warcraft Logs report.
#[poise::command(slash_command, guild_only)]
pub async fn summary(
    ctx: Context<'_>,
    #[description = "Classic or Retail Warcraft Logs report URL"] report_link: String,
) -> Result<()> {
    ctx.defer().await?;
    let Some(client) = ctx.data().wcl_client.as_ref() else {
        ctx.say(
            "Warcraft Logs is not configured. Set the bot's `WARCRAFT_LOGS_CLIENT_ID` and \
             `WARCRAFT_LOGS_CLIENT_SECRET` environment variables.",
        )
        .await?;
        return Ok(());
    };
    let locator = match parse_report_link(&report_link) {
        Ok(locator) => locator,
        Err(error) => {
            tracing::warn!(
                error = %error,
                user_id = ctx.author().id.get(),
                "Warcraft Logs summary link is invalid"
            );
            ctx.say(format!("I could not use that report link: {error}"))
                .await?;
            return Ok(());
        }
    };
    tracing::info!(
        user_id = ctx.author().id.get(),
        wcl_site = locator.site.slug(),
        report_code = %locator.code,
        requested_fight_id = locator.fight_id,
        "Loading Warcraft Logs summary preview"
    );

    let details = match client.report_fights(locator.site, &locator.code).await {
        Ok(details) => details,
        Err(error) => {
            tracing::error!(
                error = ?error,
                wcl_site = locator.site.slug(),
                report_code = %locator.code,
                "Failed to load Warcraft Logs report for summary preview"
            );
            ctx.say("Warcraft Logs could not load that report. Check that it is public and retry.")
                .await?;
            return Ok(());
        }
    };
    let fights = warcraft_logs_tracker::fight_records_from_api(&details.fights)?;
    let Some(fight) = select_summary_fight(&fights, locator.fight_id) else {
        let message = if let Some(fight_id) = locator.fight_id {
            format!(
                "Fight **{fight_id}** is not a completed boss kill in that report. \
                 Use a completed-kill link or omit the fight selector."
            )
        } else {
            "That report does not contain a completed boss kill to summarize.".to_owned()
        };
        ctx.say(message).await?;
        return Ok(());
    };
    let kill_summary = match client
        .kill_summary(locator.site, &locator.code, fight.fight_id)
        .await
    {
        Ok(summary) => summary,
        Err(error) => {
            tracing::error!(
                error = ?error,
                wcl_site = locator.site.slug(),
                report_code = %locator.code,
                fight_id = fight.fight_id,
                "Failed to load Warcraft Logs summary tables"
            );
            ctx.say(
                "Warcraft Logs could not build that fight summary yet. The report tables may \
                 still be processing; please try again shortly.",
            )
            .await?;
            return Ok(());
        }
    };
    let preview = WclPendingFight {
        subscription_id: 0,
        discord_channel_id: ctx.channel_id().to_string(),
        wcl_site: locator.site,
        wcl_guild_name: details
            .guild
            .as_ref()
            .map_or_else(|| "Raid group".to_owned(), |guild| guild.name.clone()),
        report_code: details.code,
        report_title: details.title,
        report_start_time_ms: warcraft_logs_tracker::absolute_milliseconds(details.start_time)?,
        fight: fight.clone(),
    };
    let image = warcraft_logs_discord::render_kill_summary(&preview, &kill_summary)?;
    ctx.send(
        poise::CreateReply::default()
            .embed(warcraft_logs_discord::kill_embed(&preview, &kill_summary))
            .attachment(poise::serenity_prelude::CreateAttachment::bytes(
                image,
                warcraft_logs_discord::FIGHT_IMAGE_NAME,
            )),
    )
    .await?;
    tracing::info!(
        user_id = ctx.author().id.get(),
        wcl_site = locator.site.slug(),
        report_code = %preview.report_code,
        fight_id = preview.fight.fight_id,
        "Rendered Warcraft Logs summary preview"
    );
    Ok(())
}

fn select_summary_fight(
    fights: &[WclFightRecord],
    requested_fight_id: Option<i32>,
) -> Option<&WclFightRecord> {
    match requested_fight_id {
        Some(fight_id) => fights.iter().find(|fight| fight.fight_id == fight_id),
        None => fights.iter().max_by_key(|fight| fight.end_time_ms),
    }
}

fn format_recent_reports(
    site: WarcraftLogsSite,
    guild_name: &str,
    reports: &[&WarcraftLogsReport],
) -> String {
    if reports.is_empty() {
        return format!(
            "No public Warcraft Logs reports were found for **{}**.",
            guild_name
        );
    }

    let entries = reports
        .iter()
        .enumerate()
        .map(|(index, report)| {
            let url = warcraft_logs_discord::report_url(site, &report.code);
            let title = escape_link_text(&report.title);
            let zone = report
                .zone
                .as_ref()
                .map_or("Unknown zone", |zone| zone.name.as_str());
            let timestamp = format_report_timestamp(report.start_time);
            format!(
                "{}. **[{}]({})**\n   {} • {}",
                index + 1,
                title,
                url,
                zone,
                timestamp
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "**Recent Warcraft Logs {} Reports — {}**\n\n{}",
        site.display_name(),
        guild_name,
        entries
    )
}

fn format_report_timestamp(start_time_ms: f64) -> String {
    if !start_time_ms.is_finite() || start_time_ms < 0.0 || start_time_ms > i64::MAX as f64 {
        return "Time unavailable".to_owned();
    }

    let timestamp = (start_time_ms.round() as i64).div_euclid(1_000);
    format!("<t:{timestamp}:F> (<t:{timestamp}:R>)")
}

fn escape_link_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn build_guild_locator(
    guild_link: Option<String>,
    guild: Option<String>,
    server: Option<String>,
    region: Option<String>,
    section: Option<WarcraftLogsSection>,
) -> Result<GuildLocator> {
    if let Some(guild_link) = guild_link {
        if guild.is_some() || server.is_some() || region.is_some() {
            bail!("provide either guild_link or guild/server/region, not both");
        }

        let locator = parse_guild_link(&guild_link)?;
        if let Some(section) = section {
            let requested_site = WarcraftLogsSite::from(section);
            if requested_site != locator.site() {
                bail!(
                    "the selected {} section does not match the link host {}",
                    requested_site.display_name(),
                    locator.site().host()
                );
            }
        }
        return Ok(locator);
    }

    let guild = guild
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .context("guild is required when guild_link is not provided")?;
    let server_slug = server
        .map(|value| normalize_server_slug(&value))
        .filter(|value| !value.is_empty())
        .context("server is required when guild_link is not provided")?;
    let region = region
        .as_deref()
        .and_then(normalize_region)
        .context("region must be one of US, EU, KR, TW, or CN")?;
    let site = section
        .map(WarcraftLogsSite::from)
        .unwrap_or(WarcraftLogsSite::Classic);

    Ok(GuildLocator::Identity {
        site,
        name: guild,
        server_slug,
        region: region.to_owned(),
    })
}

fn parse_report_link(value: &str) -> Result<ReportLocator> {
    let url = Url::parse(value.trim()).context("report_link must be a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("report_link must use http or https");
    }
    let host = url
        .host_str()
        .context("report_link does not contain a host")?;
    let site = WarcraftLogsSite::from_host(host).with_context(|| {
        format!(
            "unsupported Warcraft Logs host {host:?}; use www.warcraftlogs.com or classic.warcraftlogs.com"
        )
    })?;
    let segments = url
        .path_segments()
        .context("report_link does not contain a report path")?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("reports") {
        bail!("report_link must point to a Warcraft Logs report page");
    }
    let code = segments
        .get(1)
        .context("report_link is missing its report code")?
        .to_string();
    if !code
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        bail!("report_link contains an invalid report code");
    }

    let fight_value = url
        .fragment()
        .and_then(|fragment| {
            fragment.split('&').find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key == "fight").then(|| value.to_owned())
            })
        })
        .or_else(|| {
            url.query_pairs()
                .find_map(|(key, value)| (key == "fight").then(|| value.into_owned()))
        });
    let fight_id = match fight_value.as_deref() {
        None | Some("last") => None,
        Some(value) => {
            let fight_id = value
                .parse::<i32>()
                .context("report_link contains an unsupported fight selector")?;
            if fight_id <= 0 {
                bail!("report_link fight ID must be positive");
            }
            Some(fight_id)
        }
    };

    Ok(ReportLocator {
        site,
        code,
        fight_id,
    })
}

fn parse_guild_link(value: &str) -> Result<GuildLocator> {
    let url = Url::parse(value.trim()).context("guild_link must be a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("guild_link must use http or https");
    }
    let host = url
        .host_str()
        .context("guild_link does not contain a host")?;
    let site = WarcraftLogsSite::from_host(host).with_context(|| {
        format!(
            "unsupported Warcraft Logs host {host:?}; use www.warcraftlogs.com or classic.warcraftlogs.com"
        )
    })?;
    let segments = url
        .path_segments()
        .context("guild_link does not contain a guild path")?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("guild") {
        bail!("guild_link must point to a Warcraft Logs guild page");
    }

    let guild_id_segment = match segments.as_slice() {
        ["guild", guild_id] => Some(*guild_id),
        ["guild", tab, guild_id, ..]
            if matches!(*tab, "id" | "reports-list" | "page" | "calendar") =>
        {
            Some(*guild_id)
        }
        _ => None,
    };
    if let Some(guild_id) = guild_id_segment {
        let guild_id = guild_id
            .parse::<i64>()
            .context("guild_link contains an invalid guild ID")?;
        if guild_id <= 0 {
            bail!("guild_link guild ID must be positive");
        }
        return Ok(GuildLocator::Id { site, guild_id });
    }

    if segments.len() >= 4 {
        let region = decode_url_segment(segments[1])?.to_ascii_uppercase();
        let region = normalize_region(&region)
            .context("guild_link contains an unsupported region")?
            .to_owned();
        let server_slug = normalize_server_slug(&decode_url_segment(segments[2])?);
        if server_slug.is_empty() {
            bail!("guild_link contains an empty server slug");
        }
        let name = segments[3..]
            .iter()
            .map(|segment| decode_url_segment(segment))
            .collect::<Result<Vec<_>>>()?
            .join("/");
        if name.trim().is_empty() {
            bail!("guild_link contains an empty guild name");
        }
        return Ok(GuildLocator::Identity {
            site,
            name,
            server_slug,
            region,
        });
    }

    bail!("guild_link is not a recognized Warcraft Logs guild URL")
}

fn decode_url_segment(value: &str) -> Result<String> {
    percent_decode_str(value)
        .decode_utf8()
        .context("guild_link contains invalid UTF-8")
        .map(|value| value.into_owned())
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
    use super::{
        GuildLocator, ReportLocator, build_guild_locator, format_recent_reports, normalize_region,
        normalize_server_slug, parse_guild_link, parse_report_link, select_summary_fight,
    };
    use crate::db::WclFightRecord;
    use crate::warcraft_logs::{WarcraftLogsReport, WarcraftLogsSite, WarcraftLogsZone};

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

    #[test]
    fn formats_three_recent_reports_with_links() {
        let reports = (1..=3)
            .map(|index| WarcraftLogsReport {
                code: format!("code{index}"),
                title: format!("Report [{index}]"),
                start_time: f64::from(index) * 1_000.0,
                end_time: None,
                revision: 0,
                visibility: "public".to_owned(),
                zone: Some(WarcraftLogsZone {
                    name: "Test Zone".to_owned(),
                }),
            })
            .collect::<Vec<_>>();
        let references = reports.iter().collect::<Vec<_>>();

        let output = format_recent_reports(WarcraftLogsSite::Classic, "Test Guild", &references);

        assert!(output.contains("Recent Warcraft Logs Classic Reports — Test Guild"));
        assert!(
            output.contains("[Report \\[1\\]](https://classic.warcraftlogs.com/reports/code1)")
        );
        assert!(output.contains("<t:3:F> (<t:3:R>)"));
    }

    #[test]
    fn parses_classic_guild_id_links() {
        assert_eq!(
            parse_guild_link("https://classic.warcraftlogs.com/guild/id/484").unwrap(),
            GuildLocator::Id {
                site: WarcraftLogsSite::Classic,
                guild_id: 484,
            }
        );
        assert_eq!(
            parse_guild_link("https://classic.warcraftlogs.com/guild/page/484").unwrap(),
            GuildLocator::Id {
                site: WarcraftLogsSite::Classic,
                guild_id: 484,
            }
        );
        assert_eq!(
            parse_guild_link("https://classic.warcraftlogs.com/guild/484").unwrap(),
            GuildLocator::Id {
                site: WarcraftLogsSite::Classic,
                guild_id: 484,
            }
        );
    }

    #[test]
    fn parses_encoded_retail_guild_identity_links() {
        assert_eq!(
            parse_guild_link("https://www.warcraftlogs.com/guild/eu/Tarren%20Mill/My%20Guild")
                .unwrap(),
            GuildLocator::Identity {
                site: WarcraftLogsSite::Retail,
                name: "My Guild".to_owned(),
                server_slug: "tarren-mill".to_owned(),
                region: "EU".to_owned(),
            }
        );
    }

    #[test]
    fn manual_guild_lookup_defaults_to_classic() {
        assert_eq!(
            build_guild_locator(
                None,
                Some("Progress".to_owned()),
                Some("Benediction".to_owned()),
                Some("US".to_owned()),
                None,
            )
            .unwrap(),
            GuildLocator::Identity {
                site: WarcraftLogsSite::Classic,
                name: "Progress".to_owned(),
                server_slug: "benediction".to_owned(),
                region: "US".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_non_guild_and_mixed_inputs() {
        assert!(parse_guild_link("https://classic.warcraftlogs.com/reports/abc123").is_err());
        assert!(
            build_guild_locator(
                Some("https://classic.warcraftlogs.com/guild/id/484".to_owned()),
                Some("Progress".to_owned()),
                None,
                None,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn parses_classic_and_retail_report_links() {
        assert_eq!(
            parse_report_link(
                "https://classic.warcraftlogs.com/reports/AbC123#fight=7&type=summary"
            )
            .unwrap(),
            ReportLocator {
                site: WarcraftLogsSite::Classic,
                code: "AbC123".to_owned(),
                fight_id: Some(7),
            }
        );
        assert_eq!(
            parse_report_link("https://www.warcraftlogs.com/reports/Def456#fight=last").unwrap(),
            ReportLocator {
                site: WarcraftLogsSite::Retail,
                code: "Def456".to_owned(),
                fight_id: None,
            }
        );
        assert!(parse_report_link("https://classic.warcraftlogs.com/guild/id/484").is_err());
        assert!(
            parse_report_link("https://classic.warcraftlogs.com/reports/AbC123#fight=foo").is_err()
        );
    }

    #[test]
    fn selects_requested_or_latest_completed_fight() {
        let fights = vec![fight_record(1, 10_000), fight_record(2, 30_000)];

        assert_eq!(
            select_summary_fight(&fights, Some(1)).map(|fight| fight.fight_id),
            Some(1)
        );
        assert_eq!(
            select_summary_fight(&fights, None).map(|fight| fight.fight_id),
            Some(2)
        );
        assert!(select_summary_fight(&fights, Some(99)).is_none());
    }

    fn fight_record(fight_id: i32, end_time_ms: i64) -> WclFightRecord {
        WclFightRecord {
            fight_id,
            boss_name: format!("Boss {fight_id}"),
            difficulty: Some(5),
            raid_size: Some(20),
            average_item_level: Some(700.0),
            start_time_ms: 0,
            end_time_ms,
        }
    }
}
