mod commands;
mod db;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use sqlx::PgPool;

/// Shared application state available to every command handler.
#[derive(Debug)]
pub struct Data {
    pub db: PgPool,
}

/// Poise command context alias.
pub type Context<'a> = poise::Context<'a, Data, anyhow::Error>;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present (ignored when variables are already set).
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let token = std::env::var("DISCORD_TOKEN")
        .expect("DISCORD_TOKEN environment variable must be set");

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL environment variable must be set");

    let pool = db::create_pool(&database_url).await?;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::sim::sim(),
                commands::class::class(),
                commands::status::status(),
            ],
            on_error: |err| {
                Box::pin(async move {
                    tracing::error!("Command error: {:?}", err);
                    if let poise::FrameworkError::Command { ctx, .. } = err {
                        let _ = ctx.say("⚠️ An internal error occurred. Please try again later.").await;
                    }
                })
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                tracing::info!("Bot is ready!");
                Ok(Data { db: pool })
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged();

    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .await?;

    client.start().await?;

    Ok(())
}

