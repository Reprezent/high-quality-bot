use anyhow::{Context as _, Result, anyhow, bail};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(30);
const REPORT_PAGE_LIMIT: i32 = 100;
const MAX_REPORT_PAGES: i32 = 100;

const GUILD_LOOKUP_QUERY: &str = r#"
query GuildLookup($name: String!, $serverSlug: String!, $serverRegion: String!) {
  guildData {
    guild(name: $name, serverSlug: $serverSlug, serverRegion: $serverRegion) {
      id
      name
      server {
        name
        slug
        region { compactName slug }
      }
    }
  }
}
"#;

const GUILD_BY_ID_QUERY: &str = r#"
query GuildById($id: Int!) {
  guildData {
    guild(id: $id) {
      id
      name
      server {
        name
        slug
        region { compactName slug }
      }
    }
  }
}
"#;

const GUILD_REPORTS_QUERY: &str = r#"
query GuildReports(
  $guildID: Int!,
  $startTime: Float,
  $endTime: Float,
  $page: Int!,
  $limit: Int!
) {
  reportData {
    reports(
      guildID: $guildID,
      startTime: $startTime,
      endTime: $endTime,
      page: $page,
      limit: $limit
    ) {
      current_page
      has_more_pages
      data {
        code
        title
        startTime
        endTime
        revision
        visibility
        zone { name }
      }
    }
  }
  rateLimitData {
    limitPerHour
    pointsSpentThisHour
    pointsResetIn
  }
}
"#;

const REPORT_FIGHTS_QUERY: &str = r#"
query ReportFights($code: String!) {
  reportData {
    report(code: $code) {
      code
      title
      startTime
      endTime
      revision
      guild { name }
      fights(killType: Encounters) {
        id
        name
        encounterID
        kill
        inProgress
        difficulty
        size
        startTime
        endTime
        averageItemLevel
      }
    }
  }
}
"#;

const KILL_SUMMARY_QUERY: &str = r#"
query KillSummary($code: String!, $fightID: Int!) {
  reportData {
    report(code: $code) {
      summary: table(dataType: Summary, fightIDs: [$fightID])
      damage: table(dataType: DamageDone, fightIDs: [$fightID], viewBy: Source)
      healing: table(dataType: Healing, fightIDs: [$fightID], viewBy: Source)
      deaths: table(dataType: Deaths, fightIDs: [$fightID])
    }
  }
}
"#;

#[derive(Clone, Debug)]
pub struct WarcraftLogsConfig {
    pub client_id: String,
    pub client_secret: String,
    pub poll_interval: Duration,
}

impl WarcraftLogsConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let client_id = optional_env("WARCRAFT_LOGS_CLIENT_ID")?;
        let client_secret = optional_env("WARCRAFT_LOGS_CLIENT_SECRET")?;

        let (client_id, client_secret) = match (client_id, client_secret) {
            (None, None) => return Ok(None),
            (Some(client_id), Some(client_secret)) => (client_id, client_secret),
            _ => bail!(
                "WARCRAFT_LOGS_CLIENT_ID and WARCRAFT_LOGS_CLIENT_SECRET must either both be set or both be omitted"
            ),
        };

        let poll_interval_secs = match optional_env("WARCRAFT_LOGS_POLL_INTERVAL_SECS")? {
            Some(value) => value
                .parse::<u64>()
                .context("WARCRAFT_LOGS_POLL_INTERVAL_SECS must be an integer")?
                .max(30),
            None => 60,
        };

        Ok(Some(Self {
            client_id,
            client_secret,
            poll_interval: Duration::from_secs(poll_interval_secs),
        }))
    }
}

fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WarcraftLogsSite {
    Retail,
    Classic,
}

