use crate::Context;
use crate::db;
use crate::ui_import::{
    INPUT_FORMAT_INDIVIDUAL_UI_EXPORT, ImportedIndividualSim, MOP_UPSTREAM_REVISION,
    protojson_message_to_value,
};
use anyhow::{Context as _, Result, anyhow, bail};
use poise::serenity_prelude as serenity;
use uuid::Uuid;

const MAX_UI_EXPORT_BYTES: usize = 8 * 1024 * 1024;

async fn read_ui_export(
    inline_export: Option<String>,
    export_file: Option<serenity::Attachment>,
) -> Result<String> {
    match (inline_export, export_file) {
        (Some(_), Some(_)) => {
            bail!("provide either the `export_json` text or a `.json` attachment, not both")
        }
        (Some(export), None) => {
            if export.trim().is_empty() {
                bail!("the `export_json` text cannot be empty");
            }
            if export.len() > MAX_UI_EXPORT_BYTES {
                bail!(
                    "the `export_json` text exceeds the {} MiB limit; upload it as a `.json` attachment instead",
                    MAX_UI_EXPORT_BYTES / (1024 * 1024)
                );
            }
            Ok(export)
        }
        (None, Some(attachment)) => {
            if !attachment.filename.to_ascii_lowercase().ends_with(".json") {
                bail!("the attached export must have a `.json` filename");
            }
            if attachment.size as usize > MAX_UI_EXPORT_BYTES {
                bail!(
                    "the attached export exceeds the {} MiB limit",
                    MAX_UI_EXPORT_BYTES / (1024 * 1024)
                );
            }

            let bytes = attachment
                .download()
                .await
                .context("failed to download the attached WoWSims export")?;
            if bytes.len() > MAX_UI_EXPORT_BYTES {
                bail!(
                    "the attached export exceeds the {} MiB limit",
                    MAX_UI_EXPORT_BYTES / (1024 * 1024)
                );
            }

            String::from_utf8(bytes)
                .map_err(|_| anyhow!("the attached export must be UTF-8 encoded JSON"))
        }
        (None, None) => {
            bail!("provide a WoWSims individual UI JSON export as text or a `.json` attachment")
        }
    }
}

fn format_metric(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "n/a".to_string()
    }
}

