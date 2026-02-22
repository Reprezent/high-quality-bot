use anyhow::{Context, Result, anyhow};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::OnceLock;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::db;
use crate::parsing::build_player_from_run;
use crate::sim_runtime_targets::{default_mop_encounter, default_mop_raid, default_mop_sim_options};
use crate::mop_proto::mop::{
    player, AplRotation, AsyncApiResult, Debuffs, PartyBuffs, ProgressMetrics, Raid, RaidBuffs,
    RaidSimRequest, SimType,
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
    if value.is_finite() {
        value
    } else {
        f64::NAN
    }
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

fn player_spec_label(spec: &Option<player::Spec>) -> &'static str {
    match spec {
        Some(player::Spec::BloodDeathKnight(_)) => "bloodDeathKnight",
        Some(player::Spec::FrostDeathKnight(_)) => "frostDeathKnight",
        Some(player::Spec::UnholyDeathKnight(_)) => "unholyDeathKnight",
        Some(player::Spec::BalanceDruid(_)) => "balanceDruid",
        Some(player::Spec::FeralDruid(_)) => "feralDruid",
        Some(player::Spec::GuardianDruid(_)) => "guardianDruid",
        Some(player::Spec::RestorationDruid(_)) => "restorationDruid",
        Some(player::Spec::BeastMasteryHunter(_)) => "beastMasteryHunter",
        Some(player::Spec::MarksmanshipHunter(_)) => "marksmanshipHunter",
        Some(player::Spec::SurvivalHunter(_)) => "survivalHunter",
        Some(player::Spec::ArcaneMage(_)) => "arcaneMage",
        Some(player::Spec::FireMage(_)) => "fireMage",
        Some(player::Spec::FrostMage(_)) => "frostMage",
        Some(player::Spec::BrewmasterMonk(_)) => "brewmasterMonk",
        Some(player::Spec::MistweaverMonk(_)) => "mistweaverMonk",
        Some(player::Spec::WindwalkerMonk(_)) => "windwalkerMonk",
        Some(player::Spec::HolyPaladin(_)) => "holyPaladin",
        Some(player::Spec::ProtectionPaladin(_)) => "protectionPaladin",
        Some(player::Spec::RetributionPaladin(_)) => "retributionPaladin",
        Some(player::Spec::DisciplinePriest(_)) => "disciplinePriest",
        Some(player::Spec::HolyPriest(_)) => "holyPriest",
        Some(player::Spec::ShadowPriest(_)) => "shadowPriest",
        Some(player::Spec::AssassinationRogue(_)) => "assassinationRogue",
        Some(player::Spec::CombatRogue(_)) => "combatRogue",
        Some(player::Spec::SubtletyRogue(_)) => "subtletyRogue",
        Some(player::Spec::ElementalShaman(_)) => "elementalShaman",
        Some(player::Spec::EnhancementShaman(_)) => "enhancementShaman",
        Some(player::Spec::RestorationShaman(_)) => "restorationShaman",
        Some(player::Spec::AfflictionWarlock(_)) => "afflictionWarlock",
        Some(player::Spec::DemonologyWarlock(_)) => "demonologyWarlock",
        Some(player::Spec::DestructionWarlock(_)) => "destructionWarlock",
        Some(player::Spec::ArmsWarrior(_)) => "armsWarrior",
        Some(player::Spec::FuryWarrior(_)) => "furyWarrior",
        Some(player::Spec::ProtectionWarrior(_)) => "protectionWarrior",
        None => "unknown",
    }
}

fn proto_descriptor_pool() -> Result<&'static DescriptorPool> {
    static DESCRIPTOR_POOL: OnceLock<Result<DescriptorPool, String>> = OnceLock::new();

    let pool = DESCRIPTOR_POOL.get_or_init(|| {
        DescriptorPool::decode(crate::mop_proto::mop::DESCRIPTOR_SET_BYTES)
            .map_err(|error| format!("failed to decode protobuf descriptor set: {error}"))
    });

    match pool {
        Ok(pool) => Ok(pool),
        Err(error) => Err(anyhow!(error.clone())),
    }
}

