use anyhow::{Context, Result, anyhow, bail};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::mop_proto::mop::{
    Class, IndividualSimSettings, ItemSlot, Party, PartyBuffs, Player, Profession, Raid,
    RaidSimRequest, SimOptions, SimType, UnitReference, player, unit_reference,
};

pub const INPUT_FORMAT_INDIVIDUAL_UI_EXPORT: &str = "individual-ui-export";
pub const MOP_UPSTREAM_REVISION: &str = env!("MOP_UPSTREAM_REVISION");

const INDIVIDUAL_SIM_SETTINGS_API_VERSION: i32 = 15;
const INDIVIDUAL_RAID_PARTY_COUNT: usize = 5;
const PARTY_SIZE: usize = 5;
const MAX_ITERATIONS: i32 = 1_000_000;

pub struct ImportedIndividualSim {
    settings: IndividualSimSettings,
    pub class: String,
    pub spec: String,
}

pub struct NormalizedIndividualSim {
    pub request: RaidSimRequest,
    pub class: String,
    pub spec: String,
    pub effective_random_seed: i64,
    pub effective_iterations: i32,
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

fn message_descriptor(message_name: &str) -> Result<MessageDescriptor> {
    proto_descriptor_pool()?
        .get_message_by_name(message_name)
        .ok_or_else(|| anyhow!("{message_name} descriptor not found in descriptor set"))
}

pub fn parse_protojson_message<T>(message_name: &str, value: &Value) -> Result<T>
where
    T: Message + Default,
{
    let descriptor = message_descriptor(message_name)?;
    let payload = serde_json::to_string(value)
        .with_context(|| format!("failed to serialize {message_name} payload as JSON"))?;
    let mut deserializer = serde_json::Deserializer::from_str(&payload);
    let dynamic = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .with_context(|| format!("failed to decode {message_name} ProtoJSON"))?;

    dynamic
        .transcode_to::<T>()
        .with_context(|| format!("failed to transcode dynamic {message_name} message"))
}

pub fn protojson_message_to_value<T>(message_name: &str, message: &T) -> Result<Value>
where
    T: Message,
{
    let descriptor = message_descriptor(message_name)?;
    let bytes = message.encode_to_vec();
    let dynamic = DynamicMessage::decode(descriptor, &mut bytes.as_slice())
        .with_context(|| format!("failed to decode {message_name} protobuf bytes"))?;

    serde_json::to_value(dynamic)
        .with_context(|| format!("failed to serialize {message_name} as ProtoJSON"))
}

pub fn parse_normalized_raid_sim_request(payload: &Value) -> Result<RaidSimRequest> {
    parse_protojson_message("proto.RaidSimRequest", payload)
}

fn player_spec_label(spec: &Option<player::Spec>) -> Option<&'static str> {
    match spec {
        Some(player::Spec::BloodDeathKnight(_)) => Some("blood"),
        Some(player::Spec::FrostDeathKnight(_)) => Some("frost"),
        Some(player::Spec::UnholyDeathKnight(_)) => Some("unholy"),
        Some(player::Spec::BalanceDruid(_)) => Some("balance"),
        Some(player::Spec::FeralDruid(_)) => Some("feral"),
        Some(player::Spec::GuardianDruid(_)) => Some("guardian"),
        Some(player::Spec::RestorationDruid(_)) => Some("restoration"),
        Some(player::Spec::BeastMasteryHunter(_)) => Some("beastmastery"),
        Some(player::Spec::MarksmanshipHunter(_)) => Some("marksmanship"),
        Some(player::Spec::SurvivalHunter(_)) => Some("survival"),
        Some(player::Spec::ArcaneMage(_)) => Some("arcane"),
        Some(player::Spec::FireMage(_)) => Some("fire"),
        Some(player::Spec::FrostMage(_)) => Some("frost"),
        Some(player::Spec::BrewmasterMonk(_)) => Some("brewmaster"),
        Some(player::Spec::MistweaverMonk(_)) => Some("mistweaver"),
        Some(player::Spec::WindwalkerMonk(_)) => Some("windwalker"),
        Some(player::Spec::HolyPaladin(_)) => Some("holy"),
        Some(player::Spec::ProtectionPaladin(_)) => Some("protection"),
        Some(player::Spec::RetributionPaladin(_)) => Some("retribution"),
        Some(player::Spec::DisciplinePriest(_)) => Some("discipline"),
        Some(player::Spec::HolyPriest(_)) => Some("holy"),
        Some(player::Spec::ShadowPriest(_)) => Some("shadow"),
        Some(player::Spec::AssassinationRogue(_)) => Some("assassination"),
        Some(player::Spec::CombatRogue(_)) => Some("combat"),
        Some(player::Spec::SubtletyRogue(_)) => Some("subtlety"),
        Some(player::Spec::ElementalShaman(_)) => Some("elemental"),
        Some(player::Spec::EnhancementShaman(_)) => Some("enhancement"),
        Some(player::Spec::RestorationShaman(_)) => Some("restoration"),
        Some(player::Spec::AfflictionWarlock(_)) => Some("affliction"),
        Some(player::Spec::DemonologyWarlock(_)) => Some("demonology"),
        Some(player::Spec::DestructionWarlock(_)) => Some("destruction"),
        Some(player::Spec::ArmsWarrior(_)) => Some("arms"),
        Some(player::Spec::FuryWarrior(_)) => Some("fury"),
        Some(player::Spec::ProtectionWarrior(_)) => Some("protection"),
        None => None,
    }
}

