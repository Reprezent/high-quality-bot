use anyhow::{Context, Result, anyhow};
use prost::Message;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashSet;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::db;
use crate::mop_proto::mop::{
    AsyncApiResult, Debuffs, PartyBuffs, ProgressMetrics, Raid, RaidBuffs, RaidSimRequest, SimType,
};
use crate::parsing::build_player_from_run;
use crate::sim_runtime_targets::{
    default_mop_encounter, default_mop_raid, default_mop_sim_options,
};
use crate::ui_import::{
    INPUT_FORMAT_INDIVIDUAL_UI_EXPORT, parse_normalized_raid_sim_request, parse_protojson_message,
    protojson_message_to_value,
};

fn raid_player_count(raid: &Raid) -> usize {
    raid.parties
        .iter()
        .flat_map(|party| party.players.iter())
        .filter(|player| player.class != 0)
        .count()
}

fn validate_sim_request_payload(request: &RaidSimRequest) -> Result<()> {
    let raid = request
        .raid
        .as_ref()
        .ok_or_else(|| anyhow!("simulation request is missing raid payload"))?;

    if raid_player_count(raid) == 0 {
        return Err(anyhow!(
            "simulation request is invalid: raid has no players"
        ));
    }

    let encounter = request
        .encounter
        .as_ref()
        .ok_or_else(|| anyhow!("simulation request is missing encounter payload"))?;

    if encounter.targets.is_empty() {
        return Err(anyhow!(
            "simulation request is invalid: encounter has 0 targets"
        ));
    }

    Ok(())
}

fn finite_or_nan(value: f64) -> f64 {
    if value.is_finite() { value } else { f64::NAN }
}

fn extract_raid_members(progress: &ProgressMetrics) -> Vec<String> {
    let mut unique_members = HashSet::new();
    let mut raid_members = Vec::new();

    let parties = progress
        .final_raid_result
        .as_ref()
        .and_then(|result| result.raid_metrics.as_ref())
        .map(|metrics| metrics.parties.iter())
        .into_iter()
        .flatten();

    for party in parties {
        for player in &party.players {
            let name = player.name.trim();
            let resolved_name = if name.is_empty() {
                format!("Player {}", player.unit_index)
            } else {
                name.to_string()
            };

            if unique_members.insert(resolved_name.clone()) {
                raid_members.push(resolved_name);
            }
        }
    }

    raid_members
}

fn extract_raid_buffs_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("raidBuffs")
        .or_else(|| payload.get("raid_buffs"))
        .or_else(|| {
            payload
                .get("settings")
                .and_then(|settings| settings.get("raidBuffs"))
        })
        .or_else(|| {
            payload
                .get("settings")
                .and_then(|settings| settings.get("raid_buffs"))
        })
}

fn extract_debuffs_payload(payload: &Value) -> Option<&Value> {
    payload.get("debuffs").or_else(|| {
        payload
            .get("settings")
            .and_then(|settings| settings.get("debuffs"))
    })
}

fn extract_party_buffs_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("partyBuffs")
        .or_else(|| payload.get("party_buffs"))
        .or_else(|| {
            payload
                .get("settings")
                .and_then(|settings| settings.get("partyBuffs"))
        })
        .or_else(|| {
            payload
                .get("settings")
                .and_then(|settings| settings.get("party_buffs"))
        })
        .or_else(|| {
            payload
                .get("raid")
                .and_then(|raid| raid.get("parties"))
                .and_then(|parties| parties.as_array())
                .and_then(|parties| parties.first())
                .and_then(|party| party.get("buffs"))
        })
}

fn maybe_log_request_json(run_id: Uuid, request: &RaidSimRequest) {
    let enabled = std::env::var("LOG_SIM_REQUEST_JSON")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        })
        .unwrap_or(false);

    if !enabled {
        return;
    }

    match protojson_message_to_value("proto.RaidSimRequest", request)
        .and_then(|value| serde_json::to_string_pretty(&value).map_err(Into::into))
    {
        Ok(json) => {
            tracing::info!(run_id = %run_id, request_json = %json, "sending raid sim request")
        }
        Err(error) => {
            tracing::warn!(run_id = %run_id, error = ?error, "failed to serialize raid sim request json")
        }
    }
}

