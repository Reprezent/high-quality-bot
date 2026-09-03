use crate::{
    db::{self, WclFightRecord, WclReportRecord, WclSubscription},
    warcraft_logs::{RateLimitInfo, WarcraftLogsClient, WarcraftLogsReport},
    warcraft_logs_discord,
};
use anyhow::{Context as _, Result, anyhow, bail};
use poise::serenity_prelude as serenity;
use serenity::{ChannelId, CreateAttachment, CreateMessage, Http};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;

const DISCOVERY_OVERLAP_MS: i64 = 15 * 60 * 1_000;
const DISCOVERY_LOOKBACK_MS: i64 = 24 * 60 * 60 * 1_000;
const POST_CONFIRM_ATTEMPTS: usize = 3;
const POST_CONFIRM_RETRY_DELAY: Duration = Duration::from_millis(200);

pub fn spawn(
    pool: PgPool,
    client: WarcraftLogsClient,
    discord_http: Arc<Http>,
    poll_interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            interval_secs = poll_interval.as_secs(),
            "Warcraft Logs tracker started"
        );

        loop {
            let delay = match run_cycle(&pool, &client, &discord_http, poll_interval).await {
                Ok(delay) => delay,
                Err(error) => {
                    tracing::error!(error = ?error, "Warcraft Logs tracking cycle failed");
                    poll_interval
                }
            };
            tokio::time::sleep(delay).await;
        }
    })
}

async fn run_cycle(
    pool: &PgPool,
    client: &WarcraftLogsClient,
    discord_http: &Http,
    poll_interval: Duration,
) -> Result<Duration> {
    let subscriptions = db::list_wcl_subscriptions(pool)
        .await
        .context("failed to list Warcraft Logs subscriptions")?;
    let mut next_delay = poll_interval;

    for subscription in subscriptions {
        match process_subscription(pool, client, discord_http, &subscription).await {
            Ok(rate_limit) => {
                next_delay = next_delay.max(rate_limit.recommended_delay(poll_interval));
            }
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    subscription_id = subscription.id,
                    discord_guild_id = %subscription.discord_guild_id,
                    wcl_guild_id = subscription.wcl_guild_id,
                    "failed to process Warcraft Logs subscription"
                );
                let error_message = truncate_error(&error);
                if let Err(db_error) =
                    db::set_wcl_subscription_error(pool, subscription.id, &error_message).await
                {
                    tracing::error!(
                        error = ?db_error,
                        subscription_id = subscription.id,
                        "failed to persist Warcraft Logs subscription error"
                    );
                }
            }
        }
    }

    Ok(next_delay)
}

async fn process_subscription(
    pool: &PgPool,
    client: &WarcraftLogsClient,
    discord_http: &Http,
    subscription: &WclSubscription,
) -> Result<RateLimitInfo> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let start_time_ms = discovery_start_time_ms(subscription.discovery_cursor_ms, now_ms);
    let discovery = client
        .reports_since(
            subscription.wcl_site,
            subscription.wcl_guild_id,
            start_time_ms,
            now_ms,
        )
        .await
        .with_context(|| {
            format!(
                "failed to discover reports for Warcraft Logs guild {}",
                subscription.wcl_guild_id
            )
        })?;
    let reports = discovery
        .reports
        .iter()
        .filter(|report| report.visibility.eq_ignore_ascii_case("public"))
        .map(report_record_from_api)
        .collect::<Result<Vec<_>>>()?;

    db::reconcile_wcl_reports(pool, subscription.id, &reports, now_ms)
        .await
        .context("failed to persist discovered Warcraft Logs reports")?;

    let mut had_item_error = announce_reports(pool, discord_http, subscription.id).await?;
    had_item_error |= inspect_reports(pool, client, subscription.id, subscription.wcl_site).await?;
    had_item_error |= announce_fights(pool, client, discord_http, subscription.id).await?;

    if had_item_error {
        db::set_wcl_subscription_error(
            pool,
            subscription.id,
            "One or more report or fight items failed and will be retried.",
        )
        .await?;
    } else {
        db::clear_wcl_subscription_error(pool, subscription.id).await?;
    }

    Ok(discovery.rate_limit)
}