fn player_class(player: &Player) -> Result<Class> {
    let class = Class::try_from(player.class)
        .map_err(|_| anyhow!("player has unknown class value {}", player.class))?;

    if class == Class::Unknown {
        bail!("IndividualSimSettings.player.class must be set");
    }

    Ok(class)
}

fn player_class_label(class: Class) -> String {
    class
        .as_str_name()
        .trim_start_matches("Class")
        .to_ascii_lowercase()
}

fn player_spec_class(spec: &Option<player::Spec>) -> Option<Class> {
    match spec {
        Some(player::Spec::BloodDeathKnight(_))
        | Some(player::Spec::FrostDeathKnight(_))
        | Some(player::Spec::UnholyDeathKnight(_)) => Some(Class::DeathKnight),
        Some(player::Spec::BalanceDruid(_))
        | Some(player::Spec::FeralDruid(_))
        | Some(player::Spec::GuardianDruid(_))
        | Some(player::Spec::RestorationDruid(_)) => Some(Class::Druid),
        Some(player::Spec::BeastMasteryHunter(_))
        | Some(player::Spec::MarksmanshipHunter(_))
        | Some(player::Spec::SurvivalHunter(_)) => Some(Class::Hunter),
        Some(player::Spec::ArcaneMage(_))
        | Some(player::Spec::FireMage(_))
        | Some(player::Spec::FrostMage(_)) => Some(Class::Mage),
        Some(player::Spec::BrewmasterMonk(_))
        | Some(player::Spec::MistweaverMonk(_))
        | Some(player::Spec::WindwalkerMonk(_)) => Some(Class::Monk),
        Some(player::Spec::HolyPaladin(_))
        | Some(player::Spec::ProtectionPaladin(_))
        | Some(player::Spec::RetributionPaladin(_)) => Some(Class::Paladin),
        Some(player::Spec::DisciplinePriest(_))
        | Some(player::Spec::HolyPriest(_))
        | Some(player::Spec::ShadowPriest(_)) => Some(Class::Priest),
        Some(player::Spec::AssassinationRogue(_))
        | Some(player::Spec::CombatRogue(_))
        | Some(player::Spec::SubtletyRogue(_)) => Some(Class::Rogue),
        Some(player::Spec::ElementalShaman(_))
        | Some(player::Spec::EnhancementShaman(_))
        | Some(player::Spec::RestorationShaman(_)) => Some(Class::Shaman),
        Some(player::Spec::AfflictionWarlock(_))
        | Some(player::Spec::DemonologyWarlock(_))
        | Some(player::Spec::DestructionWarlock(_)) => Some(Class::Warlock),
        Some(player::Spec::ArmsWarrior(_))
        | Some(player::Spec::FuryWarrior(_))
        | Some(player::Spec::ProtectionWarrior(_)) => Some(Class::Warrior),
        None => None,
    }
}

