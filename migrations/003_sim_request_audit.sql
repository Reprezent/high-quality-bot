ALTER TABLE simulation_runs
    ADD COLUMN IF NOT EXISTS input_format TEXT NOT NULL DEFAULT 'legacy-gear-json',
    ADD COLUMN IF NOT EXISTS upstream_revision TEXT,
    ADD COLUMN IF NOT EXISTS normalized_request JSONB,
    ADD COLUMN IF NOT EXISTS effective_random_seed BIGINT,
    ADD COLUMN IF NOT EXISTS effective_iterations INTEGER;
