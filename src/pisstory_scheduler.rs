use crate::{commands::pisstory, db};
use anyhow::{Context as _, Result};
use poise::serenity_prelude::{ChannelId, CreateAttachment, CreateMessage, Http};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;

const HISTORY_HOURS: i64 = 24;
const POLL_INTERVAL: Duration = Duration::from_secs(30);

pub fn spawn(pool: PgPool, discord_http: Arc<Http>) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!("Pisstory scheduler started");
        loop {
            if let Err(error) = run_cycle(&pool, &discord_http).await {
                tracing::error!(error = ?error, "Pisstory scheduling cycle failed");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

async fn run_cycle(pool: &PgPool, discord_http: &Http) -> Result<()> {
    let subscriptions = db::claim_due_pisstory_subscriptions(pool)
        .await
        .context("failed to claim due pisstory subscriptions")?;

    for subscription in subscriptions {
        if let Err(error) = post_graph(pool, discord_http, &subscription).await {
            tracing::warn!(
                error = ?error,
                discord_guild_id = %subscription.discord_guild_id,
                discord_channel_id = %subscription.discord_channel_id,
                "Failed to post a scheduled pisstory graph"
            );
            let error_message = format!("{error:#}");
            if let Err(db_error) = db::mark_pisstory_subscription_failed(
                pool,
                &subscription.discord_guild_id,
                &subscription.discord_channel_id,
                subscription.interval_seconds,
                &error_message,
            )
            .await
            {
                tracing::error!(
                    error = ?db_error,
                    discord_guild_id = %subscription.discord_guild_id,
                    "Failed to persist pisstory scheduling error"
                );
            }
        }
    }

    Ok(())
}

async fn post_graph(
    pool: &PgPool,
    discord_http: &Http,
    subscription: &db::PisstorySubscription,
) -> Result<()> {
    let channel_id = subscription
        .discord_channel_id
        .parse::<u64>()
        .map(ChannelId::new)
        .context("stored Discord channel ID is invalid")?;
    let samples = db::get_iss_telemetry_history(pool, HISTORY_HOURS).await?;

    let message = if samples.is_empty() {
        CreateMessage::new().content(format!(
            "📉 No telemetry data recorded in the last {HISTORY_HOURS} hours."
        ))
    } else {
        let image_data = pisstory::render_chart(&samples)?;
        CreateMessage::new()
            .content(pisstory::chart_content(HISTORY_HOURS, samples.len()))
            .add_file(CreateAttachment::bytes(image_data, "piss_history.png"))
    };
    channel_id.send_message(discord_http, message).await?;

    db::mark_pisstory_subscription_posted(
        pool,
        &subscription.discord_guild_id,
        &subscription.discord_channel_id,
        subscription.interval_seconds,
    )
    .await?;
    tracing::info!(
        discord_guild_id = %subscription.discord_guild_id,
        discord_channel_id = %subscription.discord_channel_id,
        interval_seconds = subscription.interval_seconds,
        "Posted scheduled pisstory graph"
    );

    Ok(())
}
