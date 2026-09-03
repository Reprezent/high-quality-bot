use crate::{
    db::{WclPendingFight, WclReportToAnnounce},
    warcraft_logs::{KillSummary, MetricEntry, WarcraftLogsSite},
};
use anyhow::{Context as _, Result};
use image::ImageEncoder;
use plotters::prelude::*;
use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, CreateEmbedFooter, Nonce};

const WARCRAFT_LOGS_COLOR: u32 = 0xF28C28;
pub const FIGHT_IMAGE_NAME: &str = "warcraft_logs_fight.png";
const IMAGE_WIDTH: u32 = 1_000;
const IMAGE_HEIGHT: u32 = 540;
const BAR_LEFT: i32 = 92;
const BAR_RIGHT: i32 = 956;
const BAR_HEIGHT: i32 = 46;

pub fn report_url(site: WarcraftLogsSite, code: &str) -> String {
    site.report_url(code)
}

pub fn fight_url(site: WarcraftLogsSite, code: &str, fight_id: i32) -> String {
    format!("{}#fight={fight_id}&type=summary", site.report_url(code))
}

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
            &format!("Congratulations! {} defeated", fight.fight.boss_name),
            256,
        ))
        .url(&url)
        .description(format!(
            "**{}** defeated **{}** in [{}]({url}).",
            fight.wcl_guild_name, fight.fight.boss_name, fight.report_title
        ))
        .field("Difficulty", difficulty_name(fight.fight.difficulty), true)
        .field("Duration", format_duration(duration_ms), true)
        .field("Raid Size", raid_size, true)
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

pub fn render_kill_summary(fight: &WclPendingFight, summary: &KillSummary) -> Result<Vec<u8>> {
    let mut buffer = vec![0_u8; (IMAGE_WIDTH * IMAGE_HEIGHT * 3) as usize];
    let duration_seconds =
        ((fight.fight.end_time_ms - fight.fight.start_time_ms).max(1) as f64 / 1_000.0).max(1.0);

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (IMAGE_WIDTH, IMAGE_HEIGHT))
            .into_drawing_area();
        root.fill(&RGBColor(18, 18, 20))
            .context("failed to fill Warcraft Logs image background")?;
        root.draw(&Rectangle::new(
            [(0, 0), (IMAGE_WIDTH as i32, 72)],
            RGBColor(31, 31, 35).filled(),
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
            "DAMAGE PER SECOND",
            "DPS",
            summary.top_damage.as_deref(),
            duration_seconds,
            94,
        )?;
        draw_metric_section(
            &root,
            "HEALING PER SECOND",
            "HPS",
            summary.top_healing.as_deref(),
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

fn draw_metric_section(
    root: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    heading: &str,
    unit: &str,
    entries: Option<&[MetricEntry]>,
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
        root.draw(&Rectangle::new(
            [(44, y), (82, y + BAR_HEIGHT)],
            color.filled(),
        ))
        .context("failed to draw class icon")?;
        root.draw(&Text::new(
            class_icon(entry.class_name.as_deref()),
            (52, y + 10),
            ("sans-serif", 18)
                .into_font()
                .style(FontStyle::Bold)
                .color(&text_color(color)),
        ))
        .context("failed to draw class icon label")?;
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
        IMAGE_HEIGHT, IMAGE_WIDTH, class_color, fight_nonce, fight_url, format_duration,
        format_number, render_kill_summary, report_url,
    };
    use crate::{
        db::{WclFightRecord, WclPendingFight},
        warcraft_logs::{KillSummary, MetricEntry, WarcraftLogsSite},
    };
    use image::GenericImageView;
    use plotters::style::RGBColor;
    use poise::serenity_prelude::Nonce;

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
            },
            MetricEntry {
                name: "Second".to_owned(),
                total: 600_000.0,
                class_name: Some("Warrior".to_owned()),
            },
        ];
        let summary = KillSummary {
            top_damage: Some(damage.clone()),
            top_healing: Some(damage),
            deaths: Some(0),
        };

        let png = render_kill_summary(&fight, &summary).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.dimensions(), (IMAGE_WIDTH, IMAGE_HEIGHT));
        assert_eq!(class_color(Some("Mage")), RGBColor(63, 199, 235));
    }
}