fn apl_rotation_descriptor() -> Result<&'static MessageDescriptor> {
    static APL_ROTATION_DESCRIPTOR: OnceLock<Result<MessageDescriptor, String>> = OnceLock::new();

    let descriptor = APL_ROTATION_DESCRIPTOR.get_or_init(|| {
        let pool = proto_descriptor_pool().map_err(|error| format!("{error:#}"))?;

        pool.get_message_by_name("proto.APLRotation")
            .ok_or_else(|| "APLRotation descriptor not found in descriptor set".to_string())
    });

    match descriptor {
        Ok(descriptor) => Ok(descriptor),
        Err(error) => Err(anyhow!(error.clone())),
    }
}

fn parse_protojson_message<T>(message_name: &str, value: &Value) -> Result<T>
where
    T: Message + Default,
{
    let pool = proto_descriptor_pool()?;
    let descriptor = pool
        .get_message_by_name(message_name)
        .ok_or_else(|| anyhow!("{message_name} descriptor not found in descriptor set"))?;

    let payload = serde_json::to_string(value)
        .with_context(|| format!("failed to serialize {message_name} payload as JSON"))?;
    let mut deserializer = serde_json::Deserializer::from_str(&payload);
    let dynamic = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .with_context(|| format!("failed to decode {message_name} protojson"))?;

    dynamic
        .transcode_to::<T>()
        .with_context(|| format!("failed to transcode dynamic {message_name} message"))
}

fn extract_raid_buffs_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("raidBuffs")
        .or_else(|| payload.get("raid_buffs"))
        .or_else(|| payload.get("settings").and_then(|settings| settings.get("raidBuffs")))
        .or_else(|| payload.get("settings").and_then(|settings| settings.get("raid_buffs")))
}

fn extract_debuffs_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("debuffs")
        .or_else(|| payload.get("settings").and_then(|settings| settings.get("debuffs")))
}

fn extract_party_buffs_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("partyBuffs")
        .or_else(|| payload.get("party_buffs"))
        .or_else(|| payload.get("settings").and_then(|settings| settings.get("partyBuffs")))
        .or_else(|| payload.get("settings").and_then(|settings| settings.get("party_buffs")))
        .or_else(|| {
            payload
                .get("raid")
                .and_then(|raid| raid.get("parties"))
                .and_then(|parties| parties.as_array())
                .and_then(|parties| parties.first())
                .and_then(|party| party.get("buffs"))
        })
}

fn apl_rotation_to_json(rotation: &AplRotation) -> Value {
    let descriptor = match apl_rotation_descriptor() {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return serde_json::json!({
                "serializationError": format!("{error:#}"),
            });
        }
    };

    let bytes = rotation.encode_to_vec();
    match DynamicMessage::decode(descriptor.clone(), &mut bytes.as_slice()) {
        Ok(dynamic) => serde_json::to_value(dynamic).unwrap_or_else(|error| {
            serde_json::json!({
                "serializationError": format!("failed to serialize APL dynamic message to json: {error}"),
            })
        }),
        Err(error) => serde_json::json!({
            "serializationError": format!("failed to decode APL rotation bytes: {error}"),
        }),
    }
}

