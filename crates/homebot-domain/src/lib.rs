//! Provider-independent `HomeBot` product concepts.

pub mod chat;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const BOT_NAME_MAX_CHARS: usize = 48;
pub const BOT_TITLE_MAX_CHARS: usize = 80;
pub const BOT_DESCRIPTION_MAX_CHARS: usize = 2_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BotId(pub Uuid);

macro_rules! string_enum {
    ($name:ident, $error:ident, $default:ident, {$($variant:ident => $value:literal),+ $(,)?}) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
        }

        impl std::str::FromStr for $name {
            type Err = DomainError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($value => Ok(Self::$variant),)+ _ => Err(DomainError::$error) }
            }
        }
    };
}

string_enum!(BotShape, InvalidBotShape, RoundedSquare, {
    Circle => "circle",
    RoundedSquare => "rounded_square",
    Hexagon => "hexagon"
});
string_enum!(BotColor, InvalidBotColor, Violet, {
    Violet => "violet",
    Blue => "blue",
    Green => "green",
    Orange => "orange",
    Rose => "rose",
    Slate => "slate"
});
string_enum!(BotAttention, InvalidBotAttention, None, {
    None => "none",
    Working => "working",
    NeedsApproval => "needs_approval",
    Failed => "failed"
});
string_enum!(BotPermissionProfile, InvalidPermissionProfile, AskBeforeChanges, {
    ReadOnly => "read_only",
    AskBeforeChanges => "ask_before_changes",
    Trusted => "trusted"
});

/// A durable AI teammate. Provider conversations are mappings, not identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Bot {
    pub id: BotId,
    pub name: String,
    pub title: String,
    pub description: String,
    pub shape: BotShape,
    pub color: BotColor,
    pub provider_profile_id: Option<Uuid>,
    pub permission_profile: BotPermissionProfile,
    pub archived_at_ms: Option<i64>,
    pub pinned_at_ms: Option<i64>,
    pub hidden_at_ms: Option<i64>,
    pub unread_count: u32,
    pub attention: BotAttention,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Bot {
    /// Creates a Bot with a validated name and title.
    ///
    /// # Errors
    ///
    /// Returns a validation error for blank, oversized, or unsafe identity text.
    pub fn create(name: impl Into<String>, title: impl Into<String>) -> Result<Self, DomainError> {
        let mut bot = Self {
            id: BotId(Uuid::now_v7()),
            name: String::new(),
            title: String::new(),
            description: String::new(),
            shape: BotShape::default(),
            color: BotColor::default(),
            provider_profile_id: None,
            permission_profile: BotPermissionProfile::default(),
            archived_at_ms: None,
            pinned_at_ms: None,
            hidden_at_ms: None,
            unread_count: 0,
            attention: BotAttention::None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        bot.update_identity(name, title, "", bot.shape, bot.color)?;
        Ok(bot)
    }

    /// Replaces the user-facing identity fields after validation.
    ///
    /// # Errors
    ///
    /// Returns a validation error for blank, oversized, or unsafe identity text.
    pub fn update_identity(
        &mut self,
        name: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        shape: BotShape,
        color: BotColor,
    ) -> Result<(), DomainError> {
        let name = name.into();
        let title = title.into();
        let description = description.into();
        self.name = validate_text(&name, "name", BOT_NAME_MAX_CHARS, false)?;
        self.title = validate_text(&title, "title", BOT_TITLE_MAX_CHARS, true)?;
        self.description =
            validate_text(&description, "description", BOT_DESCRIPTION_MAX_CHARS, true)?;
        self.shape = shape;
        self.color = color;
        Ok(())
    }

    /// Archives a currently active Bot.
    ///
    /// # Errors
    ///
    /// Returns an error when the Bot is already archived.
    pub fn archive(&mut self, now_ms: i64) -> Result<(), DomainError> {
        if self.archived_at_ms.is_some() {
            return Err(DomainError::BotAlreadyArchived);
        }
        self.archived_at_ms = Some(now_ms);
        self.updated_at_ms = now_ms;
        Ok(())
    }

    /// Restores a currently archived Bot.
    ///
    /// # Errors
    ///
    /// Returns an error when the Bot is already active.
    pub fn restore(&mut self, now_ms: i64) -> Result<(), DomainError> {
        if self.archived_at_ms.is_none() {
            return Err(DomainError::BotNotArchived);
        }
        self.archived_at_ms = None;
        self.updated_at_ms = now_ms;
        Ok(())
    }
}

fn validate_text(
    value: &str,
    field: &'static str,
    maximum: usize,
    allow_empty: bool,
) -> Result<String, DomainError> {
    let value = value.trim().to_owned();
    if !allow_empty && value.is_empty() {
        return Err(DomainError::EmptyBotName);
    }
    let actual = value.chars().count();
    if actual > maximum {
        return Err(DomainError::TextTooLong {
            field,
            maximum,
            actual,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(DomainError::ControlCharacter { field });
    }
    Ok(value)
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DomainError {
    #[error("Bot name must not be empty")]
    EmptyBotName,
    #[error("Bot {field} is too long ({actual}; maximum {maximum})")]
    TextTooLong {
        field: &'static str,
        maximum: usize,
        actual: usize,
    },
    #[error("Bot {field} must not contain control characters")]
    ControlCharacter { field: &'static str },
    #[error("Bot shape is invalid")]
    InvalidBotShape,
    #[error("Bot color is invalid")]
    InvalidBotColor,
    #[error("Bot attention state is invalid")]
    InvalidBotAttention,
    #[error("Bot permission profile is invalid")]
    InvalidPermissionProfile,
    #[error("Bot is already archived")]
    BotAlreadyArchived,
    #[error("Bot is not archived")]
    BotNotArchived,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_identity_is_trimmed_and_validated() {
        let bot = Bot::create("  Nova ", " Research ").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            (bot.name.as_str(), bot.title.as_str()),
            ("Nova", "Research")
        );
        assert_eq!(Bot::create("   ", "Helper"), Err(DomainError::EmptyBotName));
        assert!(Bot::create("x".repeat(BOT_NAME_MAX_CHARS + 1), "Helper").is_err());
        assert!(Bot::create("No\u{0}va", "Helper").is_err());
    }

    #[test]
    fn archive_and_restore_are_explicit_state_transitions() {
        let mut bot = Bot::create("Nova", "Helper").unwrap_or_else(|error| panic!("{error}"));
        assert!(bot.archive(10).is_ok());
        assert_eq!(bot.archive(11), Err(DomainError::BotAlreadyArchived));
        assert!(bot.restore(12).is_ok());
        assert_eq!(bot.restore(13), Err(DomainError::BotNotArchived));
    }
}
