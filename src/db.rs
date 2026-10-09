use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

use crate::warcraft_logs::WarcraftLogsSite;

/// Establish a connection pool to PostgreSQL and run migrations.
pub async fn create_pool(connect_options: PgConnectOptions) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/002_iss_telemetry_history.sql"))
        .execute(&pool)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/003_sim_request_audit.sql"))
        .execute(&pool)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/004_warcraft_logs.sql"))
        .execute(&pool)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/005_pisstory_subscriptions.sql"))
        .execute(&pool)
        .await?;

    sqlx::raw_sql(include_str!("../migrations/006_wow_characters.sql"))
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
    pub input_format: String,
    pub upstream_revision: Option<String>,
    pub normalized_request: Option<serde_json::Value>,
    pub effective_random_seed: Option<i64>,
    pub effective_iterations: Option<i32>,
    pub raid_members: Vec<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewSimulationRun<'a> {
    pub run_id: Uuid,
    pub discord_user_id: &'a str,
    pub class: &'a str,
    pub spec: &'a str,
    pub source_payload: &'a serde_json::Value,
    pub input_format: &'a str,
    pub upstream_revision: Option<&'a str>,
    pub normalized_request: Option<&'a serde_json::Value>,
    pub effective_random_seed: Option<i64>,
    pub effective_iterations: Option<i32>,
}

/// Create a new simulation run record and return its server-owned ID.
pub async fn create_simulation_run(pool: &PgPool, run: &NewSimulationRun<'_>) -> Result<Uuid> {
    sqlx::query(
        r#"
        INSERT INTO simulation_runs (
            run_id,
            discord_user_id,
            class,
            spec,
            gear_payload,
            input_format,
            upstream_revision,
            normalized_request,
            effective_random_seed,
            effective_iterations,
            status
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'queued')
        "#,
    )
    .bind(run.run_id)
    .bind(run.discord_user_id)
    .bind(run.class)
    .bind(run.spec)
    .bind(run.source_payload)
    .bind(run.input_format)
    .bind(run.upstream_revision)
    .bind(run.normalized_request)
    .bind(run.effective_random_seed)
    .bind(run.effective_iterations)
    .execute(pool)
    .await?;

    Ok(run.run_id)
}

/// Retrieve a simulation run by its ID.
pub async fn get_simulation_run(pool: &PgPool, run_id: Uuid) -> Result<Option<SimulationRun>> {
    let row = sqlx::query(
        r#"
        SELECT run_id, discord_user_id, class, spec,
             gear_payload, input_format, upstream_revision, normalized_request,
             effective_random_seed, effective_iterations, raid_members, status, created_at, updated_at
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
        input_format: r.get("input_format"),
        upstream_revision: r.get("upstream_revision"),
        normalized_request: r.get("normalized_request"),
        effective_random_seed: r.get("effective_random_seed"),
        effective_iterations: r.get("effective_iterations"),
        raid_members: r.get("raid_members"),
        status: r.get("status"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }))
}

