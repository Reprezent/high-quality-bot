use anyhow::{Context, Result, anyhow};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::db;
use crate::mop_proto::mop::{
    AfflictionWarlock, AplRotation, ArcaneMage, ArmsWarrior, AssassinationRogue, BalanceDruid,
    BeastMasteryHunter, BloodDeathKnight, BrewmasterMonk, Class, CombatRogue, DeathKnightOptions,
    DemonologyWarlock, DestructionWarlock, DisciplinePriest, DruidOptions, ElementalShaman,
    EnhancementShaman, EquipmentSpec, FeralDruid, FireMage, FrostDeathKnight, FrostMage,
    FuryWarrior, Glyphs, GuardianDruid, HolyPaladin, HolyPriest, HunterOptions, ItemLevelState,
    ItemSpec, ItemSwap, MageArmor, MageOptions, MarksmanshipHunter, MistweaverMonk, MonkOptions,
    PaladinOptions, Player, PriestOptions, Profession, ProtectionPaladin, ProtectionWarrior, Race,
    RestorationDruid, RestorationShaman, RetributionPaladin, RogueOptions, ShadowPriest,
    ShamanOptions, SubtletyRogue, SurvivalHunter, UnholyDeathKnight, WarlockOptions,
    WarriorOptions, WindwalkerMonk, affliction_warlock, arcane_mage, arms_warrior,
    assassination_rogue, balance_druid, beast_mastery_hunter, blood_death_knight, brewmaster_monk,
    combat_rogue, demonology_warlock, destruction_warlock, discipline_priest, elemental_shaman,
    enhancement_shaman, feral_druid, fire_mage, frost_death_knight, frost_mage, fury_warrior,
    guardian_druid, holy_paladin, holy_priest, marksmanship_hunter, mistweaver_monk, player,
    protection_paladin, protection_warrior, restoration_druid, restoration_shaman,
    retribution_paladin, shadow_priest, subtlety_rogue, survival_hunter, unholy_death_knight,
    windwalker_monk,
};

const PLAYER_API_VERSION: i32 = 54;
const EQUIPMENT_SLOT_COUNT: usize = 16;

fn parse_i32(value: Option<&Value>) -> Option<i32> {
    value
        .and_then(|v| v.as_i64())
        .and_then(|value| i32::try_from(value).ok())
}

fn parse_item_level_state(value: Option<&Value>) -> Option<i32> {
    value.and_then(|value| {
        parse_i32(Some(value)).or_else(|| {
            value
                .as_str()
                .and_then(ItemLevelState::from_str_name)
                .map(|state| state as i32)
        })
    })
}

fn wowsims_db_json_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(value) = std::env::var("WOWSIMS_DB_JSON") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }

    paths.push(PathBuf::from("vendor/wowsims-mop/assets/database/db.json"));
    paths
}

fn load_item_scaling_states() -> Option<HashMap<i32, HashSet<i32>>> {
    let db_path = wowsims_db_json_candidates()
        .into_iter()
        .find(|path| path.is_file())?;

    let db_json = match fs::read_to_string(&db_path) {
        Ok(db_json) => db_json,
        Err(error) => {
            tracing::warn!(
                db_path = %db_path.display(),
                error = ?error,
                "failed to read WoWSims item database; item upgrades will not be normalized"
            );
            return None;
        }
    };

    let db: Value = match serde_json::from_str(&db_json) {
        Ok(db) => db,
        Err(error) => {
            tracing::warn!(
                db_path = %db_path.display(),
                error = ?error,
                "failed to parse WoWSims item database; item upgrades will not be normalized"
            );
            return None;
        }
    };

    let items = db.get("items").and_then(|items| items.as_array())?;
    let mut states_by_item = HashMap::new();

    for item in items {
        let Some(item_id) = parse_i32(item.get("id")) else {
            continue;
        };
        let Some(scaling_options) = item
            .get("scalingOptions")
            .and_then(|value| value.as_object())
        else {
            continue;
        };

        let states = scaling_options
            .keys()
            .filter_map(|state| state.parse::<i32>().ok())
            .collect::<HashSet<i32>>();

        states_by_item.insert(item_id, states);
    }

    Some(states_by_item)
}

fn item_scaling_states() -> Option<&'static HashMap<i32, HashSet<i32>>> {
    static ITEM_SCALING_STATES: OnceLock<Option<HashMap<i32, HashSet<i32>>>> = OnceLock::new();

    ITEM_SCALING_STATES
        .get_or_init(load_item_scaling_states)
        .as_ref()
}

fn normalize_item_upgrade_step(item_id: i32, upgrade_step: i32) -> i32 {
    let Some(states_by_item) = item_scaling_states() else {
        return upgrade_step;
    };
    let Some(states) = states_by_item.get(&item_id) else {
        return upgrade_step;
    };

    if states.contains(&upgrade_step) {
        return upgrade_step;
    }

    let fallback = states
        .iter()
        .copied()
        .filter(|state| *state >= ItemLevelState::Base as i32 && *state <= upgrade_step)
        .max()
        .or_else(|| {
            states
                .contains(&(ItemLevelState::Base as i32))
                .then_some(ItemLevelState::Base as i32)
        })
        .unwrap_or(ItemLevelState::Base as i32);

    tracing::warn!(
        item_id,
        requested_upgrade_step = upgrade_step,
        fallback_upgrade_step = fallback,
        "item upgrade step is not supported by WoWSims database; using supported step"
    );

    fallback
}

fn normalize_item_spec(item: &mut ItemSpec) {
    if item.id == 0 {
        return;
    }

    item.upgrade_step = normalize_item_upgrade_step(item.id, item.upgrade_step);
}

fn normalize_equipment_spec(mut equipment: EquipmentSpec) -> EquipmentSpec {
    for item in &mut equipment.items {
        normalize_item_spec(item);
    }

    equipment
}

fn normalize_item_swap(mut item_swap: ItemSwap) -> ItemSwap {
    for item in &mut item_swap.items {
        normalize_item_spec(item);
    }

    item_swap
}

