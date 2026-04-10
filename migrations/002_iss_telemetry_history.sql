CREATE TABLE IF NOT EXISTS iss_telemetry_history (
    id                    BIGSERIAL PRIMARY KEY,
    recorded_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    urine_tank_pct        DOUBLE PRECISION NOT NULL,
    waste_water_pct       DOUBLE PRECISION NOT NULL,
    clean_water_pct       DOUBLE PRECISION NOT NULL,
    processor_status      TEXT NOT NULL,
    signal_acquired       BOOLEAN NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_iss_telemetry_history_recorded_at
    ON iss_telemetry_history (recorded_at DESC);
