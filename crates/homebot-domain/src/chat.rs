use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectChat {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub bot_id: Uuid,
    pub title: String,
    pub unread_count: u32,
    pub running: bool,
    pub queued_count: u32,
    pub last_sequence: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    User,
    Bot,
    System,
}

impl MessageAuthor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Bot => "bot",
            Self::System => "system",
        }
    }
}

impl std::str::FromStr for MessageAuthor {
    type Err = ChatDomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user" => Ok(Self::User),
            "bot" => Ok(Self::Bot),
            "system" => Ok(Self::System),
            _ => Err(ChatDomainError::InvalidAuthor),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Queued,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

impl MessageStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for MessageStatus {
    type Err = ChatDomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "streaming" => Ok(Self::Streaming),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(ChatDomainError::InvalidStatus),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessagePart {
    Text {
        id: Uuid,
        ordinal: u32,
        text: String,
    },
    Attachment {
        id: Uuid,
        ordinal: u32,
        attachment_id: Uuid,
    },
    Notice {
        id: Uuid,
        ordinal: u32,
        text: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatMessage {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub author: MessageAuthor,
    pub author_bot_id: Option<Uuid>,
    pub status: MessageStatus,
    pub parts: Vec<MessagePart>,
    pub reply_to_message_id: Option<Uuid>,
    pub mentioned_bot_ids: Vec<Uuid>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub error_json: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueuedPrompt {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub content: String,
    pub attachment_ids: Vec<Uuid>,
    pub position: u32,
    pub created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ActivityStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for ActivityStatus {
    type Err = ChatDomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(ChatDomainError::InvalidActivityStatus),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionActivity {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub status: ActivityStatus,
    pub requires_attention: bool,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
    Expired,
}

impl ApprovalStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
}

impl std::str::FromStr for ApprovalStatus {
    type Err = ChatDomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "allowed" => Ok(Self::Allowed),
            "denied" => Ok(Self::Denied),
            "expired" => Ok(Self::Expired),
            _ => Err(ChatDomainError::InvalidApprovalStatus),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatApproval {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub operation_id: Uuid,
    pub capability: String,
    pub title: String,
    pub detail: String,
    pub status: ApprovalStatus,
    pub created_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}

impl ChatMessage {
    /// Creates a validated user message.
    ///
    /// # Errors
    ///
    /// Returns an error when both text and attachments are empty or text is oversized.
    pub fn user(
        chat_id: Uuid,
        content: &str,
        attachment_ids: &[Uuid],
        reply_to_message_id: Option<Uuid>,
        mentioned_bot_ids: Vec<Uuid>,
        now_ms: i64,
    ) -> Result<Self, ChatDomainError> {
        let content = content.trim();
        if content.is_empty() && attachment_ids.is_empty() {
            return Err(ChatDomainError::EmptyMessage);
        }
        if content.chars().count() > 100_000 {
            return Err(ChatDomainError::MessageTooLong);
        }
        let mut parts = Vec::new();
        if !content.is_empty() {
            parts.push(MessagePart::Text {
                id: Uuid::now_v7(),
                ordinal: 0,
                text: content.to_owned(),
            });
        }
        for attachment_id in attachment_ids {
            parts.push(MessagePart::Attachment {
                id: Uuid::now_v7(),
                ordinal: u32::try_from(parts.len()).unwrap_or(u32::MAX),
                attachment_id: *attachment_id,
            });
        }
        Ok(Self {
            id: Uuid::now_v7(),
            chat_id,
            author: MessageAuthor::User,
            author_bot_id: None,
            status: MessageStatus::Completed,
            parts,
            reply_to_message_id,
            mentioned_bot_ids,
            created_at_ms: now_ms,
            completed_at_ms: Some(now_ms),
            error_json: None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChatDomainError {
    #[error("Message must contain text or an attachment")]
    EmptyMessage,
    #[error("Message text exceeds the 100,000 character limit")]
    MessageTooLong,
    #[error("Message author is invalid")]
    InvalidAuthor,
    #[error("Message status is invalid")]
    InvalidStatus,
    #[error("Activity status is invalid")]
    InvalidActivityStatus,
    #[error("Approval status is invalid")]
    InvalidApprovalStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_requires_content_and_preserves_metadata() {
        let chat_id = Uuid::now_v7();
        assert_eq!(
            ChatMessage::user(chat_id, " ", &[], None, Vec::new(), 1),
            Err(ChatDomainError::EmptyMessage)
        );
        let reply = Uuid::now_v7();
        let mentioned = Uuid::now_v7();
        let message = ChatMessage::user(
            chat_id,
            " Hello ",
            &[Uuid::now_v7()],
            Some(reply),
            vec![mentioned],
            2,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(message.parts.len(), 2);
        assert_eq!(message.reply_to_message_id, Some(reply));
        assert_eq!(message.mentioned_bot_ids, vec![mentioned]);
    }
}