fn build_legacy_request(run: &db::SimulationRun, run_id: Uuid) -> Result<RaidSimRequest> {
    let mapped_player = build_player_from_run(run)?;
    let mut raid = default_mop_raid();

    if let Some(raid_buffs_payload) = extract_raid_buffs_payload(&run.gear_payload) {
        match parse_protojson_message::<RaidBuffs>("proto.RaidBuffs", raid_buffs_payload) {
            Ok(raid_buffs) => {
                raid.buffs = Some(raid_buffs);
            }
            Err(error) => {
                tracing::warn!(
                    run_id = %run_id,
                    error = ?error,
                    "failed to parse legacy raidBuffs from payload; using default raid buffs"
                );
            }
        }
    }

    if let Some(debuffs_payload) = extract_debuffs_payload(&run.gear_payload) {
        match parse_protojson_message::<Debuffs>("proto.Debuffs", debuffs_payload) {
            Ok(debuffs) => {
                raid.debuffs = Some(debuffs);
            }
            Err(error) => {
                tracing::warn!(
                    run_id = %run_id,
                    error = ?error,
                    "failed to parse legacy debuffs from payload; using default debuffs"
                );
            }
        }
    }

    if let Some(party_buffs_payload) = extract_party_buffs_payload(&run.gear_payload) {
        match parse_protojson_message::<PartyBuffs>("proto.PartyBuffs", party_buffs_payload) {
            Ok(party_buffs) => {
                if let Some(party) = raid.parties.get_mut(0) {
                    party.buffs = Some(party_buffs);
                }
            }
            Err(error) => {
                tracing::warn!(
                    run_id = %run_id,
                    error = ?error,
                    "failed to parse legacy partyBuffs from payload; using default party buffs"
                );
            }
        }
    }

    raid.parties[0].players[0] = mapped_player;

    Ok(RaidSimRequest {
        request_id: run_id.to_string(),
        raid: Some(raid),
        encounter: Some(default_mop_encounter()),
        sim_options: Some(default_mop_sim_options(run_id)),
        r#type: SimType::Raid as i32,
    })
}