fn parse_item_spec(value: &Value) -> Option<ItemSpec> {
    let item = value.as_object()?;
    let id = parse_i32(item.get("id"))?;

    let gems = item
        .get("gems")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| parse_i32(Some(entry)))
                .collect::<Vec<i32>>()
        })
        .unwrap_or_default();

    let mut item_spec = ItemSpec {
        id,
        random_suffix: parse_i32(
            item.get("random_suffix")
                .or_else(|| item.get("randomSuffix")),
        )
        .unwrap_or(0),
        enchant: parse_i32(item.get("enchant")).unwrap_or(0),
        gems,
        reforging: parse_i32(item.get("reforging")).unwrap_or(0),
        upgrade_step: parse_item_level_state(
            item.get("upgrade_step").or_else(|| item.get("upgradeStep")),
        )
        .unwrap_or(ItemLevelState::Base as i32),
        challenge_mode: item
            .get("challenge_mode")
            .or_else(|| item.get("challengeMode"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        tinker: parse_i32(item.get("tinker")).unwrap_or(0),
    };

    normalize_item_spec(&mut item_spec);
    Some(item_spec)
}

fn parse_equipment_items(entries: &[Value]) -> Vec<ItemSpec> {
    let mut items: Vec<ItemSpec> = entries
        .iter()
        .take(EQUIPMENT_SLOT_COUNT)
        .map(|entry| parse_item_spec(entry).unwrap_or_default())
        .collect();
    items.resize(EQUIPMENT_SLOT_COUNT, ItemSpec::default());
    items
}

fn parse_equipment_spec(gear_payload: &Value) -> EquipmentSpec {
    let nested_player_items = gear_payload
        .get("player")
        .and_then(|player| player.get("equipment"))
        .and_then(|equipment| equipment.get("items"))
        .and_then(|items| items.as_array());

    let nested_player_gear_items = gear_payload
        .get("player")
        .and_then(|player| player.get("gear"))
        .and_then(|gear| gear.get("items"))
        .and_then(|items| items.as_array());

    let nested_gear_items = gear_payload
        .get("gear")
        .and_then(|gear| gear.get("items"))
        .and_then(|items| items.as_array());

    let nested_equipment_items = gear_payload
        .get("equipment")
        .and_then(|equipment| equipment.get("items"))
        .and_then(|items| items.as_array());

    let nested_settings_player_items = gear_payload
        .get("settings")
        .and_then(|settings| settings.get("player"))
        .and_then(|player| player.get("equipment"))
        .and_then(|equipment| equipment.get("items"))
        .and_then(|items| items.as_array());

    let nested_raid_player_items = gear_payload
        .get("raid")
        .and_then(|raid| raid.get("parties"))
        .and_then(|parties| parties.as_array())
        .and_then(|parties| parties.first())
        .and_then(|party| party.get("players"))
        .and_then(|players| players.as_array())
        .and_then(|players| players.first())
        .and_then(|player| player.get("equipment"))
        .and_then(|equipment| equipment.get("items"))
        .and_then(|items| items.as_array());

    let nested_raid_settings_player_items = gear_payload
        .get("raid")
        .and_then(|raid| raid.get("settings"))
        .and_then(|settings| settings.get("raid"))
        .and_then(|raid_settings| raid_settings.get("parties"))
        .and_then(|parties| parties.as_array())
        .and_then(|parties| parties.first())
        .and_then(|party| party.get("players"))
        .and_then(|players| players.as_array())
        .and_then(|players| players.first())
        .and_then(|player| player.get("equipment"))
        .and_then(|equipment| equipment.get("items"))
        .and_then(|items| items.as_array());

    let items = if let Some(entries) = nested_player_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_player_gear_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_gear_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_equipment_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_settings_player_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_raid_player_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = nested_raid_settings_player_items {
        parse_equipment_items(entries)
    } else if let Some(entries) = gear_payload.get("items").and_then(|value| value.as_array()) {
        parse_equipment_items(entries)
    } else if let Some(payload_object) = gear_payload.as_object() {
        payload_object
            .values()
            .filter_map(parse_item_spec)
            .collect()
    } else {
        Vec::new()
    };

    normalize_equipment_spec(EquipmentSpec { items })
}

fn string_field<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload
        .get(field)
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .get("player")
                .and_then(|player| player.get(field))
                .and_then(|value| value.as_str())
        })
}