impl WarcraftLogsSite {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Retail => "retail",
            Self::Classic => "classic",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Retail => "Retail",
            Self::Classic => "Classic",
        }
    }

    pub const fn host(self) -> &'static str {
        match self {
            Self::Retail => "www.warcraftlogs.com",
            Self::Classic => "classic.warcraftlogs.com",
        }
    }

    const fn oauth_url(self) -> &'static str {
        match self {
            Self::Retail => "https://www.warcraftlogs.com/oauth/token",
            Self::Classic => "https://classic.warcraftlogs.com/oauth/token",
        }
    }

    const fn graphql_url(self) -> &'static str {
        match self {
            Self::Retail => "https://www.warcraftlogs.com/api/v2/client",
            Self::Classic => "https://classic.warcraftlogs.com/api/v2/client",
        }
    }

    pub fn from_slug(value: &str) -> Result<Self> {
        match value {
            "retail" => Ok(Self::Retail),
            "classic" => Ok(Self::Classic),
            _ => bail!("unsupported Warcraft Logs site {value:?}"),
        }
    }

    pub fn from_host(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "warcraftlogs.com" | "www.warcraftlogs.com" => Some(Self::Retail),
            "classic.warcraftlogs.com" => Some(Self::Classic),
            _ => None,
        }
    }

    pub fn report_url(self, code: &str) -> String {
        format!("https://{}/reports/{code}", self.host())
    }
}

#[derive(Debug)]
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

#[derive(Debug)]
struct ClientInner {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    oauth_url_override: Option<String>,
    graphql_url_override: Option<String>,
    tokens: RwLock<HashMap<WarcraftLogsSite, CachedToken>>,
}

#[derive(Clone, Debug)]
pub struct WarcraftLogsClient {
    inner: Arc<ClientInner>,
}

impl WarcraftLogsClient {
    pub fn new(config: &WarcraftLogsConfig) -> Result<Self> {
        Self::build(config, None, None)
    }

    fn with_endpoints(
        config: &WarcraftLogsConfig,
        oauth_url: &str,
        graphql_url: &str,
    ) -> Result<Self> {
        Self::build(
            config,
            Some(oauth_url.to_owned()),
            Some(graphql_url.to_owned()),
        )
    }

    fn build(
        config: &WarcraftLogsConfig,
        oauth_url_override: Option<String>,
        graphql_url_override: Option<String>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build Warcraft Logs HTTP client")?;

        tracing::debug!(
            "Warcraft Logs client configured with client_id={:?}, oauth_url_override={:?}, graphql_url_override={:?}",
            config.client_id,
            oauth_url_override,
            graphql_url_override
        );

        Ok(Self {
            inner: Arc::new(ClientInner {
                http,
                client_id: config.client_id.clone(),
                client_secret: config.client_secret.clone(),
                oauth_url_override,
                graphql_url_override,
                tokens: RwLock::new(HashMap::new()),
            }),
        })
    }

    pub async fn lookup_guild(
        &self,
        site: WarcraftLogsSite,
        name: &str,
        server_slug: &str,
        region: &str,
    ) -> Result<Option<WarcraftLogsGuild>> {
        tracing::debug!("Attempting to lookup guild {name} on {server_slug}.{region} at {site:?}");
        let data: GuildLookupData = self
            .graphql(
                site,
                GUILD_LOOKUP_QUERY,
                GuildLookupVariables {
                    name,
                    server_slug,
                    server_region: region,
                },
            )
            .await?;

        tracing::debug!("Got guild {:?} from Warcraft Logs", data.guild_data.guild);
        Ok(data.guild_data.guild)
    }

    pub async fn lookup_guild_by_id(
        &self,
        site: WarcraftLogsSite,
        guild_id: i64,
    ) -> Result<Option<WarcraftLogsGuild>> {
        tracing::debug!("Attempting to lookup guild with id {guild_id} at {site:?}");
        let data: GuildLookupData = self
            .graphql(site, GUILD_BY_ID_QUERY, GuildByIdVariables { id: guild_id })
            .await?;

        tracing::debug!("Got guild {:?} from Warcraft Logs", data.guild_data.guild);
        Ok(data.guild_data.guild)
    }