async fn announce_reports(
    pool: &PgPool,
    discord_http: &Http,
    subscription_id: i64,
) -> Result<bool> {
    let reports = db::list_wcl_reports_to_announce(pool, subscription_id).await?;
    let mut had_error = false;
    for report in reports {
        let result = async {
            let channel_id = parse_channel_id(&report.discord_channel_id)?;
            channel_id
                .send_message(
                    discord_http,
                    CreateMessage::new()
                        .embed(warcraft_logs_discord::report_embed(&report))
                        .nonce(warcraft_logs_discord::report_nonce(&report.code))
                        .enforce_nonce(true),
                )
                .await
                .context("Discord rejected the new-report announcement")
        }
        .await;

        match result {
            Ok(message) => {
                confirm_report_posted(
                    pool,
                    report.subscription_id,
                    &report.code,
                    &message.id.to_string(),
                )
                .await?;
            }
            Err(error) => {
                had_error = true;
                tracing::warn!(
                    error = ?error,
                    subscription_id = report.subscription_id,
                    report_code = %report.code,
                    "failed to announce Warcraft Logs report"
                );
                db::set_wcl_report_error(
                    pool,
                    report.subscription_id,
                    &report.code,
                    &truncate_error(&error),
                )
                .await?;
            }
        }
    }

    Ok(had_error)
}

async fn inspect_reports(
    pool: &PgPool,
    client: &WarcraftLogsClient,
    subscription_id: i64,
    site: crate::warcraft_logs::WarcraftLogsSite,
) -> Result<bool> {
    let reports = db::list_wcl_reports_to_inspect(pool, subscription_id).await?;
    let mut had_error = false;
    for report in reports {
        let result = async {
            let details = client.report_fights(site, &report.code).await?;
            let report_end_time_ms = details.end_time.map(absolute_milliseconds).transpose()?;
            let fights = fight_records_from_api(&details.fights)?;

            db::record_wcl_fights(
                pool,
                report.subscription_id,
                &report.code,
                details.revision,
                report_end_time_ms,
                &fights,
                !report.baseline_scanned && report.suppress_initial_kills,
            )
            .await
        }
        .await;

        if let Err(error) = result {
            had_error = true;
            tracing::warn!(
                error = ?error,
                subscription_id = report.subscription_id,
                report_code = %report.code,
                "failed to inspect Warcraft Logs report"
            );
            db::set_wcl_report_error(
                pool,
                report.subscription_id,
                &report.code,
                &truncate_error(&error),
            )
            .await?;
        }
    }

    Ok(had_error)
}

async fn announce_fights(
    pool: &PgPool,
    client: &WarcraftLogsClient,
    discord_http: &Http,
    subscription_id: i64,
) -> Result<bool> {
    let fights = db::list_pending_wcl_fights(pool, subscription_id).await?;
    let mut had_error = false;
    for fight in fights {
        let result = async {
            let summary = client
                .kill_summary(fight.wcl_site, &fight.report_code, fight.fight.fight_id)
                .await?;
            let channel_id = parse_channel_id(&fight.discord_channel_id)?;
            let mut message = match warcraft_logs_discord::render_kill_summary(&fight, &summary) {
                Ok(image) => CreateMessage::new()
                    .embed(warcraft_logs_discord::kill_embed(&fight, &summary, true))
                    .add_file(CreateAttachment::bytes(
                        image,
                        warcraft_logs_discord::FIGHT_IMAGE_NAME,
                    )),
                Err(error) => {
                    tracing::warn!(
                        error = ?error,
                        report_code = %fight.report_code,
                        fight_id = fight.fight.fight_id,
                        "failed to render Warcraft Logs fight image; posting without it"
                    );
                    CreateMessage::new()
                        .embed(warcraft_logs_discord::kill_embed(&fight, &summary, false))
                }
            };
            message = message
                .nonce(warcraft_logs_discord::fight_nonce(
                    &fight.report_code,
                    fight.fight.fight_id,
                ))
                .enforce_nonce(true);
            channel_id
                .send_message(discord_http, message)
                .await
                .context("Discord rejected the boss-kill announcement")
        }
        .await;

        match result {
            Ok(message) => {
                confirm_fight_posted(
                    pool,
                    fight.subscription_id,
                    &fight.report_code,
                    fight.fight.fight_id,
                    &message.id.to_string(),
                )
                .await?;
            }
            Err(error) => {
                had_error = true;
                tracing::warn!(
                    error = ?error,
                    subscription_id = fight.subscription_id,
                    report_code = %fight.report_code,
                    fight_id = fight.fight.fight_id,
                    "failed to announce Warcraft Logs boss kill"
                );
                db::set_wcl_fight_error(
                    pool,
                    fight.subscription_id,
                    &fight.report_code,
                    fight.fight.fight_id,
                    &truncate_error(&error),
                )
                .await?;
            }
        }
    }

    Ok(had_error)
}

