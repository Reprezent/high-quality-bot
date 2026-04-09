mod commands;
mod db;
mod iss_telemetry;
mod parsing;
mod sim_runtime;
mod sim_runtime_targets;
pub mod mop_proto;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use sqlx::postgres::PgConnectOptions;
use sqlx::PgPool;

/// Shared application state available to every command handler.
#[derive(Debug)]
pub struct Data {
    pub db: PgPool,
    pub sim_api_base_url: String,
}

/// Poise command context alias.
pub type Context<'a> = poise::Context<'a, Data, anyhow::Error>;

fn command_names(commands: &[poise::Command<Data, anyhow::Error>]) -> String {
    commands
        .iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn discord_guild_id() -> Result<Option<serenity::GuildId>> {
    match std::env::var("DISCORD_GUILD_ID") {
        Ok(raw_value) if !raw_value.trim().is_empty() => {
            let guild_id = raw_value
                .trim()
                .parse::<u64>()
                .map(serenity::GuildId::new)?;
            Ok(Some(guild_id))
        }
        Ok(_) => Ok(None),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn postgres_connect_options() -> PgConnectOptions {
    let postgres_user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "botuser".to_string());
    let postgres_password =
        std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "changeme".to_string());
    let postgres_db = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "highqualitybot".to_string());
    let postgres_host = std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "db".to_string());
    let postgres_port = std::env::var("POSTGRES_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(5432);

    PgConnectOptions::new()
        .host(&postgres_host)
        .port(postgres_port)
        .username(&postgres_user)
        .password(&postgres_password)
        .database(&postgres_db)
}

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

    let pool = db::create_pool(postgres_connect_options()).await?;
    let sim_api_base_url =
        std::env::var("WOWSIMS_API_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:3333".to_string());

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::sim::sim(),
                commands::status::status(),
                commands::health::health(),
                commands::piss::piss(),
            ],
            pre_command: |ctx| {
                Box::pin(async move {
                    tracing::info!(
                        command = ctx.command().name,
                        user_id = %ctx.author().id,
                        channel_id = %ctx.channel_id(),
                        guild_id = ctx.guild_id().map(|guild_id| guild_id.get()),
                        "Running command"
                    );
                })
            },
            post_command: |ctx| {
                Box::pin(async move {
                    tracing::info!(
                        command = ctx.command().name,
                        user_id = %ctx.author().id,
                        channel_id = %ctx.channel_id(),
                        guild_id = ctx.guild_id().map(|guild_id| guild_id.get()),
                        "Finished command"
                    );
                })
            },
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
                let registered_commands = command_names(&framework.options().commands);

                if let Some(guild_id) = discord_guild_id()? {
                    poise::builtins::register_in_guild(
                        ctx,
                        &framework.options().commands,
                        guild_id,
                    )
                    .await?;
                    tracing::info!(
                        guild_id = guild_id.get(),
                        commands = %registered_commands,
                        command_count = framework.options().commands.len(),
                        "Registered slash commands in guild"
                    );
                } else {
                    poise::builtins::register_globally(ctx, &framework.options().commands)
                        .await?;
                    tracing::info!(
                        commands = %registered_commands,
                        command_count = framework.options().commands.len(),
                        "Registered slash commands globally"
                    );
                }

                tracing::info!("Bot is ready!");
                Ok(Data {
                    db: pool,
                    sim_api_base_url,
                })
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

