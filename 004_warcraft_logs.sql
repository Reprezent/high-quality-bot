CREATE TABLE IF NOT EXISTS warcraft_logs_subscriptions (
    id                  BIGSERIAL PRIMARY KEY,
    discord_guild_id    TEXT        NOT NULL UNIQUE,
    discord_channel_id  TEXT        NOT NULL,
    wcl_guild_id        BIGINT      NOT NULL,
    wcl_guild_name      TEXT        NOT NULL,
    server_slug         TEXT        NOT NULL,
    server_name         TEXT        NOT NULL,
    region              TEXT        NOT NULL,
    baseline_time_ms    BIGINT      NOT NULL,
    discovery_cursor_ms BIGINT      NOT NULL,
    enabled             BOOLEAN     NOT NULL DEFAULT TRUE,
    last_polled_at      TIMESTAMPTZ,
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS warcraft_logs_reports (
    subscription_id    BIGINT      NOT NULL REFERENCES warcraft_logs_subscriptions(id) ON DELETE CASCADE,
    code               TEXT        NOT NULL,
    title              TEXT        NOT NULL,
    start_time_ms      BIGINT      NOT NULL,
    end_time_ms        BIGINT,
    revision           INTEGER     NOT NULL DEFAULT 0,
    zone_name          TEXT,
    visibility         TEXT        NOT NULL,
    announcement_state TEXT        NOT NULL DEFAULT 'pending'
        CHECK (announcement_state IN ('pending', 'posted', 'suppressed')),
    report_message_id  TEXT,
    baseline_scanned   BOOLEAN     NOT NULL DEFAULT FALSE,
    track_until        TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '12 hours'),
    last_inspected_at  TIMESTAMPTZ,
    last_error         TEXT,
    discovered_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (subscription_id, code)
);

CREATE INDEX IF NOT EXISTS warcraft_logs_reports_tracking_idx
    ON warcraft_logs_reports (subscription_id, discovered_at DESC);

CREATE TABLE IF NOT EXISTS warcraft_logs_fights (
    subscription_id BIGINT      NOT NULL,
    report_code     TEXT        NOT NULL,
    fight_id        INTEGER     NOT NULL,
    boss_name       TEXT        NOT NULL,
    difficulty      INTEGER,
    raid_size       INTEGER,
    average_item_level DOUBLE PRECISION,
    start_time_ms   BIGINT      NOT NULL,
    end_time_ms     BIGINT      NOT NULL,
    announcement_state TEXT     NOT NULL DEFAULT 'pending'
        CHECK (announcement_state IN ('pending', 'posted', 'suppressed')),
    discord_message_id TEXT,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (subscription_id, report_code, fight_id),
    FOREIGN KEY (subscription_id, report_code)
        REFERENCES warcraft_logs_reports(subscription_id, code) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS warcraft_logs_fights_pending_idx
    ON warcraft_logs_fights (subscription_id, announcement_state)
    WHERE announcement_state = 'pending';
