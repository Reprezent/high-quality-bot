CREATE TABLE IF NOT EXISTS wow_characters (
    discord_user_id          TEXT        NOT NULL,
    region                   TEXT        NOT NULL
        CHECK (region IN ('us', 'eu', 'kr', 'tw')),
    realm_name               TEXT        NOT NULL,
    realm_name_normalized    TEXT        NOT NULL,
    character_name           TEXT        NOT NULL,
    character_name_normalized TEXT       NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (
        discord_user_id,
        region,
        realm_name_normalized,
        character_name_normalized
    )
);