pub async fn update_simulation_run_status(pool: &PgPool, run_id: Uuid, status: &str) -> Result<()> {
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

pub async fn update_simulation_run_request(
    pool: &PgPool,
    run_id: Uuid,
    normalized_request: &serde_json::Value,
    effective_random_seed: i64,
    effective_iterations: i32,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE simulation_runs
        SET normalized_request = $2,
            effective_random_seed = $3,
            effective_iterations = $4,
            updated_at = NOW()
        WHERE run_id = $1
        "#,
    )
    .bind(run_id)
    .bind(normalized_request)
    .bind(effective_random_seed)
    .bind(effective_iterations)
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

// ---------------------------------------------------------------------------
// Warcraft Logs tracking
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WclReportRecord {
    pub code: String,
    pub title: String,
    pub start_time_ms: i64,
    pub end_time_ms: Option<i64>,
    pub revision: i32,
    pub zone_name: Option<String>,
    pub visibility: String,
}

#[derive(Clone, Debug)]
pub struct WclSubscription {
    pub id: i64,
    pub discord_guild_id: String,
    pub discord_channel_id: String,
    pub wcl_guild_id: i64,
    pub wcl_site: WarcraftLogsSite,
    pub wcl_guild_name: String,
    pub server_name: String,
    pub region: String,
    pub discovery_cursor_ms: i64,
    pub last_polled_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WclReportToAnnounce {
    pub subscription_id: i64,
    pub discord_channel_id: String,
    pub wcl_site: WarcraftLogsSite,
    pub wcl_guild_name: String,
    pub code: String,
    pub title: String,
    pub start_time_ms: i64,
    pub zone_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WclReportToInspect {
    pub subscription_id: i64,
    pub code: String,
    pub baseline_scanned: bool,
    pub suppress_initial_kills: bool,
}

#[derive(Clone, Debug)]
pub struct WclFightRecord {
    pub fight_id: i32,
    pub boss_name: String,
    pub difficulty: Option<i32>,
    pub raid_size: Option<i32>,
    pub average_item_level: Option<f64>,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
}

#[derive(Clone, Debug)]
pub struct WclPendingFight {
    pub subscription_id: i64,
    pub discord_channel_id: String,
    pub wcl_site: WarcraftLogsSite,
    pub wcl_guild_name: String,
    pub report_code: String,
    pub report_title: String,
    pub report_start_time_ms: i64,
    pub fight: WclFightRecord,
}

pub struct NewWclSubscription<'a> {
    pub discord_guild_id: &'a str,
    pub discord_channel_id: &'a str,
    pub wcl_guild_id: i64,
    pub wcl_site: WarcraftLogsSite,
    pub wcl_guild_name: &'a str,
    pub server_slug: &'a str,
    pub server_name: &'a str,
    pub region: &'a str,
    pub baseline_time_ms: i64,
}

pub async fn replace_wcl_subscription(
    pool: &PgPool,
    subscription: NewWclSubscription<'_>,
    baseline_reports: &[WclReportRecord],
    baseline_fights: &[(String, Vec<WclFightRecord>)],
) -> Result<i64> {
    let mut transaction = pool.begin().await?;

    sqlx::query(
        r#"
        DELETE FROM warcraft_logs_subscriptions
        WHERE discord_guild_id = $1
        "#,
    )
    .bind(subscription.discord_guild_id)
    .execute(&mut *transaction)
    .await?;

    let row = sqlx::query(
        r#"
        INSERT INTO warcraft_logs_subscriptions (
            discord_guild_id, discord_channel_id, wcl_guild_id, wcl_site,
            wcl_guild_name, server_slug, server_name, region,
            baseline_time_ms, discovery_cursor_ms
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)
        RETURNING id
        "#,
    )
    .bind(subscription.discord_guild_id)
    .bind(subscription.discord_channel_id)
    .bind(subscription.wcl_guild_id)
    .bind(subscription.wcl_site.slug())
    .bind(subscription.wcl_guild_name)
    .bind(subscription.server_slug)
    .bind(subscription.server_name)
    .bind(subscription.region)
    .bind(subscription.baseline_time_ms)
    .fetch_one(&mut *transaction)
    .await?;
    let subscription_id: i64 = row.get("id");

    for report in baseline_reports {
        let is_recent = report
            .end_time_ms
            .is_none_or(|end_time| end_time >= subscription.baseline_time_ms - 30 * 60 * 1_000);
        let initial_fights = baseline_fights
            .iter()
            .find(|(report_code, _)| report_code == &report.code)
            .map(|(_, fights)| fights);
        let baseline_scanned = !is_recent || initial_fights.is_some();
        sqlx::query(
            r#"
            INSERT INTO warcraft_logs_reports (
                subscription_id, code, title, start_time_ms, end_time_ms,
                revision, zone_name, visibility, announcement_state,
                baseline_scanned, track_until
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, 'suppressed', $9,
                CASE WHEN $10 THEN NOW() + INTERVAL '12 hours' ELSE NOW() END
            )
            "#,
        )
        .bind(subscription_id)
        .bind(&report.code)
        .bind(&report.title)
        .bind(report.start_time_ms)
        .bind(report.end_time_ms)
        .bind(report.revision)
        .bind(&report.zone_name)
        .bind(&report.visibility)
        .bind(baseline_scanned)
        .bind(is_recent)
        .execute(&mut *transaction)
        .await?;

        if let Some(fights) = initial_fights {
            for fight in fights {
                sqlx::query(
                    r#"
                    INSERT INTO warcraft_logs_fights (
                        subscription_id, report_code, fight_id, boss_name, difficulty,
                        raid_size, average_item_level, start_time_ms, end_time_ms,
                        announcement_state
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'suppressed')
                    "#,
                )
                .bind(subscription_id)
                .bind(&report.code)
                .bind(fight.fight_id)
                .bind(&fight.boss_name)
                .bind(fight.difficulty)
                .bind(fight.raid_size)
                .bind(fight.average_item_level)
                .bind(fight.start_time_ms)
                .bind(fight.end_time_ms)
                .execute(&mut *transaction)
                .await?;
            }
        }
    }

    transaction.commit().await?;
    Ok(subscription_id)
}

pub async fn remove_wcl_subscription(pool: &PgPool, discord_guild_id: &str) -> Result<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM warcraft_logs_subscriptions
        WHERE discord_guild_id = $1
        "#,
    )
    .bind(discord_guild_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn get_wcl_subscription(
    pool: &PgPool,
    discord_guild_id: &str,
) -> Result<Option<WclSubscription>> {
    let row = sqlx::query(
        r#"
        SELECT id, discord_guild_id, discord_channel_id, wcl_guild_id, wcl_site,
               wcl_guild_name, server_name, region, discovery_cursor_ms,
               last_polled_at, last_error
        FROM warcraft_logs_subscriptions
        WHERE discord_guild_id = $1 AND enabled = TRUE
        "#,
    )
    .bind(discord_guild_id)
    .fetch_optional(pool)
    .await?;

    row.map(|row| wcl_subscription_from_row(&row)).transpose()
}

pub async fn list_wcl_subscriptions(pool: &PgPool) -> Result<Vec<WclSubscription>> {
    let rows = sqlx::query(
        r#"
        SELECT id, discord_guild_id, discord_channel_id, wcl_guild_id, wcl_site,
               wcl_guild_name, server_name, region, discovery_cursor_ms,
               last_polled_at, last_error
        FROM warcraft_logs_subscriptions
        WHERE enabled = TRUE
        ORDER BY id
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.iter().map(wcl_subscription_from_row).collect()
}

fn wcl_subscription_from_row(row: &sqlx::postgres::PgRow) -> Result<WclSubscription> {
    Ok(WclSubscription {
        id: row.get("id"),
        discord_guild_id: row.get("discord_guild_id"),
        discord_channel_id: row.get("discord_channel_id"),
        wcl_guild_id: row.get("wcl_guild_id"),
        wcl_site: WarcraftLogsSite::from_slug(row.get("wcl_site"))?,
        wcl_guild_name: row.get("wcl_guild_name"),
        server_name: row.get("server_name"),
        region: row.get("region"),
        discovery_cursor_ms: row.get("discovery_cursor_ms"),
        last_polled_at: row.get("last_polled_at"),
        last_error: row.get("last_error"),
    })
}

pub async fn reconcile_wcl_reports(
    pool: &PgPool,
    subscription_id: i64,
    reports: &[WclReportRecord],
    discovery_cursor_ms: i64,
) -> Result<()> {
    let mut transaction = pool.begin().await?;

    for report in reports {
        sqlx::query(
            r#"
            INSERT INTO warcraft_logs_reports (
                subscription_id, code, title, start_time_ms, end_time_ms,
                revision, zone_name, visibility, announcement_state
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')
            ON CONFLICT (subscription_id, code) DO UPDATE
            SET title = EXCLUDED.title,
                start_time_ms = EXCLUDED.start_time_ms,
                end_time_ms = EXCLUDED.end_time_ms,
                zone_name = EXCLUDED.zone_name,
                visibility = EXCLUDED.visibility,
                track_until = CASE
                    WHEN EXCLUDED.revision > warcraft_logs_reports.revision
                        THEN GREATEST(
                            warcraft_logs_reports.track_until,
                            NOW() + INTERVAL '2 hours'
                        )
                    ELSE warcraft_logs_reports.track_until
                END,
                revision = GREATEST(warcraft_logs_reports.revision, EXCLUDED.revision),
                updated_at = NOW()
            "#,
        )
        .bind(subscription_id)
        .bind(&report.code)
        .bind(&report.title)
        .bind(report.start_time_ms)
        .bind(report.end_time_ms)
        .bind(report.revision)
        .bind(&report.zone_name)
        .bind(&report.visibility)
        .execute(&mut *transaction)
        .await?;
    }

    sqlx::query(
        r#"
        UPDATE warcraft_logs_subscriptions
        SET discovery_cursor_ms = GREATEST(discovery_cursor_ms, $2),
            last_polled_at = NOW(),
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(subscription_id)
    .bind(discovery_cursor_ms)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(())
}

pub async fn list_wcl_reports_to_announce(
    pool: &PgPool,
    subscription_id: i64,
) -> Result<Vec<WclReportToAnnounce>> {
    let rows = sqlx::query(
        r#"
        SELECT s.id AS subscription_id, s.discord_channel_id, s.wcl_site, s.wcl_guild_name,
               r.code, r.title, r.start_time_ms, r.zone_name
        FROM warcraft_logs_reports r
        JOIN warcraft_logs_subscriptions s ON s.id = r.subscription_id
        WHERE r.subscription_id = $1
          AND r.announcement_state = 'pending'
        ORDER BY r.start_time_ms, r.code
        "#,
    )
    .bind(subscription_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(WclReportToAnnounce {
                subscription_id: row.get("subscription_id"),
                discord_channel_id: row.get("discord_channel_id"),
                wcl_site: WarcraftLogsSite::from_slug(row.get("wcl_site"))?,
                wcl_guild_name: row.get("wcl_guild_name"),
                code: row.get("code"),
                title: row.get("title"),
                start_time_ms: row.get("start_time_ms"),
                zone_name: row.get("zone_name"),
            })
        })
        .collect()
}

pub async fn mark_wcl_report_posted(
    pool: &PgPool,
    subscription_id: i64,
    code: &str,
    message_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_reports
        SET announcement_state = 'posted', report_message_id = $3,
            last_error = NULL, updated_at = NOW()
        WHERE subscription_id = $1 AND code = $2
        "#,
    )
    .bind(subscription_id)
    .bind(code)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_wcl_report_error(
    pool: &PgPool,
    subscription_id: i64,
    code: &str,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_reports
        SET last_error = $3, updated_at = NOW()
        WHERE subscription_id = $1 AND code = $2
        "#,
    )
    .bind(subscription_id)
    .bind(code)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_wcl_reports_to_inspect(
    pool: &PgPool,
    subscription_id: i64,
) -> Result<Vec<WclReportToInspect>> {
    let rows = sqlx::query(
        r#"
        SELECT subscription_id, code, baseline_scanned,
               announcement_state = 'suppressed' AS suppress_initial_kills
        FROM warcraft_logs_reports
        WHERE subscription_id = $1
          AND track_until > NOW()
        ORDER BY COALESCE(last_inspected_at, '-infinity'::timestamptz), start_time_ms
        "#,
    )
    .bind(subscription_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| WclReportToInspect {
            subscription_id: row.get("subscription_id"),
            code: row.get("code"),
            baseline_scanned: row.get("baseline_scanned"),
            suppress_initial_kills: row.get("suppress_initial_kills"),
        })
        .collect())
}

pub async fn record_wcl_fights(
    pool: &PgPool,
    subscription_id: i64,
    report_code: &str,
    report_revision: i32,
    report_end_time_ms: Option<i64>,
    fights: &[WclFightRecord],
    suppress_new_fights: bool,
) -> Result<()> {
    let mut transaction = pool.begin().await?;
    let state = if suppress_new_fights {
        "suppressed"
    } else {
        "pending"
    };

    for fight in fights {
        sqlx::query(
            r#"
            INSERT INTO warcraft_logs_fights (
                subscription_id, report_code, fight_id, boss_name, difficulty,
                raid_size, average_item_level, start_time_ms, end_time_ms,
                announcement_state
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (subscription_id, report_code, fight_id) DO UPDATE
            SET boss_name = EXCLUDED.boss_name,
                difficulty = EXCLUDED.difficulty,
                raid_size = EXCLUDED.raid_size,
                average_item_level = EXCLUDED.average_item_level,
                start_time_ms = EXCLUDED.start_time_ms,
                end_time_ms = EXCLUDED.end_time_ms,
                updated_at = NOW()
            "#,
        )
        .bind(subscription_id)
        .bind(report_code)
        .bind(fight.fight_id)
        .bind(&fight.boss_name)
        .bind(fight.difficulty)
        .bind(fight.raid_size)
        .bind(fight.average_item_level)
        .bind(fight.start_time_ms)
        .bind(fight.end_time_ms)
        .bind(state)
        .execute(&mut *transaction)
        .await?;
    }

    sqlx::query(
        r#"
        UPDATE warcraft_logs_reports
        SET baseline_scanned = TRUE,
            revision = GREATEST(revision, $3),
            end_time_ms = COALESCE($4, end_time_ms),
            last_inspected_at = NOW(),
            last_error = NULL,
            updated_at = NOW()
        WHERE subscription_id = $1 AND code = $2
        "#,
    )
    .bind(subscription_id)
    .bind(report_code)
    .bind(report_revision)
    .bind(report_end_time_ms)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(())
}

pub async fn list_pending_wcl_fights(
    pool: &PgPool,
    subscription_id: i64,
) -> Result<Vec<WclPendingFight>> {
    let rows = sqlx::query(
        r#"
        SELECT f.subscription_id, s.discord_channel_id, s.wcl_site, s.wcl_guild_name,
               f.report_code, r.title AS report_title,
               r.start_time_ms AS report_start_time_ms,
               f.fight_id, f.boss_name, f.difficulty, f.raid_size,
               f.average_item_level, f.start_time_ms, f.end_time_ms
        FROM warcraft_logs_fights f
        JOIN warcraft_logs_reports r
          ON r.subscription_id = f.subscription_id AND r.code = f.report_code
        JOIN warcraft_logs_subscriptions s ON s.id = f.subscription_id
        WHERE f.subscription_id = $1
          AND f.announcement_state = 'pending'
        ORDER BY r.start_time_ms, f.fight_id
        "#,
    )
    .bind(subscription_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(WclPendingFight {
                subscription_id: row.get("subscription_id"),
                discord_channel_id: row.get("discord_channel_id"),
                wcl_site: WarcraftLogsSite::from_slug(row.get("wcl_site"))?,
                wcl_guild_name: row.get("wcl_guild_name"),
                report_code: row.get("report_code"),
                report_title: row.get("report_title"),
                report_start_time_ms: row.get("report_start_time_ms"),
                fight: WclFightRecord {
                    fight_id: row.get("fight_id"),
                    boss_name: row.get("boss_name"),
                    difficulty: row.get("difficulty"),
                    raid_size: row.get("raid_size"),
                    average_item_level: row.get("average_item_level"),
                    start_time_ms: row.get("start_time_ms"),
                    end_time_ms: row.get("end_time_ms"),
                },
            })
        })
        .collect()
}

pub async fn mark_wcl_fight_posted(
    pool: &PgPool,
    subscription_id: i64,
    report_code: &str,
    fight_id: i32,
    message_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_fights
        SET announcement_state = 'posted', discord_message_id = $4,
            last_error = NULL, updated_at = NOW()
        WHERE subscription_id = $1 AND report_code = $2 AND fight_id = $3
        "#,
    )
    .bind(subscription_id)
    .bind(report_code)
    .bind(fight_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_wcl_fight_error(
    pool: &PgPool,
    subscription_id: i64,
    report_code: &str,
    fight_id: i32,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_fights
        SET last_error = $4, updated_at = NOW()
        WHERE subscription_id = $1 AND report_code = $2 AND fight_id = $3
        "#,
    )
    .bind(subscription_id)
    .bind(report_code)
    .bind(fight_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_wcl_subscription_error(
    pool: &PgPool,
    subscription_id: i64,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_subscriptions
        SET last_error = $2, last_polled_at = NOW(), updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(subscription_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_wcl_subscription_error(pool: &PgPool, subscription_id: i64) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE warcraft_logs_subscriptions
        SET last_error = NULL, last_polled_at = NOW(), updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(subscription_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// World of Warcraft characters
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct NewWowCharacter<'a> {
    pub discord_user_id: &'a str,
    pub region: &'a str,
    pub realm_name: &'a str,
    pub realm_name_normalized: &'a str,
    pub character_name: &'a str,
    pub character_name_normalized: &'a str,
}

/// Store a character, returning false when the user already has it stored.
pub async fn store_wow_character(pool: &PgPool, character: NewWowCharacter<'_>) -> Result<bool> {
    let result = sqlx::query(
        r#"
        INSERT INTO wow_characters (
            discord_user_id,
            region,
            realm_name,
            realm_name_normalized,
            character_name,
            character_name_normalized
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (
            discord_user_id,
            region,
            realm_name_normalized,
            character_name_normalized
        ) DO NOTHING
        "#,
    )
    .bind(character.discord_user_id)
    .bind(character.region)
    .bind(character.realm_name)
    .bind(character.realm_name_normalized)
    .bind(character.character_name)
    .bind(character.character_name_normalized)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() == 1)
}

/// Remove one of a user's characters, returning whether it existed.
pub async fn remove_wow_character(
    pool: &PgPool,
    discord_user_id: &str,
    region: &str,
    realm_name_normalized: &str,
    character_name_normalized: &str,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM wow_characters
        WHERE discord_user_id = $1
          AND region = $2
          AND realm_name_normalized = $3
          AND character_name_normalized = $4
        "#,
    )
    .bind(discord_user_id)
    .bind(region)
    .bind(realm_name_normalized)
    .bind(character_name_normalized)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() == 1)
}

// ---------------------------------------------------------------------------
// ISS telemetry history
// ---------------------------------------------------------------------------

use crate::iss_telemetry::IssUrineTelemetry;

pub struct IssTelemetrySample {
    pub recorded_at: DateTime<Utc>,
    pub urine_tank_pct: f64,
    pub waste_water_pct: f64,
    pub clean_water_pct: f64,
    pub processor_status: String,
}

pub async fn insert_iss_telemetry(pool: &PgPool, telemetry: &IssUrineTelemetry) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO iss_telemetry_history (
            urine_tank_pct, waste_water_pct, clean_water_pct,
            processor_status, signal_acquired
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(telemetry.tank_percentage)
    .bind(telemetry.waste_water_percentage)
    .bind(telemetry.clean_water_percentage)
    .bind(&telemetry.processor_status)
    .bind(telemetry.signal_acquired)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_iss_telemetry_history(
    pool: &PgPool,
    hours: i64,
) -> Result<Vec<IssTelemetrySample>> {
    let rows = sqlx::query(
        r#"
        SELECT recorded_at, urine_tank_pct, waste_water_pct,
               clean_water_pct, processor_status
        FROM iss_telemetry_history
        WHERE recorded_at >= NOW() - make_interval(hours => $1)
        ORDER BY recorded_at ASC
        "#,
    )
    .bind(hours as i32)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| IssTelemetrySample {
            recorded_at: r.get("recorded_at"),
            urine_tank_pct: r.get("urine_tank_pct"),
            waste_water_pct: r.get("waste_water_pct"),
            clean_water_pct: r.get("clean_water_pct"),
            processor_status: r.get("processor_status"),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Pisstory subscriptions
// ---------------------------------------------------------------------------

pub struct PisstorySubscription {
    pub discord_guild_id: String,
    pub discord_channel_id: String,
    pub interval_seconds: i64,
}

pub async fn upsert_pisstory_subscription(
    pool: &PgPool,
    discord_guild_id: &str,
    discord_channel_id: &str,
    interval_seconds: i64,
    next_post_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO pisstory_subscriptions (
            discord_guild_id, discord_channel_id, interval_seconds, next_post_at
        )
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (discord_guild_id) DO UPDATE SET
            discord_channel_id = EXCLUDED.discord_channel_id,
            interval_seconds = EXCLUDED.interval_seconds,
            next_post_at = EXCLUDED.next_post_at,
            last_error = NULL,
            updated_at = NOW()
        "#,
    )
    .bind(discord_guild_id)
    .bind(discord_channel_id)
    .bind(interval_seconds)
    .bind(next_post_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn claim_due_pisstory_subscriptions(pool: &PgPool) -> Result<Vec<PisstorySubscription>> {
    let rows = sqlx::query(
        r#"
        WITH due AS (
            SELECT discord_guild_id
            FROM pisstory_subscriptions
            WHERE next_post_at <= NOW()
            ORDER BY next_post_at
            FOR UPDATE SKIP LOCKED
        )
        UPDATE pisstory_subscriptions AS subscription
        SET next_post_at = NOW()
                + (subscription.interval_seconds::DOUBLE PRECISION * INTERVAL '1 second'),
            updated_at = NOW()
        FROM due
        WHERE subscription.discord_guild_id = due.discord_guild_id
        RETURNING subscription.discord_guild_id,
                  subscription.discord_channel_id,
                  subscription.interval_seconds
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| PisstorySubscription {
            discord_guild_id: row.get("discord_guild_id"),
            discord_channel_id: row.get("discord_channel_id"),
            interval_seconds: row.get("interval_seconds"),
        })
        .collect())
}

pub async fn mark_pisstory_subscription_posted(
    pool: &PgPool,
    discord_guild_id: &str,
    discord_channel_id: &str,
    interval_seconds: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE pisstory_subscriptions
        SET last_posted_at = NOW(),
            last_error = NULL,
            updated_at = NOW()
        WHERE discord_guild_id = $1
          AND discord_channel_id = $2
          AND interval_seconds = $3
        "#,
    )
    .bind(discord_guild_id)
    .bind(discord_channel_id)
    .bind(interval_seconds)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_pisstory_subscription_failed(
    pool: &PgPool,
    discord_guild_id: &str,
    discord_channel_id: &str,
    interval_seconds: i64,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE pisstory_subscriptions
        SET next_post_at = NOW() + INTERVAL '5 minutes',
            last_error = $4,
            updated_at = NOW()
        WHERE discord_guild_id = $1
          AND discord_channel_id = $2
          AND interval_seconds = $3
        "#,
    )
    .bind(discord_guild_id)
    .bind(discord_channel_id)
    .bind(interval_seconds)
    .bind(error)
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NewWclSubscription, NewWowCharacter, WclFightRecord, WclReportRecord, get_wcl_subscription,
        list_pending_wcl_fights, list_wcl_reports_to_announce, list_wcl_reports_to_inspect,
        mark_wcl_fight_posted, mark_wcl_report_posted, reconcile_wcl_reports,
        remove_wcl_subscription, remove_wow_character, replace_wcl_subscription,
        store_wow_character,
    };
    use crate::warcraft_logs::WarcraftLogsSite;
    use sqlx::PgPool;
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
    async fn stores_and_removes_characters_for_the_owning_user() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/006_wow_characters.sql"))
            .execute(&pool)
            .await
            .unwrap();

        let owner_id = format!("character-owner-{}", Uuid::new_v4());
        let other_user_id = format!("character-other-{}", Uuid::new_v4());
        let character = NewWowCharacter {
            discord_user_id: &owner_id,
            region: "us",
            realm_name: "Area 52",
            realm_name_normalized: "area 52",
            character_name: "Thrall",
            character_name_normalized: "thrall",
        };

        assert!(store_wow_character(&pool, character).await.unwrap());
        assert!(!store_wow_character(&pool, character).await.unwrap());
        assert!(
            !remove_wow_character(&pool, &other_user_id, "us", "area 52", "thrall")
                .await
                .unwrap()
        );
        assert!(
            remove_wow_character(&pool, &owner_id, "us", "area 52", "thrall")
                .await
                .unwrap()
        );
        assert!(
            !remove_wow_character(&pool, &owner_id, "us", "area 52", "thrall")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
    async fn persists_baselines_and_deduplicates_report_and_fight_work() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/004_warcraft_logs.sql"))
            .execute(&pool)
            .await
            .unwrap();

        let discord_guild_id = format!("test-{}", Uuid::new_v4());
        let baseline_report = WclReportRecord {
            code: "baseline-report".to_owned(),
            title: "Baseline".to_owned(),
            start_time_ms: 1_000,
            end_time_ms: None,
            revision: 0,
            zone_name: Some("Test Zone".to_owned()),
            visibility: "public".to_owned(),
        };
        let baseline_fight = WclFightRecord {
            fight_id: 1,
            boss_name: "Old Boss".to_owned(),
            difficulty: Some(5),
            raid_size: Some(20),
            average_item_level: Some(700.0),
            start_time_ms: 10_000,
            end_time_ms: 70_000,
        };
        let subscription_id = replace_wcl_subscription(
            &pool,
            NewWclSubscription {
                discord_guild_id: &discord_guild_id,
                discord_channel_id: "123",
                wcl_guild_id: 42,
                wcl_site: WarcraftLogsSite::Classic,
                wcl_guild_name: "Test Guild",
                server_slug: "test-realm",
                server_name: "Test Realm",
                region: "US",
                baseline_time_ms: 100_000,
            },
            std::slice::from_ref(&baseline_report),
            &[("baseline-report".to_owned(), vec![baseline_fight])],
        )
        .await
        .unwrap();

        let subscription = get_wcl_subscription(&pool, &discord_guild_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(subscription.wcl_site, WarcraftLogsSite::Classic);
        assert!(
            list_wcl_reports_to_announce(&pool, subscription_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            list_pending_wcl_fights(&pool, subscription_id)
                .await
                .unwrap()
                .is_empty()
        );

        let new_report = WclReportRecord {
            code: "new-report".to_owned(),
            title: "New Report".to_owned(),
            start_time_ms: 200_000,
            end_time_ms: Some(300_000),
            revision: 1,
            zone_name: Some("Test Zone".to_owned()),
            visibility: "public".to_owned(),
        };
        reconcile_wcl_reports(
            &pool,
            subscription_id,
            std::slice::from_ref(&new_report),
            400_000,
        )
        .await
        .unwrap();
        reconcile_wcl_reports(
            &pool,
            subscription_id,
            std::slice::from_ref(&new_report),
            400_000,
        )
        .await
        .unwrap();

        let reports = list_wcl_reports_to_announce(&pool, subscription_id)
            .await
            .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].code, "new-report");
        mark_wcl_report_posted(&pool, subscription_id, "new-report", "456")
            .await
            .unwrap();

        let inspections = list_wcl_reports_to_inspect(&pool, subscription_id)
            .await
            .unwrap();
        let new_inspection = inspections
            .iter()
            .find(|report| report.code == "new-report")
            .unwrap();
        assert!(!new_inspection.baseline_scanned);
        assert!(!new_inspection.suppress_initial_kills);
        let baseline_inspection = inspections
            .iter()
            .find(|report| report.code == "baseline-report")
            .unwrap();
        assert!(baseline_inspection.baseline_scanned);
        assert!(baseline_inspection.suppress_initial_kills);

        let new_fight = WclFightRecord {
            fight_id: 2,
            boss_name: "New Boss".to_owned(),
            difficulty: Some(4),
            raid_size: Some(20),
            average_item_level: Some(701.0),
            start_time_ms: 20_000,
            end_time_ms: 80_000,
        };
        super::record_wcl_fights(
            &pool,
            subscription_id,
            "new-report",
            1,
            Some(300_000),
            std::slice::from_ref(&new_fight),
            false,
        )
        .await
        .unwrap();
        super::record_wcl_fights(
            &pool,
            subscription_id,
            "new-report",
            1,
            Some(300_000),
            std::slice::from_ref(&new_fight),
            false,
        )
        .await
        .unwrap();

        let fights = list_pending_wcl_fights(&pool, subscription_id)
            .await
            .unwrap();
        assert_eq!(fights.len(), 1);
        assert_eq!(fights[0].fight.boss_name, "New Boss");
        mark_wcl_fight_posted(&pool, subscription_id, "new-report", 2, "789")
            .await
            .unwrap();
        assert!(
            list_pending_wcl_fights(&pool, subscription_id)
                .await
                .unwrap()
                .is_empty()
        );

        assert!(
            remove_wcl_subscription(&pool, &discord_guild_id)
                .await
                .unwrap()
        );
        assert!(
            get_wcl_subscription(&pool, &discord_guild_id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