fn normalize_identifier(value: &str, prefix: &str) -> String {
    value
        .trim()
        .trim_start_matches(prefix)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn parse_race(payload: &Value) -> Option<i32> {
    let value = string_field(payload, "race")?;
    let race = match normalize_identifier(value, "Race").as_str() {
        "bloodelf" => Race::BloodElf,
        "draenei" => Race::Draenei,
        "dwarf" => Race::Dwarf,
        "gnome" => Race::Gnome,
        "human" => Race::Human,
        "nightelf" => Race::NightElf,
        "orc" => Race::Orc,
        "tauren" => Race::Tauren,
        "troll" => Race::Troll,
        "undead" => Race::Undead,
        "worgen" => Race::Worgen,
        "goblin" => Race::Goblin,
        "alliancepandaren" => Race::AlliancePandaren,
        "hordepandaren" => Race::HordePandaren,
        _ => return None,
    };

    Some(race as i32)
}

fn parse_profession(value: &Value) -> Option<i32> {
    parse_i32(Some(value)).or_else(|| {
        let value = value.as_str()?;
        let profession = match normalize_identifier(value, "Profession").as_str() {
            "alchemy" => Profession::Alchemy,
            "blacksmithing" => Profession::Blacksmithing,
            "enchanting" => Profession::Enchanting,
            "engineering" => Profession::Engineering,
            "herbalism" => Profession::Herbalism,
            "inscription" => Profession::Inscription,
            "jewelcrafting" => Profession::Jewelcrafting,
            "leatherworking" => Profession::Leatherworking,
            "mining" => Profession::Mining,
            "skinning" => Profession::Skinning,
            "tailoring" => Profession::Tailoring,
            "archeology" => Profession::Archeology,
            _ => return None,
        };
        Some(profession as i32)
    })
}

fn parse_professions(payload: &Value) -> (i32, i32) {
    let direct_profession = |field| {
        payload
            .get(field)
            .or_else(|| payload.get("player").and_then(|player| player.get(field)))
            .and_then(parse_profession)
    };

    let listed_professions = payload
        .get("professions")
        .and_then(|value| value.as_array());
    let listed_profession = |index: usize| {
        let profession = listed_professions?.get(index)?;
        parse_profession(profession.get("name").unwrap_or(profession))
    };

    (
        direct_profession("profession1")
            .or_else(|| listed_profession(0))
            .unwrap_or(Profession::Unknown as i32),
        direct_profession("profession2")
            .or_else(|| listed_profession(1))
            .unwrap_or(Profession::Unknown as i32),
    )
}

fn parse_talents_string(payload: &Value) -> String {
    payload
        .get("talents")
        .and_then(|value| value.as_str())
        .or_else(|| {
            payload
                .get("player")
                .and_then(|player| player.get("talentsString"))
                .and_then(|value| value.as_str())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "000000".to_string())
}

fn parse_glyph_id(value: &Value) -> Option<i32> {
    parse_i32(Some(value)).or_else(|| {
        value.as_object().and_then(|glyph| {
            parse_i32(glyph.get("spellID"))
                .or_else(|| parse_i32(glyph.get("spellId")))
                .or_else(|| parse_i32(glyph.get("id")))
        })
    })
}

fn parse_glyphs(payload: &Value) -> Glyphs {
    let glyphs = payload.get("glyphs").and_then(|value| value.as_object());

    let major_ids = glyphs
        .and_then(|glyphs| glyphs.get("major"))
        .and_then(|value| value.as_array())
        .map(|glyph_list| {
            glyph_list
                .iter()
                .filter_map(parse_glyph_id)
                .collect::<Vec<i32>>()
        })
        .unwrap_or_default();

    let minor_ids = glyphs
        .and_then(|glyphs| glyphs.get("minor"))
        .and_then(|value| value.as_array())
        .map(|glyph_list| {
            glyph_list
                .iter()
                .filter_map(parse_glyph_id)
                .collect::<Vec<i32>>()
        })
        .unwrap_or_default();

    Glyphs {
        major1: *major_ids.first().unwrap_or(&0),
        major2: *major_ids.get(1).unwrap_or(&0),
        major3: *major_ids.get(2).unwrap_or(&0),
        minor1: *minor_ids.first().unwrap_or(&0),
        minor2: *minor_ids.get(1).unwrap_or(&0),
        minor3: *minor_ids.get(2).unwrap_or(&0),
    }
}

fn class_ui_folder(class: &str) -> Option<&'static str> {
    match class {
        "deathknight" | "death_knight" | "dk" => Some("death_knight"),
        "druid" => Some("druid"),
        "hunter" => Some("hunter"),
        "mage" => Some("mage"),
        "monk" => Some("monk"),
        "paladin" => Some("paladin"),
        "priest" => Some("priest"),
        "rogue" => Some("rogue"),
        "shaman" => Some("shaman"),
        "warlock" => Some("warlock"),
        "warrior" => Some("warrior"),
        _ => None,
    }
}

fn spec_ui_folder(spec: &str) -> &str {
    match spec {
        "beastmastery" => "beast_mastery",
        "marksmanship" => "marksmanship",
        "survival" => "survival",
        "brewmaster" => "brewmaster",
        "mistweaver" => "mistweaver",
        "windwalker" => "windwalker",
        "deathknight" => "death_knight",
        other => other,
    }
}

fn ui_root_dir() -> PathBuf {
    if let Ok(value) = std::env::var("WOWSIMS_UI_DIR") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    PathBuf::from("vendor/wowsims-mop/ui")
}

fn select_vendor_apl_file(apl_dir: &Path, spec_folder: &str) -> Result<PathBuf> {
    let mut apl_files: Vec<PathBuf> = fs::read_dir(apl_dir)
        .with_context(|| format!("failed to read APL directory {}", apl_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(".apl.json"))
                .unwrap_or(false)
        })
        .collect();

    apl_files.sort();

    if apl_files.is_empty() {
        return Err(anyhow!("no .apl.json files found in {}", apl_dir.display()));
    }

    let default_file = "default.apl.json";
    let spec_file = format!("{spec_folder}.apl.json");

    if let Some(path) = apl_files.iter().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == default_file)
            .unwrap_or(false)
    }) {
        return Ok(path.clone());
    }

    if let Some(path) = apl_files.iter().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == spec_file)
            .unwrap_or(false)
    }) {
        return Ok(path.clone());
    }

    Ok(apl_files[0].clone())
}

fn parse_protojson_message<T>(message_name: &str, protojson: &str) -> Result<T>
where
    T: Message + Default,
{
    let pool = DescriptorPool::decode(crate::mop_proto::mop::DESCRIPTOR_SET_BYTES)
        .context("failed to decode protobuf descriptor set")?;

    let descriptor = pool
        .get_message_by_name(message_name)
        .ok_or_else(|| anyhow!("{message_name} descriptor not found in descriptor set"))?;

    let mut deserializer = serde_json::Deserializer::from_str(protojson);
    let dynamic = DynamicMessage::deserialize(descriptor, &mut deserializer)
        .with_context(|| format!("failed to decode {message_name} protojson"))?;

    dynamic
        .transcode_to::<T>()
        .with_context(|| format!("failed to transcode dynamic {message_name} message"))
}

fn parse_protojson_value<T>(message_name: &str, value: &Value) -> Result<T>
where
    T: Message + Default,
{
    let protojson = serde_json::to_string(value)
        .with_context(|| format!("failed to serialize {message_name} payload as JSON"))?;

    parse_protojson_message(message_name, &protojson)
}

fn parse_apl_rotation_protojson(apl_protojson: &str) -> Result<AplRotation> {
    parse_protojson_message("proto.APLRotation", apl_protojson)
}

fn extract_rotation_from_payload(payload: &Value) -> Option<&Value> {
    payload
        .get("rotation")
        .or_else(|| {
            payload
                .get("player")
                .and_then(|player| player.get("rotation"))
        })
        .or_else(|| {
            payload
                .get("settings")
                .and_then(|settings| settings.get("player"))
                .and_then(|player| player.get("rotation"))
        })
}

fn load_rotation_from_payload(payload: &Value) -> Result<Option<AplRotation>> {
    let Some(rotation_value) = extract_rotation_from_payload(payload) else {
        return Ok(None);
    };

    let rotation_json = serde_json::to_string(rotation_value)
        .context("failed to serialize payload rotation as JSON")?;

    parse_apl_rotation_protojson(&rotation_json).map(Some)
}

fn extract_full_player_payloads(payload: &Value) -> Vec<&Value> {
    let mut candidates = Vec::new();

    if let Some(player) = payload.get("player") {
        candidates.push(player);
    }

    if let Some(player) = payload
        .get("settings")
        .and_then(|settings| settings.get("player"))
    {
        candidates.push(player);
    }

    if let Some(player) = payload
        .get("raid")
        .and_then(|raid| raid.get("parties"))
        .and_then(|parties| parties.as_array())
        .and_then(|parties| parties.first())
        .and_then(|party| party.get("players"))
        .and_then(|players| players.as_array())
        .and_then(|players| players.first())
    {
        candidates.push(player);
    }

    if payload.get("class").is_some() && payload.get("player").is_none() {
        candidates.push(payload);
    }

    candidates
}

