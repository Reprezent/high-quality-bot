# high-quality-bot

A Discord bot written in Rust that runs World of Warcraft simulations via slash commands, powered by [Poise](https://github.com/serenity-rs/poise) + [Serenity](https://github.com/serenity-rs/serenity) with PostgreSQL for session persistence.

## Slash Commands

| Command | Description |
|---------|-------------|
| `/sim <gear_json>` | Queue a WoW simulation from a WoWSims JSON payload (must include player class/spec). Returns a run ID. |
| `/class <class>[:<spec>]` | Save your default class (and optionally spec) to the database. |
| `/status <run-id>` | Check the current status of a simulation run by its UUID. |
| `/health` | Check if the bot can reach PostgreSQL and the wowsims async API. |

### Examples

```
/sim {"player":{"class":"ClassWarrior","armsWarrior":{},"equipment":{"items":[{"id":123}]}}}
/class warrior:arms
/class paladin
/status 550e8400-e29b-41d4-a716-446655440000
/health
```

## Prerequisites

- [Rust](https://rustup.rs/) 1.70+
- [Docker](https://docs.docker.com/get-docker/) & [Docker Compose](https://docs.docker.com/compose/)
- A Discord bot token from the [Discord Developer Portal](https://discord.com/developers/applications)

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
| `POSTGRES_USER` | — | `botuser` | DB username (docker-compose only) |
| `POSTGRES_PASSWORD` | — | `changeme` | DB password (docker-compose only) |
| `POSTGRES_DB` | — | `highqualitybot` | DB name (docker-compose only) |
| `POSTGRES_HOST` | — | `localhost` (local) / `db` (docker-compose) | DB host used by bot |
| `POSTGRES_PORT` | — | `5432` | DB port used by bot |
| `WOWSIMS_API_BASE_URL` | — | `http://127.0.0.1:3333` (local) / `http://sim:3333` (docker-compose) | Base URL for wowsims async sim API (`/raidSimAsync`, `/asyncProgress`) |
| `LOG_SIM_REQUEST_JSON` | — | `false` | When true (`1/true/yes/on`), logs outgoing raid sim request as pretty JSON before calling backend |
| `WOWSIMS_SIM_DEBUG` | — | `false` | When true (`1/true/yes/on`), sends `simOptions.debug=true` to backend sim |
| `RUST_LOG` | — | `info` | Log level |

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

A GitHub Actions workflow (`.github/workflows/docker.yml`) automatically:

- Builds the Docker image on every push/PR to `main`
- Pushes the image to **GitHub Container Registry** (`ghcr.io/<owner>/<repo>:latest`) on pushes to `main`

The image is tagged with both `latest` and the short commit SHA.

## Database Schema

The bot automatically applies `migrations/001_initial.sql` on startup:

- **`user_preferences`** — stores each user's default class/spec keyed by Discord user ID
- **`simulation_runs`** — records every simulation run with its status, gear payload, and timestamps

## Project Structure

```
├── src/
│   ├── main.rs              # Bot entry point, framework setup
│   ├── db.rs                # Database helpers (PostgreSQL via sqlx)
│   └── commands/
│       ├── mod.rs
│       ├── sim.rs           # /sim command
│       ├── class.rs         # /class command
│       └── status.rs        # /status command
├── migrations/
│   └── 001_initial.sql      # Schema migrations
├── Dockerfile               # Multi-stage Docker build
├── docker-compose.yml       # Bot + PostgreSQL stack
└── .github/workflows/
    └── docker.yml           # CI/CD pipeline
```
