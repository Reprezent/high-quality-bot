use crate::{
    db::{WclPendingFight, WclReportToAnnounce},
    warcraft_logs::{KillSummary, MetricEntry, WarcraftLogsSite},
};
use anyhow::{Context as _, Result};
use image::{ImageEncoder, RgbImage};
use plotters::prelude::*;
use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, CreateEmbedFooter, Nonce};
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::Duration,
};

//
const WARCRAFT_LOGS_COLOR: u32 = 0xF28C28;
pub const FIGHT_IMAGE_NAME: &str = "warcraft_logs_fight.png";
const FIGHT_BACKGROUND: &[u8] = include_bytes!("../assets/warcraft_logs_background.png");
const IMAGE_WIDTH: u32 = 1_000;
const IMAGE_HEIGHT: u32 = 540;
const BAR_LEFT: i32 = 92;
const BAR_RIGHT: i32 = 956;
const BAR_HEIGHT: i32 = 46;
const ICON_SIZE: u32 = 38;
const ICON_CDN: &str = "https://render.worldofwarcraft.com/us/icons/56";
static ICON_CACHE: OnceLock<Mutex<HashMap<String, Option<RgbImage>>>> = OnceLock::new();
static ICON_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn report_url(site: WarcraftLogsSite, code: &str) -> String {
    site.report_url(code)
}

pub fn fight_url(site: WarcraftLogsSite, code: &str, fight_id: i32) -> String {
    format!("{}#fight={fight_id}&type=summary", site.report_url(code))
}

// Creates an embed for discord upon a new report being made.
pub fn report_embed(report: &WclReportToAnnounce) -> CreateEmbed {
    let url = report_url(report.wcl_site, &report.code);
    let mut embed = CreateEmbed::new()
        .color(WARCRAFT_LOGS_COLOR)
        .title(truncate(&report.title, 256))
        .url(&url)
        .description(format!(
            "A new **{}** report is available on Warcraft Logs.",
            report.wcl_guild_name
        ))
        .field(
            "Zone",
            report.zone_name.as_deref().unwrap_or("Unknown"),
            true,
        )
        .field("Report", format!("[Open Warcraft Logs]({url})"), true)
        .footer(CreateEmbedFooter::new("Warcraft Logs"));

    if let Ok(timestamp) =
        serenity::Timestamp::from_unix_timestamp(report.start_time_ms.div_euclid(1_000))
    {
        embed = embed.timestamp(timestamp);
    }

    embed
}

pub fn kill_embed(
    fight: &WclPendingFight,
    summary: &KillSummary,
    include_image: bool,
) -> CreateEmbed {
    let url = fight_url(fight.wcl_site, &fight.report_code, fight.fight.fight_id);
    let duration_ms = (fight.fight.end_time_ms - fight.fight.start_time_ms).max(0);
    let kill_time_ms = fight.report_start_time_ms + fight.fight.end_time_ms;
    let raid_size = fight
        .fight
        .raid_size
        .map(|size| size.to_string())
        .unwrap_or_else(|| "Unavailable".to_owned());
    let average_item_level = fight
        .fight
        .average_item_level
        .map(|item_level| format!("{item_level:.1}"))
        .unwrap_or_else(|| "Unavailable".to_owned());
    let deaths = summary
        .deaths
        .map(|deaths| deaths.to_string())
        .unwrap_or_else(|| "Unavailable".to_owned());

    let mut embed = CreateEmbed::new()
        .color(0x2ECC71)
        .title(truncate(
            &format!(
                "Congratulations {}! {} {} is killed!",
                fight.wcl_guild_name,
                difficulty_name(fight.fight.difficulty),
                fight.fight.boss_name
            ),
            256,
        ))
        .url(&url)
        .description(format!(
            "{} player **{} {}** in {} - [{}]({url}).",
            raid_size,
            difficulty_name(fight.fight.difficulty),
            fight.fight.boss_name,
            format_duration(duration_ms),
            fight.report_title
        ))
        .field("Average Item Level", average_item_level, true)
        .field("Deaths", deaths, true)
        .field("Full Report", format!("[View this fight]({url})"), false)
        .footer(CreateEmbedFooter::new("Warcraft Logs boss kill"));

    if include_image {
        embed = embed.image(format!("attachment://{FIGHT_IMAGE_NAME}"));
    }
    if let Ok(timestamp) = serenity::Timestamp::from_unix_timestamp(kill_time_ms.div_euclid(1_000))
    {
        embed = embed.timestamp(timestamp);
    }

    embed
}

