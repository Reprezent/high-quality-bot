use crate::db::{self, IssTelemetrySample};
use crate::Context;
use anyhow::{Context as _, Result};
use plotters::prelude::*;
use poise::serenity_prelude::CreateAttachment;

const IMG_WIDTH: u32 = 800;
const IMG_HEIGHT: u32 = 400;

fn render_chart(samples: &[IssTelemetrySample]) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; (IMG_WIDTH * IMG_HEIGHT * 3) as usize];

    {
        let root =
            BitMapBackend::with_buffer(&mut buf, (IMG_WIDTH, IMG_HEIGHT)).into_drawing_area();
        root.fill(&RGBColor(47, 49, 54))
            .context("failed to fill background")?;

        let t_min = samples.first().map(|s| s.recorded_at).unwrap_or_else(chrono::Utc::now);
        let t_max = samples.last().map(|s| s.recorded_at).unwrap_or_else(chrono::Utc::now);

        let time_range_mins = (t_max - t_min).num_minutes().max(1);
        let x_label_count = (time_range_mins / 5).clamp(2, 20) as usize;

        let mut chart = ChartBuilder::on(&root)
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(50)
            .build_cartesian_2d(t_min..t_max, 0.0..100.0)
            .context("failed to build chart")?;

        chart
            .configure_mesh()
            .x_labels(x_label_count)
            .y_labels(11)
            .y_desc("Fill %")
            .x_label_formatter(&|dt: &chrono::DateTime<chrono::Utc>| dt.format("%m/%d %H:%M").to_string())
            .y_label_formatter(&|v| format!("{v:.0}"))
            .axis_style(RGBColor(150, 150, 150))
            .label_style(("sans-serif", 14).into_font().color(&WHITE))
            .light_line_style(TRANSPARENT)
            .bold_line_style(RGBColor(90, 90, 90))
            .draw()
            .context("failed to draw mesh")?;

        let urine_color = RGBColor(255, 193, 7);
        let waste_color = RGBColor(244, 67, 54);
        let clean_color = RGBColor(33, 150, 243);

        chart
            .draw_series(LineSeries::new(
                samples.iter().map(|s| (s.recorded_at, s.urine_tank_pct)),
                urine_color.stroke_width(2),
            ))
            .context("failed to draw urine series")?
            .label("Urine Tank")
            .legend(move |(x, y)| {
                Rectangle::new([(x, y - 5), (x + 15, y + 5)], urine_color.filled())
            });

        chart
            .draw_series(LineSeries::new(
                samples.iter().map(|s| (s.recorded_at, s.waste_water_pct)),
                waste_color.stroke_width(2),
            ))
            .context("failed to draw waste series")?
            .label("Waste Water")
            .legend(move |(x, y)| {
                Rectangle::new([(x, y - 5), (x + 15, y + 5)], waste_color.filled())
            });

        chart
            .draw_series(LineSeries::new(
                samples.iter().map(|s| (s.recorded_at, s.clean_water_pct)),
                clean_color.stroke_width(2),
            ))
            .context("failed to draw clean series")?
            .label("Clean Water")
            .legend(move |(x, y)| {
                Rectangle::new([(x, y - 5), (x + 15, y + 5)], clean_color.filled())
            });

        chart
            .configure_series_labels()
            .position(SeriesLabelPosition::UpperRight)
            .background_style(RGBColor(60, 63, 68).mix(0.8))
            .border_style(RGBColor(120, 120, 120))
            .label_font(("sans-serif", 14).into_font().color(&WHITE))
            .draw()
            .context("failed to draw legend")?;

        root.present().context("failed to present chart")?;
    }

    encode_png(&buf, IMG_WIDTH, IMG_HEIGHT)
}

fn encode_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
        image::ImageEncoder::write_image(
            encoder,
            rgb,
            width,
            height,
            image::ColorType::Rgb8,
        )
        .context("failed to encode PNG")?;
    }
    Ok(png_bytes)
}

/// Show ISS water & waste telemetry history as a chart.
#[poise::command(slash_command, rename = "pisshistory")]
pub async fn pisshistory(
    ctx: Context<'_>,
    #[description = "Hours of history to show (default 24, max 168)"]
    #[min = 1]
    #[max = 168]
    hours: Option<i64>,
) -> Result<()> {
    ctx.defer().await?;

    let hours = hours.unwrap_or(24);
    let pool = &ctx.data().db;

    let samples = db::get_iss_telemetry_history(pool, hours).await?;

    if samples.is_empty() {
        ctx.say(format!(
            "📉 No telemetry data recorded in the last {hours} hour{}.",
            if hours == 1 { "" } else { "s" }
        ))
        .await?;
        return Ok(());
    }

    let image_data = render_chart(&samples)?;

    let attachment = CreateAttachment::bytes(image_data, "piss_history.png");
    let reply = poise::CreateReply::default()
        .content(format!(
            "🧑‍🚀📈 **ISS Water & Waste** — last {hours} hour{} ({} samples)",
            if hours == 1 { "" } else { "s" },
            samples.len(),
        ))
        .attachment(attachment);

    ctx.send(reply).await?;
    Ok(())
}