pub async fn run_async_simulation(
    pool: PgPool,
    sim_api_base_url: String,
    run_id: Uuid,
) -> Result<()> {
    db::update_simulation_run_status(&pool, run_id, "running").await?;

    let client = reqwest::Client::new();
    let request_id = run_id.to_string();

    let run = db::get_simulation_run(&pool, run_id)
        .await?
        .ok_or_else(|| anyhow!("simulation run not found: {}", run_id))?;

    let mut request = match run.normalized_request.as_ref() {
        Some(payload) => parse_normalized_raid_sim_request(payload).with_context(|| {
            format!("failed to load normalized simulation request for run {run_id}")
        })?,
        None if run.input_format == INPUT_FORMAT_INDIVIDUAL_UI_EXPORT => {
            return Err(anyhow!(
                "individual UI export run {run_id} is missing its normalized request"
            ));
        }
        None => build_legacy_request(&run, run_id)?,
    };
    request.request_id = request_id.clone();

    if let Err(error) = validate_sim_request_payload(&request) {
        db::update_simulation_run_status(&pool, run_id, "failed").await?;
        return Err(error);
    }

    maybe_log_request_json(run_id, &request);

    let start_url = format!("{}/raidSimAsync?requestId={}", sim_api_base_url, request_id);
    let response = client
        .post(start_url)
        .header("content-type", "application/x-protobuf")
        .body(request.encode_to_vec())
        .send()
        .await
        .context("failed to call /raidSimAsync")?;

    if !response.status().is_success() {
        db::update_simulation_run_status(&pool, run_id, "failed").await?;
        return Err(anyhow!("raidSimAsync returned HTTP {}", response.status()));
    }

    let start_body = response
        .bytes()
        .await
        .context("failed to read /raidSimAsync response")?;
    let async_result =
        AsyncApiResult::decode(start_body.as_ref()).context("failed to decode AsyncApiResult")?;

    let mut frame_index: i32 = 0;
    let mut idle_polls = 0;
    let mut transient_poll_errors = 0;
    let mut complete_without_final_polls = 0;

    loop {
        let poll_response = match client
            .post(format!("{}/asyncProgress", sim_api_base_url))
            .header("content-type", "application/x-protobuf")
            .body(async_result.encode_to_vec())
            .send()
            .await
        {
            Ok(response) => {
                transient_poll_errors = 0;
                response
            }
            Err(error) => {
                transient_poll_errors += 1;
                tracing::warn!(
                    run_id = %run_id,
                    request_id = %request_id,
                    transient_poll_errors,
                    error = ?error,
                    "transient /asyncProgress request error"
                );

                if transient_poll_errors > 30 {
                    db::update_simulation_run_status(&pool, run_id, "failed").await?;
                    return Err(anyhow!(
                        "failed to call /asyncProgress after repeated retries: {error}"
                    ));
                }

                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        if poll_response.status().as_u16() == 204 || poll_response.status().as_u16() == 404 {
            idle_polls += 1;
            if idle_polls > 300 {
                db::update_simulation_run_status(&pool, run_id, "failed").await?;
                return Err(anyhow!("timed out waiting for async progress"));
            }

            sleep(Duration::from_secs(1)).await;
            continue;
        }

        idle_polls = 0;

        if !poll_response.status().is_success() {
            db::update_simulation_run_status(&pool, run_id, "failed").await?;
            return Err(anyhow!(
                "asyncProgress returned HTTP {}",
                poll_response.status()
            ));
        }

        let progress_body = match poll_response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                transient_poll_errors += 1;
                tracing::warn!(
                    run_id = %run_id,
                    request_id = %request_id,
                    transient_poll_errors,
                    error = ?error,
                    "transient /asyncProgress read error"
                );

                if transient_poll_errors > 30 {
                    db::update_simulation_run_status(&pool, run_id, "failed").await?;
                    return Err(anyhow!(
                        "failed to read /asyncProgress response after repeated retries: {error}"
                    ));
                }

                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let progress = match ProgressMetrics::decode(progress_body.as_ref()) {
            Ok(progress) => progress,
            Err(error) => {
                transient_poll_errors += 1;
                tracing::warn!(
                    run_id = %run_id,
                    request_id = %request_id,
                    transient_poll_errors,
                    error = ?error,
                    "transient /asyncProgress decode error"
                );

                if transient_poll_errors > 30 {
                    db::update_simulation_run_status(&pool, run_id, "failed").await?;
                    return Err(anyhow!(
                        "failed to decode /asyncProgress response after repeated retries: {error}"
                    ));
                }

                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let is_final =
            progress.final_raid_result.is_some() || progress.final_weight_result.is_some();
        let iterations_done = progress.total_iterations > 0
            && progress.completed_iterations >= progress.total_iterations;
        let sims_done = progress.total_sims > 0 && progress.completed_sims >= progress.total_sims;
        let appears_complete_without_final = !is_final && (iterations_done || sims_done);

        if appears_complete_without_final {
            complete_without_final_polls += 1;
        } else {
            complete_without_final_polls = 0;
        }

        let final_raid_dps = progress
            .final_raid_result
            .as_ref()
            .and_then(|result| result.raid_metrics.as_ref())
            .and_then(|metrics| metrics.dps.as_ref())
            .map(|distribution| distribution.avg);

        let final_raid_hps = progress
            .final_raid_result
            .as_ref()
            .and_then(|result| result.raid_metrics.as_ref())
            .and_then(|metrics| metrics.hps.as_ref())
            .map(|distribution| distribution.avg);

        let safe_dps = finite_or_nan(final_raid_dps.unwrap_or(progress.dps));
        let safe_hps = finite_or_nan(final_raid_hps.unwrap_or(progress.hps));

        db::insert_simulation_progress_frame(
            &pool,
            run_id,
            frame_index,
            progress.completed_iterations,
            progress.total_iterations,
            progress.completed_sims,
            progress.total_sims,
            safe_dps,
            safe_hps,
            is_final,
        )
        .await?;

        frame_index += 1;

        if is_final || complete_without_final_polls >= 3 {
            let raid_members = extract_raid_members(&progress);
            if !raid_members.is_empty() {
                db::update_simulation_run_raid_members(&pool, run_id, &raid_members).await?;
            }

            let raid_error = progress
                .final_raid_result
                .as_ref()
                .and_then(|result| result.error.as_ref());

            if let Some(error) = raid_error {
                tracing::error!(
                    run_id = %run_id,
                    request_id = %request_id,
                    error_type = error.r#type,
                    error_message = %error.message,
                    "raid sim returned final error"
                );
            }

            if !is_final {
                tracing::warn!(
                    run_id = %run_id,
                    request_id = %request_id,
                    completed_iterations = progress.completed_iterations,
                    total_iterations = progress.total_iterations,
                    completed_sims = progress.completed_sims,
                    total_sims = progress.total_sims,
                    "marking simulation complete using fallback (no final result payload received)"
                );
            }

            let has_error = is_final && raid_error.is_some();

            let status = if has_error { "failed" } else { "complete" };
            db::update_simulation_run_status(&pool, run_id, status).await?;
            return Ok(());
        }

        sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_import::ImportedIndividualSim;

    #[tokio::test]
    #[ignore = "requires a running WoWSims async API at WOWSIMS_API_BASE_URL"]
    async fn submits_a_normalized_individual_ui_request_to_the_async_api() {
        let base_url = std::env::var("WOWSIMS_API_BASE_URL")
            .expect("WOWSIMS_API_BASE_URL must point at a running simulator");
        let mut payload: Value =
            serde_json::from_str(include_str!("../tests/fixtures/individual-ui-export.json"))
                .expect("fixture must be valid JSON");
        payload["settings"]["iterations"] = serde_json::json!(1);
        let request = ImportedIndividualSim::from_json(&payload)
            .expect("fixture must import")
            .normalize(Uuid::new_v4())
            .expect("fixture must normalize")
            .request;
        let client = reqwest::Client::new();

        let response = client
            .post(format!(
                "{base_url}/raidSimAsync?requestId={}",
                request.request_id
            ))
            .header("content-type", "application/x-protobuf")
            .body(request.encode_to_vec())
            .send()
            .await
            .expect("async API request must complete");
        assert!(
            response.status().is_success(),
            "raidSimAsync returned {}",
            response.status()
        );
        let async_result = AsyncApiResult::decode(
            response
                .bytes()
                .await
                .expect("async API response body must be readable")
                .as_ref(),
        )
        .expect("async API response must be protobuf");

        for _ in 0..120 {
            let response = client
                .post(format!("{base_url}/asyncProgress"))
                .header("content-type", "application/x-protobuf")
                .body(async_result.encode_to_vec())
                .send()
                .await
                .expect("async progress request must complete");

            if response.status().as_u16() == 204 {
                sleep(Duration::from_millis(250)).await;
                continue;
            }

            assert!(
                response.status().is_success(),
                "asyncProgress returned {}",
                response.status()
            );
            let progress = ProgressMetrics::decode(
                response
                    .bytes()
                    .await
                    .expect("async progress body must be readable")
                    .as_ref(),
            )
            .expect("async progress body must be protobuf");

            if let Some(result) = progress.final_raid_result {
                let error_message = result
                    .error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .unwrap_or("no simulator error");
                assert!(
                    result.error.is_none(),
                    "simulator rejected normalized request: {error_message}"
                );
                return;
            }

            sleep(Duration::from_millis(250)).await;
        }

        panic!("simulator did not return final metrics within 30 seconds");
    }
}
