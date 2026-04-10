use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use std::time::Duration;

const LIGHTSTREAMER_CREATE_SESSION_URL: &str =
    "https://push.lightstreamer.com/lightstreamer/create_session.txt?LS_protocol=TLCP-2.5.0";
const LIGHTSTREAMER_ADAPTER_SET: &str = "ISSLIVE";
const LIGHTSTREAMER_CLIENT_ID: &str = "mgQkwtwdysogQz2BJ4Ji%20kOj2Bg";
const SIGNAL_OK_STATUS_CLASS: &str = "24";

const URINE_TANK_ITEM: &str = "NODE3000005";
const URINE_TANK_FIELD: &str = "Value";
const URINE_PROCESSOR_ITEM: &str = "NODE3000004";
const URINE_PROCESSOR_FIELD: &str = "Value";
const WASTE_WATER_ITEM: &str = "NODE3000008";
const WASTE_WATER_FIELD: &str = "Value";
const CLEAN_WATER_ITEM: &str = "NODE3000009";
const CLEAN_WATER_FIELD: &str = "Value";
const SIGNAL_STATUS_ITEM: &str = "TIME_000001";
const SIGNAL_STATUS_FIELD: &str = "Status.Class";

#[derive(Debug, Clone, PartialEq)]
pub struct IssUrineTelemetry {
    pub tank_percentage: f64,
    pub waste_water_percentage: f64,
    pub clean_water_percentage: f64,
    pub processor_status: String,
    pub signal_acquired: bool,
}

pub async fn fetch_iss_urine_telemetry() -> Result<IssUrineTelemetry> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to build telemetry HTTP client")?;

    let tank_raw = fetch_lightstreamer_snapshot(&client, URINE_TANK_ITEM, URINE_TANK_FIELD).await?;
    let processor_raw =
        fetch_lightstreamer_snapshot(&client, URINE_PROCESSOR_ITEM, URINE_PROCESSOR_FIELD).await?;
    let waste_water_raw =
        fetch_lightstreamer_snapshot(&client, WASTE_WATER_ITEM, WASTE_WATER_FIELD).await?;
    let clean_water_raw =
        fetch_lightstreamer_snapshot(&client, CLEAN_WATER_ITEM, CLEAN_WATER_FIELD).await?;
    let signal_raw = fetch_lightstreamer_snapshot(&client, SIGNAL_STATUS_ITEM, SIGNAL_STATUS_FIELD).await?;

    let tank_percentage = tank_raw
        .parse::<f64>()
        .with_context(|| format!("invalid urine tank percentage: {tank_raw}"))?;
    let waste_water_percentage = waste_water_raw
        .parse::<f64>()
        .with_context(|| format!("invalid waste water percentage: {waste_water_raw}"))?;
    let clean_water_percentage = clean_water_raw
        .parse::<f64>()
        .with_context(|| format!("invalid clean water percentage: {clean_water_raw}"))?;

    Ok(IssUrineTelemetry {
        tank_percentage,
        waste_water_percentage,
        clean_water_percentage,
        processor_status: processor_status_label(&processor_raw),
        signal_acquired: signal_raw == SIGNAL_OK_STATUS_CLASS,
    })
}

async fn fetch_lightstreamer_snapshot(client: &Client, item: &str, field: &str) -> Result<String> {
    let body = format!(
        "LS_user=&LS_adapter_set={LIGHTSTREAMER_ADAPTER_SET}&LS_cid={LIGHTSTREAMER_CLIENT_ID}&LS_op=add&LS_subId=1&LS_group={item}&LS_schema={field}&LS_mode=MERGE&LS_snapshot=true&LS_polling=true&LS_polling_millis=5000"
    );

    let response = client
        .post(LIGHTSTREAMER_CREATE_SESSION_URL)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded; charset=utf-8")
        .body(body)
        .send()
        .await
        .with_context(|| format!("failed to fetch Lightstreamer snapshot for {item}/{field}"))?;

    let response = response
        .error_for_status()
        .with_context(|| format!("Lightstreamer snapshot request failed for {item}/{field}"))?;

    let payload = response
        .text()
        .await
        .with_context(|| format!("failed to read Lightstreamer response for {item}/{field}"))?;

    // If we got a LOOP directive without a U, line, the server wants us to
    // rebind.  Extract the session ID and poll the bind endpoint once.
    if !payload.lines().any(|l| l.starts_with("U,")) {
        let session_id = parse_session_id(&payload)
            .with_context(|| format!("no update and no session ID for {item}/{field}"))?;

        let bind_url = format!(
            "https://push.lightstreamer.com/lightstreamer/bind_session.txt?LS_protocol=TLCP-2.5.0"
        );
        let bind_body = format!(
            "LS_session={session_id}&LS_polling=true&LS_polling_millis=5000"
        );

        let bind_resp = client
            .post(&bind_url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded; charset=utf-8")
            .body(bind_body)
            .send()
            .await
            .with_context(|| format!("failed to bind session for {item}/{field}"))?
            .error_for_status()
            .with_context(|| format!("bind session request failed for {item}/{field}"))?
            .text()
            .await
            .with_context(|| format!("failed to read bind response for {item}/{field}"))?;

        return parse_lightstreamer_update(&bind_resp)
            .with_context(|| format!("failed to parse bind response for {item}/{field}"));
    }

    parse_lightstreamer_update(&payload)
        .with_context(|| format!("failed to parse Lightstreamer response for {item}/{field}"))
}

fn parse_session_id(payload: &str) -> Result<String> {
    let conok_line = payload
        .lines()
        .find(|line| line.starts_with("CONOK,"))
        .ok_or_else(|| anyhow!("missing CONOK line"))?;

    let parts: Vec<&str> = conok_line.split(',').collect();
    if parts.len() < 2 {
        bail!("malformed CONOK line: {conok_line}");
    }

    Ok(parts[1].to_string())
}

fn parse_lightstreamer_update(payload: &str) -> Result<String> {
    let update_line = payload
        .lines()
        .find(|line| line.starts_with("U,"))
        .ok_or_else(|| anyhow!("missing update line"))?;

    let parts: Vec<&str> = update_line.split(',').collect();
    if parts.len() <= 3 {
        bail!("malformed update line: {update_line}");
    }

    Ok(parts[3].trim().to_string())
}

fn processor_status_label(raw_status: &str) -> String {
    match raw_status.parse::<u32>() {
        Ok(2) => "stopped".to_string(),
        Ok(4) => "shutdown".to_string(),
        Ok(8) => "maintenance".to_string(),
        Ok(16) => "operating".to_string(),
        Ok(32) => "standby".to_string(),
        Ok(64) => "idle".to_string(),
        Ok(128) => "initializing".to_string(),
        Ok(other) => format!("unknown ({other})"),
        Err(_) => format!("unknown ({raw_status})"),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_lightstreamer_update, processor_status_label};

    #[test]
    fn parses_update_line_value() {
        let payload = "OK\nSessionId:abc\nU,1,1,27.00\n";
        let value = parse_lightstreamer_update(payload).expect("expected update value");
        assert_eq!(value, "27.00");
    }

    #[test]
    fn maps_processor_status_codes() {
        assert_eq!(processor_status_label("16"), "operating");
        assert_eq!(processor_status_label("32"), "standby");
        assert_eq!(processor_status_label("999"), "unknown (999)");
    }
}