pub async fn render_kill_summary(
    fight: &WclPendingFight,
    summary: &KillSummary,
) -> Result<Vec<u8>> {
    let icons = load_summary_icons(summary).await;
    render_kill_summary_with_icons(fight, summary, &icons)
}

fn render_kill_summary_with_icons(
    fight: &WclPendingFight,
    summary: &KillSummary,
    icons: &HashMap<String, RgbImage>,
) -> Result<Vec<u8>> {
    let background = background_image(FIGHT_BACKGROUND).unwrap_or_else(|error| {
        tracing::warn!(error = ?error, "failed to decode bundled fight background; using solid fill");
        RgbImage::from_pixel(IMAGE_WIDTH, IMAGE_HEIGHT, image::Rgb([18, 18, 18]))
    });
    let mut buffer = background.into_raw();
    let duration_seconds =
        ((fight.fight.end_time_ms - fight.fight.start_time_ms).max(1) as f64 / 1_000.0).max(1.0);

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (IMAGE_WIDTH, IMAGE_HEIGHT))
            .into_drawing_area();
        root.draw(&Rectangle::new(
            [(0, 0), (IMAGE_WIDTH as i32, IMAGE_HEIGHT as i32)],
            RGBColor(8, 8, 11).mix(0.28).filled(),
        ))
        .context("failed to shade Warcraft Logs image background")?;
        root.draw(&Rectangle::new(
            [(0, 0), (IMAGE_WIDTH as i32, 72)],
            RGBColor(24, 24, 28).mix(0.88).filled(),
        ))
        .context("failed to draw Warcraft Logs image header")?;
        root.draw(&Text::new(
            truncate(&fight.fight.boss_name, 42),
            (44, 21),
            ("sans-serif", 30).into_font().color(&WHITE),
        ))
        .context("failed to draw fight title")?;
        root.draw(&Text::new(
            format!(
                "{}  •  {}",
                difficulty_name(fight.fight.difficulty),
                format_duration(fight.fight.end_time_ms - fight.fight.start_time_ms)
            ),
            (44, 52),
            ("sans-serif", 16)
                .into_font()
                .color(&RGBColor(174, 174, 181)),
        ))
        .context("failed to draw fight details")?;

        draw_metric_section(
            &root,
            "Damage Done",
            "DPS",
            summary.top_damage.as_deref(),
            icons,
            duration_seconds,
            94,
        )?;
        draw_metric_section(
            &root,
            "Healing Done",
            "HPS",
            summary.top_healing.as_deref(),
            icons,
            duration_seconds,
            310,
        )?;
        root.present()
            .context("failed to finish Warcraft Logs image")?;
    }

    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    encoder
        .write_image(&buffer, IMAGE_WIDTH, IMAGE_HEIGHT, image::ColorType::Rgb8)
        .context("failed to encode Warcraft Logs image")?;
    Ok(png)
}

fn background_image(bytes: &[u8]) -> Result<RgbImage> {
    let background =
        image::load_from_memory(bytes).context("background asset is not a valid image")?;
    Ok(image::imageops::resize(
        &background.to_rgb8(),
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    ))
}

#[cfg(test)]
fn background_buffer(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(background_image(bytes)?.into_raw())
}

async fn load_summary_icons(summary: &KillSummary) -> HashMap<String, RgbImage> {
    let icon_names = summary
        .top_damage
        .iter()
        .chain(&summary.top_healing)
        .flatten()
        .filter_map(|entry| entry.icon_name.as_deref())
        .filter(|name| spec_icon_file(name).is_some())
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mut tasks = tokio::task::JoinSet::new();
    for icon_name in icon_names {
        tasks.spawn(load_spec_icon(icon_name));
    }

    let mut icons = HashMap::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(Some((name, icon)))) => {
                icons.insert(name, icon);
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => tracing::warn!(error = ?error, "failed to load specialization icon"),
            Err(error) => tracing::warn!(error = ?error, "specialization icon task failed"),
        }
    }
    icons
}

