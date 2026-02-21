use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

/// Establish a connection pool to PostgreSQL and run migrations.
pub async fn create_pool(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await?;

    Ok(pool)
}

// ---------------------------------------------------------------------------
// User preferences
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct UserPreference {
    pub discord_user_id: String,
    pub class: String,
    pub spec: Option<String>,
}

/// Insert or update the default class/spec for a Discord user.
pub async fn upsert_user_preference(
    pool: &PgPool,
    discord_user_id: &str,
    class: &str,
    spec: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_preferences (discord_user_id, class, spec)
        VALUES ($1, $2, $3)
        ON CONFLICT (discord_user_id)
        DO UPDATE SET class = $2, spec = $3
        "#,
    )
    .bind(discord_user_id)
    .bind(class)
    .bind(spec)
    .execute(pool)
    .await?;

    Ok(())
}

/// Retrieve the stored class/spec preference for a Discord user.
#[allow(dead_code)]
pub async fn get_user_preference(
    pool: &PgPool,
    discord_user_id: &str,
) -> Result<Option<UserPreference>> {
    let row = sqlx::query(
        "SELECT discord_user_id, class, spec FROM user_preferences WHERE discord_user_id = $1",
    )
    .bind(discord_user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserPreference {
        discord_user_id: r.get("discord_user_id"),
        class: r.get("class"),
        spec: r.get("spec"),
    }))
}

// ---------------------------------------------------------------------------
// Simulation runs
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct SimulationRun {
    pub run_id: Uuid,
    pub discord_user_id: String,
    pub class: String,
    pub spec: String,
    pub gear_payload: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Create a new simulation run record and return its ID.
pub async fn create_simulation_run(
    pool: &PgPool,
    discord_user_id: &str,
    class: &str,
    spec: &str,
    gear_payload: &serde_json::Value,
) -> Result<Uuid> {
    let run_id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO simulation_runs (run_id, discord_user_id, class, spec, gear_payload, status)
        VALUES ($1, $2, $3, $4, $5, 'queued')
        "#,
    )
    .bind(run_id)
    .bind(discord_user_id)
    .bind(class)
    .bind(spec)
    .bind(gear_payload)
    .execute(pool)
    .await?;

    Ok(run_id)
}

/// Retrieve a simulation run by its ID.
pub async fn get_simulation_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<SimulationRun>> {
    let row = sqlx::query(
        r#"
        SELECT run_id, discord_user_id, class, spec,
               gear_payload, status, created_at, updated_at
        FROM simulation_runs
        WHERE run_id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SimulationRun {
        run_id: r.get("run_id"),
        discord_user_id: r.get("discord_user_id"),
        class: r.get("class"),
        spec: r.get("spec"),
        gear_payload: r.get("gear_payload"),
        status: r.get("status"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }))
}