fn load_full_player_from_payload(payload: &Value) -> Option<Player> {
    for player_payload in extract_full_player_payloads(payload) {
        match parse_protojson_value::<Player>("proto.Player", player_payload) {
            Ok(player) => return Some(player),
            Err(error) => {
                tracing::debug!(
                    error = ?error,
                    "failed to parse payload candidate as full WoWSims player"
                );
            }
        }
    }

    None
}

fn load_vendor_default_rotation(class: &str, spec: &str) -> Result<AplRotation> {
    let class_folder = class_ui_folder(class)
        .ok_or_else(|| anyhow!("unsupported class for vendor APL lookup: {class}"))?;
    let spec_folder = spec_ui_folder(spec);

    let apl_dir = ui_root_dir()
        .join(class_folder)
        .join(spec_folder)
        .join("apls");
    let apl_file = select_vendor_apl_file(&apl_dir, spec_folder)?;
    tracing::info!(
        class = %class,
        spec = %spec,
        apl_file = %apl_file.display(),
        "selected vendor default APL file"
    );
    let apl_json = fs::read_to_string(&apl_file)
        .with_context(|| format!("failed reading vendor APL file {}", apl_file.display()))?;

    parse_apl_rotation_protojson(&apl_json).with_context(|| {
        format!(
            "failed parsing vendor APL {} for class/spec {}/{}",
            apl_file.display(),
            class,
            spec
        )
    })
}

fn resolve_player_spec(class: &str, spec: &str) -> Result<(i32, i32, player::Spec)> {
    let resolved = match (class, spec) {
        ("deathknight" | "death_knight" | "dk", "blood") => (
            Class::DeathKnight as i32,
            Race::Human as i32,
            player::Spec::BloodDeathKnight(BloodDeathKnight::default()),
        ),
        ("deathknight" | "death_knight" | "dk", "frost") => (
            Class::DeathKnight as i32,
            Race::Human as i32,
            player::Spec::FrostDeathKnight(FrostDeathKnight::default()),
        ),
        ("deathknight" | "death_knight" | "dk", "unholy") => (
            Class::DeathKnight as i32,
            Race::Human as i32,
            player::Spec::UnholyDeathKnight(UnholyDeathKnight::default()),
        ),
        ("druid", "balance") => (
            Class::Druid as i32,
            Race::NightElf as i32,
            player::Spec::BalanceDruid(BalanceDruid::default()),
        ),
        ("druid", "feral") => (
            Class::Druid as i32,
            Race::NightElf as i32,
            player::Spec::FeralDruid(FeralDruid::default()),
        ),
        ("druid", "guardian") => (
            Class::Druid as i32,
            Race::NightElf as i32,
            player::Spec::GuardianDruid(GuardianDruid::default()),
        ),
        ("druid", "restoration" | "resto") => (
            Class::Druid as i32,
            Race::NightElf as i32,
            player::Spec::RestorationDruid(RestorationDruid::default()),
        ),
        ("hunter", "beastmastery" | "beast_mastery" | "bm") => (
            Class::Hunter as i32,
            Race::Human as i32,
            player::Spec::BeastMasteryHunter(BeastMasteryHunter::default()),
        ),
        ("hunter", "marksmanship" | "mm") => (
            Class::Hunter as i32,
            Race::Human as i32,
            player::Spec::MarksmanshipHunter(MarksmanshipHunter::default()),
        ),
        ("hunter", "survival" | "sv") => (
            Class::Hunter as i32,
            Race::Human as i32,
            player::Spec::SurvivalHunter(SurvivalHunter::default()),
        ),
        ("mage", "arcane") => (
            Class::Mage as i32,
            Race::Human as i32,
            player::Spec::ArcaneMage(ArcaneMage::default()),
        ),
        ("mage", "fire") => (
            Class::Mage as i32,
            Race::Human as i32,
            player::Spec::FireMage(FireMage::default()),
        ),
        ("mage", "frost") => (
            Class::Mage as i32,
            Race::Human as i32,
            player::Spec::FrostMage(FrostMage::default()),
        ),
        ("monk", "brewmaster" | "brew") => (
            Class::Monk as i32,
            Race::AlliancePandaren as i32,
            player::Spec::BrewmasterMonk(BrewmasterMonk::default()),
        ),
        ("monk", "mistweaver" | "mist") => (
            Class::Monk as i32,
            Race::AlliancePandaren as i32,
            player::Spec::MistweaverMonk(MistweaverMonk::default()),
        ),
        ("monk", "windwalker" | "ww") => (
            Class::Monk as i32,
            Race::AlliancePandaren as i32,
            player::Spec::WindwalkerMonk(WindwalkerMonk::default()),
        ),
        ("paladin", "holy") => (
            Class::Paladin as i32,
            Race::Human as i32,
            player::Spec::HolyPaladin(HolyPaladin::default()),
        ),
        ("paladin", "protection" | "prot") => (
            Class::Paladin as i32,
            Race::Human as i32,
            player::Spec::ProtectionPaladin(ProtectionPaladin::default()),
        ),
        ("paladin", "retribution" | "ret") => (
            Class::Paladin as i32,
            Race::Human as i32,
            player::Spec::RetributionPaladin(RetributionPaladin::default()),
        ),
        ("priest", "discipline" | "disc") => (
            Class::Priest as i32,
            Race::Human as i32,
            player::Spec::DisciplinePriest(DisciplinePriest::default()),
        ),
        ("priest", "holy") => (
            Class::Priest as i32,
            Race::Human as i32,
            player::Spec::HolyPriest(HolyPriest::default()),
        ),
        ("priest", "shadow") => (
            Class::Priest as i32,
            Race::Human as i32,
            player::Spec::ShadowPriest(ShadowPriest::default()),
        ),
        ("rogue", "assassination" | "assa" | "mut") => (
            Class::Rogue as i32,
            Race::Human as i32,
            player::Spec::AssassinationRogue(AssassinationRogue::default()),
        ),
        ("rogue", "combat") => (
            Class::Rogue as i32,
            Race::Human as i32,
            player::Spec::CombatRogue(CombatRogue::default()),
        ),
        ("rogue", "subtlety" | "sub") => (
            Class::Rogue as i32,
            Race::Human as i32,
            player::Spec::SubtletyRogue(SubtletyRogue::default()),
        ),
        ("shaman", "elemental" | "ele") => (
            Class::Shaman as i32,
            Race::Orc as i32,
            player::Spec::ElementalShaman(ElementalShaman::default()),
        ),
        ("shaman", "enhancement" | "enh") => (
            Class::Shaman as i32,
            Race::Orc as i32,
            player::Spec::EnhancementShaman(EnhancementShaman::default()),
        ),
        ("shaman", "restoration" | "resto") => (
            Class::Shaman as i32,
            Race::Orc as i32,
            player::Spec::RestorationShaman(RestorationShaman::default()),
        ),
        ("warlock", "affliction" | "aff") => (
            Class::Warlock as i32,
            Race::Human as i32,
            player::Spec::AfflictionWarlock(AfflictionWarlock::default()),
        ),
        ("warlock", "demonology" | "demo") => (
            Class::Warlock as i32,
            Race::Human as i32,
            player::Spec::DemonologyWarlock(DemonologyWarlock::default()),
        ),
        ("warlock", "destruction" | "destro") => (
            Class::Warlock as i32,
            Race::Human as i32,
            player::Spec::DestructionWarlock(DestructionWarlock::default()),
        ),
        ("warrior", "arms") => (
            Class::Warrior as i32,
            Race::Human as i32,
            player::Spec::ArmsWarrior(ArmsWarrior::default()),
        ),
        ("warrior", "fury") => (
            Class::Warrior as i32,
            Race::Human as i32,
            player::Spec::FuryWarrior(FuryWarrior::default()),
        ),
        ("warrior", "protection" | "prot") => (
            Class::Warrior as i32,
            Race::Human as i32,
            player::Spec::ProtectionWarrior(ProtectionWarrior::default()),
        ),
        _ => {
            return Err(anyhow!(
                "unsupported class/spec combination: {}/{}",
                class,
                spec
            ));
        }
    };

    Ok(resolved)
}

