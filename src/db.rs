use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::{PgConnectOptions, PgPoolOptions}};
use uuid::Uuid;

/// Establish a connection pool to PostgreSQL and run migrations.
pub async fn create_pool(connect_options: PgConnectOptions) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await?;

    Ok(pool)
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
    pub raid_members: Vec<String>,
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
             gear_payload, raid_members, status, created_at, updated_at
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
        raid_members: r.get("raid_members"),
        status: r.get("status"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }))
}

pub async fn update_simulation_run_status(
    pool: &PgPool,
    run_id: Uuid,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE simulation_runs
        SET status = $2, updated_at = NOW()
        WHERE run_id = $1
        "#,
    )
    .bind(run_id)
    .bind(status)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_simulation_run_raid_members(
    pool: &PgPool,
    run_id: Uuid,
    raid_members: &[String],
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE simulation_runs
        SET raid_members = $2, updated_at = NOW()
        WHERE run_id = $1
        "#,
    )
    .bind(run_id)
    .bind(raid_members)
    .execute(pool)
    .await?;

    Ok(())
}

#[allow(dead_code)]
pub struct SimulationProgressFrame {
    pub run_id: Uuid,
    pub frame_index: i32,
    pub completed_iterations: i32,
    pub total_iterations: i32,
    pub completed_sims: i32,
    pub total_sims: i32,
    pub dps: f64,
    pub hps: f64,
    pub is_final: bool,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_simulation_progress_frame(
    pool: &PgPool,
    run_id: Uuid,
    frame_index: i32,
    completed_iterations: i32,
    total_iterations: i32,
    completed_sims: i32,
    total_sims: i32,
    dps: f64,
    hps: f64,
    is_final: bool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO simulation_progress_frames (
            run_id, frame_index, completed_iterations, total_iterations,
            completed_sims, total_sims, dps, hps, is_final
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (run_id, frame_index) DO NOTHING
        "#,
    )
    .bind(run_id)
    .bind(frame_index)
    .bind(completed_iterations)
    .bind(total_iterations)
    .bind(completed_sims)
    .bind(total_sims)
    .bind(dps)
    .bind(hps)
    .bind(is_final)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_latest_simulation_progress_frame(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<SimulationProgressFrame>> {
    let row = sqlx::query(
        r#"
        SELECT run_id, frame_index, completed_iterations, total_iterations,
               completed_sims, total_sims, dps, hps, is_final, created_at
        FROM simulation_progress_frames
        WHERE run_id = $1
        ORDER BY frame_index DESC
        LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SimulationProgressFrame {
        run_id: r.get("run_id"),
        frame_index: r.get("frame_index"),
        completed_iterations: r.get("completed_iterations"),
        total_iterations: r.get("total_iterations"),
        completed_sims: r.get("completed_sims"),
        total_sims: r.get("total_sims"),
        dps: r.get("dps"),
        hps: r.get("hps"),
        is_final: r.get("is_final"),
        created_at: r.get("created_at"),
    }))
}
