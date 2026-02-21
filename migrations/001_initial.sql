-- User default class/spec preferences
CREATE TABLE IF NOT EXISTS user_preferences (
    discord_user_id TEXT PRIMARY KEY,
    class           TEXT NOT NULL,
    spec            TEXT
);

-- Simulation run records
CREATE TABLE IF NOT EXISTS simulation_runs (
    run_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    discord_user_id TEXT        NOT NULL,
    class           TEXT        NOT NULL,
    spec            TEXT        NOT NULL,
    gear_payload    JSONB       NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'queued',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