    pub async fn reports_since(
        &self,
        site: WarcraftLogsSite,
        guild_id: i64,
        start_time_ms: i64,
        end_time_ms: i64,
    ) -> Result<ReportDiscovery> {
        let mut page = 1;
        let mut reports = Vec::new();

        let rate_limit = loop {
            tracing::debug!(
                "Attempting to fetch reports for guild {guild_id} at {site:?}, page {page}. Time window: {start_time_ms} to {end_time_ms}"
            );
            let data: GuildReportsData = self
                .graphql(
                    site,
                    GUILD_REPORTS_QUERY,
                    GuildReportsVariables {
                        guild_id,
                        start_time: Some(start_time_ms as f64),
                        end_time: Some(end_time_ms as f64),
                        page,
                        limit: REPORT_PAGE_LIMIT,
                    },
                )
                .await?;
            tracing::debug!("Done fetching reports for guild {guild_id} at {site:?}.");
            let report_page = data.report_data.reports;
            reports.extend(report_page.data);

            if !report_page.has_more_pages {
                break data.rate_limit_data;
            }
            if page >= MAX_REPORT_PAGES {
                bail!("Warcraft Logs report pagination exceeded {MAX_REPORT_PAGES} pages");
            }
            page += 1;
        };

        Ok(ReportDiscovery {
            reports,
            rate_limit,
        })
    }

    pub async fn recent_reports(
        &self,
        site: WarcraftLogsSite,
        guild_id: i64,
        limit: i32,
    ) -> Result<ReportDiscovery> {
        let data: GuildReportsData = self
            .graphql(
                site,
                GUILD_REPORTS_QUERY,
                GuildReportsVariables {
                    guild_id,
                    start_time: None,
                    end_time: None,
                    page: 1,
                    limit: limit.clamp(1, REPORT_PAGE_LIMIT),
                },
            )
            .await?;
        let mut reports = data.report_data.reports.data;
        reports.sort_by(|left, right| right.start_time.total_cmp(&left.start_time));
        reports.truncate(limit.max(1) as usize);

        Ok(ReportDiscovery {
            reports,
            rate_limit: data.rate_limit_data,
        })
    }

    pub async fn report_fights(
        &self,
        site: WarcraftLogsSite,
        code: &str,
    ) -> Result<WarcraftLogsReportDetails> {
        let data: ReportFightsData = self
            .graphql(site, REPORT_FIGHTS_QUERY, ReportCodeVariables { code })
            .await?;

        data.report_data
            .report
            .ok_or_else(|| anyhow!("Warcraft Logs report {code} was not found"))
    }

    pub async fn kill_summary(
        &self,
        site: WarcraftLogsSite,
        code: &str,
        fight_id: i32,
    ) -> Result<KillSummary> {
        let data: KillSummaryData = self
            .graphql(
                site,
                KILL_SUMMARY_QUERY,
                KillSummaryVariables { code, fight_id },
            )
            .await?;
        let report = data
            .report_data
            .report
            .ok_or_else(|| anyhow!("Warcraft Logs report {code} was not found"))?;

        Ok(KillSummary {
            top_damage: parse_metric_entries(&report.damage, "damage")?,
            top_healing: parse_metric_entries(&report.healing, "healing")?,
            deaths: parse_death_count(&report.summary, &report.deaths)?,
        })
    }

    async fn graphql<V, D>(&self, site: WarcraftLogsSite, query: &str, variables: V) -> Result<D>
    where
        V: Serialize,
        D: DeserializeOwned,
    {
        let request = GraphQlRequest { query, variables };
        let graphql_url = self
            .inner
            .graphql_url_override
            .as_deref()
            .unwrap_or_else(|| site.graphql_url());

        for attempt in 0..=1 {
            let token = self.access_token(site).await?;
            let response = self
                .inner
                .http
                .post(graphql_url)
                .bearer_auth(token)
                .header(reqwest::header::ACCEPT, "application/json")
                .json(&request)
                .send()
                .await
                .context("failed to call Warcraft Logs GraphQL API")?;

            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                self.inner.tokens.write().await.remove(&site);
                continue;
            }

            let status = response.status();
            let body = response
                .text()
                .await
                .context("failed to read Warcraft Logs GraphQL response")?;
            if !status.is_success() {
                bail!("Warcraft Logs GraphQL API returned HTTP {status}: {body}");
            }

            let response: GraphQlResponse<D> = serde_json::from_str(&body)
                .context("failed to decode Warcraft Logs GraphQL response")?;
            if !response.errors.is_empty() {
                let messages = response
                    .errors
                    .iter()
                    .map(|error| error.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                bail!("Warcraft Logs GraphQL error: {messages}");
            }

            return response
                .data
                .ok_or_else(|| anyhow!("Warcraft Logs GraphQL response contained no data"));
        }

        unreachable!("GraphQL authentication retries are bounded")
    }

