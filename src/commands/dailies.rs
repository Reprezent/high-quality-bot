use crate::Context;
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

const US_DAILY_RESET_HOUR_UTC: u32 = 15;

fn next_us_daily_reset_after(now: DateTime<Utc>) -> DateTime<Utc> {
    let today_reset = Utc
        .with_ymd_and_hms(
            now.year(),
            now.month(),
            now.day(),
            US_DAILY_RESET_HOUR_UTC,
            0,
            0,
        )
        .single()
        .expect("valid UTC daily reset timestamp");

    if now < today_reset {
        today_reset
    } else {
        today_reset + Duration::days(1)
    }
}

/// Show when World of Warcraft US realm dailies reset next.
#[poise::command(slash_command, rename = "dailies")]
pub async fn dailies(ctx: Context<'_>) -> Result<()> {
    let next_reset = next_us_daily_reset_after(Utc::now());
    let timestamp = next_reset.timestamp();

    ctx.say(format!(
        "🗓️ **WoW US Daily Reset**\n\
         • Next reset: <t:{timestamp}:F> (<t:{timestamp}:R>)\n\
         • Realm region: **US**\n\
         • Reset time: **15:00 UTC daily**"
    ))
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::next_us_daily_reset_after;
    use chrono::{TimeZone, Utc};

    #[test]
    fn returns_todays_reset_before_reset_time() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 14, 59, 59).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 5, 8, 15, 0, 0).unwrap();

        assert_eq!(next_us_daily_reset_after(now), expected);
    }

    #[test]
    fn returns_tomorrows_reset_at_reset_time() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 15, 0, 0).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();

        assert_eq!(next_us_daily_reset_after(now), expected);
    }

    #[test]
    fn returns_tomorrows_reset_after_reset_time() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 23, 30, 0).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap();

        assert_eq!(next_us_daily_reset_after(now), expected);
    }
}