fn with_default_spec_options(spec: player::Spec) -> player::Spec {
    match spec {
        player::Spec::BloodDeathKnight(mut value) => {
            if value.options.is_none() {
                value.options = Some(blood_death_knight::Options {
                    class_options: Some(DeathKnightOptions::default()),
                });
            }
            player::Spec::BloodDeathKnight(value)
        }
        player::Spec::FrostDeathKnight(mut value) => {
            if value.options.is_none() {
                value.options = Some(frost_death_knight::Options {
                    class_options: Some(DeathKnightOptions::default()),
                });
            }
            player::Spec::FrostDeathKnight(value)
        }
        player::Spec::UnholyDeathKnight(mut value) => {
            if value.options.is_none() {
                value.options = Some(unholy_death_knight::Options {
                    class_options: Some(DeathKnightOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::UnholyDeathKnight(value)
        }
        player::Spec::BalanceDruid(mut value) => {
            if value.options.is_none() {
                value.options = Some(balance_druid::Options {
                    class_options: Some(DruidOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::BalanceDruid(value)
        }
        player::Spec::FeralDruid(mut value) => {
            if value.options.is_none() {
                value.options = Some(feral_druid::Options {
                    class_options: Some(DruidOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::FeralDruid(value)
        }
        player::Spec::GuardianDruid(mut value) => {
            if value.options.is_none() {
                value.options = Some(guardian_druid::Options {
                    class_options: Some(DruidOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::GuardianDruid(value)
        }
        player::Spec::RestorationDruid(mut value) => {
            if value.options.is_none() {
                value.options = Some(restoration_druid::Options {
                    class_options: Some(DruidOptions::default()),
                });
            }
            player::Spec::RestorationDruid(value)
        }
        player::Spec::BeastMasteryHunter(mut value) => {
            if value.options.is_none() {
                value.options = Some(beast_mastery_hunter::Options {
                    class_options: Some(HunterOptions::default()),
                });
            }
            player::Spec::BeastMasteryHunter(value)
        }
        player::Spec::MarksmanshipHunter(mut value) => {
            if value.options.is_none() {
                value.options = Some(marksmanship_hunter::Options {
                    class_options: Some(HunterOptions::default()),
                });
            }
            player::Spec::MarksmanshipHunter(value)
        }
        player::Spec::SurvivalHunter(mut value) => {
            if value.options.is_none() {
                value.options = Some(survival_hunter::Options {
                    class_options: Some(HunterOptions::default()),
                });
            }
            player::Spec::SurvivalHunter(value)
        }
        player::Spec::ArcaneMage(mut value) => {
            if value.options.is_none() {
                value.options = Some(arcane_mage::Options {
                    class_options: Some(MageOptions::default()),
                });
            }
            player::Spec::ArcaneMage(value)
        }
        player::Spec::FireMage(mut value) => {
            if value.options.is_none() {
                value.options = Some(fire_mage::Options {
                    class_options: Some(MageOptions::default()),
                });
            }
            player::Spec::FireMage(value)
        }
        player::Spec::FrostMage(mut value) => {
            if value.options.is_none() {
                value.options = Some(frost_mage::Options {
                    class_options: Some(MageOptions {
                        default_mage_armor: MageArmor::FrostArmor as i32,
                    }),
                    ..Default::default()
                });
            } else if let Some(options) = value.options.as_mut() {
                if options.class_options.is_none() {
                    options.class_options = Some(MageOptions {
                        default_mage_armor: MageArmor::FrostArmor as i32,
                    });
                } else if let Some(class_options) = options.class_options.as_mut()
                    && class_options.default_mage_armor == MageArmor::None as i32
                {
                    class_options.default_mage_armor = MageArmor::FrostArmor as i32;
                }
            }
            player::Spec::FrostMage(value)
        }
        player::Spec::BrewmasterMonk(mut value) => {
            if value.options.is_none() {
                value.options = Some(brewmaster_monk::Options {
                    class_options: Some(MonkOptions::default()),
                });
            }
            player::Spec::BrewmasterMonk(value)
        }
        player::Spec::MistweaverMonk(mut value) => {
            if value.options.is_none() {
                value.options = Some(mistweaver_monk::Options {
                    class_options: Some(MonkOptions::default()),
                });
            }
            player::Spec::MistweaverMonk(value)
        }
        player::Spec::WindwalkerMonk(mut value) => {
            if value.options.is_none() {
                value.options = Some(windwalker_monk::Options {
                    class_options: Some(MonkOptions::default()),
                });
            }
            player::Spec::WindwalkerMonk(value)
        }
        player::Spec::HolyPaladin(mut value) => {
            if value.options.is_none() {
                value.options = Some(holy_paladin::Options {
                    class_options: Some(PaladinOptions::default()),
                });
            }
            player::Spec::HolyPaladin(value)
        }
        player::Spec::ProtectionPaladin(mut value) => {
            if value.options.is_none() {
                value.options = Some(protection_paladin::Options {
                    class_options: Some(PaladinOptions::default()),
                });
            }
            player::Spec::ProtectionPaladin(value)
        }
        player::Spec::RetributionPaladin(mut value) => {
            if value.options.is_none() {
                value.options = Some(retribution_paladin::Options {
                    class_options: Some(PaladinOptions::default()),
                });
            }
            player::Spec::RetributionPaladin(value)
        }
        player::Spec::DisciplinePriest(mut value) => {
            if value.options.is_none() {
                value.options = Some(discipline_priest::Options {
                    class_options: Some(PriestOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::DisciplinePriest(value)
        }
        player::Spec::HolyPriest(mut value) => {
            if value.options.is_none() {
                value.options = Some(holy_priest::Options {
                    class_options: Some(PriestOptions::default()),
                });
            }
            player::Spec::HolyPriest(value)
        }
        player::Spec::ShadowPriest(mut value) => {
            if value.options.is_none() {
                value.options = Some(shadow_priest::Options {
                    class_options: Some(PriestOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::ShadowPriest(value)
        }
        player::Spec::AssassinationRogue(mut value) => {
            if value.options.is_none() {
                value.options = Some(assassination_rogue::Options {
                    class_options: Some(RogueOptions::default()),
                });
            }
            player::Spec::AssassinationRogue(value)
        }
        player::Spec::CombatRogue(mut value) => {
            if value.options.is_none() {
                value.options = Some(combat_rogue::Options {
                    class_options: Some(RogueOptions::default()),
                });
            }
            player::Spec::CombatRogue(value)
        }
        player::Spec::SubtletyRogue(mut value) => {
            if value.options.is_none() {
                value.options = Some(subtlety_rogue::Options {
                    class_options: Some(RogueOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::SubtletyRogue(value)
        }
        player::Spec::ElementalShaman(mut value) => {
            if value.options.is_none() {
                value.options = Some(elemental_shaman::Options {
                    class_options: Some(ShamanOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::ElementalShaman(value)
        }
        player::Spec::EnhancementShaman(mut value) => {
            if value.options.is_none() {
                value.options = Some(enhancement_shaman::Options {
                    class_options: Some(ShamanOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::EnhancementShaman(value)
        }
        player::Spec::RestorationShaman(mut value) => {
            if value.options.is_none() {
                value.options = Some(restoration_shaman::Options {
                    class_options: Some(ShamanOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::RestorationShaman(value)
        }
        player::Spec::AfflictionWarlock(mut value) => {
            if value.options.is_none() {
                value.options = Some(affliction_warlock::Options {
                    class_options: Some(WarlockOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::AfflictionWarlock(value)
        }
        player::Spec::DemonologyWarlock(mut value) => {
            if value.options.is_none() {
                value.options = Some(demonology_warlock::Options {
                    class_options: Some(WarlockOptions::default()),
                });
            }
            player::Spec::DemonologyWarlock(value)
        }
        player::Spec::DestructionWarlock(mut value) => {
            if value.options.is_none() {
                value.options = Some(destruction_warlock::Options {
                    class_options: Some(WarlockOptions::default()),
                });
            }
            player::Spec::DestructionWarlock(value)
        }
        player::Spec::ArmsWarrior(mut value) => {
            if value.options.is_none() {
                value.options = Some(arms_warrior::Options {
                    class_options: Some(WarriorOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::ArmsWarrior(value)
        }
        player::Spec::FuryWarrior(mut value) => {
            if value.options.is_none() {
                value.options = Some(fury_warrior::Options {
                    class_options: Some(WarriorOptions::default()),
                    ..Default::default()
                });
            }
            player::Spec::FuryWarrior(value)
        }
        player::Spec::ProtectionWarrior(mut value) => {
            if value.options.is_none() {
                value.options = Some(protection_warrior::Options {
                    class_options: Some(WarriorOptions::default()),
                });
            }
            player::Spec::ProtectionWarrior(value)
        }
    }
}

fn with_spec_specific_defaults(spec: player::Spec) -> player::Spec {
    match spec {
        player::Spec::BloodDeathKnight(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DeathKnightOptions::default());
            }
            player::Spec::BloodDeathKnight(value)
        }
        player::Spec::FrostDeathKnight(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DeathKnightOptions::default());
            }
            player::Spec::FrostDeathKnight(value)
        }
        player::Spec::UnholyDeathKnight(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DeathKnightOptions::default());
            }
            player::Spec::UnholyDeathKnight(value)
        }
        player::Spec::BalanceDruid(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DruidOptions::default());
            }
            player::Spec::BalanceDruid(value)
        }
        player::Spec::FeralDruid(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DruidOptions::default());
            }
            player::Spec::FeralDruid(value)
        }
        player::Spec::GuardianDruid(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DruidOptions::default());
            }
            player::Spec::GuardianDruid(value)
        }
        player::Spec::RestorationDruid(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(DruidOptions::default());
            }
            player::Spec::RestorationDruid(value)
        }
        player::Spec::BeastMasteryHunter(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(HunterOptions::default());
            }
            player::Spec::BeastMasteryHunter(value)
        }
        player::Spec::MarksmanshipHunter(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(HunterOptions::default());
            }
            player::Spec::MarksmanshipHunter(value)
        }
        player::Spec::SurvivalHunter(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(HunterOptions::default());
            }
            player::Spec::SurvivalHunter(value)
        }
        player::Spec::ArcaneMage(mut value) => {
            if let Some(options) = value.options.as_mut() {
                if options.class_options.is_none() {
                    options.class_options = Some(MageOptions {
                        default_mage_armor: MageArmor::FrostArmor as i32,
                    });
                } else if let Some(class_options) = options.class_options.as_mut()
                    && class_options.default_mage_armor == MageArmor::None as i32
                {
                    class_options.default_mage_armor = MageArmor::FrostArmor as i32;
                }
            }
            player::Spec::ArcaneMage(value)
        }
        player::Spec::FireMage(mut value) => {
            if let Some(options) = value.options.as_mut() {
                if options.class_options.is_none() {
                    options.class_options = Some(MageOptions {
                        default_mage_armor: MageArmor::MoltenArmor as i32,
                    });
                } else if let Some(class_options) = options.class_options.as_mut()
                    && class_options.default_mage_armor == MageArmor::None as i32
                {
                    class_options.default_mage_armor = MageArmor::MoltenArmor as i32;
                }
            }
            player::Spec::FireMage(value)
        }
        player::Spec::FrostMage(mut value) => {
            if let Some(options) = value.options.as_mut() {
                if options.class_options.is_none() {
                    options.class_options = Some(MageOptions {
                        default_mage_armor: MageArmor::FrostArmor as i32,
                    });
                } else if let Some(class_options) = options.class_options.as_mut()
                    && class_options.default_mage_armor == MageArmor::None as i32
                {
                    class_options.default_mage_armor = MageArmor::FrostArmor as i32;
                }
            }
            player::Spec::FrostMage(value)
        }
        player::Spec::BrewmasterMonk(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(MonkOptions::default());
            }
            player::Spec::BrewmasterMonk(value)
        }
        player::Spec::MistweaverMonk(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(MonkOptions::default());
            }
            player::Spec::MistweaverMonk(value)
        }
        player::Spec::WindwalkerMonk(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(MonkOptions::default());
            }
            player::Spec::WindwalkerMonk(value)
        }
        player::Spec::HolyPaladin(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PaladinOptions::default());
            }
            player::Spec::HolyPaladin(value)
        }
        player::Spec::ProtectionPaladin(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PaladinOptions::default());
            }
            player::Spec::ProtectionPaladin(value)
        }
        player::Spec::RetributionPaladin(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PaladinOptions::default());
            }
            player::Spec::RetributionPaladin(value)
        }
        player::Spec::DisciplinePriest(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PriestOptions::default());
            }
            player::Spec::DisciplinePriest(value)
        }
        player::Spec::HolyPriest(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PriestOptions::default());
            }
            player::Spec::HolyPriest(value)
        }
        player::Spec::ShadowPriest(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(PriestOptions::default());
            }
            player::Spec::ShadowPriest(value)
        }
        player::Spec::AssassinationRogue(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(RogueOptions::default());
            }
            player::Spec::AssassinationRogue(value)
        }
        player::Spec::CombatRogue(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(RogueOptions::default());
            }
            player::Spec::CombatRogue(value)
        }
        player::Spec::SubtletyRogue(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(RogueOptions::default());
            }
            player::Spec::SubtletyRogue(value)
        }
        player::Spec::ElementalShaman(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(ShamanOptions::default());
            }
            player::Spec::ElementalShaman(value)
        }
        player::Spec::EnhancementShaman(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(ShamanOptions::default());
            }
            player::Spec::EnhancementShaman(value)
        }
        player::Spec::RestorationShaman(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(ShamanOptions::default());
            }
            player::Spec::RestorationShaman(value)
        }
        player::Spec::AfflictionWarlock(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarlockOptions::default());
            }
            player::Spec::AfflictionWarlock(value)
        }
        player::Spec::DemonologyWarlock(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarlockOptions::default());
            }
            player::Spec::DemonologyWarlock(value)
        }
        player::Spec::DestructionWarlock(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarlockOptions::default());
            }
            player::Spec::DestructionWarlock(value)
        }
        player::Spec::ArmsWarrior(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarriorOptions::default());
            }
            player::Spec::ArmsWarrior(value)
        }
        player::Spec::FuryWarrior(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarriorOptions::default());
            }
            player::Spec::FuryWarrior(value)
        }
        player::Spec::ProtectionWarrior(mut value) => {
            if let Some(options) = value.options.as_mut()
                && options.class_options.is_none()
            {
                options.class_options = Some(WarriorOptions::default());
            }
            player::Spec::ProtectionWarrior(value)
        }
    }
}

pub fn build_player_from_payload(
    discord_user_id: &str,
    class: &str,
    spec: &str,
    payload: &Value,
) -> Result<Player> {
    let class = class.trim().to_lowercase();
    let spec = spec.trim().to_lowercase();
    let (class_id, default_race_id, expected_spec_oneof) = resolve_player_spec(&class, &spec)?;

    let player_name = string_field(payload, "name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if discord_user_id.is_empty() {
                format!("{class}/{spec}")
            } else {
                format!("user-{discord_user_id}")
            }
        });
    let (profession1, profession2) = parse_professions(payload);

    let rotation = match load_rotation_from_payload(payload) {
        Ok(Some(value)) => Some(value),
        Ok(None) => Some(load_vendor_default_rotation(&class, &spec)?),
        Err(error) => {
            tracing::warn!(
                class = %class,
                spec = %spec,
                error = ?error,
                "failed to parse payload rotation, falling back to vendor default APL"
            );
            Some(load_vendor_default_rotation(&class, &spec)?)
        }
    };

    let build_spec = |spec_oneof: player::Spec| {
        with_spec_specific_defaults(with_default_spec_options(spec_oneof))
    };

    if let Some(mut player) = load_full_player_from_payload(payload) {
        let spec_oneof = match player.spec.take() {
            Some(spec_oneof)
                if std::mem::discriminant(&spec_oneof)
                    == std::mem::discriminant(&expected_spec_oneof) =>
            {
                spec_oneof
            }
            _ => expected_spec_oneof,
        };

        player.api_version = PLAYER_API_VERSION;
        player.class = class_id;
        if player.race == Race::Unknown as i32 {
            player.race = default_race_id;
        }
        if player.name.trim().is_empty() {
            player.name = player_name;
        }
        if player.equipment.is_none() {
            player.equipment = Some(parse_equipment_spec(payload));
        } else if let Some(equipment) = player.equipment.take() {
            player.equipment = Some(normalize_equipment_spec(equipment));
        }
        if player.glyphs.is_none() {
            player.glyphs = Some(parse_glyphs(payload));
        }
        if let Some(item_swap) = player.item_swap.take() {
            player.item_swap = Some(normalize_item_swap(item_swap));
        }
        if player.talents_string.trim().is_empty() {
            player.talents_string = parse_talents_string(payload);
        }
        if player.rotation.is_none() {
            player.rotation = rotation;
        }
        player.spec = Some(build_spec(spec_oneof));

        return Ok(player);
    }

    Ok(Player {
        api_version: PLAYER_API_VERSION,
        name: player_name,
        race: parse_race(payload).unwrap_or(default_race_id),
        class: class_id,
        equipment: Some(parse_equipment_spec(payload)),
        glyphs: Some(parse_glyphs(payload)),
        talents_string: parse_talents_string(payload),
        profession1,
        profession2,
        rotation,
        spec: Some(build_spec(expected_spec_oneof)),
        ..Default::default()
    })
}

