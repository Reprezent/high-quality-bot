# high-quality-bot

A Discord bot written in Rust that runs World of Warcraft simulations via slash commands, powered by [Poise](https://github.com/serenity-rs/poise) + [Serenity](https://github.com/serenity-rs/serenity) with PostgreSQL for session persistence.

## Slash Commands

| Command | Description |
|---------|-------------|
| `/sim <gear_json>` | Queue a simulation from a gear profile JSON payload containing class, spec, and gear items. The server supplies raid, encounter, and sim defaults. |
| `/class <class>[:<spec>]` | Save your default class (and optionally spec) to the database. |
| `/status <run-id>` | Check the current status of a simulation run by its UUID. |
| `/health` | Check if the bot can reach PostgreSQL and the wowsims async API. |
| `/dailies` | Show when World of Warcraft US realm dailies reset next. |
| `/piss` | Fetch the current ISS urine tank fill level from the public ISS telemetry stream. |
| `/warcraftlogs track <channel> [guild_link] [guild] [server] [region] [section]` | Post new public reports and boss kills for a Warcraft Logs guild. Requires Manage Server. |
| `/warcraftlogs status` | Show this server's Warcraft Logs tracker status. |
| `/warcraftlogs history` | Show the tracked guild's three most recent public reports. |
| `/warcraftlogs summary <report_link>` | Preview the kill-summary embed for a report's selected or latest completed boss kill. |
| `/warcraftlogs untrack` | Stop tracking Warcraft Logs in this server. Requires Manage Server. |

### Examples

```
/class warrior:arms
/class paladin
/status 550e8400-e29b-41d4-a716-446655440000
/health
/dailies
/piss
/warcraftlogs track channel:#raid-logs guild_link:https://classic.warcraftlogs.com/guild/id/484
/warcraftlogs status
/warcraftlogs history
/warcraftlogs summary report_link:https://classic.warcraftlogs.com/reports/REPORTCODE#fight=7&type=summary
```

### Running a gear profile

`/sim` accepts a compact gear profile JSON payload. It supports the flat
Warcraft Logs-style shape (`class`, `spec`, `name`, `race`, `talents`, `glyphs`,
`professions`, and `gear.items`) as well as the existing nested WoWSims player
shape. The payload must provide class and spec so the bot can select the
appropriate player implementation and default rotation.

The bot preserves the simulation-relevant player data that is supplied:

- Character name, race, talents, glyphs, professions, gear, gems, enchants,
  reforges, and upgrade levels.
- A custom rotation when the payload includes one; otherwise the bot loads the
  vendored default APL for that class/spec.

The server deliberately supplies the rest: default raid and party buffs,
debuffs, encounter target and duration, 12,500 iterations, and a generated
random seed. The generated request, seed, and iterations are persisted with the
run for diagnostics and reproducibility.

For example:

```json
{
  "class": "mage",
  "spec": "arcane",
  "race": "NightElf",
  "talents": "311222",
  "professions": [{"name": "Enchanting"}, {"name": "Tailoring"}],
  "gear": {"items": [{"id": 103900, "gems": [95347, 76700], "upgrade_step": 2}]}
}
```

## Prerequisites

- [Rust](https://rustup.rs/) 1.70+
- [Docker](https://docs.docker.com/get-docker/) & [Docker Compose](https://docs.docker.com/compose/)
- A Discord bot token from the [Discord Developer Portal](https://discord.com/developers/applications)
- A Warcraft Logs API v2 client from the [Warcraft Logs client management page](https://www.warcraftlogs.com/api/clients/) to enable log tracking

## Local Development

1. **Clone the repo and copy the example env file:**

   ```bash
   cp .env.example .env
   # Edit .env and set DISCORD_TOKEN
   ```

2. **Start PostgreSQL:**

   ```bash
   docker compose up db -d
   ```

3. **Run the bot:**

   ```bash
   cargo run
   ```

## Docker Compose (Full Stack)

```bash
cp .env.example .env
# Set DISCORD_TOKEN in .env
# Optional: set DISCORD_GUILD_ID for instant guild-scoped slash command updates
docker compose up -d
```

This starts:
- **`db`** — PostgreSQL 16 with a persistent named volume
- **`sim`** — the vendored `wowsims/mop` async simulation API on port `3333` (built with `-tags with_db` so item IDs resolve correctly)
- **`bot`** — the Discord bot, waiting for both `db` and `sim` to be healthy before starting

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DISCORD_TOKEN` | ✅ | — | Discord bot token |
| `DISCORD_GUILD_ID` | — | empty | When set, registers slash commands only in that guild for faster iteration; when unset, registers globally |
| `POSTGRES_USER` | — | `botuser` | DB username (docker-compose only) |
| `POSTGRES_PASSWORD` | — | `changeme` | DB password (docker-compose only) |
| `POSTGRES_DB` | — | `highqualitybot` | DB name (docker-compose only) |
| `POSTGRES_HOST` | — | `localhost` (local) / `db` (docker-compose) | DB host used by bot |
| `POSTGRES_PORT` | — | `5432` | DB port used by bot |
| `WOWSIMS_API_BASE_URL` | — | `http://127.0.0.1:3333` (local) / `http://sim:3333` (docker-compose) | Base URL for wowsims async sim API (`/raidSimAsync`, `/asyncProgress`) |
| `WARCRAFT_LOGS_CLIENT_ID` | — | empty | Warcraft Logs API v2 client ID. Set with `WARCRAFT_LOGS_CLIENT_SECRET` to enable tracking. |
| `WARCRAFT_LOGS_CLIENT_SECRET` | — | empty | Warcraft Logs API v2 client secret. Never expose this in Discord or commit it. |
| `WARCRAFT_LOGS_POLL_INTERVAL_SECS` | — | `60` | Base report/fight polling interval; values below 30 seconds are clamped to 30. |
| `LOG_SIM_REQUEST_JSON` | — | `false` | When true (`1/true/yes/on`), logs outgoing raid sim request as pretty JSON before calling backend |
| `WOWSIMS_SIM_DEBUG` | — | `false` | Diagnostic override: when true (`1/true/yes/on`), sends `simOptions.debug=true` to backend sim. Leave off for UI-equivalent runs. |
| `RUST_LOG` | — | `info` | Log level |

If `DISCORD_GUILD_ID` is set, Discord command updates are usually visible almost immediately in that server. Leave it unset for production-style global registration.

## Warcraft Logs Tracking

1. Create a Warcraft Logs API v2 client at <https://www.warcraftlogs.com/api/clients/>.
2. Set both `WARCRAFT_LOGS_CLIENT_ID` and `WARCRAFT_LOGS_CLIENT_SECRET`, then restart the bot.
3. Give the bot **View Channel**, **Send Messages**, and **Embed Links** permissions in the destination channel.
4. As a member with **Manage Server**, run:

   ```text
   /warcraftlogs track channel:#raid-logs guild_link:https://classic.warcraftlogs.com/guild/id/484
   ```

   You can also enter the guild manually. Manual lookup defaults to Classic; select `section:Retail` for `www.warcraftlogs.com`:

   ```text
   /warcraftlogs track channel:#raid-logs guild:"Example Guild" server:"Area 52" region:US section:Retail
   ```

The command accepts guild pages from `classic.warcraftlogs.com` and `www.warcraftlogs.com`, infers the section and guild from a copied link, and validates both the guild and Discord destination before replacing this server's existing tracker. It records the current public reports and completed kills as a baseline, so enabling tracking does not post historical announcements. Afterward the bot:

- Polls the selected Warcraft Logs section because its API does not provide custom report webhooks or GraphQL subscriptions.
- Posts a link when it discovers a new public report.
- Tracks active/revised reports and posts one congratulations embed per completed encounter kill.
- Keeps OAuth, GraphQL, history, and report links on the configured Classic or Retail host.
- Includes difficulty, kill time, duration, raid size, average item level, top damage and healing, deaths, and a fight-specific report link when those metrics are available.
- Uses durable cursors and overlapping discovery windows to avoid gaps, plus database uniqueness, confirmation retries, and Discord nonces to minimize duplicate announcements across retries or restarts.
- Slows polling automatically when Warcraft Logs hourly API points are low.

The integration uses bot-level client credentials and intentionally supports **public guild reports only** on the Classic and Retail Warcraft Logs sections. Private/unlisted reports and per-user Warcraft Logs OAuth are not supported. Warcraft Logs documents report-table JSON as non-frozen; if a summary payload changes, the bot records and retries the failed summary instead of posting invented metrics.

Use `/warcraftlogs status` to see the destination and latest polling health. Use `/warcraftlogs history` to query Warcraft Logs for the tracked guild's three newest public reports, including reports older than the local tracking baseline. Use `/warcraftlogs summary` with any public Classic or Retail report link to preview the same boss-kill embed the tracker posts; a numeric `fight` selector chooses that completed kill, while a plain report link or `fight=last` uses the latest completed kill. Use `/warcraftlogs untrack` to remove the tracker and its stored report/fight state.

## Using `wowsims/mop` Protobufs in Rust

This project includes an optional `mop-proto` feature that compiles upstream `.proto` files from `wowsims/mop` into Rust types using `prost`.

1. Add the upstream repo as a submodule:

   ```bash
   git submodule add https://github.com/wowsims/mop.git vendor/wowsims-mop
   git submodule update --init --recursive
   ```

2. Install `protoc` if it is not already available:

   ```bash
   # Ubuntu/Debian
   sudo apt-get update && sudo apt-get install -y protobuf-compiler
   ```

3. Build/check with protobuf generation enabled:

   ```bash
   cargo check --features mop-proto
   ```

Generated types are available under `crate::mop_proto::mop`.

Example:

```rust
#[cfg(feature = "mop-proto")]
use crate::mop_proto::mop::RaidSimRequest;
```

### Optional: Custom proto path

If you want to source protos from a different checkout path, set `MOP_PROTO_DIR`:

```bash
MOP_PROTO_DIR=/absolute/path/to/mop/proto cargo check --features mop-proto
```

### Updating upstream

```bash
git submodule update --remote --merge vendor/wowsims-mop
git add vendor/wowsims-mop .gitmodules
```

When advancing the submodule, also update `MOP_UPSTREAM_REVISION` in
[`build.rs`](build.rs) to the new submodule commit so persisted runs retain the
simulator revision used to normalize their request.

### Running the local async sim API

The bot now calls the wowsims async API endpoints (`raidSimAsync` + `asyncProgress`).

Run from the submodule checkout:

```bash
cd vendor/wowsims-mop
go run ./sim/web --host=127.0.0.1:3333 --launch=false --usefs=false
```

If you are using `docker compose up`, this API is started automatically via the `sim` service.

The repository also includes an automatic updater workflow:

- `.github/workflows/submodule-auto-update.yml`
- Runs every 6 hours and on manual dispatch
- Updates `vendor/wowsims-mop` and commits the new submodule pointer automatically when upstream changes

## CI/CD

The official GitHub Actions workflow (`.github/workflows/docker.yml`) automatically:

- Builds both Docker images (`Dockerfile` for bot, `Dockerfile.sim` for sim) on every PR to `main`
- On merges/pushes to `main`, pushes both images to **GitHub Container Registry**:
   - `ghcr.io/<owner>/<repo>-bot`
   - `ghcr.io/<owner>/<repo>-sim`
- Applies versioned tags on `main` pushes:
   - `latest`
   - `sha-<short-commit>`
   - `vYYYY.MM.DD-HHMMSS-<short-commit>` (UTC)

## Database Schema

The bot automatically applies its SQL migrations on startup:

- **`user_preferences`** — stores each user's default class/spec keyed by Discord user ID
- **`simulation_runs`** — records every simulation run with its status, gear payload, and timestamps
- **`warcraft_logs_subscriptions`** — stores one tracked Warcraft Logs guild and destination per Discord server
- **`warcraft_logs_reports`** — stores report discovery, revision, baseline, and announcement state
- **`warcraft_logs_fights`** — stores report-local boss fights and idempotent announcement state

## Project Structure

```
├── src/
│   ├── main.rs              # Bot entry point, framework setup
│   ├── db.rs                # Database helpers (PostgreSQL via sqlx)
│   ├── warcraft_logs.rs     # OAuth and Warcraft Logs GraphQL API client
│   ├── warcraft_logs_discord.rs # Report/kill links and Discord embeds
│   ├── warcraft_logs_tracker.rs # Polling and announcement worker
│   └── commands/
│       ├── mod.rs
│       ├── sim.rs           # /sim command
│       ├── class.rs         # /class command
│       ├── status.rs        # /status command
│       └── warcraftlogs.rs  # /warcraftlogs command group
├── migrations/
│   ├── 001_initial.sql      # Simulation schema
│   ├── 002_iss_telemetry_history.sql
│   └── 003_warcraft_logs.sql
├── Dockerfile               # Multi-stage Docker build
├── docker-compose.yml       # Bot + PostgreSQL stack
└── .github/workflows/
    └── docker.yml           # CI/CD pipeline
```
