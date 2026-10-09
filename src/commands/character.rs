use crate::{
    Context,
    db::{self, NewWowCharacter},
};
use anyhow::Result;

/// Manage your stored World of Warcraft characters.
#[poise::command(slash_command, subcommands("store", "remove"))]
pub async fn character(_: Context<'_>) -> Result<()> {
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, poise::ChoiceParameter)]
pub enum WowRegion {
    #[name = "Americas & Oceania"]
    Americas,
    #[name = "Europe"]
    Europe,
    #[name = "Korea"]
    Korea,
    #[name = "Taiwan"]
    Taiwan,
}

impl WowRegion {
    fn code(self) -> &'static str {
        match self {
            Self::Americas => "us",
            Self::Europe => "eu",
            Self::Korea => "kr",
            Self::Taiwan => "tw",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Americas => "Americas & Oceania",
            Self::Europe => "Europe",
            Self::Korea => "Korea",
            Self::Taiwan => "Taiwan",
        }
    }
}

struct CharacterIdentity {
    name: String,
    normalized_name: String,
    realm: String,
    normalized_realm: String,
}

/// Store one of your World of Warcraft characters.
#[poise::command(slash_command)]
pub async fn store(
    ctx: Context<'_>,
    #[description = "Your character's name"] name: String,
    #[description = "The realm your character is on"] realm: String,
    #[description = "The region your character is in"] region: WowRegion,
) -> Result<()> {
    let identity = match character_identity(&name, &realm) {
        Ok(identity) => identity,
        Err(message) => {
            send_private_reply(ctx, format!("⚠️ {message}")).await?;
            return Ok(());
        }
    };
    let discord_user_id = ctx.author().id.to_string();

    let inserted = db::store_wow_character(
        &ctx.data().db,
        NewWowCharacter {
            discord_user_id: &discord_user_id,
            region: region.code(),
            realm_name: &identity.realm,
            realm_name_normalized: &identity.normalized_realm,
            character_name: &identity.name,
            character_name_normalized: &identity.normalized_name,
        },
    )
    .await?;

    let message = if inserted {
        format!(
            "✅ Stored **{}-{}** ({}) for you.",
            identity.name,
            identity.realm,
            region.label()
        )
    } else {
        format!(
            "ℹ️ **{}-{}** ({}) is already stored for you.",
            identity.name,
            identity.realm,
            region.label()
        )
    };
    send_private_reply(ctx, message).await?;
    Ok(())
}

/// Remove one of your stored World of Warcraft characters.
#[poise::command(slash_command)]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "Your character's name"] name: String,
    #[description = "The realm your character is on"] realm: String,
    #[description = "The region your character is in"] region: WowRegion,
) -> Result<()> {
    let identity = match character_identity(&name, &realm) {
        Ok(identity) => identity,
        Err(message) => {
            send_private_reply(ctx, format!("⚠️ {message}")).await?;
            return Ok(());
        }
    };
    let discord_user_id = ctx.author().id.to_string();
    let removed = db::remove_wow_character(
        &ctx.data().db,
        &discord_user_id,
        region.code(),
        &identity.normalized_realm,
        &identity.normalized_name,
    )
    .await?;

    let message = if removed {
        format!(
            "🗑️ Removed **{}-{}** ({}) from your stored characters.",
            identity.name,
            identity.realm,
            region.label()
        )
    } else {
        format!(
            "ℹ️ **{}-{}** ({}) was not in your stored characters.",
            identity.name,
            identity.realm,
            region.label()
        )
    };
    send_private_reply(ctx, message).await?;
    Ok(())
}

fn character_identity(name: &str, realm: &str) -> Result<CharacterIdentity, &'static str> {
    let name = name.trim();
    let realm = realm.trim();

    if !(2..=12).contains(&name.chars().count()) || !name.chars().all(char::is_alphabetic) {
        return Err("Character names must contain only 2-12 letters.");
    }
    if !(1..=100).contains(&realm.chars().count())
        || realm.chars().any(|character| character.is_control())
    {
        return Err("Realm names must be 1–100 characters.");
    }

    Ok(CharacterIdentity {
        name: name.to_owned(),
        normalized_name: name.to_lowercase(),
        realm: realm.to_owned(),
        normalized_realm: realm.to_lowercase(),
    })
}

async fn send_private_reply(ctx: Context<'_>, message: String) -> Result<()> {
    ctx.send(
        poise::CreateReply::default()
            .content(message)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WowRegion, character_identity};

    #[test]
    fn normalizes_character_identity_for_case_insensitive_storage() {
        let identity = character_identity("  Thrall  ", "  Area 52  ").unwrap();

        assert_eq!(identity.name, "Thrall");
        assert_eq!(identity.normalized_name, "thrall");
        assert_eq!(identity.realm, "Area 52");
        assert_eq!(identity.normalized_realm, "area 52");
    }

    #[test]
    fn rejects_invalid_character_names_and_realms() {
        assert!(character_identity("A", "Area 52").is_err());
        assert!(character_identity("Two Words", "Area 52").is_err());
        assert!(character_identity("Thrall", "   ").is_err());
        assert!(character_identity("Thrall", "Bad\nRealm").is_err());
    }

    #[test]
    fn maps_regions_to_stable_storage_codes() {
        assert_eq!(WowRegion::Americas.code(), "us");
        assert_eq!(WowRegion::Europe.code(), "eu");
        assert_eq!(WowRegion::Korea.code(), "kr");
        assert_eq!(WowRegion::Taiwan.code(), "tw");
    }
}