    async fn access_token(&self, site: WarcraftLogsSite) -> Result<String> {
        {
            let cached = self.inner.tokens.read().await;
            if let Some(token) = cached.get(&site)
                && token.expires_at > Instant::now() + TOKEN_EXPIRY_MARGIN
            {
                return Ok(token.access_token.clone());
            }
        }

        let mut cached = self.inner.tokens.write().await;
        if let Some(token) = cached.get(&site)
            && token.expires_at > Instant::now() + TOKEN_EXPIRY_MARGIN
        {
            return Ok(token.access_token.clone());
        }

        let oauth_url = self
            .inner
            .oauth_url_override
            .as_deref()
            .unwrap_or_else(|| site.oauth_url());
        let response = self
            .inner
            .http
            .post(oauth_url)
            .basic_auth(&self.inner.client_id, Some(&self.inner.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .context("failed to request Warcraft Logs OAuth token")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read Warcraft Logs OAuth response")?;
        if !status.is_success() {
            bail!("Warcraft Logs OAuth returned HTTP {status}: {body}");
        }

        let token: OAuthTokenResponse =
            serde_json::from_str(&body).context("failed to decode Warcraft Logs OAuth response")?;
        if token.access_token.is_empty() {
            bail!("Warcraft Logs OAuth returned an empty access token");
        }

        let expires_in = Duration::from_secs(token.expires_in.max(1));
        let access_token = token.access_token;
        cached.insert(
            site,
            CachedToken {
                access_token: access_token.clone(),
                expires_at: Instant::now() + expires_in,
            },
        );

        Ok(access_token)
    }
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Serialize)]
struct GraphQlRequest<'a, V> {
    query: &'a str,
    variables: V,
}

#[derive(Deserialize)]
struct GraphQlResponse<D> {
    data: Option<D>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuildLookupVariables<'a> {
    name: &'a str,
    server_slug: &'a str,
    server_region: &'a str,
}