#[cfg(test)]
mod full_player_tests {
    use super::*;
    use crate::mop_proto::mop::Profession;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn simulation_run(payload: Value) -> db::SimulationRun {
        let now = Utc::now();

        db::SimulationRun {
            run_id: Uuid::new_v4(),
            discord_user_id: "test-user".to_string(),
            class: "mage".to_string(),
            spec: "arcane".to_string(),
            gear_payload: payload,
            input_format: "json".to_string(),
            upstream_revision: None,
            normalized_request: None,
            effective_random_seed: None,
            effective_iterations: None,
            raid_members: Vec::new(),
            status: "pending".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn build_player_preserves_full_player_settings() {
        let run = simulation_run(json!({
            "player": {
                "class": "ClassMage",
                "race": "RaceTroll",
                "name": "arcane-local",
                "equipment": { "items": [{ "id": 12345, "enchant": 54321 }] },
                "glyphs": { "major1": 1, "major2": 2, "major3": 3 },
                "talentsString": "123123",
                "arcaneMage": {
                    "options": {
                        "classOptions": { "defaultMageArmor": "MageArmorFrostArmor" }
                    }
                },
                "rotation": { "type": "TypeAuto" },
                "consumables": {
                    "flaskId": 76085,
                    "foodId": 74650,
                    "potId": 76093,
                    "prepotId": 76093
                },
                "profession1": "Engineering",
                "profession2": "Tailoring",
                "reactionTimeMs": 50,
                "channelClipDelayMs": 25,
                "distanceFromTarget": 20.0,
                "challengeMode": true
            }
        }));

        let player = build_player_from_run(&run).unwrap();

        assert_eq!(player.class, Class::Mage as i32);
        assert_eq!(player.race, Race::Troll as i32);
        assert_eq!(player.name, "arcane-local");
        assert_eq!(player.profession1, Profession::Engineering as i32);
        assert_eq!(player.profession2, Profession::Tailoring as i32);
        assert_eq!(player.distance_from_target, 20.0);
        assert_eq!(player.reaction_time_ms, 50);
        assert_eq!(player.channel_clip_delay_ms, 25);
        assert!(player.challenge_mode);
        assert_eq!(player.consumables.as_ref().unwrap().flask_id, 76085);
        assert_eq!(player.equipment.as_ref().unwrap().items[0].id, 12345);
        assert_eq!(player.talents_string, "123123");
        assert!(player.rotation.is_some());

        match player.spec.unwrap() {
            player::Spec::ArcaneMage(value) => {
                let class_options = value.options.unwrap().class_options.unwrap();
                assert_eq!(
                    class_options.default_mage_armor,
                    MageArmor::FrostArmor as i32
                );
            }
            _ => panic!("expected arcane mage spec"),
        }
    }

    #[test]
    fn build_player_keeps_minimal_payload_fallback() {
        let run = simulation_run(json!({
            "class": "mage",
            "spec": "arcane",
            "items": [{ "id": 12345 }],
            "talents": "111111",
            "glyphs": {
                "major": [1, 2, 3],
                "minor": [4, 5, 6]
            },
            "rotation": { "type": "TypeAuto" }
        }));

        let player = build_player_from_run(&run).unwrap();

        assert_eq!(player.class, Class::Mage as i32);
        assert_eq!(player.race, Race::Human as i32);
        assert_eq!(player.equipment.as_ref().unwrap().items[0].id, 12345);
        assert_eq!(player.glyphs.as_ref().unwrap().major1, 1);
        assert_eq!(player.talents_string, "111111");
        assert!(player.rotation.is_some());
    }

    #[test]
    fn build_player_clamps_unsupported_upgrade_steps() {
        let run = simulation_run(json!({
            "class": "mage",
            "spec": "arcane",
            "items": [
                { "id": 45703, "upgradeStep": 2 },
                { "id": 94524, "upgradeStep": 2 }
            ],
            "rotation": { "type": "TypeAuto" }
        }));

        let player = build_player_from_run(&run).unwrap();
        let items = &player.equipment.as_ref().unwrap().items;

        assert_eq!(items[0].id, 45703);
        assert_eq!(items[0].upgrade_step, ItemLevelState::Base as i32);
        assert_eq!(items[1].id, 94524);
        assert_eq!(items[1].upgrade_step, ItemLevelState::UpgradeStepTwo as i32);
    }
}

pub fn build_player_from_run(run: &db::SimulationRun) -> Result<Player> {
    build_player_from_payload(
        &run.discord_user_id,
        &run.class,
        &run.spec,
        &run.gear_payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/gear-profile.json"))
            .expect("fixture must be valid JSON")
    }

    #[test]
    fn preserves_warcraft_logs_player_and_gear_fields() {
        let player = build_player_from_payload("discord-user", "mage", "arcane", &fixture())
            .expect("fixture must build a player");
        let equipment = player.equipment.expect("player must have equipment");
        let glyphs = player.glyphs.expect("player must have glyphs");

        assert_eq!(player.name, "Autismowismo");
        assert_eq!(player.race, Race::NightElf as i32);
        assert_eq!(player.profession1, Profession::Enchanting as i32);
        assert_eq!(player.profession2, Profession::Tailoring as i32);
        assert_eq!(player.talents_string, "311222");
        assert_eq!(glyphs.major1, 146659);
        assert_eq!(glyphs.major2, 115705);
        assert_eq!(glyphs.major3, 62210);
        assert_eq!(glyphs.minor1, 63093);
        assert_eq!(glyphs.minor2, 56363);
        assert_eq!(equipment.items.len(), EQUIPMENT_SLOT_COUNT);
        assert_eq!(equipment.items[0].id, 103900);
        assert_eq!(equipment.items[0].gems, vec![95347, 76700]);
        assert_eq!(equipment.items[2].enchant, 4806);
        assert_eq!(equipment.items[14].id, 103874);
        assert_eq!(equipment.items[15].id, 0);
        assert!(player.rotation.is_some());
    }
}
