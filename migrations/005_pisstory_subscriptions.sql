CREATE TABLE IF NOT EXISTS pisstory_subscriptions (
    discord_guild_id   TEXT        PRIMARY KEY,
    discord_channel_id TEXT        NOT NULL,
    interval_seconds   BIGINT      NOT NULL CHECK (interval_seconds >= 3600),
    next_post_at       TIMESTAMPTZ NOT NULL,
    last_posted_at     TIMESTAMPTZ,
    last_error         TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS pisstory_subscriptions_due_idx
    ON pisstory_subscriptions (next_post_at);
