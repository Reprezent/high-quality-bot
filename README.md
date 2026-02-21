# high-quality-bot

A Discord bot written in Rust that runs World of Warcraft simulations via slash commands, powered by [Poise](https://github.com/serenity-rs/poise) + [Serenity](https://github.com/serenity-rs/serenity) with PostgreSQL for session persistence.

## Slash Commands

| Command | Description |
|---------|-------------|
| `/sim <class>:<spec> <gear_json>` | Queue a WoW simulation for the given class/spec with a JSON gear payload. Returns a run ID. |
| `/class <class>[:<spec>]` | Save your default class (and optionally spec) to the database. |
| `/status <run-id>` | Check the current status of a simulation run by its UUID. |

### Examples

```
/sim warrior:arms {"head":{"id":123},"chest":{"id":456}}
/class warrior:arms
/class paladin
/status 550e8400-e29b-41d4-a716-446655440000
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
- **`bot`** — the Discord bot, waiting for the database to be healthy before starting

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DISCORD_TOKEN` | ✅ | — | Discord bot token |
| `DATABASE_URL` | ✅ | — | PostgreSQL connection URL |
| `POSTGRES_USER` | — | `botuser` | DB username (docker-compose only) |
| `POSTGRES_PASSWORD` | — | `changeme` | DB password (docker-compose only) |
| `POSTGRES_DB` | — | `highqualitybot` | DB name (docker-compose only) |
| `RUST_LOG` | — | `info` | Log level |

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