async fn load_spec_icon(icon_name: String) -> Result<Option<(String, RgbImage)>> {
    if let Some(cached) = ICON_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("specialization icon cache lock poisoned")
        .get(&icon_name)
        .cloned()
    {
        return Ok(cached.map(|icon| (icon_name, icon)));
    }
    let Some(file_name) = spec_icon_file(&icon_name) else {
        return Ok(None);
    };
    match fetch_spec_icon(file_name).await {
        Ok(icon) => {
            ICON_CACHE
                .get()
                .expect("specialization icon cache initialized")
                .lock()
                .expect("specialization icon cache lock poisoned")
                .insert(icon_name.clone(), Some(icon.clone()));
            Ok(Some((icon_name, icon)))
        }
        Err(error) => {
            ICON_CACHE
                .get()
                .expect("specialization icon cache initialized")
                .lock()
                .expect("specialization icon cache lock poisoned")
                .insert(icon_name, None);
            Err(error)
        }
    }
}

async fn fetch_spec_icon(file_name: &str) -> Result<RgbImage> {
    let client = ICON_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .expect("static specialization icon client configuration is valid")
    });
    let bytes = client
        .get(format!("{ICON_CDN}/{file_name}.jpg"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let icon = image::imageops::resize(
        &image::load_from_memory(&bytes)
            .context("specialization icon response is not a valid image")?
            .to_rgb8(),
        ICON_SIZE,
        ICON_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    Ok(icon)
}

fn draw_metric_section(
    root: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    heading: &str,
    unit: &str,
    entries: Option<&[MetricEntry]>,
    icons: &HashMap<String, RgbImage>,
    duration_seconds: f64,
    top: i32,
) -> Result<()> {
    root.draw(&Text::new(
        heading,
        (44, top),
        ("sans-serif", 17)
            .into_font()
            .color(&RGBColor(242, 140, 40)),
    ))
    .context("failed to draw metric heading")?;

    let Some(entries) = entries.filter(|entries| !entries.is_empty()) else {
        root.draw(&Text::new(
            "No data available",
            (BAR_LEFT, top + 49),
            ("sans-serif", 18)
                .into_font()
                .color(&RGBColor(150, 150, 157)),
        ))
        .context("failed to draw unavailable metric label")?;
        return Ok(());
    };
    let highest = entries[0].total.max(0.0);

    for (index, entry) in entries.iter().take(3).enumerate() {
        let y = top + 22 + index as i32 * 58;
        let ratio = if highest > 0.0 {
            (entry.total / highest).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let color = class_color(entry.class_name.as_deref());
        let filled_right = BAR_LEFT + ((BAR_RIGHT - BAR_LEFT) as f64 * ratio).round() as i32;

        root.draw(&Rectangle::new(
            [(BAR_LEFT, y), (BAR_RIGHT, y + BAR_HEIGHT)],
            RGBColor(43, 43, 48).filled(),
        ))
        .context("failed to draw metric bar background")?;
        if filled_right > BAR_LEFT {
            root.draw(&Rectangle::new(
                [(BAR_LEFT, y), (filled_right, y + BAR_HEIGHT)],
                color.filled(),
            ))
            .context("failed to draw class-colored metric bar")?;
        }
        if let Some(icon) = entry
            .icon_name
            .as_ref()
            .and_then(|icon_name| icons.get(icon_name))
        {
            let icon = BitMapElement::with_owned_buffer(
                (44, y + (BAR_HEIGHT - ICON_SIZE as i32) / 2),
                (ICON_SIZE, ICON_SIZE),
                icon.clone().into_raw(),
            )
            .context("specialization icon has invalid RGB dimensions")?;
            root.draw(&icon)
                .context("failed to draw specialization icon")?;
        } else {
            root.draw(&Rectangle::new(
                [(44, y), (82, y + BAR_HEIGHT)],
                color.filled(),
            ))
            .context("failed to draw class icon fallback")?;
            root.draw(&Text::new(
                class_icon(entry.class_name.as_deref()),
                (52, y + 10),
                ("sans-serif", 18)
                    .into_font()
                    .style(FontStyle::Bold)
                    .color(&text_color(color)),
            ))
            .context("failed to draw class icon fallback label")?;
        }
        root.draw(&Text::new(
            truncate(&entry.name, 24),
            (109, y + 12),
            ("sans-serif", 20)
                .into_font()
                .style(FontStyle::Bold)
                .color(&BLACK),
        ))
        .context("failed to draw player name shadow")?;
        root.draw(&Text::new(
            truncate(&entry.name, 24),
            (108, y + 11),
            ("sans-serif", 20)
                .into_font()
                .style(FontStyle::Bold)
                .color(&WHITE),
        ))
        .context("failed to draw player name")?;
        let metric = format!(
            "{} {unit}  •  {:.0}%",
            format_number(entry.total / duration_seconds),
            ratio * 100.0
        );
        root.draw(&Text::new(
            metric.clone(),
            (731, y + 13),
            ("sans-serif", 18).into_font().color(&BLACK),
        ))
        .context("failed to draw player metric shadow")?;
        root.draw(&Text::new(
            metric,
            (730, y + 12),
            ("sans-serif", 18).into_font().color(&WHITE),
        ))
        .context("failed to draw player metric")?;
    }
    Ok(())
}

fn class_color(class_name: Option<&str>) -> RGBColor {
    match class_name.unwrap_or_default().to_ascii_lowercase().as_str() {
        "deathknight" | "death knight" => RGBColor(196, 30, 58),
        "demonhunter" | "demon hunter" => RGBColor(163, 48, 201),
        "druid" => RGBColor(255, 124, 10),
        "evoker" => RGBColor(51, 147, 127),
        "hunter" => RGBColor(170, 211, 114),
        "mage" => RGBColor(63, 199, 235),
        "monk" => RGBColor(0, 255, 152),
        "paladin" => RGBColor(244, 140, 186),
        "priest" => RGBColor(255, 255, 255),
        "rogue" => RGBColor(255, 244, 104),
        "shaman" => RGBColor(0, 112, 221),
        "warlock" => RGBColor(135, 136, 238),
        "warrior" => RGBColor(198, 155, 109),
        _ => RGBColor(128, 128, 136),
    }
}

fn class_icon(class_name: Option<&str>) -> String {
    match class_name.unwrap_or_default().to_ascii_lowercase().as_str() {
        "deathknight" | "death knight" => return "DK".to_owned(),
        "demonhunter" | "demon hunter" => return "DH".to_owned(),
        _ => {}
    }
    let words = class_name
        .unwrap_or("?")
        .split_whitespace()
        .collect::<Vec<_>>();
    if words.len() > 1 {
        words
            .iter()
            .filter_map(|word| word.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    } else {
        words
            .first()
            .unwrap_or(&"?")
            .chars()
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }
}

fn spec_icon_file(icon_name: &str) -> Option<&'static str> {
    match icon_name.to_ascii_lowercase().as_str() {
        "deathknight-blood" => Some("spell_deathknight_bloodpresence"),
        "deathknight-frost" => Some("spell_deathknight_frostpresence"),
        "deathknight-unholy" => Some("spell_deathknight_unholypresence"),
        "demonhunter-havoc" => Some("ability_demonhunter_specdps"),
        "demonhunter-vengeance" => Some("ability_demonhunter_spectank"),
        "druid-balance" => Some("spell_nature_starfall"),
        "druid-feral" => Some("ability_druid_catform"),
        "druid-guardian" => Some("ability_racial_bearform"),
        "druid-restoration" => Some("spell_nature_healingtouch"),
        "evoker-augmentation" => Some("classicon_evoker_augmentation"),
        "evoker-devastation" => Some("ability_evoker_devastation"),
        "evoker-preservation" => Some("ability_evoker_preservation"),
        "hunter-beastmastery" | "hunter-beast mastery" => Some("ability_hunter_bestialdiscipline"),
        "hunter-marksmanship" => Some("ability_hunter_focusedaim"),
        "hunter-survival" => Some("ability_hunter_camouflage"),
        "mage-arcane" => Some("spell_holy_magicalsentry"),
        "mage-fire" => Some("spell_fire_firebolt02"),
        "mage-frost" => Some("spell_frost_frostbolt02"),
        "monk-brewmaster" => Some("spell_monk_brewmaster_spec"),
        "monk-mistweaver" => Some("spell_monk_mistweaver_spec"),
        "monk-windwalker" => Some("spell_monk_windwalker_spec"),
        "paladin-holy" => Some("spell_holy_holybolt"),
        "paladin-protection" => Some("ability_paladin_shieldofthetemplar"),
        "paladin-retribution" => Some("spell_holy_auraoflight"),
        "priest-discipline" => Some("spell_holy_powerwordshield"),
        "priest-holy" => Some("spell_holy_guardianspirit"),
        "priest-shadow" => Some("spell_shadow_shadowwordpain"),
        "rogue-assassination" => Some("ability_rogue_eviscerate"),
        "rogue-combat" => Some("ability_backstab"),
        "rogue-outlaw" => Some("ability_rogue_waylay"),
        "rogue-subtlety" => Some("ability_stealth"),
        "shaman-elemental" => Some("spell_nature_lightning"),
        "shaman-enhancement" => Some("spell_nature_lightningshield"),
        "shaman-restoration" => Some("spell_nature_magicimmunity"),
        "warlock-affliction" => Some("spell_shadow_deathcoil"),
        "warlock-demonology" => Some("spell_shadow_metamorphosis"),
        "warlock-destruction" => Some("spell_shadow_rainoffire"),
        "warrior-arms" => Some("ability_warrior_savageblow"),
        "warrior-fury" => Some("ability_warrior_innerrage"),
        "warrior-protection" => Some("ability_warrior_defensivestance"),
        _ => None,
    }
}

fn text_color(background: RGBColor) -> RGBColor {
    let brightness = 0.299 * f64::from(background.0)
        + 0.587 * f64::from(background.1)
        + 0.114 * f64::from(background.2);
    if brightness > 155.0 {
        RGBColor(18, 18, 20)
    } else {
        WHITE
    }
}

pub fn report_nonce(code: &str) -> Nonce {
    nonce("wlr", code)
}

pub fn fight_nonce(code: &str, fight_id: i32) -> Nonce {
    nonce("wlk", &format!("{code}:{fight_id}"))
}

fn nonce(kind: &str, identity: &str) -> Nonce {
    let candidate = format!("{kind}:{identity}");
    if candidate.len() <= 25 {
        return Nonce::String(candidate);
    }

    let mut hash = 0xcbf29ce484222325_u64;
    for byte in candidate.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Nonce::String(format!("{kind}:{hash:016x}"))
}

fn difficulty_name(difficulty: Option<i32>) -> String {
    match difficulty {
        Some(1) => "LFR".to_owned(),
        Some(2) => "Flex".to_owned(),
        Some(3) => "Normal".to_owned(),
        Some(4) => "Heroic".to_owned(),
        Some(5) => "Mythic".to_owned(),
        Some(value) => format!("Difficulty {value}"),
        None => "Unavailable".to_owned(),
    }
}

fn format_duration(duration_ms: i64) -> String {
    let total_seconds = duration_ms / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn format_number(value: f64) -> String {
    let absolute = value.abs();
    if absolute >= 1_000_000_000.0 {
        format!("{:.2}B", value / 1_000_000_000.0)
    } else if absolute >= 1_000_000.0 {
        format!("{:.2}M", value / 1_000_000.0)
    } else if absolute >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{
        FIGHT_BACKGROUND, IMAGE_HEIGHT, IMAGE_WIDTH, background_buffer, class_color, fight_nonce,
        fight_url, format_duration, format_number, render_kill_summary_with_icons, report_url,
        spec_icon_file,
    };
    use crate::{
        db::{WclFightRecord, WclPendingFight},
        warcraft_logs::{KillSummary, MetricEntry, WarcraftLogsSite},
    };
    use image::GenericImageView;
    use plotters::style::RGBColor;
    use poise::serenity_prelude::Nonce;
    use std::collections::HashMap;

    #[test]
    fn builds_canonical_report_urls() {
        assert_eq!(
            report_url(WarcraftLogsSite::Retail, "abc123"),
            "https://www.warcraftlogs.com/reports/abc123"
        );
        assert_eq!(
            fight_url(WarcraftLogsSite::Classic, "abc123", 7),
            "https://classic.warcraftlogs.com/reports/abc123#fight=7&type=summary"
        );
    }

    #[test]
    fn formats_duration_and_large_numbers() {
        assert_eq!(format_duration(125_900), "2:05");
        assert_eq!(format_number(999.0), "999");
        assert_eq!(format_number(12_345.0), "12.3K");
        assert_eq!(format_number(9_876_543.0), "9.88M");
    }

    #[test]
    fn generated_nonce_respects_discord_limit() {
        let Nonce::String(value) = fight_nonce("a-very-long-report-code", 12345) else {
            panic!("expected string nonce");
        };
        assert!(value.len() <= 25);
        let Nonce::String(second_value) = fight_nonce("a-very-long-report-code", 12345) else {
            panic!("expected string nonce");
        };
        assert_eq!(second_value, value);
    }

    #[test]
    fn renders_class_colored_metric_bars_as_png() {
        let fight = WclPendingFight {
            subscription_id: 1,
            discord_channel_id: "1".to_owned(),
            wcl_site: WarcraftLogsSite::Retail,
            wcl_guild_name: "Guild".to_owned(),
            report_code: "abc123".to_owned(),
            report_title: "Raid".to_owned(),
            report_start_time_ms: 0,
            fight: WclFightRecord {
                fight_id: 7,
                boss_name: "Test Boss".to_owned(),
                difficulty: Some(5),
                raid_size: Some(20),
                average_item_level: Some(700.0),
                start_time_ms: 0,
                end_time_ms: 120_000,
            },
        };
        let damage = vec![
            MetricEntry {
                name: "First".to_owned(),
                total: 1_200_000.0,
                class_name: Some("Mage".to_owned()),
                icon_name: Some("Mage-Arcane".to_owned()),
            },
            MetricEntry {
                name: "Second".to_owned(),
                total: 600_000.0,
                class_name: Some("Warrior".to_owned()),
                icon_name: Some("Warrior-Arms".to_owned()),
            },
        ];
        let summary = KillSummary {
            top_damage: Some(damage.clone()),
            top_healing: Some(damage),
            deaths: Some(0),
        };

        let png = render_kill_summary_with_icons(&fight, &summary, &HashMap::new()).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.dimensions(), (IMAGE_WIDTH, IMAGE_HEIGHT));
        assert_eq!(class_color(Some("Mage")), RGBColor(63, 199, 235));
        assert_eq!(
            spec_icon_file("Priest-Discipline"),
            Some("spell_holy_powerwordshield")
        );
        assert_eq!(
            spec_icon_file("DeathKnight-Blood"),
            Some("spell_deathknight_bloodpresence")
        );
        assert_eq!(spec_icon_file("Unknown-Spec"), None);
    }

    #[test]
    fn composites_available_specialization_icons() {
        let fight = test_fight();
        let entry = MetricEntry {
            name: "First".to_owned(),
            total: 1_200_000.0,
            class_name: Some("Mage".to_owned()),
            icon_name: Some("Mage-Arcane".to_owned()),
        };
        let summary = KillSummary {
            top_damage: Some(vec![entry]),
            top_healing: None,
            deaths: Some(0),
        };
        let mut icons = HashMap::new();
        icons.insert(
            "Mage-Arcane".to_owned(),
            image::RgbImage::from_pixel(38, 38, image::Rgb([250, 10, 10])),
        );

        let png = render_kill_summary_with_icons(&fight, &summary, &icons).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgb8();
        let pixel = decoded.get_pixel(45, 121);
        assert!(pixel[0] > pixel[1] * 5);
        assert!(pixel[0] > pixel[2] * 5);
    }

    #[test]
    fn loads_bundled_background_at_canvas_size() {
        let background = background_buffer(FIGHT_BACKGROUND).unwrap();
        assert_eq!(background.len(), (IMAGE_WIDTH * IMAGE_HEIGHT * 3) as usize);
        assert!(background.windows(2).any(|pixels| pixels[0] != pixels[1]));
        assert!(background_buffer(b"not an image").is_err());
    }

    fn test_fight() -> WclPendingFight {
        WclPendingFight {
            subscription_id: 1,
            discord_channel_id: "1".to_owned(),
            wcl_site: WarcraftLogsSite::Retail,
            wcl_guild_name: "Guild".to_owned(),
            report_code: "abc123".to_owned(),
            report_title: "Raid".to_owned(),
            report_start_time_ms: 0,
            fight: WclFightRecord {
                fight_id: 7,
                boss_name: "Test Boss".to_owned(),
                difficulty: Some(5),
                raid_size: Some(20),
                average_item_level: Some(700.0),
                start_time_ms: 0,
                end_time_ms: 120_000,
            },
        }
    }
}