/// Run a World of Warcraft simulation from a complete WoWSims individual UI JSON export.
///
/// Usage: `/sim export_json:<json>` or `/sim export_file:<attachment.json>`
#[poise::command(slash_command, rename = "sim")]
pub async fn sim(
    ctx: Context<'_>,
    #[description = "Complete WoWSims individual UI JSON export"] export_json: Option<String>,
    #[description = "Complete WoWSims individual UI JSON export (.json)"] export_file: Option<
        serenity::Attachment,
    >,
) -> Result<()> {
    let source_text = match read_ui_export(export_json, export_file).await {
        Ok(source_text) => source_text,
        Err(error) => {
            ctx.say(format!("❌ {error:#}")).await?;
            return Ok(());
        }
    };
    let source_payload = match serde_json::from_str(&source_text) {
        Ok(payload) => payload,
        Err(error) => {
            ctx.say(format!(
                "❌ The WoWSims export is not valid JSON: {}",
                error
            ))
            .await?;
            return Ok(());
        }
    };
    let imported = match ImportedIndividualSim::from_json(&source_payload) {
        Ok(imported) => imported,
        Err(error) => {
            ctx.say(format!(
                "❌ Invalid WoWSims individual UI export: {error:#}"
            ))
            .await?;
            return Ok(());
        }
    };
    let run_id = Uuid::new_v4();
    let normalized = match imported.normalize(run_id) {
        Ok(normalized) => normalized,
        Err(error) => {
            ctx.say(format!(
                "❌ Could not normalize the WoWSims UI export: {error:#}"
            ))
            .await?;
            return Ok(());
        }
    };
    let normalized_request =
        protojson_message_to_value("proto.RaidSimRequest", &normalized.request)?;

    let user_id = ctx.author().id.to_string();
    let user_id_for_reply = ctx.author().id;
    let channel_id = ctx.channel_id();
    let http = ctx.serenity_context().http.clone();
    let pool = &ctx.data().db;

    db::create_simulation_run(
        pool,
        &db::NewSimulationRun {
            run_id,
            discord_user_id: &user_id,
            class: &normalized.class,
            spec: &normalized.spec,
            source_payload: &source_payload,
            input_format: INPUT_FORMAT_INDIVIDUAL_UI_EXPORT,
            upstream_revision: Some(MOP_UPSTREAM_REVISION),
            normalized_request: Some(&normalized_request),
            effective_random_seed: Some(normalized.effective_random_seed),
            effective_iterations: Some(normalized.effective_iterations),
        },
    )
    .await?;

    let pool_for_task = pool.clone();
    let sim_api_base_url = ctx.data().sim_api_base_url.clone();
    tokio::spawn(async move {
        if let Err(err) = crate::sim_runtime::run_async_simulation(
            pool_for_task.clone(),
            sim_api_base_url,
            run_id,
        )
        .await
        {
            tracing::error!(run_id = %run_id, error = ?err, "async simulation failed");
            let _ = db::update_simulation_run_status(&pool_for_task, run_id, "failed").await;
        }

        let completion_message = match db::get_simulation_run(&pool_for_task, run_id).await {
            Ok(Some(run)) => {
                let mention = format!("<@{}>", user_id_for_reply.get());

                let progress_line =
                    match db::get_latest_simulation_progress_frame(&pool_for_task, run_id).await {
                        Ok(Some(frame)) if frame.total_iterations > 0 => {
                            let dps = format_metric(frame.dps);
                            let hps = format_metric(frame.hps);
                            format!(
                                "• Final Progress: **{}/{} iterations** ({:.1}%) | DPS {} | HPS {}",
                                frame.completed_iterations,
                                frame.total_iterations,
                                (frame.completed_iterations as f64 / frame.total_iterations as f64)
                                    * 100.0,
                                dps,
                                hps,
                            )
                        }
                        Ok(Some(frame)) => format!(
                            "• Final Progress: frame #{}, sims {}/{}",
                            frame.frame_index, frame.completed_sims, frame.total_sims
                        ),
                        Ok(None) => "• Final Progress: no frames recorded".to_string(),
                        Err(_) => "• Final Progress: unavailable".to_string(),
                    };

                let status_emoji = if run.status == "complete" {
                    "✅"
                } else {
                    "❌"
                };
                let status_label = if run.status == "complete" {
                    "Complete"
                } else {
                    "Failed"
                };

                format!(
                    "{status_emoji} {mention} sim **{class}/{spec}**: **{status_label}**.\n\
                     {progress_line}\n\
                     • Run ID: `{run_id}`",
                    class = run.class,
                    spec = run.spec,
                )
            }
            Ok(None) => format!(
                "❌ <@{}> sim `{run_id}` finished, but details were not found.",
                user_id_for_reply.get()
            ),
            Err(_) => format!(
                "❌ <@{}> sim `{run_id}` finished, but I couldn't load final status.",
                user_id_for_reply.get()
            ),
        };

        if let Err(error) = channel_id.say(&http, completion_message).await {
            tracing::warn!(run_id = %run_id, error = ?error, "failed to send simulation completion message");
        }
    });

    // Acknowledge quickly so Discord doesn't time out
    ctx.say(format!(
        "✅ Got your WoWSims UI export for **{class}/{spec}**!\n\
         • Iterations: **{iterations}**\n\
         • Seed: `{seed}`\n\
         • Run ID: `{run_id}`",
        class = normalized.class,
        spec = normalized.spec,
        iterations = normalized.effective_iterations,
        seed = normalized.effective_random_seed,
    ))
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_empty_inline_export() {
        let error = read_ui_export(Some(" \n\t".to_string()), None)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn accepts_non_empty_inline_export() {
        let export = r#"{"apiVersion":15}"#.to_string();

        let received = read_ui_export(Some(export.clone()), None)
            .await
            .expect("inline export should be accepted");

        assert_eq!(received, export);
    }
}