fn validate_tank_reference(tank: &UnitReference) -> Result<()> {
    let reference_type = unit_reference::Type::try_from(tank.r#type).map_err(|_| {
        anyhow!(
            "tank assignment has unknown unit reference type {}",
            tank.r#type
        )
    })?;

    if reference_type != unit_reference::Type::Player || tank.index != 0 {
        bail!(
            "individual UI exports may only assign the imported player as a tank (Player index 0)"
        );
    }

    Ok(())
}

fn debug_simulation_enabled() -> bool {
    std::env::var("WOWSIMS_SIM_DEBUG")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn generated_seed(run_id: Uuid) -> i64 {
    ((run_id.as_u128() & 0xFFFF_FFFF) as i64).max(1)
}

fn empty_party() -> Party {
    Party {
        players: (0..PARTY_SIZE).map(|_| Player::default()).collect(),
        buffs: Some(PartyBuffs::default()),
    }
}

fn sanitize_blacksmith_sockets(player: &mut Player) {
    if [player.profession1, player.profession2].contains(&(Profession::Blacksmithing as i32)) {
        return;
    }

    let Some(equipment) = player.equipment.as_mut() else {
        return;
    };

    // The UI clears only these profession-granted final socket positions before simming.
    for slot in [ItemSlot::Wrist, ItemSlot::Hands] {
        let Some(item) = equipment.items.get_mut(slot as usize) else {
            continue;
        };

        if item.id != 0
            && let Some(gem) = item.gems.last_mut()
        {
            *gem = 0;
        }
    }
}

impl ImportedIndividualSim {
    pub fn from_json(payload: &Value) -> Result<Self> {
        let settings: IndividualSimSettings =
            parse_protojson_message("proto.IndividualSimSettings", payload)?;

        if settings.api_version <= 0 {
            bail!("payload is not a complete WoWSims individual UI export: apiVersion is missing");
        }

        if settings.api_version > INDIVIDUAL_SIM_SETTINGS_API_VERSION {
            bail!(
                "WoWSims individual UI export apiVersion {} is newer than supported version {} (upstream {}). Update the bot's wowsims submodule before importing it.",
                settings.api_version,
                INDIVIDUAL_SIM_SETTINGS_API_VERSION,
                MOP_UPSTREAM_REVISION,
            );
        }

        let player = settings.player.as_ref().ok_or_else(|| {
            anyhow!("WoWSims individual UI export is missing player configuration")
        })?;
        let player_class = player_class(player)?;
        let spec = player_spec_label(&player.spec)
            .ok_or_else(|| anyhow!("WoWSims individual UI export is missing player spec"))?
            .to_string();
        let spec_class = player_spec_class(&player.spec)
            .ok_or_else(|| anyhow!("WoWSims individual UI export is missing player spec"))?;
        if player_class != spec_class {
            bail!(
                "WoWSims individual UI export player.class ({}) does not match its {} spec",
                player_class.as_str_name(),
                spec
            );
        }
        let class = player_class_label(player_class);

        let encounter = settings
            .encounter
            .as_ref()
            .ok_or_else(|| anyhow!("WoWSims individual UI export is missing encounter settings"))?;
        if encounter.targets.is_empty() {
            bail!("WoWSims individual UI export encounter must include at least one target");
        }

        let sim_settings = settings.settings.as_ref().ok_or_else(|| {
            anyhow!("WoWSims individual UI export is missing simulation settings")
        })?;
        if !(1..=MAX_ITERATIONS).contains(&sim_settings.iterations) {
            bail!("WoWSims individual UI export iterations must be between 1 and {MAX_ITERATIONS}");
        }
        if sim_settings.fixed_rng_seed < 0 {
            bail!("WoWSims individual UI export fixedRngSeed cannot be negative");
        }

        if settings.target_dummies < 0 {
            bail!("WoWSims individual UI export targetDummies cannot be negative");
        }

        for tank in &settings.tanks {
            validate_tank_reference(tank)?;
        }

        Ok(Self {
            settings,
            class,
            spec,
        })
    }

    pub fn normalize(&self, run_id: Uuid) -> Result<NormalizedIndividualSim> {
        let mut player = self
            .settings
            .player
            .as_ref()
            .ok_or_else(|| anyhow!("validated individual UI export lost player configuration"))?
            .clone();
        sanitize_blacksmith_sockets(&mut player);
        let encounter = self
            .settings
            .encounter
            .as_ref()
            .ok_or_else(|| anyhow!("validated individual UI export lost encounter settings"))?
            .clone();
        let sim_settings =
            self.settings.settings.as_ref().ok_or_else(|| {
                anyhow!("validated individual UI export lost simulation settings")
            })?;

        let effective_random_seed = if sim_settings.fixed_rng_seed == 0 {
            generated_seed(run_id)
        } else {
            sim_settings.fixed_rng_seed
        };

        let mut parties: Vec<Party> = (0..INDIVIDUAL_RAID_PARTY_COUNT)
            .map(|_| empty_party())
            .collect();
        let first_party = parties
            .first_mut()
            .ok_or_else(|| anyhow!("failed to create individual UI raid party"))?;
        first_party.players[0] = player;
        first_party.buffs = Some(self.settings.party_buffs.clone().unwrap_or_default());

        let request = RaidSimRequest {
            request_id: run_id.to_string(),
            raid: Some(Raid {
                parties,
                num_active_parties: INDIVIDUAL_RAID_PARTY_COUNT as i32,
                buffs: Some(self.settings.raid_buffs.clone().unwrap_or_default()),
                debuffs: Some(self.settings.debuffs.clone().unwrap_or_default()),
                tanks: self.settings.tanks.clone(),
                stagger_stormstrikes: false,
                target_dummies: self.settings.target_dummies,
            }),
            encounter: Some(encounter),
            sim_options: Some(SimOptions {
                iterations: sim_settings.iterations,
                random_seed: effective_random_seed,
                debug: debug_simulation_enabled(),
                debug_first_iteration: true,
                is_test: false,
                save_all_values: false,
                interactive: false,
                use_labeled_rands: false,
            }),
            r#type: SimType::Individual as i32,
        };

        Ok(NormalizedIndividualSim {
            request,
            class: self.class.clone(),
            spec: self.spec.clone(),
            effective_random_seed,
            effective_iterations: sim_settings.iterations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mop_proto::mop::{EquipmentSpec, ItemSpec};
    use serde_json::json;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/individual-ui-export.json"))
            .expect("fixture must be valid JSON")
    }

    #[test]
    fn normalizes_complete_individual_ui_export_without_dropping_runner_fields() {
        let imported = ImportedIndividualSim::from_json(&fixture()).expect("fixture must import");
        let run_id = Uuid::parse_str("e1e96748-7a13-4f89-a9e5-b1f4ad20c4cf").unwrap();
        let normalized = imported.normalize(run_id).expect("fixture must normalize");
        let raid = normalized
            .request
            .raid
            .as_ref()
            .expect("raid must be present");
        let player = &raid.parties[0].players[0];
        let encounter = normalized
            .request
            .encounter
            .as_ref()
            .expect("encounter must be present");
        let sim_options = normalized
            .request
            .sim_options
            .as_ref()
            .expect("sim options must be present");

        assert_eq!(normalized.class, "warrior");
        assert_eq!(normalized.spec, "arms");
        assert_eq!(normalized.request.r#type, SimType::Individual as i32);
        assert_eq!(normalized.request.request_id, run_id.to_string());
        assert_eq!(raid.parties.len(), INDIVIDUAL_RAID_PARTY_COUNT);
        assert!(
            raid.parties
                .iter()
                .all(|party| party.players.len() == PARTY_SIZE)
        );
        assert_eq!(raid.num_active_parties, INDIVIDUAL_RAID_PARTY_COUNT as i32);
        assert!(raid.buffs.as_ref().unwrap().bloodlust);
        assert!(raid.debuffs.as_ref().unwrap().physical_vulnerability);
        assert_eq!(raid.target_dummies, 3);
        assert_eq!(raid.tanks.len(), 1);
        assert_eq!(player.name, "Parity Warrior");
        assert_eq!(player.race, 11);
        assert_eq!(player.profession1, 2);
        assert_eq!(player.profession2, 4);
        assert_eq!(player.reaction_time_ms, 150);
        assert_eq!(player.channel_clip_delay_ms, 75);
        assert!(player.in_front_of_target);
        assert_eq!(player.distance_from_target, 8.5);
        assert_eq!(player.rotation.as_ref().unwrap().priority_list.len(), 1);
        assert_eq!(player.equipment.as_ref().unwrap().items[0].id, 87123);
        assert_eq!(encounter.duration, 360.0);
        assert_eq!(encounter.duration_variation, 15.0);
        assert_eq!(encounter.targets[0].name, "Parity Target");
        assert_eq!(sim_options.iterations, 25_000);
        assert_eq!(sim_options.random_seed, 712_367);
        assert!(sim_options.debug_first_iteration);
        assert!(!sim_options.debug);
    }

    #[test]
    fn serializes_and_reloads_canonical_request() {
        let imported = ImportedIndividualSim::from_json(&fixture()).expect("fixture must import");
        let normalized = imported
            .normalize(Uuid::nil())
            .expect("fixture must normalize");
        let value = protojson_message_to_value("proto.RaidSimRequest", &normalized.request)
            .expect("request must serialize");
        let reparsed = parse_normalized_raid_sim_request(&value).expect("request must deserialize");

        assert_eq!(reparsed, normalized.request);
    }

    #[test]
    fn rejects_future_ui_export_version() {
        let mut payload = fixture();
        payload["apiVersion"] = json!(INDIVIDUAL_SIM_SETTINGS_API_VERSION + 1);

        let error = match ImportedIndividualSim::from_json(&payload) {
            Ok(_) => panic!("future UI export must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("newer than supported version"));
    }

    #[test]
    fn rejects_missing_simulation_settings() {
        let mut payload = fixture();
        payload.as_object_mut().unwrap().remove("settings");

        let error = match ImportedIndividualSim::from_json(&payload) {
            Ok(_) => panic!("export without simulation settings must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("missing simulation settings"));
    }

    #[test]
    fn rejects_tank_references_to_players_not_present_in_individual_mode() {
        let mut payload = fixture();
        payload["tanks"] = json!([{"type": "Player", "index": 1}]);

        let error = match ImportedIndividualSim::from_json(&payload) {
            Ok(_) => panic!("invalid tank assignment must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("Player index 0"));
    }

    #[test]
    fn rejects_player_class_and_spec_mismatches() {
        let mut payload = fixture();
        payload["player"]["class"] = json!("ClassMage");

        let error = match ImportedIndividualSim::from_json(&payload) {
            Ok(_) => panic!("mismatched class and spec must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn generates_a_reproducible_seed_when_the_ui_has_no_fixed_seed() {
        let mut payload = fixture();
        payload["settings"]["fixedRngSeed"] = json!("0");
        let run_id = Uuid::parse_str("e1e96748-7a13-4f89-a9e5-b1f4ad20c4cf").unwrap();

        let normalized = ImportedIndividualSim::from_json(&payload)
            .unwrap()
            .normalize(run_id)
            .unwrap();

        assert_eq!(normalized.effective_random_seed, generated_seed(run_id));
    }

    #[test]
    fn removes_blacksmith_socket_gems_for_players_without_blacksmithing() {
        let mut items = vec![ItemSpec::default(); ItemSlot::Hands as usize + 1];
        items[ItemSlot::Wrist as usize] = ItemSpec {
            id: 1,
            gems: vec![101, 102],
            ..Default::default()
        };
        items[ItemSlot::Hands as usize] = ItemSpec {
            id: 2,
            gems: vec![201, 202],
            ..Default::default()
        };
        let mut player = Player {
            profession1: Profession::Engineering as i32,
            profession2: Profession::Jewelcrafting as i32,
            equipment: Some(EquipmentSpec { items }),
            ..Default::default()
        };

        sanitize_blacksmith_sockets(&mut player);

        let equipment = player.equipment.unwrap();
        assert_eq!(equipment.items[ItemSlot::Wrist as usize].gems, vec![101, 0]);
        assert_eq!(equipment.items[ItemSlot::Hands as usize].gems, vec![201, 0]);
    }

    #[test]
    fn retains_blacksmith_socket_gems_for_blacksmiths() {
        let mut items = vec![ItemSpec::default(); ItemSlot::Hands as usize + 1];
        items[ItemSlot::Wrist as usize] = ItemSpec {
            id: 1,
            gems: vec![101, 102],
            ..Default::default()
        };
        let mut player = Player {
            profession1: Profession::Blacksmithing as i32,
            equipment: Some(EquipmentSpec { items }),
            ..Default::default()
        };

        sanitize_blacksmith_sockets(&mut player);

        assert_eq!(
            player.equipment.unwrap().items[ItemSlot::Wrist as usize].gems,
            vec![101, 102]
        );
    }
}
