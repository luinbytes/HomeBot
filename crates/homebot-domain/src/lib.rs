//! Provider-independent HomeBot product concepts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity for a persistent Bot.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BotId(pub Uuid);

/// A durable AI teammate. Provider conversations are mappings, not identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Bot {
    pub id: BotId,
    pub name: String,
    pub title: String,
    pub description: String,
}

impl Bot {
    /// Creates a Bot after applying domain-level validation.
    pub fn create(name: impl Into<String>, title: impl Into<String>) -> Result<Self, DomainError> {
        let name = name.into().trim().to_owned();
        if name.is_empty() {
            return Err(DomainError::EmptyBotName);
        }

        Ok(Self {
            id: BotId(Uuid::now_v7()),
            name,
            title: title.into().trim().to_owned(),
            description: String::new(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DomainError {
    #[error("Bot name must not be empty")]
    EmptyBotName,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_name_must_not_be_blank() {
        assert_eq!(Bot::create("   ", "Helper"), Err(DomainError::EmptyBotName));
    }
}