async fn confirm_report_posted(
    pool: &PgPool,
    subscription_id: i64,
    report_code: &str,
    message_id: &str,
) -> Result<()> {
    for attempt in 1..=POST_CONFIRM_ATTEMPTS {
        match db::mark_wcl_report_posted(pool, subscription_id, report_code, message_id).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt == POST_CONFIRM_ATTEMPTS => return Err(error),
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    subscription_id,
                    report_code,
                    attempt,
                    "failed to confirm Warcraft Logs report post; retrying"
                );
                tokio::time::sleep(POST_CONFIRM_RETRY_DELAY).await;
            }
        }
    }
    unreachable!("post confirmation attempts are non-zero")
}

async fn confirm_fight_posted(
    pool: &PgPool,
    subscription_id: i64,
    report_code: &str,
    fight_id: i32,
    message_id: &str,
) -> Result<()> {
    for attempt in 1..=POST_CONFIRM_ATTEMPTS {
        match db::mark_wcl_fight_posted(pool, subscription_id, report_code, fight_id, message_id)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) if attempt == POST_CONFIRM_ATTEMPTS => return Err(error),
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    subscription_id,
                    report_code,
                    fight_id,
                    attempt,
                    "failed to confirm Warcraft Logs fight post; retrying"
                );
                tokio::time::sleep(POST_CONFIRM_RETRY_DELAY).await;
            }
        }
    }
    unreachable!("post confirmation attempts are non-zero")
}

pub(crate) fn report_record_from_api(report: &WarcraftLogsReport) -> Result<WclReportRecord> {
    Ok(WclReportRecord {
        code: report.code.clone(),
        title: report.title.clone(),
        start_time_ms: absolute_milliseconds(report.start_time)?,
        end_time_ms: report.end_time.map(absolute_milliseconds).transpose()?,
        revision: report.revision,
        zone_name: report.zone.as_ref().map(|zone| zone.name.clone()),
        visibility: report.visibility.clone(),
    })
}

pub(crate) fn fight_records_from_api(
    fights: &[crate::warcraft_logs::WarcraftLogsFight],
) -> Result<Vec<WclFightRecord>> {
    fights
        .iter()
        .filter(|fight| fight.is_completed_boss_kill())
        .map(|fight| {
            Ok(WclFightRecord {
                fight_id: fight.id,
                boss_name: fight.name.clone(),
                difficulty: fight.difficulty,
                raid_size: fight.size,
                average_item_level: fight.average_item_level,
                start_time_ms: relative_milliseconds(fight.start_time)?,
                end_time_ms: relative_milliseconds(fight.end_time)?,
            })
        })
        .collect()
}

pub(crate) fn absolute_milliseconds(value: f64) -> Result<i64> {
    milliseconds(value, "absolute")
}

fn relative_milliseconds(value: f64) -> Result<i64> {
    milliseconds(value, "relative")
}

fn milliseconds(value: f64, kind: &str) -> Result<i64> {
    if !value.is_finite() || value < 0.0 || value > i64::MAX as f64 {
        bail!("Warcraft Logs returned an invalid {kind} millisecond timestamp: {value}");
    }
    Ok(value.round() as i64)
}

fn discovery_start_time_ms(cursor_ms: i64, now_ms: i64) -> i64 {
    (cursor_ms - DISCOVERY_OVERLAP_MS).min(now_ms - DISCOVERY_LOOKBACK_MS)
}

fn parse_channel_id(value: &str) -> Result<ChannelId> {
    let id = value
        .parse::<u64>()
        .with_context(|| format!("stored Discord channel ID {value:?} is invalid"))?;
    if id == 0 {
        return Err(anyhow!("stored Discord channel ID must not be zero"));
    }
    Ok(ChannelId::new(id))
}

fn truncate_error(error: &anyhow::Error) -> String {
    error.to_string().chars().take(2_000).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::{absolute_milliseconds, discovery_start_time_ms, parse_channel_id};

    #[test]
    fn validates_api_timestamps() {
        assert_eq!(absolute_milliseconds(1234.4).unwrap(), 1234);
        assert!(absolute_milliseconds(f64::NAN).is_err());
        assert!(absolute_milliseconds(-1.0).is_err());
    }

    #[test]
    fn validates_stored_channel_ids() {
        assert_eq!(parse_channel_id("123").unwrap().get(), 123);
        assert!(parse_channel_id("0").is_err());
        assert!(parse_channel_id("not-an-id").is_err());
    }

    #[test]
    fn discovery_covers_delayed_uploads_and_long_outages() {
        let now = 10 * 24 * 60 * 60 * 1_000;
        assert_eq!(
            discovery_start_time_ms(now - 60_000, now),
            now - 24 * 60 * 60 * 1_000
        );

        let cursor_before_long_outage = now - 3 * 24 * 60 * 60 * 1_000;
        assert_eq!(
            discovery_start_time_ms(cursor_before_long_outage, now),
            cursor_before_long_outage - 15 * 60 * 1_000
        );
    }
}
