use crate::{Context, db};
use anyhow::{Result, anyhow};
use chrono::{TimeDelta, Utc};
use poise::serenity_prelude::{ChannelType, GuildChannel, Permissions};

const MIN_INTERVAL_SECONDS: i64 = 60 * 60;

/// Schedule a /pisstory graph to be posted periodically.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn pisscribe(
    ctx: Context<'_>,
    #[description = "Text channel where graphs should be posted"] channel: GuildChannel,
    #[description = "Posting interval, for example 2h, 1d, or 1w 2d"] period: String,
) -> Result<()> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow!("pisscribe invoked outside a Discord guild"))?;

    if channel.guild_id != guild_id {
        ctx.say("The destination channel must belong to this Discord server.")
            .await?;
        return Ok(());
    }

    if !matches!(channel.kind, ChannelType::Text | ChannelType::News) {
        ctx.say("Choose a standard text or announcement channel.")
            .await?;
        return Ok(());
    }

    let interval_seconds = match parse_period(&period) {
        Ok(seconds) => seconds,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };

    let bot_user = ctx.http().get_current_user().await?;
    let bot_member = guild_id.member(ctx.http(), bot_user.id).await?;
    let partial_guild = guild_id.to_partial_guild(ctx.http()).await?;
    let permissions = partial_guild.user_permissions_in(&channel, &bot_member);
    let required =
        Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::ATTACH_FILES;
    if !permissions.contains(required) {
        ctx.say(format!(
            "I need View Channel, Send Messages, and Attach Files permissions in <#{}>.",
            channel.id
        ))
        .await?;
        return Ok(());
    }

    let delay = TimeDelta::try_seconds(interval_seconds)
        .ok_or_else(|| anyhow!("period exceeds the supported duration"))?;
    let next_post_at = Utc::now() + delay;
    db::upsert_pisstory_subscription(
        &ctx.data().db,
        &guild_id.get().to_string(),
        &channel.id.get().to_string(),
        interval_seconds,
        next_post_at,
    )
    .await?;

    ctx.say(format!(
        "✅ A new `/pisstory` graph will be posted in <#{}> every **{}**. The first post will be <t:{}:R>.",
        channel.id,
        format_period(interval_seconds),
        next_post_at.timestamp(),
    ))
    .await?;

    Ok(())
}

fn parse_period(input: &str) -> Result<i64, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a period such as `2h`, `1d`, or `1w 2d`. Minimum: `1h`.".into());
    }

    let mut total = 0_i64;
    let mut chars = input.char_indices().peekable();
    while chars.peek().is_some() {
        while matches!(chars.peek(), Some((_, character)) if character.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let number_start = chars.peek().map(|(index, _)| *index).unwrap_or(0);
        let mut number_end = number_start;
        while let Some((index, character)) = chars.peek().copied() {
            if !character.is_ascii_digit() {
                break;
            }
            number_end = index + character.len_utf8();
            chars.next();
        }
        if number_end == number_start {
            return Err(period_help());
        }

        let value = input[number_start..number_end]
            .parse::<i64>()
            .map_err(|_| period_help())?;
        let Some((_, unit)) = chars.next() else {
            return Err(period_help());
        };
        let unit_seconds = match unit.to_ascii_lowercase() {
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            'w' => 7 * 24 * 60 * 60,
            _ => return Err(period_help()),
        };
        total = total
            .checked_add(value.checked_mul(unit_seconds).ok_or_else(period_help)?)
            .ok_or_else(period_help)?;

        if matches!(chars.peek(), Some((_, character)) if !character.is_whitespace() && !character.is_ascii_digit())
        {
            return Err(period_help());
        }
    }

    if total < MIN_INTERVAL_SECONDS {
        return Err("The minimum period is `1h`.".into());
    }

    TimeDelta::try_seconds(total).ok_or_else(|| "That period is too large.".to_owned())?;
    Ok(total)
}

fn period_help() -> String {
    "Use hours, days, or weeks, for example `2h`, `1d`, or `1w 2d`. Minimum: `1h`.".into()
}

fn format_period(seconds: i64) -> String {
    let mut remainder = seconds;
    let mut parts = Vec::new();
    for (unit, unit_seconds) in [('w', 604_800), ('d', 86_400), ('h', 3_600)] {
        let amount = remainder / unit_seconds;
        if amount > 0 {
            parts.push(format!("{amount}{unit}"));
            remainder %= unit_seconds;
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{format_period, parse_period};

    #[test]
    fn parses_supported_periods() {
        assert_eq!(parse_period("2h").unwrap(), 7_200);
        assert_eq!(parse_period("1d").unwrap(), 86_400);
        assert_eq!(parse_period("1w 2d 3h").unwrap(), 788_400);
        assert_eq!(parse_period("1W2D").unwrap(), 777_600);
    }

    #[test]
    fn rejects_invalid_or_short_periods() {
        assert_eq!(parse_period("59m").unwrap_err(), super::period_help());
        assert_eq!(parse_period("30").unwrap_err(), super::period_help());
        assert_eq!(
            parse_period("0h").unwrap_err(),
            "The minimum period is `1h`."
        );
        assert_eq!(parse_period("1h nope").unwrap_err(), super::period_help());
    }

    #[test]
    fn formats_periods_canonically() {
        assert_eq!(format_period(3_600), "1h");
        assert_eq!(format_period(788_400), "1w 2d 3h");
    }
}
