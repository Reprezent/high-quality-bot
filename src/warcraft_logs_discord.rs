use crate::{
    db::{WclPendingFight, WclReportToAnnounce},
    warcraft_logs::{KillSummary, MetricEntry},
};
use poise::serenity_prelude as serenity;
use serenity::{CreateEmbed, CreateEmbedFooter, Nonce};

const WARCRAFT_LOGS_COLOR: u32 = 0xF28C28;

pub fn report_url(code: &str) -> String {
    format!("https://www.warcraftlogs.com/reports/{code}")
}

pub fn fight_url(code: &str, fight_id: i32) -> String {
    format!("https://www.warcraftlogs.com/reports/{code}#fight={fight_id}&type=summary")
}

pub fn report_embed(report: &WclReportToAnnounce) -> CreateEmbed {
    let url = report_url(&report.code);
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

pub fn kill_embed(fight: &WclPendingFight, summary: &KillSummary) -> CreateEmbed {
    let url = fight_url(&fight.report_code, fight.fight.fight_id);
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
        .field(
            "Top Damage",
            format_metric_entries(summary.top_damage.as_deref()),
            false,
        )
        .field(
            "Top Healing",
            format_metric_entries(summary.top_healing.as_deref()),
            false,
        )
        .field("Full Report", format!("[View this fight]({url})"), false)
        .footer(CreateEmbedFooter::new("Warcraft Logs boss kill"));

    if let Ok(timestamp) = serenity::Timestamp::from_unix_timestamp(kill_time_ms.div_euclid(1_000))
    {
        embed = embed.timestamp(timestamp);
    }

    embed
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

fn format_metric_entries(entries: Option<&[MetricEntry]>) -> String {
    let Some(entries) = entries else {
        return "Unavailable".to_owned();
    };
    if entries.is_empty() {
        return "No entries returned".to_owned();
    }

    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{}. **{}** — {}",
                index + 1,
                entry.name,
                format_number(entry.total)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    use super::{fight_nonce, fight_url, format_duration, format_number, report_url};
    use poise::serenity_prelude::Nonce;

    #[test]
    fn builds_canonical_report_urls() {
        assert_eq!(
            report_url("abc123"),
            "https://www.warcraftlogs.com/reports/abc123"
        );
        assert_eq!(
            fight_url("abc123", 7),
            "https://www.warcraftlogs.com/reports/abc123#fight=7&type=summary"
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
}
