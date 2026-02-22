use uuid::Uuid;

use crate::mop_proto::mop::{
    Debuffs, Encounter, MobType, Party, PartyBuffs, Player, Raid, RaidBuffs, SimOptions,
    SpellSchool, Target,
};

const ENCOUNTER_API_VERSION: i32 = 2;
const STAT_ATTACK_POWER_INDEX: usize = 12;
const STAT_ARMOR_INDEX: usize = 17;
const STAT_HEALTH_INDEX: usize = 19;

pub(crate) fn default_raid_target_stats() -> Vec<f64> {
    let mut stats = vec![0.0; 22];
    stats[STAT_ATTACK_POWER_INDEX] = 650.0;
    stats[STAT_ARMOR_INDEX] = 24_835.0;
    stats[STAT_HEALTH_INDEX] = 120_016_403.0;
    stats
}

pub(crate) fn default_mop_encounter() -> Encounter {
    Encounter {
        api_version: ENCOUNTER_API_VERSION,
        duration: 300.0,
        duration_variation: 60.0,
        execute_proportion_20: 0.2,
        execute_proportion_25: 0.25,
        execute_proportion_35: 0.35,
        execute_proportion_45: 0.45,
        execute_proportion_90: 0.9,
        use_health: false,
        targets: vec![Target {
            id: 31_146,
            name: "Raid Target".to_string(),
            level: 93,
            mob_type: MobType::Mechanical as i32,
            stats: default_raid_target_stats(),
            min_base_damage: 550_000.0,
            damage_spread: 0.4,
            swing_speed: 2.0,
            dual_wield: false,
            dual_wield_penalty: false,
            parry_haste: false,
            suppress_dodge: false,
            spell_school: SpellSchool::Physical as i32,
            tank_index: 0,
            second_tank_index: 0,
            disabled_at_start: false,
            target_inputs: Vec::new(),
        }],
    }
}

pub(crate) fn default_mop_raid() -> Raid {
    let parties = (0..5)
        .map(|_| Party {
            players: (0..5).map(|_| Player::default()).collect(),
            buffs: Some(PartyBuffs::default()),
        })
        .collect();

    Raid {
        parties,
        num_active_parties: 5,
        buffs: Some(RaidBuffs {
            unholy_aura: true,
            arcane_brilliance: true,
            mind_quickening: true,
            leader_of_the_pack: true,
            blessing_of_might: true,
            blessing_of_kings: true,
            bloodlust: true,
            skull_banner_count: 2,
            stormlash_totem_count: 4,
            ..Default::default()
        }),
        debuffs: Some(Debuffs {
            curse_of_elements: true,
            ..Default::default()
        }),
        tanks: Vec::new(),
        stagger_stormstrikes: false,
        target_dummies: 0,
    }
}

pub(crate) fn default_mop_sim_options(run_id: Uuid) -> SimOptions {
    let random_seed = ((run_id.as_u128() & 0xFFFF_FFFF) as i64).max(1);
    let debug = std::env::var("WOWSIMS_SIM_DEBUG")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        })
        .unwrap_or(false);

    SimOptions {
        iterations: 12_500,
        random_seed,
        debug,
        debug_first_iteration: true,
        is_test: false,
        save_all_values: false,
        interactive: false,
        use_labeled_rands: false,
    }
}