#[derive(Serialize)]
struct GuildByIdVariables {
    id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuildLookupData {
    guild_data: GuildLookupContainer,
}

#[derive(Deserialize)]
struct GuildLookupContainer {
    guild: Option<WarcraftLogsGuild>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WarcraftLogsGuild {
    pub id: i64,
    pub name: String,
    pub server: WarcraftLogsServer,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WarcraftLogsServer {
    pub name: String,
    pub slug: String,
    pub region: WarcraftLogsRegion,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarcraftLogsRegion {
    pub compact_name: String,
    pub slug: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuildReportsVariables {
    #[serde(rename = "guildID")]
    guild_id: i64,
    start_time: Option<f64>,
    end_time: Option<f64>,
    page: i32,
    limit: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuildReportsData {
    report_data: ReportPaginationContainer,
    rate_limit_data: RateLimitInfo,
}

#[derive(Deserialize)]
struct ReportPaginationContainer {
    reports: ReportPage,
}

#[derive(Deserialize)]
struct ReportPage {
    #[allow(dead_code)]
    current_page: i32,
    has_more_pages: bool,
    data: Vec<WarcraftLogsReport>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarcraftLogsReport {
    pub code: String,
    pub title: String,
    pub start_time: f64,
    pub end_time: Option<f64>,
    pub revision: i32,
    pub visibility: String,
    pub zone: Option<WarcraftLogsZone>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WarcraftLogsZone {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct ReportDiscovery {
    pub reports: Vec<WarcraftLogsReport>,
    pub rate_limit: RateLimitInfo,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    pub limit_per_hour: f64,
    pub points_spent_this_hour: f64,
    pub points_reset_in: f64,
}

impl RateLimitInfo {
    pub fn remaining(&self) -> f64 {
        (self.limit_per_hour - self.points_spent_this_hour).max(0.0)
    }

    pub fn recommended_delay(&self, normal: Duration) -> Duration {
        if !self.limit_per_hour.is_finite()
            || !self.points_spent_this_hour.is_finite()
            || !self.points_reset_in.is_finite()
            || self.limit_per_hour <= 0.0
            || self.remaining() > self.limit_per_hour / 10.0
        {
            return normal;
        }

        let delay_secs = self.points_reset_in.max(1.0) / self.remaining().max(1.0);
        if delay_secs > Duration::MAX.as_secs_f64() {
            return Duration::MAX;
        }

        normal.max(Duration::from_secs_f64(delay_secs))
    }
}

#[derive(Serialize)]
struct ReportCodeVariables<'a> {
    code: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportFightsData {
    report_data: ReportDetailsContainer,
}

#[derive(Deserialize)]
struct ReportDetailsContainer {
    report: Option<WarcraftLogsReportDetails>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarcraftLogsReportDetails {
    pub code: String,
    pub title: String,
    pub start_time: f64,
    pub end_time: Option<f64>,
    pub revision: i32,
    pub guild: Option<WarcraftLogsReportGuild>,
    pub fights: Vec<WarcraftLogsFight>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WarcraftLogsReportGuild {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarcraftLogsFight {
    pub id: i32,
    pub name: String,
    #[serde(rename = "encounterID")]
    pub encounter_id: i32,
    pub kill: bool,
    pub in_progress: bool,
    pub difficulty: Option<i32>,
    pub size: Option<i32>,
    pub start_time: f64,
    pub end_time: f64,
    pub average_item_level: Option<f64>,
}

impl WarcraftLogsFight {
    pub fn is_completed_boss_kill(&self) -> bool {
        self.encounter_id != 0 && self.kill && !self.in_progress
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KillSummaryVariables<'a> {
    code: &'a str,
    #[serde(rename = "fightID")]
    fight_id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KillSummaryData {
    report_data: KillSummaryContainer,
}

#[derive(Deserialize)]
struct KillSummaryContainer {
    report: Option<KillSummaryTables>,
}

#[derive(Deserialize)]
struct KillSummaryTables {
    summary: Value,
    damage: Value,
    healing: Value,
    deaths: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricEntry {
    pub name: String,
    pub total: f64,
    pub class_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KillSummary {
    pub top_damage: Option<Vec<MetricEntry>>,
    pub top_healing: Option<Vec<MetricEntry>>,
    pub deaths: Option<u64>,
}

fn parse_metric_entries(table: &Value, label: &str) -> Result<Option<Vec<MetricEntry>>> {
    if table.is_null() {
        return Ok(None);
    }

    let entries = table
        .pointer("/data/entries")
        .or_else(|| table.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Warcraft Logs {label} table did not contain an entries array"))?;

    let mut parsed = entries
        .iter()
        .filter_map(|entry| {
            Some(MetricEntry {
                name: entry.get("name")?.as_str()?.to_owned(),
                total: entry.get("total")?.as_f64()?,
                class_name: entry
                    .get("icon")
                    .and_then(Value::as_str)
                    .and_then(|icon| icon.split('-').next())
                    .or_else(|| entry.get("type").and_then(Value::as_str))
                    .map(str::to_owned),
            })
        })
        .collect::<Vec<_>>();
    parsed.sort_by(|left, right| right.total.total_cmp(&left.total));
    parsed.truncate(3);

    Ok(Some(parsed))
}

fn parse_death_count(summary: &Value, deaths: &Value) -> Result<Option<u64>> {
    if let Some(events) = summary
        .pointer("/data/deathEvents")
        .or_else(|| summary.get("deathEvents"))
        .and_then(Value::as_array)
    {
        return Ok(Some(events.len() as u64));
    }

    if deaths.is_null() {
        return Ok(None);
    }

    let entries = deaths
        .pointer("/data/entries")
        .or_else(|| deaths.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Warcraft Logs deaths table did not contain an entries array"))?;

    let mut explicit_count = 0_u64;
    let mut found_explicit_count = false;
    for entry in entries {
        for key in ["deathCount", "deaths"] {
            if let Some(count) = entry.get(key).and_then(Value::as_u64) {
                explicit_count += count;
                found_explicit_count = true;
                break;
            }
        }
        if let Some(events) = entry.get("deathEvents").and_then(Value::as_array) {
            explicit_count += events.len() as u64;
            found_explicit_count = true;
        }
    }

    Ok(Some(if found_explicit_count {
        explicit_count
    } else {
        entries.len() as u64
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        GuildReportsVariables, KillSummaryVariables, RateLimitInfo, WarcraftLogsClient,
        WarcraftLogsConfig, WarcraftLogsFight, WarcraftLogsSite, parse_death_count,
        parse_metric_entries,
    };
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    #[test]
    fn recognizes_only_completed_encounter_kills() {
        let fight = WarcraftLogsFight {
            id: 1,
            name: "Boss".to_owned(),
            encounter_id: 42,
            kill: true,
            in_progress: false,
            difficulty: Some(5),
            size: Some(20),
            start_time: 1_000.0,
            end_time: 61_000.0,
            average_item_level: Some(700.0),
        };

        assert!(fight.is_completed_boss_kill());
        assert!(
            !WarcraftLogsFight {
                kill: false,
                ..fight.clone()
            }
            .is_completed_boss_kill()
        );
        assert!(
            !WarcraftLogsFight {
                encounter_id: 0,
                ..fight.clone()
            }
            .is_completed_boss_kill()
        );
        assert!(
            !WarcraftLogsFight {
                in_progress: true,
                ..fight
            }
            .is_completed_boss_kill()
        );
    }

    #[test]
    fn routes_retail_and_classic_to_matching_hosts() {
        assert_eq!(
            WarcraftLogsSite::Retail.graphql_url(),
            "https://www.warcraftlogs.com/api/v2/client"
        );
        assert_eq!(
            WarcraftLogsSite::Classic.graphql_url(),
            "https://classic.warcraftlogs.com/api/v2/client"
        );
        assert_eq!(
            WarcraftLogsSite::Classic.oauth_url(),
            "https://classic.warcraftlogs.com/oauth/token"
        );
        assert_eq!(
            WarcraftLogsSite::Classic.report_url("abc"),
            "https://classic.warcraftlogs.com/reports/abc"
        );
    }

    #[test]
    fn serializes_graphql_id_variables_with_exact_acronym_case() {
        let reports = serde_json::to_value(GuildReportsVariables {
            guild_id: 42,
            start_time: None,
            end_time: None,
            page: 1,
            limit: 3,
        })
        .unwrap();
        assert_eq!(reports["guildID"], 42);
        assert!(reports.get("guildId").is_none());

        let summary = serde_json::to_value(KillSummaryVariables {
            code: "abc",
            fight_id: 7,
        })
        .unwrap();
        assert_eq!(summary["fightID"], 7);
        assert!(summary.get("fightId").is_none());
    }

    #[test]
    fn parses_and_sorts_top_metric_entries() {
        let table = json!({
            "data": {
                "entries": [
                    {"name": "Third", "total": 30.0, "icon": "Priest-Discipline"},
                    {"name": "First", "total": 100.0, "type": "Mage"},
                    {"name": "Fourth", "total": 10.0},
                    {"name": "Second", "total": 50.0}
                ]
            }
        });

        let entries = parse_metric_entries(&table, "damage").unwrap().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second", "Third"]
        );
        assert_eq!(entries[0].class_name.as_deref(), Some("Mage"));
        assert_eq!(entries[2].class_name.as_deref(), Some("Priest"));
    }

    #[test]
    fn parses_deaths_from_summary_events() {
        let summary = json!({"data": {"deathEvents": [{}, {}, {}]}});
        assert_eq!(parse_death_count(&summary, &Value::Null).unwrap(), Some(3));
    }

    #[test]
    fn increases_delay_when_hourly_points_are_low() {
        let rate_limit = RateLimitInfo {
            limit_per_hour: 100.0,
            points_spent_this_hour: 99.0,
            points_reset_in: 600.0,
        };

        assert_eq!(
            rate_limit.recommended_delay(Duration::from_secs(60)),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn rejects_non_null_metric_tables_with_unknown_shapes() {
        let error = parse_metric_entries(&json!({"unexpected": []}), "damage").unwrap_err();
        assert!(error.to_string().contains("entries array"));
        assert_eq!(parse_metric_entries(&Value::Null, "damage").unwrap(), None);
    }

    #[tokio::test]
    async fn paginates_reports_and_reuses_oauth_token() {
        let responses = vec![
            json!({
                "access_token": "test-token",
                "expires_in": 3600
            })
            .to_string(),
            json!({
                "data": {
                    "reportData": {
                        "reports": {
                            "current_page": 1,
                            "has_more_pages": true,
                            "data": [{
                                "code": "first",
                                "title": "First",
                                "startTime": 1000.0,
                                "endTime": 2000.0,
                                "revision": 0,
                                "visibility": "public",
                                "zone": {"name": "Test Zone"}
                            }]
                        }
                    },
                    "rateLimitData": {
                        "limitPerHour": 1000,
                        "pointsSpentThisHour": 11.03,
                        "pointsResetIn": 3000
                    }
                }
            })
            .to_string(),
            json!({
                "data": {
                    "reportData": {
                        "reports": {
                            "current_page": 2,
                            "has_more_pages": false,
                            "data": [{
                                "code": "second",
                                "title": "Second",
                                "startTime": 3000.0,
                                "endTime": null,
                                "revision": 1,
                                "visibility": "public",
                                "zone": null
                            }]
                        }
                    },
                    "rateLimitData": {
                        "limitPerHour": 1000,
                        "pointsSpentThisHour": 20.75,
                        "pointsResetIn": 2990
                    }
                }
            })
            .to_string(),
        ];
        let (base_url, requests) = start_json_server(responses).await;
        let config = test_config();
        let client = WarcraftLogsClient::with_endpoints(
            &config,
            &format!("{base_url}/oauth"),
            &format!("{base_url}/graphql"),
        )
        .unwrap();

        let discovery = client
            .reports_since(WarcraftLogsSite::Retail, 42, 0, 10_000)
            .await
            .unwrap();
        assert_eq!(
            discovery
                .reports
                .iter()
                .map(|report| report.code.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(discovery.rate_limit.points_spent_this_hour, 20.75);

        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("POST /oauth "));
        assert!(requests[1].starts_with("POST /graphql "));
        assert!(requests[1].contains("\"guildID\":42"));
        assert!(!requests[1].contains("\"guildId\":42"));
        assert!(requests[1].contains("\"page\":1"));
        assert!(requests[2].contains("\"page\":2"));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("POST /oauth "))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn fetches_recent_reports_without_a_time_window() {
        let responses = vec![
            json!({
                "access_token": "test-token",
                "expires_in": 3600
            })
            .to_string(),
            json!({
                "data": {
                    "reportData": {
                        "reports": {
                            "current_page": 1,
                            "has_more_pages": false,
                            "data": [
                                {
                                    "code": "older",
                                    "title": "Older",
                                    "startTime": 1000.0,
                                    "endTime": 2000.0,
                                    "revision": 0,
                                    "visibility": "public",
                                    "zone": null
                                },
                                {
                                    "code": "newer",
                                    "title": "Newer",
                                    "startTime": 3000.0,
                                    "endTime": 4000.0,
                                    "revision": 0,
                                    "visibility": "public",
                                    "zone": {"name": "Test Zone"}
                                }
                            ]
                        }
                    },
                    "rateLimitData": {
                        "limitPerHour": 1000,
                        "pointsSpentThisHour": 10,
                        "pointsResetIn": 3000
                    }
                }
            })
            .to_string(),
        ];
        let (base_url, requests) = start_json_server(responses).await;
        let config = test_config();
        let client = WarcraftLogsClient::with_endpoints(
            &config,
            &format!("{base_url}/oauth"),
            &format!("{base_url}/graphql"),
        )
        .unwrap();

        let discovery = client
            .recent_reports(WarcraftLogsSite::Retail, 42, 3)
            .await
            .unwrap();

        assert_eq!(
            discovery
                .reports
                .iter()
                .map(|report| report.code.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        let requests = requests.await.unwrap();
        assert!(requests[1].contains("\"startTime\":null"));
        assert!(requests[1].contains("\"endTime\":null"));
        assert!(requests[1].contains("\"limit\":3"));
    }

    #[tokio::test]
    async fn looks_up_classic_guilds_by_id() {
        let responses = vec![
            json!({
                "access_token": "test-token",
                "expires_in": 3600
            })
            .to_string(),
            json!({
                "data": {
                    "guildData": {
                        "guild": {
                            "id": 484,
                            "name": "Progress",
                            "server": {
                                "name": "Benediction",
                                "slug": "benediction",
                                "region": {
                                    "compactName": "US",
                                    "slug": "us"
                                }
                            }
                        }
                    }
                }
            })
            .to_string(),
        ];
        let (base_url, requests) = start_json_server(responses).await;
        let config = test_config();
        let client = WarcraftLogsClient::with_endpoints(
            &config,
            &format!("{base_url}/oauth"),
            &format!("{base_url}/graphql"),
        )
        .unwrap();

        let guild = client
            .lookup_guild_by_id(WarcraftLogsSite::Classic, 484)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(guild.id, 484);
        assert_eq!(guild.name, "Progress");
        let requests = requests.await.unwrap();
        assert!(requests[1].contains("GuildById"));
        assert!(requests[1].contains("\"id\":484"));
    }

    #[tokio::test]
    async fn loads_report_guild_and_completed_fights_for_preview() {
        let responses = vec![
            json!({
                "access_token": "test-token",
                "expires_in": 3600
            })
            .to_string(),
            json!({
                "data": {
                    "reportData": {
                        "report": {
                            "code": "AbC123",
                            "title": "Raid Night",
                            "startTime": 1000.0,
                            "endTime": 90000.0,
                            "revision": 0,
                            "guild": {"name": "Progress"},
                            "fights": [{
                                "id": 7,
                                "name": "Test Boss",
                                "encounterID": 123,
                                "kill": true,
                                "inProgress": false,
                                "difficulty": 5,
                                "size": 20,
                                "startTime": 10000.0,
                                "endTime": 70000.0,
                                "averageItemLevel": 700.0
                            }]
                        }
                    }
                }
            })
            .to_string(),
        ];
        let (base_url, requests) = start_json_server(responses).await;
        let config = test_config();
        let client = WarcraftLogsClient::with_endpoints(
            &config,
            &format!("{base_url}/oauth"),
            &format!("{base_url}/graphql"),
        )
        .unwrap();

        let report = client
            .report_fights(WarcraftLogsSite::Classic, "AbC123")
            .await
            .unwrap();

        assert_eq!(report.guild.unwrap().name, "Progress");
        assert!(report.fights[0].is_completed_boss_kill());
        assert!(requests.await.unwrap()[1].contains("ReportFights"));
    }

    #[tokio::test]
    async fn surfaces_graphql_errors() {
        let responses = vec![
            json!({
                "access_token": "test-token",
                "expires_in": 3600
            })
            .to_string(),
            json!({
                "errors": [{"message": "guild lookup failed"}]
            })
            .to_string(),
        ];
        let (base_url, requests) = start_json_server(responses).await;
        let config = test_config();
        let client = WarcraftLogsClient::with_endpoints(
            &config,
            &format!("{base_url}/oauth"),
            &format!("{base_url}/graphql"),
        )
        .unwrap();

        let error = client
            .lookup_guild(WarcraftLogsSite::Retail, "Guild", "realm", "US")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("guild lookup failed"));
        assert_eq!(requests.await.unwrap().len(), 2);
    }

    fn test_config() -> WarcraftLogsConfig {
        WarcraftLogsConfig {
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            poll_interval: Duration::from_secs(60),
        }
    }

    async fn start_json_server(responses: Vec<String>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(read_http_request(&mut socket).await);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            requests
        });

        (format!("http://{address}"), handle)
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1_024];
        let expected_length = loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0, "connection closed before HTTP headers arrived");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = find_bytes(&bytes, b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                break header_end + 4 + content_length;
            }
        };

        while bytes.len() < expected_length {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0, "connection closed before HTTP body arrived");
            bytes.extend_from_slice(&buffer[..count]);
        }

        String::from_utf8(bytes).unwrap()
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}
