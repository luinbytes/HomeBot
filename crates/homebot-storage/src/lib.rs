//! Durable `SQLite` persistence and resumable event outbox.

use homebot_domain::{
    Bot, BotAttention, BotId, DomainError,
    chat::{ChatDomainError, ChatMessage, DirectChat, MessagePart, QueuedPrompt},
};
use serde_json::Value;
use sqlx::{
    Row, SqlitePool,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, str::FromStr, time::Duration};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 5;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database migration failed: {0}")]
    Migration(#[from] MigrateError),
    #[error("database operation failed: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error("backup or restore failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("database path is not valid UTF-8")]
    InvalidPath,
    #[error("domain validation failed: {0}")]
    Domain(#[from] DomainError),
    #[error("Bot was not found")]
    BotNotFound,
    #[error("A Bot with that name already exists")]
    DuplicateBotName,
    #[error("chat validation failed: {0}")]
    ChatDomain(#[from] ChatDomainError),
    #[error("Chat was not found")]
    ChatNotFound,
    #[error("Message was not found")]
    MessageNotFound,
    #[error("An attachment is unavailable")]
    AttachmentUnavailable,
    #[error("database JSON is invalid: {0}")]
    Serialization(String),
}

#[derive(Clone, Debug)]
pub struct Storage {
    pool: SqlitePool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxEvent {
    pub sequence: u64,
    pub event_id: Uuid,
    pub owner_id: Uuid,
    pub event_kind: String,
    pub payload: Value,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyClaim {
    Claimed { operation_id: Uuid },
    Replayed { operation_id: Uuid },
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayWindow {
    Available(Vec<OutboxEvent>),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub storage_path: Option<String>,
    pub status: String,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
    pub finalized_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachmentClaim {
    Claimed(AttachmentRecord),
    Replayed(AttachmentRecord),
    Conflict,
}

impl Storage {
    /// Opens or creates a database, applies migrations, and verifies integrity.
    ///
    /// # Errors
    ///
    /// Fails closed if the database cannot be opened, migrated, or verified.
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        let options =
            SqliteConnectOptions::from_str(path.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        let storage = Self { pool };
        storage.verify_integrity().await?;
        Ok(storage)
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Lists the owner's active or complete Bot roster.
    ///
    /// # Errors
    ///
    /// Returns an error for database or integrity failures.
    pub async fn list_bots(
        &self,
        owner_id: Uuid,
        include_archived: bool,
    ) -> Result<Vec<Bot>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM bots WHERE owner_id = ? AND (? OR archived_at_ms IS NULL)
             ORDER BY archived_at_ms IS NOT NULL, name COLLATE NOCASE, id",
        )
        .bind(owner_id.to_string())
        .bind(include_archived)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(bot_from_row).collect()
    }

    /// Loads one owner-scoped Bot.
    ///
    /// # Errors
    ///
    /// Returns `BotNotFound` or a database/integrity failure.
    pub async fn get_bot(&self, owner_id: Uuid, bot_id: Uuid) -> Result<Bot, StorageError> {
        let row = sqlx::query("SELECT * FROM bots WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(bot_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::BotNotFound)?;
        bot_from_row(&row)
    }

    /// Persists a validated Bot while enforcing owner-scoped name uniqueness.
    ///
    /// # Errors
    ///
    /// Returns `DuplicateBotName` or a database failure.
    pub async fn create_bot(
        &self,
        owner_id: Uuid,
        mut bot: Bot,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        bot.created_at_ms = now_ms;
        bot.updated_at_ms = now_ms;
        let result = sqlx::query(
            "INSERT INTO bots (
                id, owner_id, name, title, description, provider_profile_id, shape, color,
                permission_profile, archived_at_ms, unread_count, attention, created_at_ms, updated_at_ms
             )
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
             WHERE NOT EXISTS (
                SELECT 1 FROM bots WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))
             )",
        )
        .bind(bot.id.0.to_string())
        .bind(owner_id.to_string())
        .bind(&bot.name)
        .bind(&bot.title)
        .bind(&bot.description)
        .bind(bot.provider_profile_id.map(|id| id.to_string()))
        .bind(bot.shape.as_str())
        .bind(bot.color.as_str())
        .bind(bot.permission_profile.as_str())
        .bind(bot.archived_at_ms)
        .bind(bot.unread_count)
        .bind(bot.attention.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(&bot.name)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::DuplicateBotName);
        }
        Ok(bot)
    }

    /// Persists mutable Bot settings.
    ///
    /// # Errors
    ///
    /// Returns not-found, duplicate-name, validation, or database errors.
    pub async fn update_bot(
        &self,
        owner_id: Uuid,
        mut bot: Bot,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        bot.updated_at_ms = now_ms;
        let result = sqlx::query(
            "UPDATE bots SET name = ?, title = ?, description = ?, provider_profile_id = ?,
                shape = ?, color = ?, permission_profile = ?, updated_at_ms = ?
             WHERE owner_id = ? AND id = ? AND NOT EXISTS (
                SELECT 1 FROM bots duplicate
                WHERE duplicate.owner_id = ? AND duplicate.id <> ?
                  AND lower(trim(duplicate.name)) = lower(trim(?))
             )",
        )
        .bind(&bot.name)
        .bind(&bot.title)
        .bind(&bot.description)
        .bind(bot.provider_profile_id.map(|id| id.to_string()))
        .bind(bot.shape.as_str())
        .bind(bot.color.as_str())
        .bind(bot.permission_profile.as_str())
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(bot.id.0.to_string())
        .bind(owner_id.to_string())
        .bind(bot.id.0.to_string())
        .bind(&bot.name)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            if self.get_bot(owner_id, bot.id.0).await.is_ok() {
                return Err(StorageError::DuplicateBotName);
            }
            return Err(StorageError::BotNotFound);
        }
        Ok(bot)
    }

    /// Applies an explicit archive or restore transition.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-transition, or database errors.
    pub async fn set_bot_archived(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        archived: bool,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let mut bot = self.get_bot(owner_id, bot_id).await?;
        if archived {
            bot.archive(now_ms)?;
        } else {
            bot.restore(now_ms)?;
        }
        sqlx::query(
            "UPDATE bots SET archived_at_ms = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(bot.archived_at_ms)
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(bot_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(bot)
    }

    /// Clears an existing Bot's unread count.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn mark_bot_read(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let result = sqlx::query(
            "UPDATE bots SET unread_count = 0, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(bot_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::BotNotFound);
        }
        self.get_bot(owner_id, bot_id).await
    }

    /// Updates server-owned unread and attention state.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn set_bot_attention(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        unread_count: u32,
        attention: BotAttention,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let result = sqlx::query(
            "UPDATE bots SET unread_count = ?, attention = ?, updated_at_ms = ?
             WHERE owner_id = ? AND id = ?",
        )
        .bind(unread_count)
        .bind(attention.as_str())
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(bot_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::BotNotFound);
        }
        self.get_bot(owner_id, bot_id).await
    }

    /// Creates or returns the owner's one direct chat for a Bot.
    ///
    /// # Errors
    ///
    /// Returns not-found when the Bot is not owned, or a database/integrity error.
    pub async fn create_direct_chat(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        proposed_chat_id: Uuid,
        now_ms: i64,
    ) -> Result<DirectChat, StorageError> {
        let bot = self.get_bot(owner_id, bot_id).await?;
        if bot.archived_at_ms.is_some() {
            return Err(StorageError::BotNotFound);
        }
        sqlx::query(
            "INSERT INTO chats (
                id, owner_id, kind, title, direct_bot_id, created_at_ms, updated_at_ms
             ) VALUES (?, ?, 'direct', ?, ?, ?, ?)
             ON CONFLICT(owner_id, direct_bot_id) WHERE kind = 'direct' AND direct_bot_id IS NOT NULL
             DO NOTHING",
        )
        .bind(proposed_chat_id.to_string())
        .bind(owner_id.to_string())
        .bind(&bot.name)
        .bind(bot_id.to_string())
        .bind(now_ms)
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        self.get_direct_chat_for_bot(owner_id, bot_id).await
    }

    /// Loads the owner's direct chat for one Bot.
    ///
    /// # Errors
    ///
    /// Returns not-found or database/integrity errors.
    pub async fn get_direct_chat_for_bot(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
    ) -> Result<DirectChat, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM chats
             WHERE owner_id = ? AND kind = 'direct' AND direct_bot_id = ?",
        )
        .bind(owner_id.to_string())
        .bind(bot_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::ChatNotFound)?;
        direct_chat_from_row(&row)
    }

    /// Loads an owner-scoped direct chat.
    ///
    /// # Errors
    ///
    /// Returns not-found or database/integrity errors.
    pub async fn get_direct_chat(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<DirectChat, StorageError> {
        let row =
            sqlx::query("SELECT * FROM chats WHERE owner_id = ? AND id = ? AND kind = 'direct'")
                .bind(owner_id.to_string())
                .bind(chat_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StorageError::ChatNotFound)?;
        direct_chat_from_row(&row)
    }

    /// Lists all direct chats for an owner.
    ///
    /// # Errors
    ///
    /// Returns database or integrity errors.
    pub async fn list_direct_chats(&self, owner_id: Uuid) -> Result<Vec<DirectChat>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM chats WHERE owner_id = ? AND kind = 'direct'
             ORDER BY updated_at_ms DESC, id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(direct_chat_from_row).collect()
    }

    /// Appends a validated user message and rich parts atomically.
    ///
    /// # Errors
    ///
    /// Returns validation, ownership, attachment, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_user_message(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        message_id: Uuid,
        content: &str,
        attachment_ids: &[Uuid],
        reply_to_message_id: Option<Uuid>,
        mentioned_bot_ids: Vec<Uuid>,
        now_ms: i64,
    ) -> Result<ChatMessage, StorageError> {
        let _ = self.get_direct_chat(owner_id, chat_id).await?;
        let mut message = ChatMessage::user(
            chat_id,
            content,
            attachment_ids,
            reply_to_message_id,
            mentioned_bot_ids,
            now_ms,
        )?;
        message.id = message_id;
        let mut transaction = self.pool.begin().await?;
        validate_message_references(&mut transaction, owner_id, &message).await?;
        sqlx::query(
            "INSERT INTO messages (
                id, chat_id, author_kind, status, reply_to_message_id,
                mentioned_bot_ids_json, created_at_ms, completed_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(message.id.to_string())
        .bind(chat_id.to_string())
        .bind(message.author.as_str())
        .bind(message.status.as_str())
        .bind(message.reply_to_message_id.map(|id| id.to_string()))
        .bind(serde_json::to_value(&message.mentioned_bot_ids).map_err(|error| json_error(&error))?)
        .bind(now_ms)
        .bind(message.completed_at_ms)
        .execute(&mut *transaction)
        .await?;
        for part in &message.parts {
            let (part_id, ordinal) = message_part_identity(part);
            sqlx::query(
                "INSERT INTO message_parts (id, message_id, ordinal, kind, content_json)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(part_id.to_string())
            .bind(message.id.to_string())
            .bind(i64::from(ordinal))
            .bind(message_part_kind(part))
            .bind(serde_json::to_value(part).map_err(|error| json_error(&error))?)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("UPDATE chats SET updated_at_ms = ? WHERE id = ? AND owner_id = ?")
            .bind(now_ms)
            .bind(chat_id.to_string())
            .bind(owner_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(message)
    }

    /// Loads all durable messages and rich parts for a direct chat.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn chat_messages(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<ChatMessage>, StorageError> {
        let _ = self.get_direct_chat(owner_id, chat_id).await?;
        let rows =
            sqlx::query("SELECT * FROM messages WHERE chat_id = ? ORDER BY created_at_ms, id")
                .bind(chat_id.to_string())
                .fetch_all(&self.pool)
                .await?;
        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let mut message = chat_message_from_row(&row)?;
            let parts: Vec<Value> = sqlx::query_scalar(
                "SELECT content_json FROM message_parts
                 WHERE message_id = ? ORDER BY ordinal",
            )
            .bind(message.id.to_string())
            .fetch_all(&self.pool)
            .await?;
            message.parts = parts
                .into_iter()
                .map(|value| serde_json::from_value(value).map_err(|error| json_error(&error)))
                .collect::<Result<_, _>>()?;
            messages.push(message);
        }
        Ok(messages)
    }

    /// Queues a follow-up while a direct chat is running.
    ///
    /// # Errors
    ///
    /// Returns validation, ownership, attachment, or database errors.
    pub async fn enqueue_prompt(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        prompt_id: Uuid,
        content: &str,
        attachment_ids: &[Uuid],
        now_ms: i64,
    ) -> Result<QueuedPrompt, StorageError> {
        let chat = self.get_direct_chat(owner_id, chat_id).await?;
        if !chat.running {
            return Err(StorageError::Integrity(
                "cannot queue a prompt while the chat is idle".to_owned(),
            ));
        }
        let validation =
            ChatMessage::user(chat_id, content, attachment_ids, None, Vec::new(), now_ms)?;
        let mut transaction = self.pool.begin().await?;
        validate_message_references(&mut transaction, owner_id, &validation).await?;
        let next_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(max(position) + 1, 0) FROM queued_prompts WHERE chat_id = ?",
        )
        .bind(chat_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO queued_prompts (
                id, owner_id, chat_id, content, attachment_ids_json, position, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(prompt_id.to_string())
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .bind(content.trim())
        .bind(serde_json::to_value(attachment_ids).map_err(|error| json_error(&error))?)
        .bind(next_position)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE chats SET queued_count = queued_count + 1, updated_at_ms = ?
             WHERE id = ? AND owner_id = ?",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(QueuedPrompt {
            id: prompt_id,
            owner_id,
            chat_id,
            content: content.trim().to_owned(),
            attachment_ids: attachment_ids.to_vec(),
            position: u32::try_from(next_position)
                .map_err(|_| StorageError::Integrity("invalid queue position".to_owned()))?,
            created_at_ms: now_ms,
        })
    }

    /// Loads queued prompts in stable execution order.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn queued_prompts(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<QueuedPrompt>, StorageError> {
        let _ = self.get_direct_chat(owner_id, chat_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM queued_prompts
             WHERE owner_id = ? AND chat_id = ? ORDER BY position",
        )
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(queued_prompt_from_row).collect()
    }

    /// Updates the authoritative running state of a direct chat.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn set_chat_running(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        running: bool,
        now_ms: i64,
    ) -> Result<DirectChat, StorageError> {
        let result = sqlx::query(
            "UPDATE chats SET running = ?, updated_at_ms = ? WHERE id = ? AND owner_id = ?",
        )
        .bind(running)
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::ChatNotFound);
        }
        self.get_direct_chat(owner_id, chat_id).await
    }

    /// Runs `SQLite` structural and foreign-key checks without attempting repair.
    ///
    /// # Errors
    ///
    /// Returns an integrity error when corruption or broken references are reported.
    pub async fn verify_integrity(&self) -> Result<(), StorageError> {
        let result: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&self.pool)
            .await?;
        if result != "ok" {
            return Err(StorageError::Integrity(result));
        }
        if sqlx::query("PRAGMA foreign_key_check")
            .fetch_optional(&self.pool)
            .await?
            .is_some()
        {
            return Err(StorageError::Integrity(
                "foreign key check reported an invalid reference".to_owned(),
            ));
        }
        Ok(())
    }

    /// Atomically appends a durable event and returns its monotonic sequence.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the event cannot be committed.
    pub async fn append_event(
        &self,
        owner_id: Uuid,
        event_kind: &str,
        payload: &Value,
        created_at_ms: i64,
    ) -> Result<OutboxEvent, StorageError> {
        let event_id = Uuid::now_v7();
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query("INSERT INTO event_outbox (event_id, owner_id, event_kind, payload_json, created_at_ms) VALUES (?, ?, ?, ?, ?)")
            .bind(event_id.to_string()).bind(owner_id.to_string()).bind(event_kind).bind(payload).bind(created_at_ms)
            .execute(&mut *transaction).await?;
        let sequence = u64::try_from(result.last_insert_rowid())
            .map_err(|_| StorageError::Integrity("negative outbox sequence".to_owned()))?;
        let mut stored_payload = payload.clone();
        if let Value::Object(object) = &mut stored_payload
            && object.contains_key("sequence")
        {
            object.insert("sequence".to_owned(), Value::from(sequence));
            sqlx::query("UPDATE event_outbox SET payload_json = ? WHERE sequence = ?")
                .bind(&stored_payload)
                .bind(i64::try_from(sequence).map_err(|_| {
                    StorageError::Integrity("outbox sequence exceeds SQLite range".to_owned())
                })?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(OutboxEvent {
            sequence,
            event_id,
            owner_id,
            event_kind: event_kind.to_owned(),
            payload: stored_payload,
            created_at_ms,
        })
    }

    /// Claims an idempotency key or returns the durable prior operation.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the claim cannot be resolved atomically.
    pub async fn claim_idempotency(
        &self,
        key: Uuid,
        request_hash: &str,
        proposed_operation_id: Uuid,
        created_at_ms: i64,
    ) -> Result<IdempotencyClaim, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO idempotency_records (key, request_hash, operation_id, created_at_ms) VALUES (?, ?, ?, ?) ON CONFLICT(key) DO NOTHING",
        )
        .bind(key.to_string())
        .bind(request_hash)
        .bind(proposed_operation_id.to_string())
        .bind(created_at_ms)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        let outcome = if inserted == 1 {
            IdempotencyClaim::Claimed {
                operation_id: proposed_operation_id,
            }
        } else {
            let row = sqlx::query(
                "SELECT request_hash, operation_id FROM idempotency_records WHERE key = ?",
            )
            .bind(key.to_string())
            .fetch_one(&mut *transaction)
            .await?;
            let existing_hash: String = row.try_get("request_hash")?;
            if existing_hash == request_hash {
                let operation_id: String = row.try_get("operation_id")?;
                IdempotencyClaim::Replayed {
                    operation_id: parse_uuid(&operation_id)?,
                }
            } else {
                IdempotencyClaim::Conflict
            }
        };
        transaction.commit().await?;
        Ok(outcome)
    }

    /// Claims an attachment-create key and durably stores pending metadata.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the claim cannot be resolved atomically.
    pub async fn claim_attachment_create(
        &self,
        idempotency_key: Uuid,
        request_hash: &str,
        proposed: &AttachmentRecord,
    ) -> Result<AttachmentClaim, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT request_hash, attachment_id FROM attachment_create_requests WHERE idempotency_key = ?",
        )
        .bind(idempotency_key.to_string())
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = existing {
            let existing_hash: String = row.try_get("request_hash")?;
            if existing_hash != request_hash {
                transaction.commit().await?;
                return Ok(AttachmentClaim::Conflict);
            }
            let attachment_id: String = row.try_get("attachment_id")?;
            let record = fetch_attachment(&mut *transaction, &attachment_id, &proposed.owner_id)
                .await?
                .ok_or_else(|| {
                    StorageError::Integrity(
                        "attachment idempotency record references missing metadata".to_owned(),
                    )
                })?;
            transaction.commit().await?;
            return Ok(AttachmentClaim::Replayed(record));
        }

        let size_bytes = i64::try_from(proposed.size_bytes).map_err(|_| {
            StorageError::Integrity("attachment size exceeds SQLite range".to_owned())
        })?;
        sqlx::query("INSERT INTO attachments (id, owner_id, filename, media_type, size_bytes, sha256, storage_path, status, expires_at_ms, created_at_ms, finalized_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(proposed.id.to_string())
            .bind(proposed.owner_id.to_string())
            .bind(&proposed.filename)
            .bind(&proposed.media_type)
            .bind(size_bytes)
            .bind(&proposed.sha256)
            .bind(&proposed.storage_path)
            .bind(&proposed.status)
            .bind(proposed.expires_at_ms)
            .bind(proposed.created_at_ms)
            .bind(proposed.finalized_at_ms)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("INSERT INTO attachment_create_requests (idempotency_key, request_hash, attachment_id) VALUES (?, ?, ?)")
            .bind(idempotency_key.to_string())
            .bind(request_hash)
            .bind(proposed.id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(AttachmentClaim::Claimed(proposed.clone()))
    }

    /// Returns attachment metadata visible to an owner.
    ///
    /// # Errors
    ///
    /// Returns a storage error if metadata cannot be decoded.
    pub async fn attachment(
        &self,
        owner_id: Uuid,
        attachment_id: Uuid,
    ) -> Result<Option<AttachmentRecord>, StorageError> {
        let mut connection = self.pool.acquire().await?;
        fetch_attachment(&mut *connection, &attachment_id.to_string(), &owner_id).await
    }

    /// Atomically makes a verified pending attachment consumable.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the state transition cannot be stored.
    pub async fn mark_attachment_ready(
        &self,
        owner_id: Uuid,
        attachment_id: Uuid,
        storage_path: &str,
        finalized_at_ms: i64,
    ) -> Result<bool, StorageError> {
        let updated = sqlx::query("UPDATE attachments SET status = 'ready', storage_path = ?, finalized_at_ms = ? WHERE id = ? AND owner_id = ? AND status = 'pending' AND expires_at_ms >= ?")
            .bind(storage_path)
            .bind(finalized_at_ms)
            .bind(attachment_id.to_string())
            .bind(owner_id.to_string())
            .bind(finalized_at_ms)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(updated == 1)
    }

    /// Replays owner-authorised events strictly after `sequence`.
    ///
    /// # Errors
    ///
    /// Returns a storage error when retained events cannot be decoded.
    pub async fn events_after(
        &self,
        owner_id: Uuid,
        sequence: u64,
        limit: u32,
    ) -> Result<Vec<OutboxEvent>, StorageError> {
        let sequence = i64::try_from(sequence).map_err(|_| {
            StorageError::Integrity("outbox cursor exceeds SQLite range".to_owned())
        })?;
        let rows = sqlx::query("SELECT sequence, event_id, owner_id, event_kind, payload_json, created_at_ms FROM event_outbox WHERE owner_id = ? AND sequence > ? ORDER BY sequence ASC LIMIT ?")
            .bind(owner_id.to_string()).bind(sequence).bind(i64::from(limit.clamp(1, 1_000))).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_event).collect()
    }

    /// Returns retained events after a cursor, or reports that a snapshot is required.
    ///
    /// A cursor is unavailable when retention has advanced past it or when it is
    /// ahead of the durable stream. This prevents silently accepting a gap.
    ///
    /// # Errors
    ///
    /// Returns a storage error when retention metadata or events cannot be read.
    pub async fn replay_after(
        &self,
        owner_id: Uuid,
        sequence: u64,
        limit: u32,
    ) -> Result<ReplayWindow, StorageError> {
        let sequence_i64 = i64::try_from(sequence).map_err(|_| {
            StorageError::Integrity("outbox cursor exceeds SQLite range".to_owned())
        })?;
        let minimum: Option<i64> = sqlx::query_scalar(
            "SELECT minimum_resume_sequence FROM event_retention_cursors WHERE owner_id = ?",
        )
        .bind(owner_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let floor = minimum.map_or(0, |value| u64::try_from(value).unwrap_or(0));
        let latest = self.latest_sequence(owner_id).await?.max(floor);
        if minimum.is_some_and(|floor| sequence_i64 < floor) || sequence > latest {
            return Ok(ReplayWindow::Unavailable);
        }
        Ok(ReplayWindow::Available(
            self.events_after(owner_id, sequence, limit).await?,
        ))
    }

    /// Prunes an owner's events through a sequence and advances its resume floor atomically.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the retention boundary cannot be committed.
    pub async fn prune_events_through(
        &self,
        owner_id: Uuid,
        sequence: u64,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let sequence = i64::try_from(sequence).map_err(|_| {
            StorageError::Integrity("outbox cursor exceeds SQLite range".to_owned())
        })?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO event_retention_cursors (owner_id, minimum_resume_sequence, updated_at_ms) VALUES (?, ?, ?) ON CONFLICT(owner_id) DO UPDATE SET minimum_resume_sequence = max(minimum_resume_sequence, excluded.minimum_resume_sequence), updated_at_ms = excluded.updated_at_ms",
        )
        .bind(owner_id.to_string())
        .bind(sequence)
        .bind(updated_at_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM event_outbox WHERE owner_id = ? AND sequence <= ?")
            .bind(owner_id.to_string())
            .bind(sequence)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Returns the latest global outbox sequence visible to an owner.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the cursor cannot be read or represented.
    pub async fn latest_sequence(&self, owner_id: Uuid) -> Result<u64, StorageError> {
        let value: Option<i64> =
            sqlx::query_scalar("SELECT max(sequence) FROM event_outbox WHERE owner_id = ?")
                .bind(owner_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        u64::try_from(value.unwrap_or(0))
            .map_err(|_| StorageError::Integrity("negative outbox sequence".to_owned()))
    }

    /// Creates a consistent backup without overwriting an existing destination.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination exists or `SQLite` cannot write it.
    pub async fn backup(&self, destination: &Path) -> Result<(), StorageError> {
        if destination.exists() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "backup destination already exists",
            )));
        }
        sqlx::query("VACUUM INTO ?")
            .bind(destination.to_str().ok_or(StorageError::InvalidPath)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Restores only into a new destination, preserving any existing data.
    ///
    /// # Errors
    ///
    /// Returns an I/O error rather than overwriting an existing database.
    pub fn restore(backup: &Path, destination: &Path) -> Result<(), StorageError> {
        let mut source = std::fs::File::open(backup)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()?;
        Ok(())
    }
}

fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<OutboxEvent, StorageError> {
    let sequence: i64 = row.try_get("sequence")?;
    let event_id: String = row.try_get("event_id")?;
    let owner_id: String = row.try_get("owner_id")?;
    Ok(OutboxEvent {
        sequence: u64::try_from(sequence)
            .map_err(|_| StorageError::Integrity("negative outbox sequence".to_owned()))?,
        event_id: parse_uuid(&event_id)?,
        owner_id: parse_uuid(&owner_id)?,
        event_kind: row.try_get("event_kind")?,
        payload: row.try_get("payload_json")?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn bot_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Bot, StorageError> {
    let id: String = row.try_get("id")?;
    let provider_profile_id: Option<String> = row.try_get("provider_profile_id")?;
    let unread_count: i64 = row.try_get("unread_count")?;
    let shape: String = row.try_get("shape")?;
    let color: String = row.try_get("color")?;
    let permission_profile: String = row.try_get("permission_profile")?;
    let attention: String = row.try_get("attention")?;
    Ok(Bot {
        id: BotId(parse_uuid(&id)?),
        name: row.try_get("name")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        shape: shape.parse()?,
        color: color.parse()?,
        provider_profile_id: provider_profile_id.as_deref().map(parse_uuid).transpose()?,
        permission_profile: permission_profile.parse()?,
        archived_at_ms: row.try_get("archived_at_ms")?,
        unread_count: u32::try_from(unread_count)
            .map_err(|_| StorageError::Integrity("invalid Bot unread count".to_owned()))?,
        attention: attention.parse()?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn direct_chat_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<DirectChat, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    let bot_id: String = row.try_get("direct_bot_id")?;
    let unread_count: i64 = row.try_get("unread_count")?;
    let queued_count: i64 = row.try_get("queued_count")?;
    let last_sequence: i64 = row.try_get("last_sequence")?;
    Ok(DirectChat {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        bot_id: parse_uuid(&bot_id)?,
        title: row.try_get("title")?,
        unread_count: u32::try_from(unread_count)
            .map_err(|_| StorageError::Integrity("invalid chat unread count".to_owned()))?,
        running: row.try_get("running")?,
        queued_count: u32::try_from(queued_count)
            .map_err(|_| StorageError::Integrity("invalid queued count".to_owned()))?,
        last_sequence: u64::try_from(last_sequence)
            .map_err(|_| StorageError::Integrity("invalid chat sequence".to_owned()))?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn chat_message_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ChatMessage, StorageError> {
    let id: String = row.try_get("id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let author: String = row.try_get("author_kind")?;
    let author_bot_id: Option<String> = row.try_get("author_bot_id")?;
    let status: String = row.try_get("status")?;
    let reply_to: Option<String> = row.try_get("reply_to_message_id")?;
    let mentioned: Value = row.try_get("mentioned_bot_ids_json")?;
    let error_json: Option<Value> = row.try_get("error_json")?;
    Ok(ChatMessage {
        id: parse_uuid(&id)?,
        chat_id: parse_uuid(&chat_id)?,
        author: author.parse()?,
        author_bot_id: author_bot_id.as_deref().map(parse_uuid).transpose()?,
        status: status.parse()?,
        parts: Vec::new(),
        reply_to_message_id: reply_to.as_deref().map(parse_uuid).transpose()?,
        mentioned_bot_ids: serde_json::from_value(mentioned).map_err(|error| json_error(&error))?,
        created_at_ms: row.try_get("created_at_ms")?,
        completed_at_ms: row.try_get("completed_at_ms")?,
        error_json,
    })
}

fn queued_prompt_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<QueuedPrompt, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let attachment_ids: Value = row.try_get("attachment_ids_json")?;
    let position: i64 = row.try_get("position")?;
    Ok(QueuedPrompt {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        chat_id: parse_uuid(&chat_id)?,
        content: row.try_get("content")?,
        attachment_ids: serde_json::from_value(attachment_ids)
            .map_err(|error| json_error(&error))?,
        position: u32::try_from(position)
            .map_err(|_| StorageError::Integrity("invalid queue position".to_owned()))?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

async fn validate_message_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: Uuid,
    message: &ChatMessage,
) -> Result<(), StorageError> {
    if let Some(reply_id) = message.reply_to_message_id {
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM messages WHERE id = ? AND chat_id = ?")
                .bind(reply_id.to_string())
                .bind(message.chat_id.to_string())
                .fetch_one(&mut **transaction)
                .await?;
        if exists != 1 {
            return Err(StorageError::MessageNotFound);
        }
    }
    for bot_id in &message.mentioned_bot_ids {
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bots WHERE id = ? AND owner_id = ?")
                .bind(bot_id.to_string())
                .bind(owner_id.to_string())
                .fetch_one(&mut **transaction)
                .await?;
        if exists != 1 {
            return Err(StorageError::BotNotFound);
        }
    }
    for part in &message.parts {
        if let MessagePart::Attachment { attachment_id, .. } = part {
            let exists: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM attachments
                 WHERE id = ? AND owner_id = ? AND status = 'ready'",
            )
            .bind(attachment_id.to_string())
            .bind(owner_id.to_string())
            .fetch_one(&mut **transaction)
            .await?;
            if exists != 1 {
                return Err(StorageError::AttachmentUnavailable);
            }
        }
    }
    Ok(())
}

const fn message_part_kind(part: &MessagePart) -> &'static str {
    match part {
        MessagePart::Text { .. } => "text",
        MessagePart::Attachment { .. } => "attachment",
        MessagePart::Notice { .. } => "notice",
    }
}

const fn message_part_identity(part: &MessagePart) -> (Uuid, u32) {
    match part {
        MessagePart::Text { id, ordinal, .. }
        | MessagePart::Attachment { id, ordinal, .. }
        | MessagePart::Notice { id, ordinal, .. } => (*id, *ordinal),
    }
}

fn json_error(error: &serde_json::Error) -> StorageError {
    StorageError::Serialization(error.to_string())
}

async fn fetch_attachment<'e, E>(
    executor: E,
    attachment_id: &str,
    owner_id: &Uuid,
) -> Result<Option<AttachmentRecord>, StorageError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query("SELECT id, owner_id, filename, media_type, size_bytes, sha256, storage_path, status, expires_at_ms, created_at_ms, finalized_at_ms FROM attachments WHERE id = ? AND owner_id = ?")
        .bind(attachment_id)
        .bind(owner_id.to_string())
        .fetch_optional(executor)
        .await?;
    row.map(|row| {
        let id: String = row.try_get("id")?;
        let owner_id: String = row.try_get("owner_id")?;
        let size_bytes: i64 = row.try_get("size_bytes")?;
        Ok(AttachmentRecord {
            id: parse_uuid(&id)?,
            owner_id: parse_uuid(&owner_id)?,
            filename: row.try_get("filename")?,
            media_type: row.try_get("media_type")?,
            size_bytes: u64::try_from(size_bytes)
                .map_err(|_| StorageError::Integrity("negative attachment size".to_owned()))?,
            sha256: row.try_get("sha256")?,
            storage_path: row.try_get("storage_path")?,
            status: row.try_get("status")?,
            expires_at_ms: row.try_get("expires_at_ms")?,
            created_at_ms: row.try_get("created_at_ms")?,
            finalized_at_ms: row.try_get("finalized_at_ms")?,
        })
    })
    .transpose()
}

fn parse_uuid(value: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(value)
        .map_err(|_| StorageError::Integrity("database contains an invalid UUID".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn bot_lifecycle_is_owner_scoped_unique_and_durable() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let owner = Uuid::now_v7();
        let other_owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let mut nova = Bot::create("Nova", "Research")?;
        nova.description = "Finds useful context".to_owned();
        let nova = storage.create_bot(owner, nova, 10).await?;
        assert!(matches!(
            storage
                .create_bot(owner, Bot::create(" nova ", "Duplicate")?, 11)
                .await,
            Err(StorageError::DuplicateBotName)
        ));
        assert!(
            storage
                .create_bot(other_owner, Bot::create("Nova", "Separate owner")?, 11)
                .await
                .is_ok()
        );

        let mut edited = nova.clone();
        edited.update_identity(
            "Nova",
            "Lead researcher",
            "Updated",
            homebot_domain::BotShape::Hexagon,
            homebot_domain::BotColor::Blue,
        )?;
        let edited = storage.update_bot(owner, edited, 12).await?;
        assert_eq!(edited.shape, homebot_domain::BotShape::Hexagon);
        let archived = storage.set_bot_archived(owner, nova.id.0, true, 13).await?;
        assert_eq!(archived.archived_at_ms, Some(13));
        assert!(storage.list_bots(owner, false).await?.is_empty());
        assert_eq!(storage.list_bots(owner, true).await?.len(), 1);

        storage.pool.close().await;
        let reopened = Storage::open(&database).await?;
        let restored = reopened
            .set_bot_archived(owner, nova.id.0, false, 14)
            .await?;
        assert_eq!(restored.title, "Lead researcher");
        let attention = reopened
            .set_bot_attention(owner, nova.id.0, 3, BotAttention::NeedsApproval, 15)
            .await?;
        assert_eq!(attention.unread_count, 3);
        assert_eq!(
            reopened
                .mark_bot_read(owner, nova.id.0, 16)
                .await?
                .unread_count,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn direct_chat_messages_are_unique_owner_scoped_and_restart_durable()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let owner = Uuid::now_v7();
        let other_owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let chat_id = Uuid::now_v7();
        let chat = storage
            .create_direct_chat(owner, bot.id.0, chat_id, 2)
            .await?;
        assert_eq!(chat.id, chat_id);
        assert_eq!(
            storage
                .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 3)
                .await?
                .id,
            chat_id
        );
        let first_id = Uuid::now_v7();
        let first = storage
            .append_user_message(
                owner,
                chat_id,
                first_id,
                " Hello ",
                &[],
                None,
                vec![bot.id.0],
                4,
            )
            .await?;
        assert_eq!(first.id, first_id);
        let second = storage
            .append_user_message(
                owner,
                chat_id,
                Uuid::now_v7(),
                "Reply",
                &[],
                Some(first_id),
                Vec::new(),
                5,
            )
            .await?;
        assert_eq!(second.reply_to_message_id, Some(first_id));
        storage.set_chat_running(owner, chat_id, true, 6).await?;
        let queued = storage
            .enqueue_prompt(owner, chat_id, Uuid::now_v7(), "Next", &[], 7)
            .await?;
        assert_eq!(queued.position, 0);
        assert_eq!(storage.queued_prompts(owner, chat_id).await?.len(), 1);
        assert!(matches!(
            storage.get_direct_chat(other_owner, chat_id).await,
            Err(StorageError::ChatNotFound)
        ));
        storage.pool.close().await;

        let reopened = Storage::open(&database).await?;
        let messages = reopened.chat_messages(owner, chat_id).await?;
        assert_eq!(messages.len(), 2);
        assert_eq!(reopened.queued_prompts(owner, chat_id).await?.len(), 1);
        assert!(matches!(
            &messages[0].parts[0],
            MessagePart::Text { text, .. } if text == "Hello"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn clean_install_restart_outbox_and_backup_are_durable() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let backup = directory.path().join("backup.db");
        let owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let first = storage
            .append_event(owner, "bot_changed", &json!({"name":"Ada"}), 1)
            .await?;
        let second = storage
            .append_event(owner, "message_delta", &json!({"delta":"Hi"}), 2)
            .await?;
        assert_eq!((first.sequence, second.sequence), (1, 2));
        storage.backup(&backup).await?;
        storage.pool.close().await;
        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened.events_after(owner, 0, 100).await?,
            vec![first, second]
        );
        reopened.pool.close().await;
        let restored_path = directory.path().join("restored.db");
        Storage::restore(&backup, &restored_path)?;
        let restored = Storage::open(&restored_path).await?;
        assert_eq!(restored.events_after(owner, 1, 100).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_writers_receive_unique_monotonic_sequences() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let owner = Uuid::now_v7();
        let payload = json!({});
        let (left, right) = tokio::join!(
            storage.append_event(owner, "left", &payload, 1),
            storage.append_event(owner, "right", &payload, 1)
        );
        let mut sequences = [left?.sequence, right?.sequence];
        sequences.sort_unstable();
        assert_eq!(sequences, [1, 2]);
        Ok(())
    }

    #[tokio::test]
    async fn restore_never_overwrites_existing_data() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source.db");
        let destination = directory.path().join("destination.db");
        std::fs::write(&source, b"source")?;
        std::fs::write(&destination, b"valuable")?;
        assert!(Storage::restore(&source, &destination).is_err());
        assert_eq!(std::fs::read(&destination)?, b"valuable");
        Ok(())
    }

    #[tokio::test]
    async fn clean_install_enables_wal_and_all_foundational_tables() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(storage.pool())
            .await?;
        assert_eq!(journal_mode, "wal");
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
                .fetch_all(storage.pool())
                .await?;
        for required in [
            "approvals",
            "attachments",
            "bots",
            "chats",
            "event_outbox",
            "event_retention_cursors",
            "messages",
            "paired_devices",
            "plugins",
            "provider_profiles",
            "routine_runs",
            "routines",
            "secret_references",
            "skills",
            "vcs_metadata",
            "workspaces",
        ] {
            assert!(
                tables.iter().any(|table| table == required),
                "missing {required}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn corrupted_database_fails_closed() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        std::fs::write(&database, b"not a sqlite database")?;
        assert!(Storage::open(&database).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn wal_allows_reader_during_writer_activity() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let owner = Uuid::now_v7();
        let payload = json!({"safe": true});
        let (write, read): (Result<OutboxEvent, StorageError>, Result<i64, sqlx::Error>) = tokio::join!(
            storage.append_event(owner, "concurrent", &payload, 1),
            sqlx::query_scalar("SELECT count(*) FROM bots").fetch_one(storage.pool())
        );
        assert_eq!(write?.sequence, 1);
        assert_eq!(read?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn failed_schema_transaction_rolls_back_every_change() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let mut transaction = storage.pool().begin().await?;
        sqlx::query("CREATE TABLE migration_probe (id INTEGER PRIMARY KEY)")
            .execute(&mut *transaction)
            .await?;
        let failure = sqlx::query("CREATE TABLE broken (")
            .execute(&mut *transaction)
            .await;
        assert!(failure.is_err());
        transaction.rollback().await?;
        let table_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'migration_probe'",
        )
        .fetch_one(storage.pool())
        .await?;
        assert_eq!(table_count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn idempotency_replays_same_request_and_rejects_key_reuse() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let key = Uuid::now_v7();
        let operation = Uuid::now_v7();
        assert_eq!(
            storage
                .claim_idempotency(key, "hash-a", operation, 1)
                .await?,
            IdempotencyClaim::Claimed {
                operation_id: operation
            }
        );
        assert_eq!(
            storage
                .claim_idempotency(key, "hash-a", Uuid::now_v7(), 2)
                .await?,
            IdempotencyClaim::Replayed {
                operation_id: operation
            }
        );
        assert_eq!(
            storage
                .claim_idempotency(key, "hash-b", Uuid::now_v7(), 3)
                .await?,
            IdempotencyClaim::Conflict
        );
        Ok(())
    }

    #[tokio::test]
    async fn replay_window_detects_pruned_and_future_cursors() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let owner = Uuid::now_v7();
        storage
            .append_event(owner, "one", &json!({"sequence": 0}), 1)
            .await?;
        let second = storage
            .append_event(owner, "two", &json!({"sequence": 0}), 2)
            .await?;
        storage.prune_events_through(owner, 1, 3).await?;

        assert_eq!(
            storage.replay_after(owner, 0, 100).await?,
            ReplayWindow::Unavailable
        );
        assert_eq!(
            storage.replay_after(owner, 1, 100).await?,
            ReplayWindow::Available(vec![second])
        );
        storage.prune_events_through(owner, 2, 4).await?;
        assert_eq!(
            storage.replay_after(owner, 2, 100).await?,
            ReplayWindow::Available(Vec::new())
        );
        assert_eq!(
            storage.replay_after(owner, 3, 100).await?,
            ReplayWindow::Unavailable
        );
        Ok(())
    }

    #[tokio::test]
    async fn attachment_create_is_idempotent_and_ready_transition_is_guarded()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let key = Uuid::now_v7();
        let record = AttachmentRecord {
            id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            filename: "report.txt".to_owned(),
            media_type: "text/plain".to_owned(),
            size_bytes: 5,
            sha256: "a".repeat(64),
            storage_path: None,
            status: "pending".to_owned(),
            expires_at_ms: 100,
            created_at_ms: 1,
            finalized_at_ms: None,
        };
        assert!(matches!(
            storage.claim_attachment_create(key, "hash", &record).await?,
            AttachmentClaim::Claimed(value) if value == record
        ));
        assert!(matches!(
            storage.claim_attachment_create(key, "hash", &record).await?,
            AttachmentClaim::Replayed(value) if value == record
        ));
        assert_eq!(
            storage
                .claim_attachment_create(key, "other", &record)
                .await?,
            AttachmentClaim::Conflict
        );
        assert!(
            storage
                .mark_attachment_ready(record.owner_id, record.id, "objects/hash", 50)
                .await?
        );
        assert!(
            !storage
                .mark_attachment_ready(record.owner_id, record.id, "objects/hash", 51)
                .await?
        );
        assert_eq!(
            storage
                .attachment(record.owner_id, record.id)
                .await?
                .map(|value| value.status),
            Some("ready".to_owned())
        );
        Ok(())
    }
}
