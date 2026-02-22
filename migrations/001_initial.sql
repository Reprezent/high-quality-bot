CREATE EXTENSION IF NOT EXISTS pgcrypto;

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
    raid_members    TEXT[]      NOT NULL DEFAULT '{}',
    status          TEXT        NOT NULL DEFAULT 'queued',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

ALTER TABLE simulation_runs
    ADD COLUMN IF NOT EXISTS raid_members TEXT[] NOT NULL DEFAULT '{}';

-- Progress writeframes captured while a simulation is running.
CREATE TABLE IF NOT EXISTS simulation_progress_frames (
    id                   BIGSERIAL PRIMARY KEY,
    run_id               UUID        NOT NULL REFERENCES simulation_runs(run_id) ON DELETE CASCADE,
    frame_index          INTEGER     NOT NULL,
    completed_iterations INTEGER     NOT NULL DEFAULT 0,
    total_iterations     INTEGER     NOT NULL DEFAULT 0,
    completed_sims       INTEGER     NOT NULL DEFAULT 0,
    total_sims           INTEGER     NOT NULL DEFAULT 0,
    dps                  DOUBLE PRECISION NOT NULL DEFAULT 0,
    hps                  DOUBLE PRECISION NOT NULL DEFAULT 0,
    is_final             BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (run_id, frame_index)
);