fn request_to_json(request: &RaidSimRequest) -> Value {
    let raid = request.raid.as_ref();
    let encounter = request.encounter.as_ref();
    let sim_options = request.sim_options.as_ref();

    let parties = raid
        .map(|raid| {
            raid.parties
                .iter()
                .enumerate()
                .map(|(party_index, party)| {
                    let players: Vec<Value> = party
                        .players
                        .iter()
                        .enumerate()
                        .filter(|(_, player)| player.class != 0)
                        .map(|(player_index, player)| {
                            let equipment_items: Vec<Value> = player
                                .equipment
                                .as_ref()
                                .map(|equipment| {
                                    equipment
                                        .items
                                        .iter()
                                        .map(|item| {
                                            serde_json::json!({
                                                "id": item.id,
                                                "enchant": item.enchant,
                                                "gems": item.gems,
                                                "reforging": item.reforging,
                                                "randomSuffix": item.random_suffix,
                                                "upgradeStep": item.upgrade_step,
                                                "challengeMode": item.challenge_mode,
                                                "tinker": item.tinker,
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            serde_json::json!({
                                "partyIndex": party_index,
                                "playerIndex": player_index,
                                "name": player.name,
                                "class": player.class,
                                "race": player.race,
                                "talentsString": player.talents_string,
                                "spec": player_spec_label(&player.spec),
                                "rotationPresent": player.rotation.is_some(),
                                "rotationApl": player.rotation.as_ref().map(apl_rotation_to_json),
                                "equipment": {
                                    "items": equipment_items,
                                },
                            })
                        })
                        .collect();

                    serde_json::json!({
                        "partyIndex": party_index,
                        "players": players,
                    })
                })
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default();

    let targets = encounter
        .map(|encounter| {
            encounter
                .targets
                .iter()
                .map(|target| {
                    serde_json::json!({
                        "id": target.id,
                        "name": target.name,
                        "level": target.level,
                        "mobType": target.mob_type,
                        "stats": target.stats,
                        "minBaseDamage": target.min_base_damage,
                        "damageSpread": target.damage_spread,
                        "swingSpeed": target.swing_speed,
                        "tankIndex": target.tank_index,
                    })
                })
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default();

    serde_json::json!({
        "requestId": request.request_id,
        "type": request.r#type,
        "raid": {
            "numActiveParties": raid.map(|r| r.num_active_parties).unwrap_or_default(),
            "parties": parties,
        },
        "encounter": {
            "apiVersion": encounter.map(|e| e.api_version).unwrap_or_default(),
            "duration": encounter.map(|e| e.duration).unwrap_or_default(),
            "durationVariation": encounter.map(|e| e.duration_variation).unwrap_or_default(),
            "executeProportion20": encounter.map(|e| e.execute_proportion_20).unwrap_or_default(),
            "executeProportion25": encounter.map(|e| e.execute_proportion_25).unwrap_or_default(),
            "executeProportion35": encounter.map(|e| e.execute_proportion_35).unwrap_or_default(),
            "executeProportion45": encounter.map(|e| e.execute_proportion_45).unwrap_or_default(),
            "executeProportion90": encounter.map(|e| e.execute_proportion_90).unwrap_or_default(),
            "useHealth": encounter.map(|e| e.use_health).unwrap_or_default(),
            "targets": targets,
        },
        "simOptions": {
            "iterations": sim_options.map(|s| s.iterations).unwrap_or_default(),
            "randomSeed": sim_options.map(|s| s.random_seed).unwrap_or_default(),
            "debug": sim_options.map(|s| s.debug).unwrap_or_default(),
            "debugFirstIteration": sim_options.map(|s| s.debug_first_iteration).unwrap_or_default(),
            "interactive": sim_options.map(|s| s.interactive).unwrap_or_default(),
            "useLabeledRands": sim_options.map(|s| s.use_labeled_rands).unwrap_or_default(),
        }
    })
}

fn maybe_log_request_json(run_id: Uuid, request: &RaidSimRequest) {
    let enabled = std::env::var("LOG_SIM_REQUEST_JSON")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        })
        .unwrap_or(true);

    if !enabled {
        return;
    }

    match serde_json::to_string_pretty(&request_to_json(request)) {
        Ok(json) => tracing::info!(run_id = %run_id, request_json = %json, "sending raid sim request"),
        Err(error) => tracing::warn!(run_id = %run_id, error = ?error, "failed to serialize raid sim request json"),
    }
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

    let mapped_player = build_player_from_run(&run)?;
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
                    "failed to parse raidBuffs from payload; using default raid buffs"
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
                    "failed to parse debuffs from payload; using default debuffs"
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
                    "failed to parse partyBuffs from payload; using default party buffs"
                );
            }
        }
    }

    raid.parties[0].players[0] = mapped_player;

    let request = RaidSimRequest {
        request_id: request_id.clone(),
        raid: Some(raid),
        encounter: Some(default_mop_encounter()),
        sim_options: Some(default_mop_sim_options(run_id)),
        r#type: SimType::Raid as i32,
        ..Default::default()
    };

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

    let start_body = response.bytes().await.context("failed to read /raidSimAsync response")?;
    let async_result = AsyncApiResult::decode(start_body.as_ref())
        .context("failed to decode AsyncApiResult")?;

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
                    return Err(anyhow!("failed to call /asyncProgress after repeated retries: {error}"));
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
                    return Err(anyhow!("failed to read /asyncProgress response after repeated retries: {error}"));
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
                    return Err(anyhow!("failed to decode /asyncProgress response after repeated retries: {error}"));
                }

                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let is_final = progress.final_raid_result.is_some() || progress.final_weight_result.is_some();
        let iterations_done =
            progress.total_iterations > 0 && progress.completed_iterations >= progress.total_iterations;
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