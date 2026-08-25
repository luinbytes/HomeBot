//! Durable `SQLite` persistence and resumable event outbox.

use homebot_domain::{
    Bot, BotAttention, BotId, DomainError,
    chat::{
        ChatApproval, ChatDomainError, ChatMessage, DirectChat, ExecutionActivity, GroupBotStatus,
        GroupChat, GroupParticipant, GroupParticipantRole, MessageAuthor, MessagePart,
        MessageStatus, OwnershipHandoff, QueuedPrompt, QueuedPromptKind,
    },
};
use homebot_routines::{
    OverlapPolicy, RecordedAction, RoutineDefinition, RoutineExecutionResult,
    RoutineTriggerDefinition,
};
use homebot_skills::{AppliedSkill, SkillDefinition};
use homebot_vcs::{CheckpointPhase, ConversationReconciliation, WorkspaceMode};
use serde_json::Value;
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 24;
static MIGRATOR: std::sync::LazyLock<sqlx::migrate::Migrator> = std::sync::LazyLock::new(|| {
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    use std::borrow::Cow;

    let migrations = [
        (1, "initial", include_str!("../migrations/0001_initial.sql")),
        (
            2,
            "event retention",
            include_str!("../migrations/0002_event_retention.sql"),
        ),
        (
            3,
            "attachments",
            include_str!("../migrations/0003_attachments.sql"),
        ),
        (
            4,
            "bot lifecycle",
            include_str!("../migrations/0004_bot_lifecycle.sql"),
        ),
        (
            5,
            "direct chat",
            include_str!("../migrations/0005_direct_chat.sql"),
        ),
        (
            6,
            "group coordination",
            include_str!("../migrations/0006_group_coordination.sql"),
        ),
        (
            7,
            "activity artifacts",
            include_str!("../migrations/0007_activity_artifacts.sql"),
        ),
        (
            8,
            "secret references",
            include_str!("../migrations/0008_secret_references.sql"),
        ),
        (9, "plugins", include_str!("../migrations/0009_plugins.sql")),
        (
            10,
            "routines",
            include_str!("../migrations/0010_routines.sql"),
        ),
        (
            11,
            "routine scheduler",
            include_str!("../migrations/0011_routine_scheduler.sql"),
        ),
        (12, "skills", include_str!("../migrations/0012_skills.sql")),
        (
            13,
            "workspaces",
            include_str!("../migrations/0013_workspaces.sql"),
        ),
        (
            14,
            "checkpoints",
            include_str!("../migrations/0014_checkpoints.sql"),
        ),
        (
            15,
            "vcs operations",
            include_str!("../migrations/0015_vcs_operations.sql"),
        ),
        (
            16,
            "working context",
            include_str!("../migrations/0016_working_context.sql"),
        ),
        (
            17,
            "device pairing",
            include_str!("../migrations/0017_device_pairing.sql"),
        ),
        (
            18,
            "bot parity",
            include_str!("../migrations/0018_bot_parity.sql"),
        ),
        (
            19,
            "message reactions",
            include_str!("../migrations/0019_message_reactions.sql"),
        ),
        (
            20,
            "message references",
            include_str!("../migrations/0020_message_references.sql"),
        ),
        (
            21,
            "capability rules",
            include_str!("../migrations/0021_capability_rules.sql"),
        ),
        (
            22,
            "browser sessions",
            include_str!("../migrations/0022_browser_sessions.sql"),
        ),
        (
            23,
            "browser takeover leases",
            include_str!("../migrations/0023_browser_takeover_leases.sql"),
        ),
        (
            24,
            "pairing provenance",
            include_str!("../migrations/0024_pairing_provenance.sql"),
        ),
    ]
    .into_iter()
    .map(|(version, description, sql)| {
        Migration::new(
            version,
            Cow::Borrowed(description),
            MigrationType::Simple,
            Cow::Borrowed(sql),
            false,
        )
    })
    .collect::<Vec<_>>();
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
});

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
    #[error("database schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
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
    #[error("Approval was not found or is no longer pending")]
    ApprovalNotFound,
    #[error("Group chat requires two to six distinct active Bots")]
    InvalidGroupParticipants,
    #[error("Group coordination turn limit was reached or the group was stopped")]
    CoordinationLimitReached,
    #[error("Group ownership handoff is invalid")]
    InvalidOwnershipHandoff,
    #[error("An attachment is unavailable")]
    AttachmentUnavailable,
    #[error("Secret reference was not found")]
    SecretNotFound,
    #[error("A secret with that label already exists")]
    DuplicateSecretLabel,
    #[error("Plugin was not found")]
    PluginNotFound,
    #[error("A plugin with that name already exists")]
    DuplicatePluginName,
    #[error("Routine was not found")]
    RoutineNotFound,
    #[error("A routine with that name already exists")]
    DuplicateRoutineName,
    #[error("Routine recording was not found or is no longer active")]
    RoutineRecordingNotFound,
    #[error("Routine trigger was not found")]
    RoutineTriggerNotFound,
    #[error("Routine job was not found")]
    RoutineJobNotFound,
    #[error("Skill was not found")]
    SkillNotFound,
    #[error("A Skill with that name already exists")]
    DuplicateSkillName,
    #[error("Repository workspace was not found")]
    WorkspaceNotFound,
    #[error("This repository is already registered")]
    DuplicateWorkspacePath,
    #[error("This chat already has a workspace")]
    DuplicateChatWorkspace,
    #[error("Turn checkpoint was not found")]
    CheckpointNotFound,
    #[error("A working-context operation is already running")]
    WorkingContextBusy,
    #[error("Pairing credential was not found")]
    PairingNotFound,
    #[error("Pairing credential expired")]
    PairingExpired,
    #[error("Pairing credential was already used")]
    PairingConsumed,
    #[error("Pairing origin did not match the generated endpoint")]
    PairingOriginMismatch,
    #[error("Pairing exchange is rate limited")]
    PairingRateLimited,
    #[error("Device session was not found")]
    DeviceSessionNotFound,
    #[error("Browser takeover lease changed concurrently")]
    BrowserTakeoverConflict,
    #[error("database JSON is invalid: {0}")]
    Serialization(String),
}

fn map_unique<T>(
    result: Result<T, sqlx::Error>,
    duplicate: StorageError,
) -> Result<T, StorageError> {
    match result {
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Err(duplicate),
        Err(error) => Err(StorageError::Sql(error)),
        Ok(value) => Ok(value),
    }
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
pub struct ArtifactRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub activity_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub storage_path: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRoute {
    pub profile_id: Uuid,
    pub adapter_kind: String,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderProfileRecord {
    pub id: Uuid,
    pub adapter_kind: String,
    pub display_name: String,
    pub configuration: Value,
    pub secret_reference_id: Option<Uuid>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingContextRecord {
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub provider_profile_id: Uuid,
    pub interaction_mode: String,
    pub used_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub compaction_status: String,
    pub generation: u32,
    pub compacted_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingCredentialRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub endpoint: String,
    pub expected_origin: String,
    pub endpoint_kind: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub consumed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceSessionRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub endpoint_kind: String,
    pub created_at_ms: i64,
    pub last_seen_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretReferenceRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub locator: String,
    pub label: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub configuration: Value,
    pub enabled: bool,
    pub connection_id: Uuid,
    pub transport: String,
    pub status: String,
    pub auth_status: String,
    pub error_message: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginToolRecord {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
}

pub struct PluginConnectionUpdate<'a> {
    pub enabled: bool,
    pub status: &'a str,
    pub auth_status: &'a str,
    pub error_message: Option<&'a str>,
    pub tools: &'a [PluginToolRecord],
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub draft: bool,
    pub active_version_id: Uuid,
    pub version: u32,
    pub definition: RoutineDefinition,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineRecordingRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    pub description: String,
    pub actions: Vec<RecordedAction>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineRunRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub routine_id: Uuid,
    pub routine_version_id: Uuid,
    pub bot_id: Uuid,
    pub status: String,
    pub trigger: Value,
    pub dry_run: bool,
    pub inputs: Value,
    pub results: Option<Vec<RoutineExecutionResult>>,
    pub error_message: Option<String>,
    pub attempt_count: u16,
    pub scheduled_for_ms: Option<i64>,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineTriggerRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub routine_id: Uuid,
    pub definition: RoutineTriggerDefinition,
    pub enabled: bool,
    pub last_evaluated_at_ms: Option<i64>,
    pub next_fire_at_ms: Option<i64>,
    pub last_event_sequence: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoutineJobRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub trigger_id: Uuid,
    pub routine_id: Uuid,
    pub routine_version_id: Uuid,
    pub delivery_key: String,
    pub trigger: Value,
    pub inputs: Value,
    pub status: String,
    pub attempt_count: u16,
    pub scheduled_for_ms: i64,
    pub next_attempt_at_ms: i64,
    pub cancel_requested: bool,
    pub error_message: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub description: String,
    pub active_version_id: Uuid,
    pub version: u32,
    pub definition: SkillDefinition,
    pub bot_ids: Vec<Uuid>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryWorkspaceRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub root_path: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatWorkspaceRecord {
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub workspace_id: Uuid,
    pub mode: WorkspaceMode,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    pub base_ref: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnCheckpointRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub workspace_id: Uuid,
    pub message_id: Option<Uuid>,
    pub phase: CheckpointPhase,
    pub git_ref: String,
    pub commit_oid: String,
    pub provider_profile_id: Option<Uuid>,
    pub provider_conversation_id: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointRestoreRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub checkpoint_id: Uuid,
    pub safety_checkpoint_id: Uuid,
    pub reconciliation: ConversationReconciliation,
    pub previous_provider_conversation_id: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VcsOperationResultRecord {
    pub idempotency_key: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub action: String,
    pub response: Value,
    pub created_at_ms: i64,
}

pub struct QueuedPromptInput<'a> {
    pub content: &'a str,
    pub attachment_ids: &'a [Uuid],
    pub applied_skills: &'a [AppliedSkill],
    pub references: &'a [(MessageReferenceKind, Uuid)],
    pub kind: QueuedPromptKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromotedQueuedPrompt {
    pub prompt: QueuedPrompt,
    pub message: ChatMessage,
    pub applied_skills: Vec<AppliedSkill>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageReactionRecord {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_user: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityRuleRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub capability: String,
    pub effect: String,
    pub device_id: Option<Uuid>,
    pub bot_id: Option<Uuid>,
    pub chat_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub action_prefix: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityRuleAuditRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub rule_id: Uuid,
    pub action: String,
    pub snapshot: Value,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserProfileRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub display_name: String,
    pub directory_ref: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserSessionRecord {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub chat_id: Uuid,
    pub bot_id: Uuid,
    pub profile_id: Uuid,
    pub runtime_session_id: Option<Uuid>,
    pub profile_name: String,
    pub directory_ref: String,
    pub current_url: Option<String>,
    pub controller: String,
    pub status: String,
    pub pending_approval_id: Option<Uuid>,
    pub controlling_device_id: Option<Uuid>,
    pub takeover_expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRecord {
    pub kind: SearchRecordKind,
    pub title: String,
    pub snippet: String,
    pub chat_id: Option<Uuid>,
    pub message_id: Option<Uuid>,
    pub artifact_id: Option<Uuid>,
    pub routine_id: Option<Uuid>,
    pub created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchRecordKind {
    Message,
    File,
    Link,
    Routine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageReferenceKind {
    Bot,
    Group,
    Routine,
    Plugin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageReferenceRecord {
    pub kind: MessageReferenceKind,
    pub target_id: Uuid,
    pub target_version_id: Option<Uuid>,
    pub label_snapshot: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutineJobClaim {
    Claimed,
    Replayed,
}

pub struct RoutineUpdate<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub definition: &'a RoutineDefinition,
    pub draft: bool,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachmentClaim {
    Claimed(AttachmentRecord),
    Replayed(AttachmentRecord),
    Conflict,
}

impl Storage {
    /// Creates or updates a server-owned provider profile without persisting secret values.
    ///
    /// # Errors
    /// Returns database or serialization errors.
    pub async fn upsert_provider_profile(
        &self,
        profile: &ProviderProfileRecord,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO provider_profiles
             (id, adapter_kind, display_name, configuration_json, secret_reference_id,
              created_at_ms, updated_at_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               adapter_kind = excluded.adapter_kind,
               display_name = excluded.display_name,
               configuration_json = excluded.configuration_json,
               secret_reference_id = excluded.secret_reference_id,
               updated_at_ms = excluded.updated_at_ms",
        )
        .bind(profile.id.to_string())
        .bind(&profile.adapter_kind)
        .bind(&profile.display_name)
        .bind(&profile.configuration)
        .bind(profile.secret_reference_id.map(|id| id.to_string()))
        .bind(profile.created_at_ms)
        .bind(profile.updated_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists safe provider profile configuration. Secret values are never stored here.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn provider_profiles(&self) -> Result<Vec<ProviderProfileRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, adapter_kind, display_name, configuration_json, secret_reference_id,
                    created_at_ms, updated_at_ms
             FROM provider_profiles ORDER BY display_name, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProviderProfileRecord {
                    id: parse_uuid(&row.try_get::<String, _>("id")?)?,
                    adapter_kind: row.try_get("adapter_kind")?,
                    display_name: row.try_get("display_name")?,
                    configuration: row.try_get("configuration_json")?,
                    secret_reference_id: row
                        .try_get::<Option<String>, _>("secret_reference_id")?
                        .map(|id| parse_uuid(&id))
                        .transpose()?,
                    created_at_ms: row.try_get("created_at_ms")?,
                    updated_at_ms: row.try_get("updated_at_ms")?,
                })
            })
            .collect()
    }

    /// Searches owner-authorised durable content in stable recency order.
    ///
    /// Links are derived from matching message text without changing the transcript. Results
    /// retain immutable IDs so renames cannot make a previously returned target ambiguous.
    ///
    /// # Errors
    /// Returns database, UUID, or JSON integrity errors.
    #[allow(clippy::too_many_lines)]
    pub async fn search(
        &self,
        owner_id: Uuid,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchRecord>, StorageError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", query.to_lowercase());
        let candidate_limit = i64::from(limit.clamp(1, 100)).saturating_mul(4);
        let rows = sqlx::query(
            "SELECT m.id AS message_id, m.chat_id, m.created_at_ms,
                    json_extract(p.content_json, '$.text') AS body
             FROM message_parts p
             JOIN messages m ON m.id = p.message_id
             JOIN chats c ON c.id = m.chat_id
             WHERE c.owner_id = ? AND p.kind IN ('text', 'notice')
               AND lower(CAST(json_extract(p.content_json, '$.text') AS TEXT)) LIKE ?
             ORDER BY m.created_at_ms DESC, m.id, p.ordinal LIMIT ?",
        )
        .bind(owner_id.to_string())
        .bind(&pattern)
        .bind(candidate_limit)
        .fetch_all(&self.pool)
        .await?;
        let mut results = Vec::new();
        for row in rows {
            let message_id = parse_uuid(&row.try_get::<String, _>("message_id")?)?;
            let chat_id = parse_uuid(&row.try_get::<String, _>("chat_id")?)?;
            let created_at_ms = row.try_get("created_at_ms")?;
            let body: String = row.try_get("body")?;
            results.push(SearchRecord {
                kind: SearchRecordKind::Message,
                title: "Message".to_owned(),
                snippet: search_snippet(&body, query),
                chat_id: Some(chat_id),
                message_id: Some(message_id),
                artifact_id: None,
                routine_id: None,
                created_at_ms,
            });
            for link in
                links_in(&body).filter(|link| link.to_lowercase().contains(&query.to_lowercase()))
            {
                results.push(SearchRecord {
                    kind: SearchRecordKind::Link,
                    title: link.clone(),
                    snippet: search_snippet(&body, &link),
                    chat_id: Some(chat_id),
                    message_id: Some(message_id),
                    artifact_id: None,
                    routine_id: None,
                    created_at_ms,
                });
            }
        }
        let rows = sqlx::query(
            "SELECT id, chat_id, message_id, name, kind, created_at_ms FROM artifacts
             WHERE owner_id = ? AND (lower(name) LIKE ? OR lower(kind) LIKE ?)
             ORDER BY created_at_ms DESC, id LIMIT ?",
        )
        .bind(owner_id.to_string())
        .bind(&pattern)
        .bind(&pattern)
        .bind(candidate_limit)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            results.push(SearchRecord {
                kind: SearchRecordKind::File,
                title: row.try_get("name")?,
                snippet: row.try_get("kind")?,
                chat_id: Some(parse_uuid(&row.try_get::<String, _>("chat_id")?)?),
                message_id: row
                    .try_get::<Option<String>, _>("message_id")?
                    .map(|value| parse_uuid(&value))
                    .transpose()?,
                artifact_id: Some(parse_uuid(&row.try_get::<String, _>("id")?)?),
                routine_id: None,
                created_at_ms: row.try_get("created_at_ms")?,
            });
        }
        let rows = sqlx::query(
            "SELECT a.id, a.filename, a.media_type, m.id AS message_id, m.chat_id,
                    m.created_at_ms
             FROM message_parts p
             JOIN messages m ON m.id = p.message_id
             JOIN chats c ON c.id = m.chat_id
             JOIN attachments a ON a.id = json_extract(p.content_json, '$.attachment_id')
             WHERE c.owner_id = ? AND a.owner_id = ? AND p.kind = 'attachment'
               AND a.status = 'ready'
               AND (lower(a.filename) LIKE ? OR lower(a.media_type) LIKE ?)
             ORDER BY m.created_at_ms DESC, a.id LIMIT ?",
        )
        .bind(owner_id.to_string())
        .bind(owner_id.to_string())
        .bind(&pattern)
        .bind(&pattern)
        .bind(candidate_limit)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            results.push(SearchRecord {
                kind: SearchRecordKind::File,
                title: row.try_get("filename")?,
                snippet: row.try_get("media_type")?,
                chat_id: Some(parse_uuid(&row.try_get::<String, _>("chat_id")?)?),
                message_id: Some(parse_uuid(&row.try_get::<String, _>("message_id")?)?),
                artifact_id: None,
                routine_id: None,
                created_at_ms: row.try_get("created_at_ms")?,
            });
        }
        let rows = sqlx::query(
            "SELECT id, name, description, updated_at_ms FROM routines
             WHERE owner_id = ? AND bot_id IS NOT NULL
               AND (lower(name) LIKE ? OR lower(description) LIKE ?)
             ORDER BY updated_at_ms DESC, id LIMIT ?",
        )
        .bind(owner_id.to_string())
        .bind(&pattern)
        .bind(&pattern)
        .bind(candidate_limit)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            results.push(SearchRecord {
                kind: SearchRecordKind::Routine,
                title: row.try_get("name")?,
                snippet: row.try_get("description")?,
                chat_id: None,
                message_id: None,
                artifact_id: None,
                routine_id: Some(parse_uuid(&row.try_get::<String, _>("id")?)?),
                created_at_ms: row.try_get("updated_at_ms")?,
            });
        }
        results.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| left.title.cmp(&right.title))
        });
        results.truncate(limit.clamp(1, 100) as usize);
        Ok(results)
    }

    /// Lists owner-scoped routines at their active immutable versions.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn list_routines(&self, owner_id: Uuid) -> Result<Vec<RoutineRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT r.id, r.owner_id, r.bot_id, r.name, r.description, r.enabled, r.draft,
                    r.active_version_id, v.version, v.definition_json, r.created_at_ms, r.updated_at_ms
             FROM routines r JOIN routine_versions v ON v.id = r.active_version_id
             WHERE r.owner_id = ? AND r.bot_id IS NOT NULL ORDER BY r.name COLLATE NOCASE, r.id",
        ).bind(owner_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(routine_from_row).collect()
    }

    /// Loads one owner-scoped routine and active version.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn routine(&self, owner_id: Uuid, id: Uuid) -> Result<RoutineRecord, StorageError> {
        let row = sqlx::query(
            "SELECT r.id, r.owner_id, r.bot_id, r.name, r.description, r.enabled, r.draft,
                    r.active_version_id, v.version, v.definition_json, r.created_at_ms, r.updated_at_ms
             FROM routines r JOIN routine_versions v ON v.id = r.active_version_id
             WHERE r.owner_id = ? AND r.id = ? AND r.bot_id IS NOT NULL",
        ).bind(owner_id.to_string()).bind(id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::RoutineNotFound)?;
        routine_from_row(&row)
    }

    /// Loads one immutable routine version with its current owner-scoped routine metadata.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn routine_version(
        &self,
        owner_id: Uuid,
        routine_id: Uuid,
        version_id: Uuid,
    ) -> Result<RoutineRecord, StorageError> {
        let row = sqlx::query(
            "SELECT r.id, r.owner_id, r.bot_id, r.name, r.description, r.enabled, r.draft,
                    r.active_version_id, r.created_at_ms, r.updated_at_ms,
                    v.id AS selected_version_id, v.version, v.definition_json
             FROM routines r JOIN routine_versions v ON v.routine_id = r.id
             WHERE r.owner_id = ? AND r.id = ? AND v.id = ?",
        )
        .bind(owner_id.to_string())
        .bind(routine_id.to_string())
        .bind(version_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::RoutineNotFound)?;
        let mut routine = routine_from_row_selected(&row, "selected_version_id")?;
        routine.active_version_id = version_id;
        Ok(routine)
    }

    /// Registers one canonical repository path for an owner.
    ///
    /// # Errors
    /// Returns duplicate-path or database errors.
    pub async fn create_repository_workspace(
        &self,
        record: &RepositoryWorkspaceRecord,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("INSERT INTO repository_workspaces (id, owner_id, name, root_path, root_path_normalized, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(&record.name)
            .bind(&record.root_path).bind(&record.root_path).bind(record.created_at_ms).bind(record.updated_at_ms)
            .execute(&self.pool).await;
        map_unique(result, StorageError::DuplicateWorkspacePath).map(|_| ())
    }

    /// Lists owner-scoped repository registrations.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn list_repository_workspaces(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<RepositoryWorkspaceRecord>, StorageError> {
        let rows = sqlx::query("SELECT id, owner_id, name, root_path, created_at_ms, updated_at_ms FROM repository_workspaces WHERE owner_id = ? ORDER BY name, id")
            .bind(owner_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(repository_workspace_from_row).collect()
    }

    /// Loads one owner-scoped repository registration.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn repository_workspace(
        &self,
        owner_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<RepositoryWorkspaceRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, name, root_path, created_at_ms, updated_at_ms FROM repository_workspaces WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string()).bind(workspace_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::WorkspaceNotFound)?;
        repository_workspace_from_row(&row)
    }

    /// Associates one chat with either its primary repository or an isolated worktree.
    ///
    /// # Errors
    /// Returns missing owner resources, duplicate association, or database errors.
    pub async fn attach_chat_workspace(
        &self,
        record: &ChatWorkspaceRecord,
    ) -> Result<(), StorageError> {
        let _ = self
            .repository_workspace(record.owner_id, record.workspace_id)
            .await?;
        let chat_exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM chats WHERE owner_id = ? AND id = ?")
                .bind(record.owner_id.to_string())
                .bind(record.chat_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        if chat_exists == 0 {
            return Err(StorageError::ChatNotFound);
        }
        let result = sqlx::query("INSERT INTO chat_workspaces (owner_id, chat_id, workspace_id, mode, worktree_path, branch_name, base_ref, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.owner_id.to_string()).bind(record.chat_id.to_string()).bind(record.workspace_id.to_string())
            .bind(workspace_mode(record.mode)).bind(&record.worktree_path).bind(&record.branch_name).bind(&record.base_ref)
            .bind(record.created_at_ms).bind(record.updated_at_ms).execute(&self.pool).await;
        map_unique(result, StorageError::DuplicateChatWorkspace).map(|_| ())
    }

    /// Loads an optional owner-scoped chat workspace.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn chat_workspace(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Option<ChatWorkspaceRecord>, StorageError> {
        let row = sqlx::query("SELECT owner_id, chat_id, workspace_id, mode, worktree_path, branch_name, base_ref, created_at_ms, updated_at_ms FROM chat_workspaces WHERE owner_id = ? AND chat_id = ?")
            .bind(owner_id.to_string()).bind(chat_id.to_string()).fetch_optional(&self.pool).await?;
        row.as_ref().map(chat_workspace_from_row).transpose()
    }

    /// Lists all owner-scoped chat workspace associations.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn list_chat_workspaces(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<ChatWorkspaceRecord>, StorageError> {
        let rows = sqlx::query("SELECT owner_id, chat_id, workspace_id, mode, worktree_path, branch_name, base_ref, created_at_ms, updated_at_ms FROM chat_workspaces WHERE owner_id = ? ORDER BY chat_id")
            .bind(owner_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(chat_workspace_from_row).collect()
    }

    /// Removes only the durable association after external cleanup has succeeded.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn detach_chat_workspace(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM chat_workspaces WHERE owner_id = ? AND chat_id = ?")
            .bind(owner_id.to_string())
            .bind(chat_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::WorkspaceNotFound);
        }
        Ok(())
    }

    /// Persists one hidden-ref checkpoint after validating its owner-scoped chat workspace.
    ///
    /// # Errors
    /// Returns missing workspace/chat or database errors.
    pub async fn create_turn_checkpoint(
        &self,
        record: &TurnCheckpointRecord,
    ) -> Result<(), StorageError> {
        let association = self
            .chat_workspace(record.owner_id, record.chat_id)
            .await?
            .ok_or(StorageError::WorkspaceNotFound)?;
        if association.workspace_id != record.workspace_id {
            return Err(StorageError::WorkspaceNotFound);
        }
        sqlx::query("INSERT INTO turn_checkpoints (id, owner_id, chat_id, workspace_id, message_id, phase, git_ref, commit_oid, provider_profile_id, provider_conversation_id, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.chat_id.to_string())
            .bind(record.workspace_id.to_string()).bind(record.message_id.map(|id| id.to_string()))
            .bind(checkpoint_phase(record.phase)).bind(&record.git_ref).bind(&record.commit_oid)
            .bind(record.provider_profile_id.map(|id| id.to_string())).bind(&record.provider_conversation_id)
            .bind(record.created_at_ms).execute(&self.pool).await?;
        Ok(())
    }

    /// Lists a chat's checkpoints in stable creation order.
    ///
    /// # Errors
    /// Returns ownership, database, or integrity errors.
    pub async fn turn_checkpoints(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<TurnCheckpointRecord>, StorageError> {
        let rows = sqlx::query("SELECT id, owner_id, chat_id, workspace_id, message_id, phase, git_ref, commit_oid, provider_profile_id, provider_conversation_id, created_at_ms FROM turn_checkpoints WHERE owner_id = ? AND chat_id = ? ORDER BY created_at_ms, id")
            .bind(owner_id.to_string()).bind(chat_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(turn_checkpoint_from_row).collect()
    }

    /// Loads one owner-scoped checkpoint.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn turn_checkpoint(
        &self,
        owner_id: Uuid,
        checkpoint_id: Uuid,
    ) -> Result<TurnCheckpointRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, chat_id, workspace_id, message_id, phase, git_ref, commit_oid, provider_profile_id, provider_conversation_id, created_at_ms FROM turn_checkpoints WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string()).bind(checkpoint_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::CheckpointNotFound)?;
        turn_checkpoint_from_row(&row)
    }

    /// Atomically stores a restore safety checkpoint/audit row and forks the provider mapping.
    ///
    /// # Errors
    /// Returns ownership, database, or integrity errors.
    pub async fn record_checkpoint_restore(
        &self,
        safety: &TurnCheckpointRecord,
        restore: &CheckpointRestoreRecord,
        bot_id: Uuid,
    ) -> Result<(), StorageError> {
        let target = self
            .turn_checkpoint(restore.owner_id, restore.checkpoint_id)
            .await?;
        if target.chat_id != restore.chat_id
            || safety.chat_id != restore.chat_id
            || target.workspace_id != safety.workspace_id
            || safety.phase != CheckpointPhase::RestoreSafety
        {
            return Err(StorageError::CheckpointNotFound);
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO turn_checkpoints (id, owner_id, chat_id, workspace_id, message_id, phase, git_ref, commit_oid, provider_profile_id, provider_conversation_id, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(safety.id.to_string()).bind(safety.owner_id.to_string()).bind(safety.chat_id.to_string())
            .bind(safety.workspace_id.to_string()).bind(safety.message_id.map(|id| id.to_string()))
            .bind(checkpoint_phase(safety.phase)).bind(&safety.git_ref).bind(&safety.commit_oid)
            .bind(safety.provider_profile_id.map(|id| id.to_string())).bind(&safety.provider_conversation_id)
            .bind(safety.created_at_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO checkpoint_restores (id, owner_id, chat_id, checkpoint_id, safety_checkpoint_id, reconciliation, previous_provider_conversation_id, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(restore.id.to_string()).bind(restore.owner_id.to_string()).bind(restore.chat_id.to_string())
            .bind(restore.checkpoint_id.to_string()).bind(restore.safety_checkpoint_id.to_string())
            .bind(conversation_reconciliation(restore.reconciliation)).bind(&restore.previous_provider_conversation_id)
            .bind(restore.created_at_ms).execute(&mut *transaction).await?;
        if restore.reconciliation == ConversationReconciliation::Forked
            && let Some(profile_id) = target.provider_profile_id
        {
            sqlx::query("DELETE FROM provider_conversations WHERE bot_id = ? AND chat_id = ? AND provider_profile_id = ?")
                .bind(bot_id.to_string()).bind(restore.chat_id.to_string()).bind(profile_id.to_string())
                .execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Loads one idempotent owner-scoped checkpoint restore audit row.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn checkpoint_restore(
        &self,
        owner_id: Uuid,
        restore_id: Uuid,
    ) -> Result<CheckpointRestoreRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, chat_id, checkpoint_id, safety_checkpoint_id, reconciliation, previous_provider_conversation_id, created_at_ms FROM checkpoint_restores WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string()).bind(restore_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::CheckpointNotFound)?;
        checkpoint_restore_from_row(&row)
    }

    /// Stores the exact response for a Git mutation so network retries never repeat it.
    ///
    /// # Errors
    /// Returns database or serialization errors.
    pub async fn record_vcs_operation_result(
        &self,
        record: &VcsOperationResultRecord,
    ) -> Result<(), StorageError> {
        let response = serde_json::to_string(&record.response)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO vcs_operation_results (idempotency_key, owner_id, chat_id, action, response_json, created_at_ms) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(record.idempotency_key.to_string()).bind(record.owner_id.to_string())
            .bind(record.chat_id.to_string()).bind(&record.action).bind(response)
            .bind(record.created_at_ms).execute(&self.pool).await?;
        Ok(())
    }

    /// Loads an exact owner/chat/action-scoped Git mutation response for idempotent replay.
    ///
    /// # Errors
    /// Returns database or corrupt-JSON errors.
    pub async fn vcs_operation_result(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        idempotency_key: Uuid,
        action: &str,
    ) -> Result<Option<VcsOperationResultRecord>, StorageError> {
        let row = sqlx::query("SELECT idempotency_key, owner_id, chat_id, action, response_json, created_at_ms FROM vcs_operation_results WHERE owner_id = ? AND chat_id = ? AND idempotency_key = ? AND action = ?")
            .bind(owner_id.to_string()).bind(chat_id.to_string()).bind(idempotency_key.to_string())
            .bind(action).fetch_optional(&self.pool).await?;
        row.as_ref().map(vcs_operation_result_from_row).transpose()
    }

    /// Creates a Skill and immutable version 1 atomically.
    ///
    /// # Errors
    /// Returns duplicate-name, serialization, or database errors.
    pub async fn create_skill(&self, record: &SkillRecord) -> Result<(), StorageError> {
        let definition = serde_json::to_string(&record.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        let duplicate: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE owner_id = ? AND name_normalized = ? AND deleted_at_ms IS NULL")
            .bind(record.owner_id.to_string()).bind(normalize_skill_name(&record.name)).fetch_one(&mut *transaction).await?;
        if duplicate > 0 {
            return Err(StorageError::DuplicateSkillName);
        }
        sqlx::query("INSERT INTO skills (id, owner_id, name, name_normalized, description, active_version_id, version, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(&record.name)
            .bind(normalize_skill_name(&record.name)).bind(&record.description).bind(record.active_version_id.to_string())
            .bind(i64::from(record.version)).bind(record.created_at_ms).bind(record.updated_at_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO skill_versions (id, skill_id, version, definition_json, name, description, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(record.active_version_id.to_string()).bind(record.id.to_string()).bind(i64::from(record.version))
            .bind(definition).bind(&record.name).bind(&record.description).bind(record.created_at_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Installs one curated Assistant Pack as a Skill, Bot assignment, routine, and trigger.
    ///
    /// # Errors
    /// Returns missing-Bot, duplicate-name, serialization, integrity, or database errors.
    pub async fn install_assistant_pack(
        &self,
        skill: &SkillRecord,
        routine: &RoutineRecord,
        trigger: &RoutineTriggerRecord,
    ) -> Result<(), StorageError> {
        if skill.owner_id != routine.owner_id
            || routine.owner_id != trigger.owner_id
            || skill.bot_ids.as_slice() != [routine.bot_id]
            || trigger.routine_id != routine.id
        {
            return Err(StorageError::Integrity(
                "Assistant Pack records do not share one owner, Bot, and routine".to_owned(),
            ));
        }
        let skill_definition = serde_json::to_string(&skill.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let routine_definition = serde_json::to_string(&routine.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let trigger_definition = serde_json::to_string(&trigger.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let last_event_sequence = i64::try_from(trigger.last_event_sequence)
            .map_err(|_| StorageError::Integrity("event cursor exceeds SQLite range".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        let bot_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bots WHERE owner_id = ? AND id = ?")
                .bind(routine.owner_id.to_string())
                .bind(routine.bot_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if bot_count == 0 {
            return Err(StorageError::BotNotFound);
        }
        let duplicate_skill: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE owner_id = ? AND name_normalized = ? AND deleted_at_ms IS NULL")
            .bind(skill.owner_id.to_string()).bind(normalize_skill_name(&skill.name)).fetch_one(&mut *transaction).await?;
        if duplicate_skill > 0 {
            return Err(StorageError::DuplicateSkillName);
        }
        let duplicate_routine: i64 = sqlx::query_scalar("SELECT count(*) FROM routines WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))")
            .bind(routine.owner_id.to_string()).bind(&routine.name).fetch_one(&mut *transaction).await?;
        if duplicate_routine > 0 {
            return Err(StorageError::DuplicateRoutineName);
        }
        sqlx::query("INSERT INTO skills (id, owner_id, name, name_normalized, description, active_version_id, version, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(skill.id.to_string()).bind(skill.owner_id.to_string()).bind(&skill.name)
            .bind(normalize_skill_name(&skill.name)).bind(&skill.description).bind(skill.active_version_id.to_string())
            .bind(i64::from(skill.version)).bind(skill.created_at_ms).bind(skill.updated_at_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO skill_versions (id, skill_id, version, definition_json, name, description, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(skill.active_version_id.to_string()).bind(skill.id.to_string()).bind(i64::from(skill.version))
            .bind(skill_definition).bind(&skill.name).bind(&skill.description).bind(skill.created_at_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO skill_bot_assignments (owner_id, skill_id, bot_id, assigned_at_ms) VALUES (?, ?, ?, ?)")
            .bind(skill.owner_id.to_string()).bind(skill.id.to_string()).bind(routine.bot_id.to_string())
            .bind(skill.created_at_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO routines (id, owner_id, bot_id, name, description, active_version_id, enabled, draft, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(routine.id.to_string()).bind(routine.owner_id.to_string()).bind(routine.bot_id.to_string())
            .bind(&routine.name).bind(&routine.description).bind(routine.active_version_id.to_string())
            .bind(routine.enabled).bind(routine.draft).bind(routine.created_at_ms).bind(routine.updated_at_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO routine_versions (id, routine_id, version, definition_json, created_at_ms) VALUES (?, ?, ?, ?, ?)")
            .bind(routine.active_version_id.to_string()).bind(routine.id.to_string()).bind(i64::from(routine.version))
            .bind(routine_definition).bind(routine.created_at_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO routine_triggers (id, owner_id, routine_id, kind, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(trigger.id.to_string()).bind(trigger.owner_id.to_string()).bind(trigger.routine_id.to_string())
            .bind(trigger_kind(&trigger.definition)).bind(trigger_definition).bind(trigger.enabled)
            .bind(trigger.last_evaluated_at_ms).bind(trigger.next_fire_at_ms).bind(last_event_sequence)
            .bind(trigger.created_at_ms).bind(trigger.updated_at_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Lists active owner-scoped Skills with current versions and Bot assignments.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn list_skills(&self, owner_id: Uuid) -> Result<Vec<SkillRecord>, StorageError> {
        let rows = sqlx::query("SELECT s.id, s.owner_id, s.name, s.description, s.active_version_id, s.version, v.definition_json, s.created_at_ms, s.updated_at_ms FROM skills s JOIN skill_versions v ON v.id = s.active_version_id WHERE s.owner_id = ? AND s.deleted_at_ms IS NULL ORDER BY s.name_normalized, s.id")
            .bind(owner_id.to_string()).fetch_all(&self.pool).await?;
        let mut records = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut record = skill_from_row(row)?;
            record.bot_ids = self.skill_bot_ids(owner_id, record.id).await?;
            records.push(record);
        }
        Ok(records)
    }

    /// Loads one active owner-scoped Skill.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn skill(&self, owner_id: Uuid, skill_id: Uuid) -> Result<SkillRecord, StorageError> {
        let row = sqlx::query("SELECT s.id, s.owner_id, s.name, s.description, s.active_version_id, s.version, v.definition_json, s.created_at_ms, s.updated_at_ms FROM skills s JOIN skill_versions v ON v.id = s.active_version_id WHERE s.owner_id = ? AND s.id = ? AND s.deleted_at_ms IS NULL")
            .bind(owner_id.to_string()).bind(skill_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::SkillNotFound)?;
        let mut record = skill_from_row(&row)?;
        record.bot_ids = self.skill_bot_ids(owner_id, skill_id).await?;
        Ok(record)
    }

    /// Loads an immutable Skill version for idempotent mutation replay.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn skill_version(
        &self,
        owner_id: Uuid,
        version_id: Uuid,
    ) -> Result<SkillRecord, StorageError> {
        let row = sqlx::query("SELECT s.id, s.owner_id, v.name, v.description, v.id AS active_version_id, v.version, v.definition_json, s.created_at_ms, v.created_at_ms AS updated_at_ms FROM skills s JOIN skill_versions v ON v.skill_id = s.id WHERE s.owner_id = ? AND v.id = ?")
            .bind(owner_id.to_string()).bind(version_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::SkillNotFound)?;
        let mut record = skill_from_row(&row)?;
        record.bot_ids = self.skill_bot_ids(owner_id, record.id).await?;
        Ok(record)
    }

    /// Creates an immutable Skill version and advances its active pointer atomically.
    ///
    /// # Errors
    /// Returns not-found, duplicate-name, serialization, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_skill(
        &self,
        owner_id: Uuid,
        skill_id: Uuid,
        name: &str,
        description: &str,
        definition: &SkillDefinition,
        version_id: Uuid,
        updated_at_ms: i64,
    ) -> Result<SkillRecord, StorageError> {
        let definition_json = serde_json::to_string(definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        // Make the first statement a write so SQLite acquires the write lock before this
        // transaction observes a snapshot. This avoids SQLITE_BUSY_SNAPSHOT when the scheduler
        // persists an unrelated event between a read and the version update.
        let normalized = normalize_skill_name(name);
        let version: Option<i64> = sqlx::query_scalar("UPDATE skills SET name = ?, name_normalized = ?, description = ?, active_version_id = ?, version = version + 1, updated_at_ms = ? WHERE owner_id = ? AND id = ? AND deleted_at_ms IS NULL AND version < 4294967295 AND NOT EXISTS (SELECT 1 FROM skills AS other WHERE other.owner_id = ? AND other.id != ? AND other.name_normalized = ? AND other.deleted_at_ms IS NULL) RETURNING version")
            .bind(name).bind(&normalized).bind(description).bind(version_id.to_string()).bind(updated_at_ms)
            .bind(owner_id.to_string()).bind(skill_id.to_string()).bind(owner_id.to_string()).bind(skill_id.to_string()).bind(&normalized)
            .fetch_optional(&mut *transaction).await?;
        let Some(version) = version else {
            transaction.rollback().await?;
            let current: Option<i64> = sqlx::query_scalar("SELECT version FROM skills WHERE owner_id = ? AND id = ? AND deleted_at_ms IS NULL")
                .bind(owner_id.to_string()).bind(skill_id.to_string()).fetch_optional(&self.pool).await?;
            let Some(current) = current else {
                return Err(StorageError::SkillNotFound);
            };
            let duplicate: i64 = sqlx::query_scalar("SELECT count(*) FROM skills WHERE owner_id = ? AND id != ? AND name_normalized = ? AND deleted_at_ms IS NULL")
                .bind(owner_id.to_string()).bind(skill_id.to_string()).bind(&normalized).fetch_one(&self.pool).await?;
            return if duplicate > 0 {
                Err(StorageError::DuplicateSkillName)
            } else {
                Err(StorageError::Integrity(format!(
                    "Skill version {current} cannot be incremented"
                )))
            };
        };
        sqlx::query("INSERT INTO skill_versions (id, skill_id, version, definition_json, name, description, created_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(version_id.to_string()).bind(skill_id.to_string()).bind(version).bind(definition_json).bind(name).bind(description).bind(updated_at_ms)
            .execute(&mut *transaction).await?;
        transaction.commit().await?;
        self.skill(owner_id, skill_id).await
    }

    /// Soft-deletes a Skill while preserving versions referenced by historical messages.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn delete_skill(
        &self,
        owner_id: Uuid,
        skill_id: Uuid,
        deleted_at_ms: i64,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query("UPDATE skills SET deleted_at_ms = ?, name_normalized = name_normalized || '#' || id, updated_at_ms = ? WHERE owner_id = ? AND id = ? AND deleted_at_ms IS NULL")
            .bind(deleted_at_ms).bind(deleted_at_ms).bind(owner_id.to_string()).bind(skill_id.to_string()).execute(&mut *transaction).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::SkillNotFound);
        }
        sqlx::query("DELETE FROM skill_bot_assignments WHERE owner_id = ? AND skill_id = ?")
            .bind(owner_id.to_string())
            .bind(skill_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Assigns or unassigns a Skill without granting any referenced tool capability.
    ///
    /// # Errors
    /// Returns missing Skill/Bot or database errors.
    pub async fn set_skill_assignment(
        &self,
        owner_id: Uuid,
        skill_id: Uuid,
        bot_id: Uuid,
        enabled: bool,
        assigned_at_ms: i64,
    ) -> Result<(), StorageError> {
        let _ = self.skill(owner_id, skill_id).await?;
        let _ = self.get_bot(owner_id, bot_id).await?;
        if enabled {
            sqlx::query("INSERT INTO skill_bot_assignments (owner_id, skill_id, bot_id, assigned_at_ms) VALUES (?, ?, ?, ?) ON CONFLICT(skill_id, bot_id) DO UPDATE SET assigned_at_ms = excluded.assigned_at_ms")
                .bind(owner_id.to_string()).bind(skill_id.to_string()).bind(bot_id.to_string()).bind(assigned_at_ms).execute(&self.pool).await?;
        } else {
            sqlx::query("DELETE FROM skill_bot_assignments WHERE owner_id = ? AND skill_id = ? AND bot_id = ?")
                .bind(owner_id.to_string()).bind(skill_id.to_string()).bind(bot_id.to_string()).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Resolves exact active versions assigned to a Bot or explicitly selected for a turn.
    ///
    /// # Errors
    /// Returns when an explicit Skill is missing, or on database/decoding errors.
    pub async fn resolve_applied_skills(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        explicit_skill_ids: &[Uuid],
    ) -> Result<Vec<AppliedSkill>, StorageError> {
        let assigned: HashSet<Uuid> = self
            .skill_bot_ids_for_bot(owner_id, bot_id)
            .await?
            .into_iter()
            .collect();
        let explicit: HashSet<Uuid> = explicit_skill_ids.iter().copied().collect();
        let mut resolved = self
            .list_skills(owner_id)
            .await?
            .into_iter()
            .filter(|skill| assigned.contains(&skill.id) || explicit.contains(&skill.id))
            .map(|skill| AppliedSkill {
                skill_id: skill.id,
                version_id: skill.active_version_id,
                name: skill.name,
                version: skill.version,
                definition: skill.definition,
            })
            .collect::<Vec<_>>();
        if explicit
            .iter()
            .any(|id| !resolved.iter().any(|skill| skill.skill_id == *id))
        {
            return Err(StorageError::SkillNotFound);
        }
        resolved.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.skill_id.cmp(&right.skill_id))
        });
        Ok(resolved)
    }

    async fn skill_bot_ids(
        &self,
        owner_id: Uuid,
        skill_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT bot_id FROM skill_bot_assignments WHERE owner_id = ? AND skill_id = ? ORDER BY bot_id")
            .bind(owner_id.to_string()).bind(skill_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(|value| parse_uuid(value)).collect()
    }

    async fn skill_bot_ids_for_bot(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT skill_id FROM skill_bot_assignments WHERE owner_id = ? AND bot_id = ? ORDER BY skill_id")
            .bind(owner_id.to_string()).bind(bot_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(|value| parse_uuid(value)).collect()
    }

    /// Creates a routine and immutable version 1 atomically.
    ///
    /// # Errors
    /// Returns missing-Bot, duplicate-name, serialization, or database errors.
    pub async fn create_routine(&self, record: &RoutineRecord) -> Result<(), StorageError> {
        let bot_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bots WHERE owner_id = ? AND id = ?")
                .bind(record.owner_id.to_string())
                .bind(record.bot_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        if bot_count == 0 {
            return Err(StorageError::BotNotFound);
        }
        let definition = serde_json::to_string(&record.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO routines (id, owner_id, bot_id, name, description, active_version_id, enabled, draft, created_at_ms, updated_at_ms)
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ? WHERE NOT EXISTS (
               SELECT 1 FROM routines WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))
             )",
        ).bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.bot_id.to_string())
            .bind(&record.name).bind(&record.description).bind(record.active_version_id.to_string())
            .bind(record.enabled).bind(record.draft).bind(record.created_at_ms).bind(record.updated_at_ms)
            .bind(record.owner_id.to_string()).bind(&record.name).execute(&mut *transaction).await?;
        if inserted.rows_affected() == 0 {
            return Err(StorageError::DuplicateRoutineName);
        }
        sqlx::query("INSERT INTO routine_versions (id, routine_id, version, definition_json, created_at_ms) VALUES (?, ?, 1, ?, ?)")
            .bind(record.active_version_id.to_string()).bind(record.id.to_string()).bind(definition)
            .bind(record.created_at_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Creates a new immutable version while retaining historical definitions.
    ///
    /// # Errors
    /// Returns not-found, duplicate-name, serialization, or database errors.
    pub async fn update_routine(
        &self,
        owner_id: Uuid,
        id: Uuid,
        update: RoutineUpdate<'_>,
    ) -> Result<RoutineRecord, StorageError> {
        let current = self.routine(owner_id, id).await?;
        let duplicate: i64 = sqlx::query_scalar("SELECT count(*) FROM routines WHERE owner_id = ? AND id != ? AND lower(trim(name)) = lower(trim(?))")
            .bind(owner_id.to_string()).bind(id.to_string()).bind(update.name).fetch_one(&self.pool).await?;
        if duplicate > 0 {
            return Err(StorageError::DuplicateRoutineName);
        }
        let version = current
            .version
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("routine version overflow".to_owned()))?;
        let version_id = Uuid::now_v7();
        let encoded = serde_json::to_string(update.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO routine_versions (id, routine_id, version, definition_json, created_at_ms) VALUES (?, ?, ?, ?, ?)")
            .bind(version_id.to_string()).bind(id.to_string()).bind(i64::from(version)).bind(encoded)
            .bind(update.updated_at_ms).execute(&mut *transaction).await?;
        sqlx::query("UPDATE routines SET name = ?, description = ?, active_version_id = ?, draft = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?")
            .bind(update.name).bind(update.description).bind(version_id.to_string()).bind(update.draft).bind(update.updated_at_ms)
            .bind(owner_id.to_string()).bind(id.to_string()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        self.routine(owner_id, id).await
    }

    /// Enables or disables an owner-scoped routine without changing its version.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn set_routine_enabled(
        &self,
        owner_id: Uuid,
        id: Uuid,
        enabled: bool,
        updated_at_ms: i64,
    ) -> Result<RoutineRecord, StorageError> {
        let result = sqlx::query(
            "UPDATE routines SET enabled = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(enabled)
        .bind(updated_at_ms)
        .bind(owner_id.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineNotFound);
        }
        self.routine(owner_id, id).await
    }

    /// Deletes an owner-scoped routine and all versions/runs.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn delete_routine(&self, owner_id: Uuid, id: Uuid) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM routines WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineNotFound);
        }
        Ok(())
    }

    /// Starts a durable demonstration recording.
    ///
    /// # Errors
    /// Returns missing-Bot, serialization, or database errors.
    pub async fn create_routine_recording(
        &self,
        record: &RoutineRecordingRecord,
    ) -> Result<(), StorageError> {
        let bot_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bots WHERE owner_id = ? AND id = ?")
                .bind(record.owner_id.to_string())
                .bind(record.bot_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        if bot_count == 0 {
            return Err(StorageError::BotNotFound);
        }
        sqlx::query("INSERT INTO routine_recordings (id, owner_id, bot_id, name, description, actions_json, status, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, '[]', 'recording', ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.bot_id.to_string())
            .bind(&record.name).bind(&record.description).bind(record.created_at_ms).bind(record.updated_at_ms)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Appends one structured action to an active recording.
    ///
    /// # Errors
    /// Returns not-found, serialization, or database errors.
    pub async fn append_routine_recording_action(
        &self,
        owner_id: Uuid,
        id: Uuid,
        action: &RecordedAction,
        updated_at_ms: i64,
    ) -> Result<RoutineRecordingRecord, StorageError> {
        let mut recording = self.routine_recording(owner_id, id).await?;
        if recording.actions.len() >= 256 {
            return Err(StorageError::Integrity(
                "routine recording step limit reached".to_owned(),
            ));
        }
        recording.actions.push(action.clone());
        let actions = serde_json::to_string(&recording.actions)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        sqlx::query("UPDATE routine_recordings SET actions_json = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ? AND status = 'recording'")
            .bind(actions).bind(updated_at_ms).bind(owner_id.to_string()).bind(id.to_string()).execute(&self.pool).await?;
        self.routine_recording(owner_id, id).await
    }

    /// Loads an active recording.
    ///
    /// # Errors
    /// Returns not-found, serialization, database, or integrity errors.
    pub async fn routine_recording(
        &self,
        owner_id: Uuid,
        id: Uuid,
    ) -> Result<RoutineRecordingRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, bot_id, name, description, actions_json, created_at_ms, updated_at_ms FROM routine_recordings WHERE owner_id = ? AND id = ? AND status = 'recording'")
            .bind(owner_id.to_string()).bind(id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::RoutineRecordingNotFound)?;
        routine_recording_from_row(&row)
    }

    /// Finishes or cancels a recording so it cannot accept more actions.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn close_routine_recording(
        &self,
        owner_id: Uuid,
        id: Uuid,
        finished: bool,
        updated_at_ms: i64,
    ) -> Result<RoutineRecordingRecord, StorageError> {
        let recording = self.routine_recording(owner_id, id).await?;
        let status = if finished { "finished" } else { "cancelled" };
        let result = sqlx::query("UPDATE routine_recordings SET status = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ? AND status = 'recording'")
            .bind(status).bind(updated_at_ms).bind(owner_id.to_string()).bind(id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineRecordingNotFound);
        }
        Ok(recording)
    }

    /// Persists a manual or dry-run result bound to the exact active version.
    ///
    /// # Errors
    /// Returns serialization or database errors.
    pub async fn create_routine_run(&self, record: &RoutineRunRecord) -> Result<(), StorageError> {
        let results = record
            .results
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO routine_runs (id, owner_id, routine_id, routine_version_id, bot_id, status, trigger_json, dry_run, input_json, result_json, error_message, attempt_count, scheduled_for_ms, started_at_ms, finished_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.routine_id.to_string())
            .bind(record.routine_version_id.to_string()).bind(record.bot_id.to_string()).bind(&record.status).bind(record.trigger.to_string()).bind(record.dry_run)
            .bind(record.inputs.to_string()).bind(results).bind(&record.error_message).bind(i64::from(record.attempt_count))
            .bind(record.scheduled_for_ms).bind(record.started_at_ms).bind(record.finished_at_ms)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Lists recent manual/dry runs with redacted structured results.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn routine_runs(
        &self,
        owner_id: Uuid,
        routine_id: Uuid,
    ) -> Result<Vec<RoutineRunRecord>, StorageError> {
        let rows = sqlx::query("SELECT rr.id, rr.owner_id, rr.routine_id, rr.routine_version_id, coalesce(rr.bot_id, r.bot_id) AS bot_id, rr.status, rr.trigger_json, rr.dry_run, rr.input_json, rr.result_json, rr.error_message, rr.attempt_count, rr.scheduled_for_ms, rr.started_at_ms, rr.finished_at_ms FROM routine_runs rr JOIN routines r ON r.id = rr.routine_id WHERE rr.owner_id = ? AND rr.routine_id = ? ORDER BY rr.started_at_ms DESC, rr.id LIMIT 100")
            .bind(owner_id.to_string()).bind(routine_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(routine_run_from_row).collect()
    }

    /// Creates an owner-scoped schedule, webhook, event, or plugin trigger.
    ///
    /// # Errors
    /// Returns not-found, serialization, or database errors.
    pub async fn create_routine_trigger(
        &self,
        record: &RoutineTriggerRecord,
    ) -> Result<(), StorageError> {
        let _ = self.routine(record.owner_id, record.routine_id).await?;
        let definition = serde_json::to_string(&record.definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let last_event_sequence = i64::try_from(record.last_event_sequence)
            .map_err(|_| StorageError::Integrity("event cursor exceeds SQLite range".to_owned()))?;
        sqlx::query("INSERT INTO routine_triggers (id, owner_id, routine_id, kind, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.routine_id.to_string())
            .bind(trigger_kind(&record.definition)).bind(definition).bind(record.enabled)
            .bind(record.last_evaluated_at_ms).bind(record.next_fire_at_ms).bind(last_event_sequence).bind(record.created_at_ms).bind(record.updated_at_ms)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// Lists durable owner-scoped triggers, optionally narrowed to one routine.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn routine_triggers(
        &self,
        owner_id: Uuid,
        routine_id: Option<Uuid>,
    ) -> Result<Vec<RoutineTriggerRecord>, StorageError> {
        let rows = if let Some(routine_id) = routine_id {
            sqlx::query("SELECT id, owner_id, routine_id, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms FROM routine_triggers WHERE owner_id = ? AND routine_id = ? ORDER BY created_at_ms, id")
                .bind(owner_id.to_string()).bind(routine_id.to_string()).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT id, owner_id, routine_id, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms FROM routine_triggers WHERE owner_id = ? ORDER BY created_at_ms, id")
                .bind(owner_id.to_string()).fetch_all(&self.pool).await?
        };
        rows.iter().map(routine_trigger_from_row).collect()
    }

    /// Loads one owner-scoped routine trigger.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn routine_trigger(
        &self,
        owner_id: Uuid,
        trigger_id: Uuid,
    ) -> Result<RoutineTriggerRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, routine_id, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms FROM routine_triggers WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string()).bind(trigger_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::RoutineTriggerNotFound)?;
        routine_trigger_from_row(&row)
    }

    /// Deletes one owner-scoped trigger and its pending jobs/delivery dedupe records.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn delete_routine_trigger(
        &self,
        owner_id: Uuid,
        trigger_id: Uuid,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM routine_triggers WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(trigger_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineTriggerNotFound);
        }
        Ok(())
    }

    /// Advances a trigger's durable schedule cursor after jobs are atomically enqueued.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn advance_routine_trigger(
        &self,
        owner_id: Uuid,
        trigger_id: Uuid,
        last_evaluated_at_ms: i64,
        next_fire_at_ms: Option<i64>,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("UPDATE routine_triggers SET last_evaluated_at_ms = ?, next_fire_at_ms = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?")
            .bind(last_evaluated_at_ms).bind(next_fire_at_ms).bind(updated_at_ms)
            .bind(owner_id.to_string()).bind(trigger_id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineTriggerNotFound);
        }
        Ok(())
    }

    /// Advances an event trigger cursor after every preceding durable outbox event is handled.
    ///
    /// # Errors
    /// Returns not-found, range, or database errors.
    pub async fn advance_routine_trigger_event_cursor(
        &self,
        owner_id: Uuid,
        trigger_id: Uuid,
        sequence: u64,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let sequence = i64::try_from(sequence)
            .map_err(|_| StorageError::Integrity("event cursor exceeds SQLite range".to_owned()))?;
        let result = sqlx::query("UPDATE routine_triggers SET last_event_sequence = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?")
            .bind(sequence).bind(updated_at_ms).bind(owner_id.to_string()).bind(trigger_id.to_string())
            .execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineTriggerNotFound);
        }
        Ok(())
    }

    /// Atomically deduplicates an external delivery and enqueues its exact routine version once.
    ///
    /// # Errors
    /// Returns not-found, serialization, conflict, or database errors.
    pub async fn enqueue_routine_job(
        &self,
        record: &RoutineJobRecord,
    ) -> Result<RoutineJobClaim, StorageError> {
        let trigger = serde_json::to_string(&record.trigger)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let inputs = serde_json::to_string(&record.inputs)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        let mut transaction = self.pool.begin().await?;
        let delivery = sqlx::query("INSERT OR IGNORE INTO routine_trigger_deliveries (trigger_id, delivery_key, received_at_ms) VALUES (?, ?, ?)")
            .bind(record.trigger_id.to_string()).bind(&record.delivery_key).bind(record.created_at_ms)
            .execute(&mut *transaction).await?;
        if delivery.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(RoutineJobClaim::Replayed);
        }
        sqlx::query("INSERT INTO routine_jobs (id, owner_id, trigger_id, routine_id, routine_version_id, delivery_key, trigger_json, input_json, status, attempt_count, scheduled_for_ms, next_attempt_at_ms, cancel_requested, error_message, created_at_ms, started_at_ms, finished_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(record.trigger_id.to_string())
            .bind(record.routine_id.to_string()).bind(record.routine_version_id.to_string()).bind(&record.delivery_key)
            .bind(trigger).bind(inputs).bind(&record.status).bind(i64::from(record.attempt_count))
            .bind(record.scheduled_for_ms).bind(record.next_attempt_at_ms).bind(record.cancel_requested)
            .bind(&record.error_message).bind(record.created_at_ms).bind(record.started_at_ms).bind(record.finished_at_ms)
            .execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(RoutineJobClaim::Claimed)
    }

    /// Claims the next due job while enforcing its routine overlap policy transactionally.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn claim_next_routine_job(
        &self,
        owner_id: Uuid,
        now_ms: i64,
    ) -> Result<Option<RoutineJobRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query("SELECT j.id, j.owner_id, j.trigger_id, j.routine_id, j.routine_version_id, j.delivery_key, j.trigger_json, j.input_json, j.status, j.attempt_count, j.scheduled_for_ms, j.next_attempt_at_ms, j.cancel_requested, j.error_message, j.created_at_ms, j.started_at_ms, j.finished_at_ms, t.configuration_json FROM routine_jobs j JOIN routine_triggers t ON t.id = j.trigger_id WHERE j.owner_id = ? AND j.status IN ('queued','retry_wait') AND j.next_attempt_at_ms <= ? ORDER BY j.next_attempt_at_ms, j.scheduled_for_ms, j.id LIMIT 100")
            .bind(owner_id.to_string()).bind(now_ms).fetch_all(&mut *transaction).await?;
        for row in rows {
            let definition: RoutineTriggerDefinition =
                serde_json::from_str(row.try_get("configuration_json")?)
                    .map_err(|error| StorageError::Serialization(error.to_string()))?;
            let attempt_count = u16::try_from(row.try_get::<i64, _>("attempt_count")?)
                .map_err(|_| StorageError::Integrity("invalid routine attempt count".to_owned()))?;
            let routine_id: String = row.try_get("routine_id")?;
            let active: i64 = sqlx::query_scalar("SELECT count(*) FROM routine_jobs WHERE owner_id = ? AND routine_id = ? AND status = 'running'")
                .bind(owner_id.to_string()).bind(&routine_id).fetch_one(&mut *transaction).await?;
            let can_run = match definition.overlap_policy {
                OverlapPolicy::Skip | OverlapPolicy::Queue => active == 0,
                OverlapPolicy::Parallel { maximum } => active < i64::from(maximum.max(1)),
            };
            let id: String = row.try_get("id")?;
            if attempt_count >= definition.retry_policy.maximum_attempts.max(1) {
                sqlx::query("UPDATE routine_jobs SET status = 'failed', error_message = 'Routine execution was interrupted at its final attempt', finished_at_ms = ? WHERE id = ? AND status IN ('queued','retry_wait')")
                    .bind(now_ms).bind(&id).execute(&mut *transaction).await?;
                continue;
            }
            if !can_run && matches!(definition.overlap_policy, OverlapPolicy::Skip) {
                sqlx::query("UPDATE routine_jobs SET status = 'skipped', error_message = 'Skipped because the routine was already running', finished_at_ms = ? WHERE id = ? AND status IN ('queued','retry_wait')")
                    .bind(now_ms).bind(&id).execute(&mut *transaction).await?;
                continue;
            }
            if !can_run {
                continue;
            }
            let updated = sqlx::query("UPDATE routine_jobs SET status = 'running', attempt_count = attempt_count + 1, started_at_ms = coalesce(started_at_ms, ?) WHERE id = ? AND status IN ('queued','retry_wait') AND cancel_requested = 0")
                .bind(now_ms).bind(&id).execute(&mut *transaction).await?;
            if updated.rows_affected() == 1 {
                let claimed = sqlx::query("SELECT id, owner_id, trigger_id, routine_id, routine_version_id, delivery_key, trigger_json, input_json, status, attempt_count, scheduled_for_ms, next_attempt_at_ms, cancel_requested, error_message, created_at_ms, started_at_ms, finished_at_ms FROM routine_jobs WHERE id = ?")
                    .bind(&id).fetch_one(&mut *transaction).await?;
                let claimed = routine_job_from_row(&claimed)?;
                transaction.commit().await?;
                return Ok(Some(claimed));
            }
        }
        transaction.commit().await?;
        Ok(None)
    }

    /// Completes a claimed job with a terminal status.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn finish_routine_job(
        &self,
        owner_id: Uuid,
        job_id: Uuid,
        status: &str,
        error_message: Option<&str>,
        finished_at_ms: i64,
    ) -> Result<(), StorageError> {
        if !matches!(status, "succeeded" | "failed" | "cancelled") {
            return Err(StorageError::Integrity(
                "invalid routine job terminal status".to_owned(),
            ));
        }
        let result = sqlx::query("UPDATE routine_jobs SET status = ?, error_message = ?, finished_at_ms = ? WHERE owner_id = ? AND id = ? AND status = 'running'")
            .bind(status).bind(error_message).bind(finished_at_ms).bind(owner_id.to_string()).bind(job_id.to_string())
            .execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineJobNotFound);
        }
        Ok(())
    }

    /// Schedules bounded exponential retry or terminally fails a running job.
    ///
    /// # Errors
    /// Returns not-found, serialization, integrity, or database errors.
    pub async fn retry_or_fail_routine_job(
        &self,
        owner_id: Uuid,
        job_id: Uuid,
        error_message: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        let row = sqlx::query("SELECT j.attempt_count, t.configuration_json FROM routine_jobs j JOIN routine_triggers t ON t.id = j.trigger_id WHERE j.owner_id = ? AND j.id = ? AND j.status = 'running'")
            .bind(owner_id.to_string()).bind(job_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::RoutineJobNotFound)?;
        let attempts = u16::try_from(row.try_get::<i64, _>("attempt_count")?)
            .map_err(|_| StorageError::Integrity("invalid routine job attempt count".to_owned()))?;
        let configuration: String = row.try_get("configuration_json")?;
        let definition: RoutineTriggerDefinition = serde_json::from_str(&configuration)
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        if attempts >= definition.retry_policy.maximum_attempts.max(1) {
            self.finish_routine_job(owner_id, job_id, "failed", Some(error_message), now_ms)
                .await?;
            return Ok(false);
        }
        let exponent = u32::from(attempts.saturating_sub(1)).min(31);
        let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
        let backoff_seconds = u64::from(definition.retry_policy.initial_backoff_seconds)
            .saturating_mul(multiplier)
            .min(u64::from(
                definition.retry_policy.maximum_backoff_seconds.max(1),
            ));
        let backoff_ms = i64::try_from(backoff_seconds.saturating_mul(1_000)).unwrap_or(i64::MAX);
        sqlx::query("UPDATE routine_jobs SET status = 'retry_wait', error_message = ?, next_attempt_at_ms = ? WHERE owner_id = ? AND id = ? AND status = 'running'")
            .bind(error_message).bind(now_ms.saturating_add(backoff_ms)).bind(owner_id.to_string()).bind(job_id.to_string())
            .execute(&self.pool).await?;
        Ok(true)
    }

    /// Requests cancellation and immediately cancels jobs that have not started.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn cancel_routine_job(
        &self,
        owner_id: Uuid,
        job_id: Uuid,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("UPDATE routine_jobs SET cancel_requested = 1, status = CASE WHEN status IN ('queued','retry_wait') THEN 'cancelled' ELSE status END, finished_at_ms = CASE WHEN status IN ('queued','retry_wait') THEN ? ELSE finished_at_ms END WHERE owner_id = ? AND id = ? AND status IN ('queued','retry_wait','running')")
            .bind(now_ms).bind(owner_id.to_string()).bind(job_id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::RoutineJobNotFound);
        }
        Ok(())
    }

    /// Recovers jobs left running by a process interruption without losing cancellation intent.
    ///
    /// # Errors
    /// Returns a database error.
    pub async fn recover_interrupted_routine_jobs(
        &self,
        owner_id: Uuid,
        now_ms: i64,
    ) -> Result<u64, StorageError> {
        let result = sqlx::query("UPDATE routine_jobs SET status = CASE WHEN cancel_requested = 1 THEN 'cancelled' ELSE 'retry_wait' END, next_attempt_at_ms = CASE WHEN cancel_requested = 1 THEN next_attempt_at_ms ELSE ? END, error_message = 'Routine execution was interrupted by server restart', finished_at_ms = CASE WHEN cancel_requested = 1 THEN ? ELSE NULL END WHERE owner_id = ? AND status = 'running'")
            .bind(now_ms).bind(now_ms).bind(owner_id.to_string()).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// Lists recent durable jobs for run-history and recovery projections.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn routine_jobs(
        &self,
        owner_id: Uuid,
        routine_id: Uuid,
    ) -> Result<Vec<RoutineJobRecord>, StorageError> {
        let rows = sqlx::query("SELECT id, owner_id, trigger_id, routine_id, routine_version_id, delivery_key, trigger_json, input_json, status, attempt_count, scheduled_for_ms, next_attempt_at_ms, cancel_requested, error_message, created_at_ms, started_at_ms, finished_at_ms FROM routine_jobs WHERE owner_id = ? AND routine_id = ? ORDER BY created_at_ms DESC, id LIMIT 100")
            .bind(owner_id.to_string()).bind(routine_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(routine_job_from_row).collect()
    }

    /// Loads one owner-scoped durable routine job.
    ///
    /// # Errors
    /// Returns not-found, database, serialization, or integrity errors.
    pub async fn routine_job(
        &self,
        owner_id: Uuid,
        job_id: Uuid,
    ) -> Result<RoutineJobRecord, StorageError> {
        let row = sqlx::query("SELECT id, owner_id, trigger_id, routine_id, routine_version_id, delivery_key, trigger_json, input_json, status, attempt_count, scheduled_for_ms, next_attempt_at_ms, cancel_requested, error_message, created_at_ms, started_at_ms, finished_at_ms FROM routine_jobs WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string()).bind(job_id.to_string()).fetch_optional(&self.pool).await?
            .ok_or(StorageError::RoutineJobNotFound)?;
        routine_job_from_row(&row)
    }
    /// Lists owner-scoped plugin registry records.
    ///
    /// # Errors
    /// Returns a database or integrity error.
    pub async fn list_plugins(&self, owner_id: Uuid) -> Result<Vec<PluginRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT p.id, p.owner_id, p.name, p.description, p.kind, p.configuration_json,
                    p.enabled, c.id AS connection_id, c.transport, c.status, c.auth_status,
                    c.error_message, c.updated_at_ms
             FROM plugins p JOIN mcp_connections c ON c.plugin_id = p.id
             WHERE p.owner_id = ? ORDER BY p.name COLLATE NOCASE, p.id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(plugin_from_row).collect()
    }

    /// Loads an owner-scoped plugin registry record.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn plugin(&self, owner_id: Uuid, id: Uuid) -> Result<PluginRecord, StorageError> {
        let row = sqlx::query(
            "SELECT p.id, p.owner_id, p.name, p.description, p.kind, p.configuration_json,
                    p.enabled, c.id AS connection_id, c.transport, c.status, c.auth_status,
                    c.error_message, c.updated_at_ms
             FROM plugins p JOIN mcp_connections c ON c.plugin_id = p.id
             WHERE p.owner_id = ? AND p.id = ?",
        )
        .bind(owner_id.to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::PluginNotFound)?;
        plugin_from_row(&row)
    }

    /// Atomically creates a local MCP plugin and its connection metadata.
    ///
    /// # Errors
    /// Returns duplicate-name, database, or integrity errors.
    pub async fn create_plugin(&self, record: &PluginRecord) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO plugins (id, owner_id, name, description, kind, configuration_json, enabled, created_at_ms, updated_at_ms)
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ? WHERE NOT EXISTS (
               SELECT 1 FROM plugins WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))
             )",
        )
        .bind(record.id.to_string()).bind(record.owner_id.to_string()).bind(&record.name)
        .bind(&record.description).bind(&record.kind).bind(record.configuration.to_string())
        .bind(record.enabled).bind(record.updated_at_ms).bind(record.updated_at_ms)
        .bind(record.owner_id.to_string()).bind(&record.name)
        .execute(&mut *transaction).await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::DuplicatePluginName);
        }
        sqlx::query(
            "INSERT INTO mcp_connections (id, plugin_id, transport, configuration_json, status, auth_status, error_message, updated_at_ms)
             VALUES (?, ?, ?, '{}', ?, ?, ?, ?)",
        )
        .bind(record.connection_id.to_string()).bind(record.id.to_string()).bind(&record.transport)
        .bind(&record.status).bind(&record.auth_status).bind(&record.error_message).bind(record.updated_at_ms)
        .execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Updates server-owned connection state and replaces discovered tool metadata.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn update_plugin_connection(
        &self,
        owner_id: Uuid,
        plugin_id: Uuid,
        update: PluginConnectionUpdate<'_>,
    ) -> Result<PluginRecord, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let update_result = sqlx::query(
            "UPDATE plugins SET enabled = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(update.enabled)
        .bind(update.updated_at_ms)
        .bind(owner_id.to_string())
        .bind(plugin_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if update_result.rows_affected() == 0 {
            return Err(StorageError::PluginNotFound);
        }
        sqlx::query("UPDATE mcp_connections SET status = ?, auth_status = ?, error_message = ?, updated_at_ms = ? WHERE plugin_id = ?")
            .bind(update.status).bind(update.auth_status).bind(update.error_message).bind(update.updated_at_ms).bind(plugin_id.to_string())
            .execute(&mut *transaction).await?;
        sqlx::query("DELETE FROM mcp_tools WHERE plugin_id = ?")
            .bind(plugin_id.to_string())
            .execute(&mut *transaction)
            .await?;
        for tool in update.tools {
            sqlx::query("INSERT INTO mcp_tools (plugin_id, name, title, description, input_schema_json) VALUES (?, ?, ?, ?, ?)")
                .bind(plugin_id.to_string()).bind(&tool.name).bind(&tool.title).bind(&tool.description)
                .bind(tool.input_schema.to_string()).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        self.plugin(owner_id, plugin_id).await
    }

    /// Lists safe discovery metadata for one owner-scoped plugin.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn plugin_tools(
        &self,
        owner_id: Uuid,
        plugin_id: Uuid,
    ) -> Result<Vec<PluginToolRecord>, StorageError> {
        let _ = self.plugin(owner_id, plugin_id).await?;
        let rows = sqlx::query("SELECT name, title, description, input_schema_json FROM mcp_tools WHERE plugin_id = ? ORDER BY name")
            .bind(plugin_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(plugin_tool_from_row).collect()
    }

    /// Sets one Bot's availability without changing capability policy.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn set_plugin_assignment(
        &self,
        owner_id: Uuid,
        plugin_id: Uuid,
        bot_id: Uuid,
        enabled: bool,
    ) -> Result<(), StorageError> {
        let _ = self.plugin(owner_id, plugin_id).await?;
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM bots WHERE id = ?")
            .bind(bot_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        if exists == 0 {
            return Err(StorageError::BotNotFound);
        }
        sqlx::query("INSERT INTO plugin_bot_assignments (plugin_id, bot_id, owner_id, enabled) VALUES (?, ?, ?, ?) ON CONFLICT(plugin_id, bot_id) DO UPDATE SET enabled = excluded.enabled")
            .bind(plugin_id.to_string()).bind(bot_id.to_string()).bind(owner_id.to_string()).bind(enabled).execute(&self.pool).await?;
        Ok(())
    }

    /// Lists Bots to which an owner-scoped plugin is assigned.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn plugin_bot_ids(
        &self,
        owner_id: Uuid,
        plugin_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let _ = self.plugin(owner_id, plugin_id).await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT bot_id FROM plugin_bot_assignments WHERE owner_id = ? AND plugin_id = ? AND enabled = 1 ORDER BY bot_id")
            .bind(owner_id.to_string()).bind(plugin_id.to_string()).fetch_all(&self.pool).await?;
        rows.into_iter().map(|id| parse_uuid(&id)).collect()
    }

    /// Deletes a plugin and cascades its connection, tools, and assignments.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn delete_plugin(&self, owner_id: Uuid, plugin_id: Uuid) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM plugins WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(plugin_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::PluginNotFound);
        }
        Ok(())
    }
    /// Opens or creates a database, applies migrations, and verifies integrity.
    ///
    /// # Errors
    ///
    /// Fails closed if the database cannot be opened, migrated, or verified.
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        let existing_database = path.metadata().is_ok_and(|metadata| metadata.len() > 0);
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
        verify_pool_integrity(&pool).await?;
        let applied_version = schema_version(&pool).await?;
        if applied_version > SCHEMA_VERSION {
            return Err(StorageError::SchemaTooNew {
                found: applied_version,
                supported: SCHEMA_VERSION,
            });
        }
        if existing_database && applied_version < SCHEMA_VERSION {
            create_verified_migration_backup(&pool, path, applied_version).await?;
        }
        MIGRATOR.run(&pool).await?;
        let storage = Self { pool };
        sqlx::query(
            "UPDATE chat_working_contexts SET compaction_status = 'failed',
                last_error = 'HomeBot restarted before the context operation completed'
             WHERE compaction_status = 'running'",
        )
        .execute(&storage.pool)
        .await?;
        storage.verify_integrity().await?;
        Ok(storage)
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Lists owner-scoped secret metadata. Values never enter `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns a database or integrity error.
    pub async fn list_secret_references(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<SecretReferenceRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, owner_id, locator, label, created_at_ms, updated_at_ms
             FROM secret_references WHERE owner_id = ? ORDER BY label COLLATE NOCASE, id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(secret_reference_from_row).collect()
    }

    /// Loads owner-scoped secret metadata without resolving its value.
    ///
    /// # Errors
    ///
    /// Returns `SecretNotFound` or a database/integrity error.
    pub async fn secret_reference(
        &self,
        owner_id: Uuid,
        id: Uuid,
    ) -> Result<SecretReferenceRecord, StorageError> {
        let row = sqlx::query(
            "SELECT id, owner_id, locator, label, created_at_ms, updated_at_ms
             FROM secret_references WHERE owner_id = ? AND id = ?",
        )
        .bind(owner_id.to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::SecretNotFound)?;
        secret_reference_from_row(&row)
    }

    /// Persists only an opaque locator and display metadata.
    ///
    /// # Errors
    ///
    /// Returns `DuplicateSecretLabel` or a database error.
    pub async fn create_secret_reference(
        &self,
        record: &SecretReferenceRecord,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            "INSERT INTO secret_references
             (id, owner_id, provider, locator, label, created_at_ms, updated_at_ms)
             SELECT ?, ?, 'os_keyring', ?, ?, ?, ?
             WHERE NOT EXISTS (
               SELECT 1 FROM secret_references
               WHERE owner_id = ? AND lower(trim(label)) = lower(trim(?))
             )",
        )
        .bind(record.id.to_string())
        .bind(record.owner_id.to_string())
        .bind(&record.locator)
        .bind(&record.label)
        .bind(record.created_at_ms)
        .bind(record.updated_at_ms)
        .bind(record.owner_id.to_string())
        .bind(&record.label)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::DuplicateSecretLabel);
        }
        Ok(())
    }

    /// Updates secret display metadata; the value stays in the OS credential store.
    ///
    /// # Errors
    ///
    /// Returns a not-found, duplicate-label, or database error.
    pub async fn update_secret_reference(
        &self,
        owner_id: Uuid,
        id: Uuid,
        label: &str,
        now_ms: i64,
    ) -> Result<SecretReferenceRecord, StorageError> {
        let result = sqlx::query(
            "UPDATE secret_references SET label = ?, updated_at_ms = ?
             WHERE owner_id = ? AND id = ?
             AND NOT EXISTS (
               SELECT 1 FROM secret_references other
               WHERE other.owner_id = ? AND other.id <> ?
                 AND lower(trim(other.label)) = lower(trim(?))
             )",
        )
        .bind(label)
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(id.to_string())
        .bind(owner_id.to_string())
        .bind(id.to_string())
        .bind(label)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            if self.secret_reference(owner_id, id).await.is_ok() {
                return Err(StorageError::DuplicateSecretLabel);
            }
            return Err(StorageError::SecretNotFound);
        }
        self.secret_reference(owner_id, id).await
    }

    /// Deletes only metadata after the caller has deleted the OS credential.
    ///
    /// # Errors
    ///
    /// Returns `SecretNotFound` or a database error.
    pub async fn delete_secret_reference(
        &self,
        owner_id: Uuid,
        id: Uuid,
    ) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM secret_references WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::SecretNotFound);
        }
        Ok(())
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
             ORDER BY archived_at_ms IS NOT NULL,
                      hidden_at_ms IS NOT NULL,
                      pinned_at_ms IS NULL,
                      pinned_at_ms DESC,
                      name COLLATE NOCASE, id",
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

    /// Changes a Bot's durable roster pin without making client state authoritative.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn set_bot_pinned(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        pinned: bool,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let result = sqlx::query(
            "UPDATE bots SET pinned_at_ms = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(pinned.then_some(now_ms))
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

    /// Changes whether a Bot is hidden from the normal roster.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn set_bot_hidden(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
        hidden: bool,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let result = sqlx::query(
            "UPDATE bots SET hidden_at_ms = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ?",
        )
        .bind(hidden.then_some(now_ms))
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

    /// Permanently removes an owner-scoped Bot. Foreign keys remove its chats,
    /// provider mappings, routine state, and assignments; machine files are not touched.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn delete_bot(&self, owner_id: Uuid, bot_id: Uuid) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM bots WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(bot_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::BotNotFound);
        }
        Ok(())
    }

    /// Duplicates a Bot's profile, enabled Skill assignments, routines, and
    /// triggers while deliberately excluding chats, provider conversations,
    /// learned history, attachments, recordings, and run history.
    ///
    /// # Errors
    /// Returns not-found, integrity, serialization, uniqueness, or database errors.
    #[allow(clippy::too_many_lines)]
    pub async fn duplicate_bot_configuration(
        &self,
        owner_id: Uuid,
        source_bot_id: Uuid,
        duplicate_id: Uuid,
        now_ms: i64,
    ) -> Result<Bot, StorageError> {
        let source = self.get_bot(owner_id, source_bot_id).await?;
        let mut ordinal = 1_u32;
        let duplicate_name = loop {
            let suffix = if ordinal == 1 {
                " copy".to_owned()
            } else {
                format!(" copy {ordinal}")
            };
            let keep = homebot_domain::BOT_NAME_MAX_CHARS.saturating_sub(suffix.chars().count());
            let base = source.name.chars().take(keep).collect::<String>();
            let candidate = format!("{base}{suffix}");
            let exists: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM bots WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))",
            )
            .bind(owner_id.to_string())
            .bind(&candidate)
            .fetch_one(&self.pool)
            .await?;
            if exists == 0 {
                break candidate;
            }
            ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| StorageError::Integrity("Bot copy suffix overflow".to_owned()))?;
        };

        let mut duplicate = source.clone();
        duplicate.id = BotId(duplicate_id);
        duplicate.name = duplicate_name.clone();
        duplicate.archived_at_ms = None;
        duplicate.pinned_at_ms = None;
        duplicate.hidden_at_ms = None;
        duplicate.unread_count = 0;
        duplicate.attention = BotAttention::None;
        duplicate.created_at_ms = now_ms;
        duplicate.updated_at_ms = now_ms;

        let routines = sqlx::query(
            "SELECT r.id, r.name, r.description, r.enabled, r.draft, v.definition_json
             FROM routines r JOIN routine_versions v ON v.id = r.active_version_id
             WHERE r.owner_id = ? AND r.bot_id = ? ORDER BY r.created_at_ms, r.id",
        )
        .bind(owner_id.to_string())
        .bind(source_bot_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO bots (id, owner_id, name, title, description, provider_profile_id, shape, color,
              permission_profile, archived_at_ms, unread_count, attention, created_at_ms, updated_at_ms,
              pinned_at_ms, hidden_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0, 'none', ?, ?, NULL, NULL)",
        )
        .bind(duplicate_id.to_string()).bind(owner_id.to_string()).bind(&duplicate_name)
        .bind(&duplicate.title).bind(&duplicate.description)
        .bind(duplicate.provider_profile_id.map(|id| id.to_string()))
        .bind(duplicate.shape.as_str()).bind(duplicate.color.as_str())
        .bind(duplicate.permission_profile.as_str()).bind(now_ms).bind(now_ms)
        .execute(&mut *transaction).await?;
        sqlx::query(
            "INSERT INTO skill_bot_assignments (owner_id, skill_id, bot_id, assigned_at_ms)
             SELECT owner_id, skill_id, ?, ? FROM skill_bot_assignments
             WHERE owner_id = ? AND bot_id = ?",
        )
        .bind(duplicate_id.to_string())
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(source_bot_id.to_string())
        .execute(&mut *transaction)
        .await?;

        for row in routines {
            let source_routine_id: String = row.try_get("id")?;
            let original_name: String = row.try_get("name")?;
            let base_name = format!("{original_name} ({duplicate_name})");
            let mut routine_ordinal = 1_u32;
            let routine_name = loop {
                let candidate = if routine_ordinal == 1 {
                    base_name.clone()
                } else {
                    format!("{base_name} {routine_ordinal}")
                };
                let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM routines WHERE owner_id = ? AND lower(trim(name)) = lower(trim(?))")
                    .bind(owner_id.to_string()).bind(&candidate).fetch_one(&mut *transaction).await?;
                if exists == 0 {
                    break candidate;
                }
                routine_ordinal = routine_ordinal.checked_add(1).ok_or_else(|| {
                    StorageError::Integrity("routine copy suffix overflow".to_owned())
                })?;
            };
            let routine_id = Uuid::now_v7();
            let version_id = Uuid::now_v7();
            let definition_json: String = row.try_get("definition_json")?;
            sqlx::query("INSERT INTO routines (id, owner_id, bot_id, name, description, active_version_id, enabled, draft, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(routine_id.to_string()).bind(owner_id.to_string()).bind(duplicate_id.to_string())
                .bind(routine_name).bind(row.try_get::<String, _>("description")?).bind(version_id.to_string())
                .bind(row.try_get::<bool, _>("enabled")?).bind(row.try_get::<bool, _>("draft")?)
                .bind(now_ms).bind(now_ms).execute(&mut *transaction).await?;
            sqlx::query("INSERT INTO routine_versions (id, routine_id, version, definition_json, created_at_ms) VALUES (?, ?, 1, ?, ?)")
                .bind(version_id.to_string()).bind(routine_id.to_string()).bind(definition_json).bind(now_ms)
                .execute(&mut *transaction).await?;
            let triggers = sqlx::query("SELECT configuration_json, enabled FROM routine_triggers WHERE owner_id = ? AND routine_id = ? ORDER BY created_at_ms, id")
                .bind(owner_id.to_string()).bind(source_routine_id).fetch_all(&mut *transaction).await?;
            for trigger in triggers {
                let configuration_json: String = trigger.try_get("configuration_json")?;
                let trigger_definition: RoutineTriggerDefinition =
                    serde_json::from_str(&configuration_json)
                        .map_err(|error| StorageError::Serialization(error.to_string()))?;
                let kind = trigger_kind(&trigger_definition);
                sqlx::query("INSERT INTO routine_triggers (id, owner_id, routine_id, kind, configuration_json, enabled, last_evaluated_at_ms, next_fire_at_ms, last_event_sequence, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, 0, ?, ?)")
                    .bind(Uuid::now_v7().to_string()).bind(owner_id.to_string()).bind(routine_id.to_string())
                    .bind(kind).bind(configuration_json).bind(trigger.try_get::<bool, _>("enabled")?)
                    .bind(now_ms).bind(now_ms).execute(&mut *transaction).await?;
            }
        }
        transaction.commit().await?;
        Ok(duplicate)
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

    /// Creates a durable multi-Bot group with bounded coordination policy.
    ///
    /// # Errors
    ///
    /// Rejects fewer than two or more than six distinct active owned Bots, an invalid owner, unsafe policy
    /// bounds, or database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_group_chat(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        title: &str,
        bot_ids: &[Uuid],
        ownership_bot_id: Uuid,
        coordination_max_turns: u32,
        max_parallel_bots: u32,
        now_ms: i64,
    ) -> Result<GroupChat, StorageError> {
        let distinct = bot_ids.iter().copied().collect::<HashSet<_>>();
        if !(2..=6).contains(&distinct.len())
            || !distinct.contains(&ownership_bot_id)
            || !(1..=64).contains(&coordination_max_turns)
            || !(1..=8).contains(&max_parallel_bots)
        {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let placeholders = std::iter::repeat_n("?", distinct.len())
            .collect::<Vec<_>>()
            .join(",");
        let participant_query = format!(
            "SELECT count(*) FROM bots WHERE owner_id = ? AND archived_at_ms IS NULL
             AND id IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar::<_, i64>(&participant_query).bind(owner_id.to_string());
        for bot_id in &distinct {
            query = query.bind(bot_id.to_string());
        }
        let active_count = query.fetch_one(&self.pool).await?;
        if usize::try_from(active_count).ok() != Some(distinct.len()) {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO chats (
                id, owner_id, kind, title, ownership_bot_id, coordination_max_turns,
                max_parallel_bots, created_at_ms, updated_at_ms
             ) VALUES (?, ?, 'group', ?, ?, ?, ?, ?, ?)",
        )
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .bind(title)
        .bind(ownership_bot_id.to_string())
        .bind(coordination_max_turns)
        .bind(max_parallel_bots)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        for bot_id in &distinct {
            let role = if *bot_id == ownership_bot_id {
                GroupParticipantRole::Owner
            } else {
                GroupParticipantRole::Member
            };
            sqlx::query("INSERT INTO chat_participants (chat_id, bot_id, role) VALUES (?, ?, ?)")
                .bind(chat_id.to_string())
                .bind(bot_id.to_string())
                .bind(role.as_str())
                .execute(&mut *transaction)
                .await?;
            sqlx::query(
                "INSERT INTO group_bot_states (chat_id, bot_id, status, updated_at_ms)
                 VALUES (?, ?, 'idle', ?)",
            )
            .bind(chat_id.to_string())
            .bind(bot_id.to_string())
            .bind(now_ms)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        self.get_group_chat(owner_id, chat_id).await
    }

    /// Loads one owner-scoped group chat.
    ///
    /// # Errors
    ///
    /// Returns not-found, database, or integrity errors.
    pub async fn get_group_chat(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<GroupChat, StorageError> {
        let row =
            sqlx::query("SELECT * FROM chats WHERE owner_id = ? AND id = ? AND kind = 'group'")
                .bind(owner_id.to_string())
                .bind(chat_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StorageError::ChatNotFound)?;
        group_chat_from_row(&row)
    }

    /// Lists all owner-scoped groups in stable recency order.
    ///
    /// # Errors
    ///
    /// Returns database or integrity errors.
    pub async fn list_group_chats(&self, owner_id: Uuid) -> Result<Vec<GroupChat>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM chats WHERE owner_id = ? AND kind = 'group'
             ORDER BY updated_at_ms DESC, id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(group_chat_from_row).collect()
    }

    /// Renames an owner-scoped group without changing its identity or history.
    ///
    /// # Errors
    /// Returns validation, ownership, or database errors.
    pub async fn rename_group_chat(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        title: &str,
        now_ms: i64,
    ) -> Result<GroupChat, StorageError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let changed = sqlx::query("UPDATE chats SET title = ?, updated_at_ms = ? WHERE owner_id = ? AND id = ? AND kind = 'group'")
            .bind(title).bind(now_ms).bind(owner_id.to_string()).bind(chat_id.to_string())
            .execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::ChatNotFound);
        }
        self.get_group_chat(owner_id, chat_id).await
    }

    /// Lists durable participants and their parallel execution state.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn group_participants(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<GroupParticipant>, StorageError> {
        let _ = self.get_group_chat(owner_id, chat_id).await?;
        let rows = sqlx::query(
            "SELECT p.chat_id, p.bot_id, p.role, s.status, s.active_operation_id, s.updated_at_ms
             FROM chat_participants p JOIN group_bot_states s
               ON s.chat_id = p.chat_id AND s.bot_id = p.bot_id
             WHERE p.chat_id = ? ORDER BY p.role = 'owner' DESC, p.bot_id",
        )
        .bind(chat_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(group_participant_from_row).collect()
    }

    /// Adds one active owned Bot to an existing group.
    ///
    /// # Errors
    ///
    /// Rejects archived, foreign, or duplicate Bots and database failures.
    pub async fn add_group_participant(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        bot_id: Uuid,
        now_ms: i64,
    ) -> Result<GroupParticipant, StorageError> {
        let _ = self.get_group_chat(owner_id, chat_id).await?;
        let bot = self.get_bot(owner_id, bot_id).await?;
        if bot.archived_at_ms.is_some() {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE chats SET updated_at_ms = ? WHERE owner_id = ? AND id = ? AND kind = 'group'",
        )
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .execute(&mut *transaction)
        .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM chat_participants WHERE chat_id = ?")
                .bind(chat_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if count >= 6 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        sqlx::query(
            "INSERT INTO chat_participants (chat_id, bot_id, role) VALUES (?, ?, 'member')",
        )
        .bind(chat_id.to_string())
        .bind(bot_id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO group_bot_states (chat_id, bot_id, status, updated_at_ms)
             VALUES (?, ?, 'idle', ?)",
        )
        .bind(chat_id.to_string())
        .bind(bot_id.to_string())
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.group_participants(owner_id, chat_id)
            .await?
            .into_iter()
            .find(|participant| participant.bot_id == bot_id)
            .ok_or(StorageError::InvalidGroupParticipants)
    }

    /// Removes a non-owner Bot while preserving the two-Bot minimum.
    ///
    /// # Errors
    ///
    /// Rejects owner removal, fewer than two remaining Bots, or database failures.
    pub async fn remove_group_participant(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        bot_id: Uuid,
    ) -> Result<(), StorageError> {
        let group = self.get_group_chat(owner_id, chat_id).await?;
        if group.ownership_bot_id == bot_id {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE chats SET updated_at_ms = updated_at_ms WHERE owner_id = ? AND id = ? AND kind = 'group'")
            .bind(owner_id.to_string()).bind(chat_id.to_string()).execute(&mut *transaction).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM chat_participants WHERE chat_id = ?")
                .bind(chat_id.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if count <= 2 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let removed = sqlx::query("DELETE FROM chat_participants WHERE chat_id = ? AND bot_id = ?")
            .bind(chat_id.to_string())
            .bind(bot_id.to_string())
            .execute(&mut *transaction)
            .await?;
        if removed.rows_affected() != 1 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Appends a completed Bot-to-Bot group message with validated shared context.
    ///
    /// # Errors
    ///
    /// Rejects nonparticipants, foreign mentions/context, empty content, or database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_group_bot_message(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        message_id: Uuid,
        author_bot_id: Uuid,
        content: &str,
        mentioned_bot_ids: &[Uuid],
        shared_context_message_ids: &[Uuid],
        now_ms: i64,
    ) -> Result<ChatMessage, StorageError> {
        let _ = self.get_group_chat(owner_id, chat_id).await?;
        let participants = self.group_participants(owner_id, chat_id).await?;
        let participant_ids = participants
            .iter()
            .map(|participant| participant.bot_id)
            .collect::<HashSet<_>>();
        if !participant_ids.contains(&author_bot_id)
            || mentioned_bot_ids
                .iter()
                .any(|bot_id| !participant_ids.contains(bot_id))
        {
            return Err(StorageError::InvalidGroupParticipants);
        }
        let content = content.trim();
        if content.is_empty() || content.chars().count() > 100_000 {
            return Err(StorageError::ChatDomain(ChatDomainError::EmptyMessage));
        }
        for context_id in shared_context_message_ids {
            let exists: i64 =
                sqlx::query_scalar("SELECT count(*) FROM messages WHERE id = ? AND chat_id = ?")
                    .bind(context_id.to_string())
                    .bind(chat_id.to_string())
                    .fetch_one(&self.pool)
                    .await?;
            if exists != 1 {
                return Err(StorageError::MessageNotFound);
            }
        }
        let part = MessagePart::Text {
            id: Uuid::now_v7(),
            ordinal: 0,
            text: content.to_owned(),
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO messages (
                id, chat_id, author_bot_id, author_kind, status, mentioned_bot_ids_json,
                shared_context_message_ids_json, created_at_ms, completed_at_ms
             ) VALUES (?, ?, ?, 'bot', 'completed', ?, ?, ?, ?)",
        )
        .bind(message_id.to_string())
        .bind(chat_id.to_string())
        .bind(author_bot_id.to_string())
        .bind(serde_json::to_value(mentioned_bot_ids).map_err(|error| json_error(&error))?)
        .bind(serde_json::to_value(shared_context_message_ids).map_err(|error| json_error(&error))?)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO message_parts (id, message_id, ordinal, kind, content_json)
             VALUES (?, ?, 0, 'text', ?)",
        )
        .bind(message_part_identity(&part).0.to_string())
        .bind(message_id.to_string())
        .bind(serde_json::to_value(&part).map_err(|error| json_error(&error))?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(ChatMessage {
            id: message_id,
            chat_id,
            author: MessageAuthor::Bot,
            author_bot_id: Some(author_bot_id),
            status: MessageStatus::Completed,
            parts: vec![part],
            reply_to_message_id: None,
            mentioned_bot_ids: mentioned_bot_ids.to_vec(),
            shared_context_message_ids: shared_context_message_ids.to_vec(),
            created_at_ms: now_ms,
            completed_at_ms: Some(now_ms),
            error_json: None,
        })
    }

    /// Appends a user message to a group with participant mentions and shared context.
    ///
    /// # Errors
    ///
    /// Rejects nonparticipant mentions, foreign context, empty content, or database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_group_user_message(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        message_id: Uuid,
        content: &str,
        mentioned_bot_ids: &[Uuid],
        shared_context_message_ids: &[Uuid],
        reply_to_message_id: Option<Uuid>,
        references: &[(MessageReferenceKind, Uuid)],
        now_ms: i64,
    ) -> Result<ChatMessage, StorageError> {
        let _ = self.get_group_chat(owner_id, chat_id).await?;
        let participants = self.group_participants(owner_id, chat_id).await?;
        let participant_ids = participants
            .iter()
            .map(|participant| participant.bot_id)
            .collect::<HashSet<_>>();
        if mentioned_bot_ids
            .iter()
            .any(|bot_id| !participant_ids.contains(bot_id))
        {
            return Err(StorageError::InvalidGroupParticipants);
        }
        for context_id in shared_context_message_ids {
            let referenced_message = self.message(owner_id, *context_id).await?;
            if referenced_message.chat_id != chat_id {
                return Err(StorageError::MessageNotFound);
            }
        }
        if let Some(reply_to) = reply_to_message_id
            && self.message(owner_id, reply_to).await?.chat_id != chat_id
        {
            return Err(StorageError::MessageNotFound);
        }
        let mut message = ChatMessage::user(
            chat_id,
            content,
            &[],
            reply_to_message_id,
            mentioned_bot_ids.to_vec(),
            now_ms,
        )?;
        message.id = message_id;
        message.shared_context_message_ids = shared_context_message_ids.to_vec();
        let mut transaction = self.pool.begin().await?;
        let resolved_references =
            resolve_typed_references(&mut transaction, owner_id, references).await?;
        sqlx::query(
            "INSERT INTO messages (
                id, chat_id, author_kind, status, reply_to_message_id, mentioned_bot_ids_json,
                shared_context_message_ids_json, created_at_ms, completed_at_ms
             ) VALUES (?, ?, 'user', 'completed', ?, ?, ?, ?, ?)",
        )
        .bind(message_id.to_string())
        .bind(chat_id.to_string())
        .bind(reply_to_message_id.map(|id| id.to_string()))
        .bind(serde_json::to_value(mentioned_bot_ids).map_err(|error| json_error(&error))?)
        .bind(serde_json::to_value(shared_context_message_ids).map_err(|error| json_error(&error))?)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        for part in &message.parts {
            let (part_id, ordinal) = message_part_identity(part);
            sqlx::query(
                "INSERT INTO message_parts (id, message_id, ordinal, kind, content_json)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(part_id.to_string())
            .bind(message_id.to_string())
            .bind(i64::from(ordinal))
            .bind(message_part_kind(part))
            .bind(serde_json::to_value(part).map_err(|error| json_error(&error))?)
            .execute(&mut *transaction)
            .await?;
        }
        insert_message_reference_records(&mut transaction, message.id, &resolved_references)
            .await?;
        transaction.commit().await?;
        Ok(message)
    }

    /// Atomically consumes one coordination turn without exceeding the persisted budget.
    ///
    /// # Errors
    ///
    /// Returns `CoordinationLimitReached` after stop or budget exhaustion.
    pub async fn record_group_coordination_turn(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<GroupChat, StorageError> {
        let result = sqlx::query(
            "UPDATE chats SET coordination_turns_used = coordination_turns_used + 1,
                updated_at_ms = ?
             WHERE id = ? AND owner_id = ? AND kind = 'group' AND stop_requested = 0
               AND coordination_turns_used < coordination_max_turns",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::CoordinationLimitReached);
        }
        self.get_group_chat(owner_id, chat_id).await
    }

    /// Updates one participant's visible parallel execution state.
    ///
    /// # Errors
    ///
    /// Rejects nonparticipants, operations beyond the parallel limit, or database failures.
    pub async fn set_group_bot_status(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        bot_id: Uuid,
        status: GroupBotStatus,
        operation_id: Option<Uuid>,
        now_ms: i64,
    ) -> Result<GroupParticipant, StorageError> {
        let group = self.get_group_chat(owner_id, chat_id).await?;
        if status == GroupBotStatus::Running {
            let running: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM group_bot_states WHERE chat_id = ? AND status = 'running'
                   AND bot_id <> ?",
            )
            .bind(chat_id.to_string())
            .bind(bot_id.to_string())
            .fetch_one(&self.pool)
            .await?;
            if u32::try_from(running).unwrap_or(u32::MAX) >= group.max_parallel_bots {
                return Err(StorageError::CoordinationLimitReached);
            }
        }
        let result = sqlx::query(
            "UPDATE group_bot_states SET status = ?, active_operation_id = ?, updated_at_ms = ?
             WHERE chat_id = ? AND bot_id = ?",
        )
        .bind(status.as_str())
        .bind(operation_id.map(|id| id.to_string()))
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(bot_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::InvalidGroupParticipants);
        }
        self.group_participants(owner_id, chat_id)
            .await?
            .into_iter()
            .find(|participant| participant.bot_id == bot_id)
            .ok_or(StorageError::InvalidGroupParticipants)
    }

    /// Transfers explicit group ownership between participants and records the handoff.
    ///
    /// # Errors
    ///
    /// Rejects stale owners, nonparticipants, self-handoffs, or database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn handoff_group_ownership(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        handoff_id: Uuid,
        from_bot_id: Uuid,
        to_bot_id: Uuid,
        message_id: Option<Uuid>,
        reason: &str,
        now_ms: i64,
    ) -> Result<OwnershipHandoff, StorageError> {
        let group = self.get_group_chat(owner_id, chat_id).await?;
        let participants = self.group_participants(owner_id, chat_id).await?;
        if from_bot_id == to_bot_id
            || group.ownership_bot_id != from_bot_id
            || !participants
                .iter()
                .any(|participant| participant.bot_id == to_bot_id)
        {
            return Err(StorageError::InvalidOwnershipHandoff);
        }
        if let Some(message_id) = message_id {
            let message = self.message(owner_id, message_id).await?;
            if message.chat_id != chat_id {
                return Err(StorageError::InvalidOwnershipHandoff);
            }
        }
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > 2_000 {
            return Err(StorageError::InvalidOwnershipHandoff);
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE chats SET ownership_bot_id = ?, updated_at_ms = ? WHERE id = ?")
            .bind(to_bot_id.to_string())
            .bind(now_ms)
            .bind(chat_id.to_string())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE chat_participants SET role = 'member' WHERE chat_id = ?")
            .bind(chat_id.to_string())
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE chat_participants SET role = 'owner' WHERE chat_id = ? AND bot_id = ?")
            .bind(chat_id.to_string())
            .bind(to_bot_id.to_string())
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO group_handoffs (
                id, chat_id, from_bot_id, to_bot_id, message_id, reason, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(handoff_id.to_string())
        .bind(chat_id.to_string())
        .bind(from_bot_id.to_string())
        .bind(to_bot_id.to_string())
        .bind(message_id.map(|id| id.to_string()))
        .bind(reason)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OwnershipHandoff {
            id: handoff_id,
            chat_id,
            from_bot_id,
            to_bot_id,
            message_id,
            reason: reason.to_owned(),
            created_at_ms: now_ms,
        })
    }

    /// Lists immutable ownership handoffs for a group.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn group_handoffs(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<OwnershipHandoff>, StorageError> {
        let _ = self.get_group_chat(owner_id, chat_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM group_handoffs WHERE chat_id = ? ORDER BY created_at_ms, id",
        )
        .bind(chat_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(group_handoff_from_row).collect()
    }

    /// Stops every active Bot state and prevents further coordination turns.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn stop_group_chat(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<GroupChat, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE chats SET stop_requested = 1, running = 0, updated_at_ms = ?
             WHERE id = ? AND owner_id = ? AND kind = 'group'",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::ChatNotFound);
        }
        sqlx::query(
            "UPDATE group_bot_states SET status = 'stopped', active_operation_id = NULL,
                updated_at_ms = ? WHERE chat_id = ?",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_group_chat(owner_id, chat_id).await
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
        applied_skills: &[AppliedSkill],
        references: &[(MessageReferenceKind, Uuid)],
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
        let resolved_references =
            resolve_typed_references(&mut transaction, owner_id, references).await?;
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
        for (ordinal, skill) in applied_skills.iter().enumerate() {
            let result = sqlx::query("INSERT INTO message_skill_versions (message_id, skill_id, skill_version_id, ordinal) SELECT ?, s.id, v.id, ? FROM skills s JOIN skill_versions v ON v.skill_id = s.id WHERE s.owner_id = ? AND s.id = ? AND v.id = ?")
                .bind(message.id.to_string()).bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
                .bind(owner_id.to_string()).bind(skill.skill_id.to_string()).bind(skill.version_id.to_string())
                .execute(&mut *transaction).await?;
            if result.rows_affected() != 1 {
                return Err(StorageError::SkillNotFound);
            }
        }
        insert_message_reference_records(&mut transaction, message.id, &resolved_references)
            .await?;
        sqlx::query("UPDATE chats SET updated_at_ms = ? WHERE id = ? AND owner_id = ?")
            .bind(now_ms)
            .bind(chat_id.to_string())
            .bind(owner_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(message)
    }

    /// Loads immutable Skill versions applied to a historical message in original order.
    ///
    /// # Errors
    /// Returns database, serialization, or integrity errors.
    pub async fn message_applied_skills(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
    ) -> Result<Vec<AppliedSkill>, StorageError> {
        let rows = sqlx::query("SELECT s.id AS skill_id, v.id AS version_id, v.name, v.version, v.definition_json FROM message_skill_versions msv JOIN messages m ON m.id = msv.message_id JOIN chats c ON c.id = m.chat_id JOIN skills s ON s.id = msv.skill_id JOIN skill_versions v ON v.id = msv.skill_version_id WHERE c.owner_id = ? AND m.id = ? ORDER BY msv.ordinal")
            .bind(owner_id.to_string()).bind(message_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let definition: String = row.try_get("definition_json")?;
                Ok(AppliedSkill {
                    skill_id: parse_uuid(row.try_get("skill_id")?)?,
                    version_id: parse_uuid(row.try_get("version_id")?)?,
                    name: row.try_get("name")?,
                    version: u32::try_from(row.try_get::<i64, _>("version")?)
                        .map_err(|_| StorageError::Integrity("invalid Skill version".to_owned()))?,
                    definition: serde_json::from_str(&definition)
                        .map_err(|error| StorageError::Serialization(error.to_string()))?,
                })
            })
            .collect()
    }

    /// Adds or removes the owner's reaction and returns the reconciled message totals.
    ///
    /// # Errors
    /// Returns validation, ownership, integrity, or database errors.
    pub async fn set_message_reaction(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
        emoji: &str,
        active: bool,
        now_ms: i64,
    ) -> Result<Vec<MessageReactionRecord>, StorageError> {
        let emoji = emoji.trim();
        if emoji.is_empty() || emoji.chars().count() > 16 || emoji.chars().any(char::is_control) {
            return Err(StorageError::Integrity(
                "reaction must contain 1 to 16 visible characters".to_owned(),
            ));
        }
        let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM messages m JOIN chats c ON c.id = m.chat_id WHERE c.owner_id = ? AND m.id = ?")
            .bind(owner_id.to_string()).bind(message_id.to_string()).fetch_one(&self.pool).await?;
        if owned == 0 {
            return Err(StorageError::MessageNotFound);
        }
        if active {
            sqlx::query("INSERT INTO message_reactions (owner_id, message_id, emoji, created_at_ms) VALUES (?, ?, ?, ?) ON CONFLICT(owner_id, message_id, emoji) DO NOTHING")
                .bind(owner_id.to_string()).bind(message_id.to_string()).bind(emoji).bind(now_ms)
                .execute(&self.pool).await?;
        } else {
            sqlx::query(
                "DELETE FROM message_reactions WHERE owner_id = ? AND message_id = ? AND emoji = ?",
            )
            .bind(owner_id.to_string())
            .bind(message_id.to_string())
            .bind(emoji)
            .execute(&self.pool)
            .await?;
        }
        self.message_reactions(owner_id, message_id).await
    }

    /// Returns deterministic reaction totals for an owner-scoped message.
    ///
    /// # Errors
    /// Returns ownership, integrity, or database errors.
    pub async fn message_reactions(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
    ) -> Result<Vec<MessageReactionRecord>, StorageError> {
        let rows = sqlx::query("SELECT r.emoji, count(*) AS reaction_count, max(r.owner_id = ?) AS reacted_by_user FROM message_reactions r JOIN messages m ON m.id = r.message_id JOIN chats c ON c.id = m.chat_id WHERE c.owner_id = ? AND r.message_id = ? GROUP BY r.emoji ORDER BY min(r.created_at_ms), r.emoji")
            .bind(owner_id.to_string()).bind(owner_id.to_string()).bind(message_id.to_string())
            .fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok(MessageReactionRecord {
                    emoji: row.try_get("emoji")?,
                    count: u32::try_from(row.try_get::<i64, _>("reaction_count")?).map_err(
                        |_| StorageError::Integrity("invalid reaction count".to_owned()),
                    )?,
                    reacted_by_user: row.try_get("reacted_by_user")?,
                })
            })
            .collect()
    }

    /// Resolves typed references against owner-scoped current state and stores immutable labels
    /// and version IDs on the message. Replaying the same ordered references is idempotent.
    ///
    /// # Errors
    /// Returns not-found, ownership, or database integrity errors.
    pub async fn set_message_references(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
        references: &[(MessageReferenceKind, Uuid)],
    ) -> Result<Vec<MessageReferenceRecord>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM messages m JOIN chats c ON c.id = m.chat_id WHERE c.owner_id = ? AND m.id = ?")
            .bind(owner_id.to_string()).bind(message_id.to_string()).fetch_one(&mut *transaction).await?;
        if owned == 0 {
            return Err(StorageError::MessageNotFound);
        }
        let resolved = resolve_typed_references(&mut transaction, owner_id, references).await?;
        sqlx::query("DELETE FROM message_references WHERE message_id = ?")
            .bind(message_id.to_string())
            .execute(&mut *transaction)
            .await?;
        insert_message_reference_records(&mut transaction, message_id, &resolved).await?;
        transaction.commit().await?;
        Ok(resolved)
    }

    /// Loads immutable typed references in message order.
    ///
    /// # Errors
    /// Returns ownership, database, or integrity errors.
    pub async fn message_references(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
    ) -> Result<Vec<MessageReferenceRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT r.kind, r.target_id, r.target_version_id, r.label_snapshot
             FROM message_references r JOIN messages m ON m.id = r.message_id
             JOIN chats c ON c.id = m.chat_id
             WHERE c.owner_id = ? AND r.message_id = ? ORDER BY r.ordinal",
        )
        .bind(owner_id.to_string())
        .bind(message_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(MessageReferenceRecord {
                    kind: MessageReferenceKind::from_str(row.try_get("kind")?)?,
                    target_id: parse_uuid(row.try_get("target_id")?)?,
                    target_version_id: row
                        .try_get::<Option<String>, _>("target_version_id")?
                        .map(|value| parse_uuid(&value))
                        .transpose()?,
                    label_snapshot: row.try_get("label_snapshot")?,
                })
            })
            .collect()
    }

    /// Creates or updates a narrow owner-scoped capability rule and appends immutable audit.
    ///
    /// # Errors
    /// Returns scope validation, ownership, or database errors.
    pub async fn upsert_capability_rule(
        &self,
        rule: &CapabilityRuleRecord,
    ) -> Result<CapabilityRuleRecord, StorageError> {
        validate_capability_rule(rule)?;
        let mut transaction = self.pool.begin().await?;
        validate_capability_rule_scopes(&mut transaction, rule).await?;
        let existing_owner: Option<String> =
            sqlx::query_scalar("SELECT owner_id FROM capability_rules WHERE id = ?")
                .bind(rule.id.to_string())
                .fetch_optional(&mut *transaction)
                .await?;
        if existing_owner
            .as_deref()
            .is_some_and(|owner| owner != rule.owner_id.to_string())
        {
            return Err(StorageError::Integrity(
                "capability rule is not owner accessible".to_owned(),
            ));
        }
        let existed = existing_owner.is_some();
        sqlx::query("INSERT INTO capability_rules (id, owner_id, capability, effect, device_id, bot_id, chat_id, workspace_id, action_prefix, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET capability = excluded.capability, effect = excluded.effect, device_id = excluded.device_id, bot_id = excluded.bot_id, chat_id = excluded.chat_id, workspace_id = excluded.workspace_id, action_prefix = excluded.action_prefix, updated_at_ms = excluded.updated_at_ms WHERE capability_rules.owner_id = excluded.owner_id")
            .bind(rule.id.to_string()).bind(rule.owner_id.to_string()).bind(&rule.capability).bind(&rule.effect)
            .bind(rule.device_id.map(|id| id.to_string())).bind(rule.bot_id.map(|id| id.to_string()))
            .bind(rule.chat_id.map(|id| id.to_string())).bind(rule.workspace_id.map(|id| id.to_string()))
            .bind(&rule.action_prefix).bind(rule.created_at_ms).bind(rule.updated_at_ms)
            .execute(&mut *transaction).await?;
        insert_capability_audit(
            &mut transaction,
            rule,
            if existed { "updated" } else { "created" },
            rule.updated_at_ms,
        )
        .await?;
        transaction.commit().await?;
        self.capability_rule(rule.owner_id, rule.id).await
    }

    /// Lists current rules in deterministic deny-first evaluation order.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn capability_rules(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<CapabilityRuleRecord>, StorageError> {
        let rows = sqlx::query("SELECT * FROM capability_rules WHERE owner_id = ? ORDER BY CASE effect WHEN 'deny' THEN 0 WHEN 'require_approval' THEN 1 ELSE 2 END, created_at_ms, id")
            .bind(owner_id.to_string()).fetch_all(&self.pool).await?;
        rows.iter().map(capability_rule_from_row).collect()
    }

    /// Deletes one owner-scoped rule while retaining an immutable audit snapshot.
    ///
    /// # Errors
    /// Returns not-found, ownership, or database errors.
    pub async fn delete_capability_rule(
        &self,
        owner_id: Uuid,
        rule_id: Uuid,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let rule = self.capability_rule(owner_id, rule_id).await?;
        let mut transaction = self.pool.begin().await?;
        insert_capability_audit(&mut transaction, &rule, "deleted", now_ms).await?;
        sqlx::query("DELETE FROM capability_rules WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(rule_id.to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Lists immutable capability-rule audit records.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn capability_rule_audit(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<CapabilityRuleAuditRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM capability_rule_audit WHERE owner_id = ? ORDER BY created_at_ms, id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(capability_audit_from_row).collect()
    }

    async fn capability_rule(
        &self,
        owner_id: Uuid,
        rule_id: Uuid,
    ) -> Result<CapabilityRuleRecord, StorageError> {
        let row = sqlx::query("SELECT * FROM capability_rules WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(rule_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::Integrity(
                "capability rule was not found".to_owned(),
            ))?;
        capability_rule_from_row(&row)
    }

    /// Creates or reuses one owner-scoped browser profile metadata record.
    ///
    /// Browser cookies and credentials stay in the server-owned profile directory and never
    /// enter this record.
    ///
    /// # Errors
    /// Returns validation, ownership, or database errors.
    pub async fn upsert_browser_profile(
        &self,
        profile: &BrowserProfileRecord,
    ) -> Result<BrowserProfileRecord, StorageError> {
        if profile.display_name.trim().is_empty()
            || profile.display_name.chars().count() > 80
            || profile.directory_ref.is_empty()
            || profile.directory_ref.chars().count() > 160
            || profile.directory_ref.contains('/')
            || profile.directory_ref.contains('\\')
        {
            return Err(StorageError::Integrity(
                "invalid browser profile metadata".to_owned(),
            ));
        }
        sqlx::query("INSERT INTO browser_profiles (id, owner_id, display_name, directory_ref, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name, updated_at_ms = excluded.updated_at_ms WHERE browser_profiles.owner_id = excluded.owner_id")
            .bind(profile.id.to_string()).bind(profile.owner_id.to_string())
            .bind(profile.display_name.trim()).bind(&profile.directory_ref)
            .bind(profile.created_at_ms).bind(profile.updated_at_ms)
            .execute(&self.pool).await?;
        self.browser_profile(profile.owner_id, profile.id).await
    }

    /// Persists an active server-owned browser session after validating the Bot/chat scope.
    ///
    /// # Errors
    /// Returns ownership, participant, state, or database errors.
    pub async fn create_browser_session(
        &self,
        session: &BrowserSessionRecord,
    ) -> Result<BrowserSessionRecord, StorageError> {
        validate_browser_state(&session.controller, &session.status)?;
        let mut transaction = self.pool.begin().await?;
        let profile_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM browser_profiles WHERE id = ? AND owner_id = ?",
        )
        .bind(session.profile_id.to_string())
        .bind(session.owner_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        let participant_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chats c LEFT JOIN chat_participants p ON p.chat_id = c.id AND p.bot_id = ? WHERE c.id = ? AND c.owner_id = ? AND (c.direct_bot_id = ? OR p.bot_id IS NOT NULL)",
        )
        .bind(session.bot_id.to_string())
        .bind(session.chat_id.to_string())
        .bind(session.owner_id.to_string())
        .bind(session.bot_id.to_string())
        .fetch_one(&mut *transaction)
        .await?;
        if profile_count != 1 || participant_count != 1 {
            return Err(StorageError::Integrity(
                "browser session scope is not owner accessible".to_owned(),
            ));
        }
        sqlx::query("INSERT INTO browser_sessions (id, owner_id, chat_id, bot_id, profile_id, runtime_session_id, current_url, controller, status, pending_approval_id, controlling_device_id, takeover_expires_at_ms, created_at_ms, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(session.id.to_string()).bind(session.owner_id.to_string())
            .bind(session.chat_id.to_string()).bind(session.bot_id.to_string())
            .bind(session.profile_id.to_string())
            .bind(session.runtime_session_id.map(|id| id.to_string())).bind(&session.current_url)
            .bind(&session.controller).bind(&session.status)
            .bind(session.pending_approval_id.map(|id| id.to_string()))
            .bind(session.controlling_device_id.map(|id| id.to_string()))
            .bind(session.takeover_expires_at_ms)
            .bind(session.created_at_ms).bind(session.updated_at_ms)
            .execute(&mut *transaction).await?;
        transaction.commit().await?;
        self.browser_session(session.owner_id, session.id).await
    }

    /// Lists owner-scoped browser sessions, optionally narrowed to one chat.
    ///
    /// # Errors
    /// Returns database or integrity errors.
    pub async fn browser_sessions(
        &self,
        owner_id: Uuid,
        chat_id: Option<Uuid>,
    ) -> Result<Vec<BrowserSessionRecord>, StorageError> {
        let rows = sqlx::query("SELECT s.*, p.display_name AS profile_name, p.directory_ref FROM browser_sessions s JOIN browser_profiles p ON p.id = s.profile_id AND p.owner_id = s.owner_id WHERE s.owner_id = ? AND (? IS NULL OR s.chat_id = ?) ORDER BY s.updated_at_ms DESC, s.id")
            .bind(owner_id.to_string()).bind(chat_id.map(|id| id.to_string()))
            .bind(chat_id.map(|id| id.to_string())).fetch_all(&self.pool).await?;
        rows.iter().map(browser_session_from_row).collect()
    }

    /// Replaces the mutable safe projection for an owner-scoped browser session.
    ///
    /// # Errors
    /// Returns state, ownership, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_browser_session(
        &self,
        owner_id: Uuid,
        session_id: Uuid,
        controller: &str,
        status: &str,
        current_url: Option<&str>,
        pending_approval_id: Option<Uuid>,
        now_ms: i64,
    ) -> Result<BrowserSessionRecord, StorageError> {
        validate_browser_state(controller, status)?;
        let updated = sqlx::query("UPDATE browser_sessions SET controller = ?, status = ?, current_url = COALESCE(?, current_url), pending_approval_id = ?, updated_at_ms = ? WHERE id = ? AND owner_id = ?")
            .bind(controller).bind(status).bind(current_url)
            .bind(pending_approval_id.map(|id| id.to_string())).bind(now_ms)
            .bind(session_id.to_string()).bind(owner_id.to_string())
            .execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::Integrity(
                "browser session was not found".to_owned(),
            ));
        }
        self.browser_session(owner_id, session_id).await
    }

    /// Atomically acquires the device-owned human takeover lease.
    ///
    /// # Errors
    /// Returns state, ownership, or database errors.
    pub async fn claim_browser_takeover(
        &self,
        owner_id: Uuid,
        session_id: Uuid,
        controlling_device_id: Uuid,
        takeover_expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<BrowserSessionRecord, StorageError> {
        if takeover_expires_at_ms <= now_ms {
            return Err(StorageError::Integrity(
                "browser takeover lease must expire in the future".to_owned(),
            ));
        }
        let device_id = controlling_device_id.to_string();
        let updated = sqlx::query("UPDATE browser_sessions SET controller = 'user', status = 'active', pending_approval_id = NULL, controlling_device_id = ?, takeover_expires_at_ms = ?, updated_at_ms = ? WHERE id = ? AND owner_id = ? AND (controller != 'user' OR controlling_device_id = ? OR takeover_expires_at_ms IS NULL OR takeover_expires_at_ms <= ?)")
            .bind(&device_id)
            .bind(takeover_expires_at_ms)
            .bind(now_ms)
            .bind(session_id.to_string())
            .bind(owner_id.to_string())
            .bind(&device_id)
            .bind(now_ms)
            .execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::BrowserTakeoverConflict);
        }
        self.browser_session(owner_id, session_id).await
    }

    /// Atomically returns a human-controlled browser to its Bot.
    ///
    /// A device can release its own lease after expiry, but cannot release another device's
    /// lease. A session that is already Bot-controlled is an idempotent success.
    ///
    /// # Errors
    /// Returns state, ownership, or database errors.
    pub async fn release_browser_takeover(
        &self,
        owner_id: Uuid,
        session_id: Uuid,
        controlling_device_id: Uuid,
        now_ms: i64,
    ) -> Result<BrowserSessionRecord, StorageError> {
        let updated = sqlx::query("UPDATE browser_sessions SET controller = 'bot', status = 'active', pending_approval_id = NULL, controlling_device_id = NULL, takeover_expires_at_ms = NULL, updated_at_ms = ? WHERE id = ? AND owner_id = ? AND (controller != 'user' OR controlling_device_id = ?)")
            .bind(now_ms)
            .bind(session_id.to_string())
            .bind(owner_id.to_string())
            .bind(controlling_device_id.to_string())
            .execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::BrowserTakeoverConflict);
        }
        self.browser_session(owner_id, session_id).await
    }

    /// Attaches a live runtime target to an existing durable browser projection.
    ///
    /// # Errors
    /// Returns ownership or database errors.
    pub async fn activate_browser_session(
        &self,
        owner_id: Uuid,
        session_id: Uuid,
        runtime_session_id: Uuid,
        now_ms: i64,
    ) -> Result<BrowserSessionRecord, StorageError> {
        let updated = sqlx::query("UPDATE browser_sessions SET runtime_session_id = ?, status = 'active', pending_approval_id = NULL, updated_at_ms = ? WHERE id = ? AND owner_id = ?")
            .bind(runtime_session_id.to_string()).bind(now_ms)
            .bind(session_id.to_string()).bind(owner_id.to_string())
            .execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::Integrity(
                "browser session was not found".to_owned(),
            ));
        }
        self.browser_session(owner_id, session_id).await
    }

    async fn browser_profile(
        &self,
        owner_id: Uuid,
        profile_id: Uuid,
    ) -> Result<BrowserProfileRecord, StorageError> {
        let row = sqlx::query("SELECT * FROM browser_profiles WHERE id = ? AND owner_id = ?")
            .bind(profile_id.to_string())
            .bind(owner_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StorageError::Integrity("browser profile was not found".to_owned()))?;
        Ok(BrowserProfileRecord {
            id: parse_uuid(row.try_get("id")?)?,
            owner_id: parse_uuid(row.try_get("owner_id")?)?,
            display_name: row.try_get("display_name")?,
            directory_ref: row.try_get("directory_ref")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }

    /// Loads one owner-scoped browser session.
    ///
    /// # Errors
    /// Returns not-found, database, or integrity errors.
    pub async fn browser_session(
        &self,
        owner_id: Uuid,
        session_id: Uuid,
    ) -> Result<BrowserSessionRecord, StorageError> {
        let row = sqlx::query("SELECT s.*, p.display_name AS profile_name, p.directory_ref FROM browser_sessions s JOIN browser_profiles p ON p.id = s.profile_id AND p.owner_id = s.owner_id WHERE s.id = ? AND s.owner_id = ?")
            .bind(session_id.to_string()).bind(owner_id.to_string())
            .fetch_optional(&self.pool).await?
            .ok_or_else(|| StorageError::Integrity("browser session was not found".to_owned()))?;
        browser_session_from_row(&row)
    }

    /// Creates the stable assistant message that receives provider deltas.
    ///
    /// # Errors
    ///
    /// Returns ownership, participant, or database errors.
    pub async fn create_bot_message(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        bot_id: Uuid,
        message_id: Uuid,
        now_ms: i64,
    ) -> Result<ChatMessage, StorageError> {
        let chat: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, direct_bot_id FROM chats WHERE id = ? AND owner_id = ?")
                .bind(chat_id.to_string())
                .bind(owner_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        let Some((kind, direct_bot_id)) = chat else {
            return Err(StorageError::ChatNotFound);
        };
        let allowed = if kind == "direct" {
            direct_bot_id.as_deref() == Some(&bot_id.to_string())
        } else if kind == "group" {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM chat_participants WHERE chat_id = ? AND bot_id = ?",
            )
            .bind(chat_id.to_string())
            .bind(bot_id.to_string())
            .fetch_one(&self.pool)
            .await?
                == 1
        } else {
            false
        };
        if !allowed {
            return Err(StorageError::BotNotFound);
        }
        let part = MessagePart::Text {
            id: Uuid::now_v7(),
            ordinal: 0,
            text: String::new(),
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO messages (
                id, chat_id, author_bot_id, author_kind, status,
                mentioned_bot_ids_json, created_at_ms
             ) VALUES (?, ?, ?, 'bot', 'streaming', '[]', ?)",
        )
        .bind(message_id.to_string())
        .bind(chat_id.to_string())
        .bind(bot_id.to_string())
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO message_parts (id, message_id, ordinal, kind, content_json)
             VALUES (?, ?, 0, 'text', ?)",
        )
        .bind(message_part_identity(&part).0.to_string())
        .bind(message_id.to_string())
        .bind(serde_json::to_value(&part).map_err(|error| json_error(&error))?)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE chats SET running = 1, updated_at_ms = ? WHERE id = ? AND owner_id = ?",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(ChatMessage {
            id: message_id,
            chat_id,
            author: MessageAuthor::Bot,
            author_bot_id: Some(bot_id),
            status: MessageStatus::Streaming,
            parts: vec![part],
            reply_to_message_id: None,
            mentioned_bot_ids: Vec::new(),
            shared_context_message_ids: Vec::new(),
            created_at_ms: now_ms,
            completed_at_ms: None,
            error_json: None,
        })
    }

    /// Appends a streamed text delta to the assistant message's stable text part.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-state, ownership, or database errors.
    pub async fn append_bot_message_delta(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
        delta: &str,
    ) -> Result<ChatMessage, StorageError> {
        let result = sqlx::query(
            "UPDATE message_parts
             SET content_json = json_set(
                 content_json, '$.text', json_extract(content_json, '$.text') || ?
             )
             WHERE message_id = ? AND ordinal = 0 AND kind = 'text'
               AND EXISTS (
                   SELECT 1 FROM messages m JOIN chats c ON c.id = m.chat_id
                   WHERE m.id = message_parts.message_id
                     AND m.status = 'streaming' AND c.owner_id = ?
               )",
        )
        .bind(delta)
        .bind(message_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::MessageNotFound);
        }
        self.message(owner_id, message_id).await
    }

    /// Moves a streamed assistant message into one terminal state.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-state, ownership, or database errors.
    pub async fn finish_bot_message(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
        status: MessageStatus,
        error: Option<&Value>,
        now_ms: i64,
    ) -> Result<ChatMessage, StorageError> {
        if !matches!(
            status,
            MessageStatus::Completed | MessageStatus::Failed | MessageStatus::Cancelled
        ) {
            return Err(StorageError::Integrity(
                "assistant message terminal status is invalid".to_owned(),
            ));
        }
        let result = sqlx::query(
            "UPDATE messages SET status = ?, error_json = ?, completed_at_ms = ?
             WHERE id = ? AND status = 'streaming' AND chat_id IN (
                SELECT id FROM chats WHERE owner_id = ?
             )",
        )
        .bind(status.as_str())
        .bind(error)
        .bind(now_ms)
        .bind(message_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::MessageNotFound);
        }
        self.message(owner_id, message_id).await
    }

    /// Loads one owner-scoped message with rich parts.
    ///
    /// # Errors
    ///
    /// Returns not-found, ownership, database, or integrity errors.
    pub async fn message(
        &self,
        owner_id: Uuid,
        message_id: Uuid,
    ) -> Result<ChatMessage, StorageError> {
        let row = sqlx::query(
            "SELECT m.* FROM messages m JOIN chats c ON c.id = m.chat_id
             WHERE m.id = ? AND c.owner_id = ?",
        )
        .bind(message_id.to_string())
        .bind(owner_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::MessageNotFound)?;
        let mut message = chat_message_from_row(&row)?;
        let parts: Vec<Value> = sqlx::query_scalar(
            "SELECT content_json FROM message_parts WHERE message_id = ? ORDER BY ordinal",
        )
        .bind(message_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        message.parts = parts
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|error| json_error(&error)))
            .collect::<Result<_, _>>()?;
        Ok(message)
    }

    /// Resolves the normalized adapter and optional model configured for a Bot.
    ///
    /// # Errors
    ///
    /// Returns database, ownership, or integrity errors.
    pub async fn provider_route_for_bot(
        &self,
        owner_id: Uuid,
        bot_id: Uuid,
    ) -> Result<Option<ProviderRoute>, StorageError> {
        let row = sqlx::query(
            "SELECT p.id, p.adapter_kind, p.configuration_json
             FROM bots b JOIN provider_profiles p ON p.id = b.provider_profile_id
             WHERE b.id = ? AND b.owner_id = ? AND b.archived_at_ms IS NULL",
        )
        .bind(bot_id.to_string())
        .bind(owner_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let profile_id = parse_uuid(&row.try_get::<String, _>("id")?)?;
            let configuration: Value = row.try_get("configuration_json")?;
            let model = configuration
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Ok(ProviderRoute {
                profile_id,
                adapter_kind: row.try_get("adapter_kind")?,
                model,
            })
        })
        .transpose()
    }

    /// Loads or creates the durable provider working-context projection for a direct chat.
    ///
    /// # Errors
    /// Returns ownership, validation, or database errors.
    pub async fn working_context(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        profile_id: Uuid,
        now_ms: i64,
    ) -> Result<WorkingContextRecord, StorageError> {
        let _ = self.get_direct_chat(owner_id, chat_id).await?;
        sqlx::query(
            "INSERT INTO chat_working_contexts (
                owner_id, chat_id, provider_profile_id, interaction_mode,
                compaction_status, generation, updated_at_ms
             ) VALUES (?, ?, ?, 'default', 'idle', 0, ?)
             ON CONFLICT(chat_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                provider_profile_id = excluded.provider_profile_id,
                interaction_mode = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.interaction_mode ELSE 'default' END,
                used_tokens = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.used_tokens ELSE NULL END,
                context_window_tokens = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.context_window_tokens ELSE NULL END,
                compaction_status = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.compaction_status ELSE 'idle' END,
                generation = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.generation ELSE 0 END,
                compacted_at_ms = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.compacted_at_ms ELSE NULL END,
                last_error = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.last_error ELSE NULL END,
                updated_at_ms = CASE
                    WHEN chat_working_contexts.provider_profile_id = excluded.provider_profile_id
                    THEN chat_working_contexts.updated_at_ms ELSE excluded.updated_at_ms END",
        )
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .bind(profile_id.to_string())
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        self.load_working_context(owner_id, chat_id).await
    }

    /// Loads an existing working-context projection.
    ///
    /// # Errors
    /// Returns not-found, integrity, or database errors.
    pub async fn load_working_context(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<WorkingContextRecord, StorageError> {
        let row =
            sqlx::query("SELECT * FROM chat_working_contexts WHERE owner_id = ? AND chat_id = ?")
                .bind(owner_id.to_string())
                .bind(chat_id.to_string())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StorageError::ChatNotFound)?;
        working_context_from_row(&row)
    }

    /// Changes the interaction mode without touching Bot identity or transcript history.
    ///
    /// # Errors
    /// Returns not-found or database errors.
    pub async fn set_working_context_mode(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        mode: &str,
        now_ms: i64,
    ) -> Result<WorkingContextRecord, StorageError> {
        if !matches!(mode, "default" | "plan") {
            return Err(StorageError::Integrity(
                "invalid interaction mode".to_owned(),
            ));
        }
        let result = sqlx::query(
            "UPDATE chat_working_contexts SET interaction_mode = ?, updated_at_ms = ?
             WHERE owner_id = ? AND chat_id = ?",
        )
        .bind(mode)
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::ChatNotFound);
        }
        self.load_working_context(owner_id, chat_id).await
    }

    /// Records provider-neutral working-context usage.
    ///
    /// # Errors
    /// Returns database errors.
    pub async fn update_working_context_usage(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        used_tokens: u64,
        context_window_tokens: Option<u64>,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE chat_working_contexts SET used_tokens = ?,
                context_window_tokens = COALESCE(?, context_window_tokens), updated_at_ms = ?
             WHERE owner_id = ? AND chat_id = ?",
        )
        .bind(i64::try_from(used_tokens).unwrap_or(i64::MAX))
        .bind(context_window_tokens.map(|value| i64::try_from(value).unwrap_or(i64::MAX)))
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Marks a compaction/reset lifecycle transition and optionally advances its generation.
    ///
    /// # Errors
    /// Returns validation, not-found, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_working_context_compaction(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        status: &str,
        advance_generation: bool,
        clear_usage: bool,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<WorkingContextRecord, StorageError> {
        if !matches!(status, "idle" | "running" | "completed" | "failed") {
            return Err(StorageError::Integrity(
                "invalid compaction status".to_owned(),
            ));
        }
        let result = sqlx::query(
            "UPDATE chat_working_contexts SET compaction_status = ?,
                generation = generation + ?,
                used_tokens = CASE WHEN ? THEN NULL ELSE used_tokens END,
                compacted_at_ms = CASE WHEN ? THEN ? ELSE compacted_at_ms END,
                last_error = ?, updated_at_ms = ?
             WHERE owner_id = ? AND chat_id = ?",
        )
        .bind(status)
        .bind(i64::from(advance_generation))
        .bind(clear_usage)
        .bind(status == "completed")
        .bind(now_ms)
        .bind(error)
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::ChatNotFound);
        }
        self.load_working_context(owner_id, chat_id).await
    }

    /// Atomically starts a working-context operation unless another one is already running.
    ///
    /// # Errors
    /// Returns busy, not-found, or database errors.
    pub async fn begin_working_context_compaction(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<WorkingContextRecord, StorageError> {
        let result = sqlx::query(
            "UPDATE chat_working_contexts SET compaction_status = 'running',
                last_error = NULL, updated_at_ms = ?
             WHERE owner_id = ? AND chat_id = ? AND compaction_status != 'running'",
        )
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            return self.load_working_context(owner_id, chat_id).await;
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM chat_working_contexts WHERE owner_id = ? AND chat_id = ?",
        )
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .is_some()
        {
            Err(StorageError::WorkingContextBusy)
        } else {
            Err(StorageError::ChatNotFound)
        }
    }

    /// Stores the provider conversation mapping independently of Bot identity.
    ///
    /// # Errors
    ///
    /// Returns database errors.
    pub async fn set_provider_conversation(
        &self,
        bot_id: Uuid,
        chat_id: Uuid,
        profile_id: Uuid,
        conversation_id: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO provider_conversations (
                bot_id, chat_id, provider_profile_id, external_conversation_id
             ) VALUES (?, ?, ?, ?)
             ON CONFLICT(bot_id, chat_id, provider_profile_id)
             DO UPDATE SET external_conversation_id = excluded.external_conversation_id",
        )
        .bind(bot_id.to_string())
        .bind(chat_id.to_string())
        .bind(profile_id.to_string())
        .bind(conversation_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Removes only the provider conversation mapping, preserving the `HomeBot` chat and transcript.
    ///
    /// # Errors
    /// Returns database errors.
    pub async fn reset_provider_conversation(
        &self,
        bot_id: Uuid,
        chat_id: Uuid,
        profile_id: Uuid,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "DELETE FROM provider_conversations
             WHERE bot_id = ? AND chat_id = ? AND provider_profile_id = ?",
        )
        .bind(bot_id.to_string())
        .bind(chat_id.to_string())
        .bind(profile_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads a provider conversation mapping for one Bot, chat, and profile.
    ///
    /// # Errors
    ///
    /// Returns database errors.
    pub async fn provider_conversation(
        &self,
        bot_id: Uuid,
        chat_id: Uuid,
        profile_id: Uuid,
    ) -> Result<Option<String>, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT external_conversation_id FROM provider_conversations
             WHERE bot_id = ? AND chat_id = ? AND provider_profile_id = ?",
        )
        .bind(bot_id.to_string())
        .bind(chat_id.to_string())
        .bind(profile_id.to_string())
        .fetch_optional(&self.pool)
        .await?)
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
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM chats WHERE owner_id = ? AND id = ?")
                .bind(owner_id.to_string())
                .bind(chat_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        if exists != 1 {
            return Err(StorageError::ChatNotFound);
        }
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
        input: QueuedPromptInput<'_>,
        now_ms: i64,
    ) -> Result<QueuedPrompt, StorageError> {
        let chat = self.get_direct_chat(owner_id, chat_id).await?;
        if !chat.running {
            return Err(StorageError::Integrity(
                "cannot queue a prompt while the chat is idle".to_owned(),
            ));
        }
        let validation = ChatMessage::user(
            chat_id,
            input.content,
            input.attachment_ids,
            None,
            Vec::new(),
            now_ms,
        )?;
        let mut transaction = self.pool.begin().await?;
        let reserved = sqlx::query(
            "UPDATE chats SET updated_at_ms = updated_at_ms
             WHERE id = ? AND owner_id = ? AND running = 1",
        )
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if reserved.rows_affected() != 1 {
            return Err(StorageError::Integrity(
                "cannot queue a prompt while the chat is idle".to_owned(),
            ));
        }
        validate_message_references(&mut transaction, owner_id, &validation).await?;
        let next_position =
            queue_position_for_insert(&mut transaction, chat_id, input.kind).await?;
        let resolved_references =
            resolve_typed_references(&mut transaction, owner_id, input.references).await?;
        sqlx::query(
            "INSERT INTO queued_prompts (
                id, owner_id, chat_id, content, attachment_ids_json, skill_ids_json,
                skill_version_ids_json, prompt_kind, position, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(prompt_id.to_string())
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .bind(input.content.trim())
        .bind(serde_json::to_value(input.attachment_ids).map_err(|error| json_error(&error))?)
        .bind(
            serde_json::to_value(
                input
                    .applied_skills
                    .iter()
                    .map(|skill| skill.skill_id)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| json_error(&error))?,
        )
        .bind(
            serde_json::to_value(
                input
                    .applied_skills
                    .iter()
                    .map(|skill| skill.version_id)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| json_error(&error))?,
        )
        .bind(input.kind.as_str())
        .bind(next_position)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        insert_queued_prompt_references(&mut transaction, prompt_id, &resolved_references).await?;
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
        queued_prompt_result(prompt_id, owner_id, chat_id, &input, next_position, now_ms)
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

    /// Atomically promotes the oldest queued prompt into durable transcript history.
    ///
    /// The chat is reserved as running in the same transaction, so a concurrent send queues
    /// behind this prompt instead of starting a second provider turn.
    ///
    /// # Errors
    /// Returns validation, ownership, attachment, Skill-version, or database errors.
    pub async fn promote_next_queued_prompt(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<Option<PromotedQueuedPrompt>, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let reserved = sqlx::query(
            "UPDATE chats SET updated_at_ms = updated_at_ms
             WHERE id = ? AND owner_id = ? AND running = 0 AND queued_count > 0",
        )
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if reserved.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT q.* FROM queued_prompts q JOIN chats c ON c.id = q.chat_id
             WHERE q.owner_id = ? AND q.chat_id = ? AND c.running = 0
             ORDER BY q.position, q.created_at_ms, q.id LIMIT 1",
        )
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let prompt = queued_prompt_from_row(&row)?;
        if prompt.skill_ids.len() != prompt.skill_version_ids.len() {
            return Err(StorageError::Integrity(
                "queued prompt Skill versions are inconsistent".to_owned(),
            ));
        }
        let message = insert_promoted_message(&mut transaction, owner_id, &prompt, now_ms).await?;
        reserve_promoted_chat(&mut transaction, owner_id, &prompt, now_ms).await?;
        transaction.commit().await?;
        let applied_skills = self.message_applied_skills(owner_id, message.id).await?;
        Ok(Some(PromotedQueuedPrompt {
            prompt,
            message,
            applied_skills,
        }))
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

    /// Increments direct-chat and Bot unread state after a terminal Bot response.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn increment_chat_unread(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<DirectChat, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let bot_id: Option<String> = sqlx::query_scalar(
            "UPDATE chats SET unread_count = unread_count + 1, updated_at_ms = ?
             WHERE id = ? AND owner_id = ? RETURNING direct_bot_id",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .flatten();
        let Some(bot_id) = bot_id else {
            return Err(StorageError::ChatNotFound);
        };
        sqlx::query(
            "UPDATE bots SET unread_count = unread_count + 1, updated_at_ms = ?
             WHERE id = ? AND owner_id = ?",
        )
        .bind(now_ms)
        .bind(bot_id)
        .bind(owner_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_direct_chat(owner_id, chat_id).await
    }

    /// Clears direct-chat and corresponding Bot unread state.
    ///
    /// # Errors
    ///
    /// Returns not-found or database errors.
    pub async fn mark_chat_read(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        now_ms: i64,
    ) -> Result<(DirectChat, bool), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let bot_id: Option<String> = sqlx::query_scalar(
            "UPDATE chats SET unread_count = 0, updated_at_ms = ?
             WHERE id = ? AND owner_id = ? AND unread_count != 0 RETURNING direct_bot_id",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(owner_id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .flatten();
        let changed = bot_id.is_some();
        if let Some(bot_id) = bot_id {
            sqlx::query(
                "UPDATE bots SET unread_count = 0, updated_at_ms = ? WHERE id = ? AND owner_id = ?",
            )
            .bind(now_ms)
            .bind(bot_id)
            .bind(owner_id.to_string())
            .execute(&mut *transaction)
            .await?;
        } else {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id = ? AND owner_id = ?)",
            )
            .bind(chat_id.to_string())
            .bind(owner_id.to_string())
            .fetch_one(&mut *transaction)
            .await?;
            if !exists {
                return Err(StorageError::ChatNotFound);
            }
        }
        transaction.commit().await?;
        Ok((self.get_direct_chat(owner_id, chat_id).await?, changed))
    }

    /// Inserts or updates a normalized execution activity.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn upsert_activity(
        &self,
        owner_id: Uuid,
        activity: &ExecutionActivity,
    ) -> Result<(), StorageError> {
        self.require_owned_chat(owner_id, activity.chat_id).await?;
        sqlx::query(
            "INSERT INTO execution_activities (
                id, chat_id, message_id, kind, status, detail_json, title, detail,
                requires_attention, started_at_ms, finished_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                title = excluded.title,
                detail = excluded.detail,
                detail_json = excluded.detail_json,
                requires_attention = excluded.requires_attention,
                finished_at_ms = excluded.finished_at_ms
             WHERE execution_activities.chat_id = excluded.chat_id",
        )
        .bind(activity.id.to_string())
        .bind(activity.chat_id.to_string())
        .bind(activity.message_id.map(|id| id.to_string()))
        .bind(&activity.kind)
        .bind(activity.status.as_str())
        .bind(&activity.presentation_json)
        .bind(&activity.title)
        .bind(&activity.detail)
        .bind(activity.requires_attention)
        .bind(activity.started_at_ms)
        .bind(activity.finished_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists normalized activities for an owner-scoped chat.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn chat_activities(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<ExecutionActivity>, StorageError> {
        self.require_owned_chat(owner_id, chat_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM execution_activities
             WHERE chat_id = ? ORDER BY started_at_ms, id",
        )
        .bind(chat_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(activity_from_row).collect()
    }

    /// Terminalizes unfinished provider interactions for one message operation.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn finish_provider_interactions(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
        message_id: Uuid,
        operation_id: Uuid,
        now_ms: i64,
    ) -> Result<(Vec<ExecutionActivity>, Vec<ChatApproval>), StorageError> {
        self.require_owned_chat(owner_id, chat_id).await?;
        let mut transaction = self.pool.begin().await?;
        let activity_rows = sqlx::query(
            "UPDATE execution_activities
             SET status = 'cancelled', finished_at_ms = ?
             WHERE chat_id = ? AND message_id = ? AND status IN ('pending', 'running')
             RETURNING *",
        )
        .bind(now_ms)
        .bind(chat_id.to_string())
        .bind(message_id.to_string())
        .fetch_all(&mut *transaction)
        .await?;
        let approval_rows = sqlx::query(
            "UPDATE approvals
             SET status = 'expired', decided_at_ms = ?
             WHERE owner_id = ? AND chat_id = ? AND message_id = ?
               AND operation_id = ? AND status = 'pending'
             RETURNING *",
        )
        .bind(now_ms)
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .bind(message_id.to_string())
        .bind(operation_id.to_string())
        .fetch_all(&mut *transaction)
        .await?;
        let activities = activity_rows
            .iter()
            .map(activity_from_row)
            .collect::<Result<_, _>>()?;
        let approvals = approval_rows
            .iter()
            .map(approval_from_row)
            .collect::<Result<_, _>>()?;
        transaction.commit().await?;
        Ok((activities, approvals))
    }

    /// Marks provider turns left in progress by a server restart as retryable failures.
    ///
    /// # Errors
    ///
    /// Returns database errors.
    pub async fn recover_interrupted_chat_turns(
        &self,
        owner_id: Uuid,
        now_ms: i64,
    ) -> Result<u64, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let owner_id = owner_id.to_string();
        sqlx::query(
            "UPDATE execution_activities SET status = 'failed', finished_at_ms = ?
             WHERE status IN ('pending', 'running') AND message_id IN (
                 SELECT m.id FROM messages m JOIN chats c ON c.id = m.chat_id
                 WHERE m.status = 'streaming' AND c.owner_id = ?
             )",
        )
        .bind(now_ms)
        .bind(&owner_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE approvals SET status = 'expired', decided_at_ms = ?
             WHERE status = 'pending' AND owner_id = ? AND message_id IN (
                 SELECT m.id FROM messages m WHERE m.status = 'streaming'
             )",
        )
        .bind(now_ms)
        .bind(&owner_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE group_bot_states
             SET status = 'failed', active_operation_id = NULL, updated_at_ms = ?
             WHERE status = 'running' AND chat_id IN (
                 SELECT id FROM chats WHERE owner_id = ? AND kind = 'group'
             ) AND EXISTS (
                 SELECT 1 FROM messages m
                 WHERE m.chat_id = group_bot_states.chat_id
                   AND m.author_bot_id = group_bot_states.bot_id
                   AND m.status = 'streaming'
             )",
        )
        .bind(now_ms)
        .bind(&owner_id)
        .execute(&mut *transaction)
        .await?;
        let messages = sqlx::query(
            "UPDATE messages SET status = 'failed', error_json = ?, completed_at_ms = ?
             WHERE status = 'streaming' AND chat_id IN (
                 SELECT id FROM chats WHERE owner_id = ?
             )",
        )
        .bind(serde_json::json!({
            "code": "provider_unavailable",
            "message": "HomeBot restarted before the provider turn completed",
            "retryable": true,
            "request_id": null,
            "retry_after_ms": null,
            "details": null
        }))
        .bind(now_ms)
        .bind(&owner_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE chats SET running = 0, updated_at_ms = ?
             WHERE owner_id = ? AND running = 1",
        )
        .bind(now_ms)
        .bind(&owner_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(messages)
    }

    /// Creates a pending approval for a chat operation.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn create_chat_approval(&self, approval: &ChatApproval) -> Result<(), StorageError> {
        self.require_owned_chat(approval.owner_id, approval.chat_id)
            .await?;
        sqlx::query(
            "INSERT INTO approvals (
                id, owner_id, chat_id, message_id, operation_id, capability, status,
                request_json, title, detail, created_at_ms, decided_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, '{}', ?, ?, ?, ?)",
        )
        .bind(approval.id.to_string())
        .bind(approval.owner_id.to_string())
        .bind(approval.chat_id.to_string())
        .bind(approval.message_id.map(|id| id.to_string()))
        .bind(approval.operation_id.to_string())
        .bind(&approval.capability)
        .bind(approval.status.as_str())
        .bind(&approval.title)
        .bind(&approval.detail)
        .bind(approval.created_at_ms)
        .bind(approval.decided_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Resolves a pending approval exactly once.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-transition, or database errors.
    pub async fn decide_chat_approval(
        &self,
        owner_id: Uuid,
        approval_id: Uuid,
        allow: bool,
        now_ms: i64,
    ) -> Result<ChatApproval, StorageError> {
        let status = if allow { "allowed" } else { "denied" };
        let result = sqlx::query(
            "UPDATE approvals SET status = ?, decided_at_ms = ?
             WHERE id = ? AND owner_id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(now_ms)
        .bind(approval_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::ApprovalNotFound);
        }
        self.chat_approval(owner_id, approval_id).await
    }

    /// Lists approvals for an owner-scoped chat.
    ///
    /// # Errors
    ///
    /// Returns ownership, database, or integrity errors.
    pub async fn chat_approvals(
        &self,
        owner_id: Uuid,
        chat_id: Uuid,
    ) -> Result<Vec<ChatApproval>, StorageError> {
        self.require_owned_chat(owner_id, chat_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM approvals WHERE owner_id = ? AND chat_id = ?
             ORDER BY created_at_ms, id",
        )
        .bind(owner_id.to_string())
        .bind(chat_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(approval_from_row).collect()
    }

    /// Loads one owner-scoped approval.
    ///
    /// # Errors
    ///
    /// Returns not-found, database, or integrity errors.
    pub async fn chat_approval(
        &self,
        owner_id: Uuid,
        approval_id: Uuid,
    ) -> Result<ChatApproval, StorageError> {
        let row = sqlx::query("SELECT * FROM approvals WHERE owner_id = ? AND id = ?")
            .bind(owner_id.to_string())
            .bind(approval_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::ApprovalNotFound)?;
        approval_from_row(&row)
    }

    async fn require_owned_chat(&self, owner_id: Uuid, chat_id: Uuid) -> Result<(), StorageError> {
        let exists: i64 =
            sqlx::query_scalar("SELECT count(*) FROM chats WHERE id = ? AND owner_id = ?")
                .bind(chat_id.to_string())
                .bind(owner_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        if exists == 1 {
            Ok(())
        } else {
            Err(StorageError::ChatNotFound)
        }
    }

    /// Runs `SQLite` structural and foreign-key checks without attempting repair.
    ///
    /// # Errors
    ///
    /// Returns an integrity error when corruption or broken references are reported.
    pub async fn verify_integrity(&self) -> Result<(), StorageError> {
        verify_pool_integrity(&self.pool).await?;
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

    /// Persists only the digest and metadata for a short-lived pairing credential.
    ///
    /// # Errors
    /// Returns an integrity or database error for invalid or duplicate records.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_pairing_credential(
        &self,
        owner_id: Uuid,
        id: Uuid,
        token_digest: &[u8; 32],
        native_proof_digest: &[u8; 32],
        endpoint: &str,
        expected_origin: &str,
        endpoint_kind: &str,
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<PairingCredentialRecord, StorageError> {
        if expires_at_ms <= created_at_ms {
            return Err(StorageError::Integrity(
                "pairing expiry must be after creation".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO pairing_credentials (
                id, owner_id, token_digest, native_proof_digest, endpoint, expected_origin,
                endpoint_kind, created_at_ms, expires_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(owner_id.to_string())
        .bind(token_digest.as_slice())
        .bind(native_proof_digest.as_slice())
        .bind(endpoint)
        .bind(expected_origin)
        .bind(endpoint_kind)
        .bind(created_at_ms)
        .bind(expires_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(PairingCredentialRecord {
            id,
            owner_id,
            endpoint: endpoint.to_owned(),
            expected_origin: expected_origin.to_owned(),
            endpoint_kind: endpoint_kind.to_owned(),
            created_at_ms,
            expires_at_ms,
            consumed_at_ms: None,
        })
    }

    /// Atomically consumes one pairing credential and creates a revocable device session.
    ///
    /// Every attempt is durably rate limited without retaining the supplied raw token.
    ///
    /// # Errors
    /// Returns precise expired, consumed, origin, rate-limit, validation, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn exchange_pairing_credential(
        &self,
        owner_id: Uuid,
        token_digest: &[u8; 32],
        native_proof_digest: Option<&[u8; 32]>,
        request_origin: Option<&str>,
        source_digest: &[u8; 32],
        device_id: Uuid,
        device_name: &str,
        session_digest: &[u8; 32],
        now_ms: i64,
        rate_window_ms: i64,
    ) -> Result<DeviceSessionRecord, StorageError> {
        let name = validated_device_name(device_name)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM pairing_exchange_attempts WHERE attempted_at_ms < ?")
            .bind(now_ms.saturating_sub(rate_window_ms))
            .execute(&mut *transaction)
            .await?;
        let row = sqlx::query(
            "SELECT id, expected_origin, native_proof_digest, endpoint_kind, expires_at_ms,
                    consumed_at_ms, failed_attempts
             FROM pairing_credentials WHERE owner_id = ? AND token_digest = ?",
        )
        .bind(owner_id.to_string())
        .bind(token_digest.as_slice())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            if !record_unknown_pairing_attempt(
                &mut transaction,
                token_digest,
                source_digest,
                now_ms,
            )
            .await?
            {
                transaction.rollback().await?;
                return Err(StorageError::PairingRateLimited);
            }
            transaction.commit().await?;
            return Err(StorageError::PairingNotFound);
        };
        let pairing_id: String = row.try_get("id")?;
        let expected_origin: String = row.try_get("expected_origin")?;
        let expected_native_proof: Option<Vec<u8>> = row.try_get("native_proof_digest")?;
        let endpoint_kind: String = row.try_get("endpoint_kind")?;
        let expires_at_ms: i64 = row.try_get("expires_at_ms")?;
        let consumed_at_ms: Option<i64> = row.try_get("consumed_at_ms")?;
        let failed_attempts: i64 = row.try_get("failed_attempts")?;
        if let Err(error) =
            pairing_credential_state(consumed_at_ms, expires_at_ms, failed_attempts, now_ms)
        {
            transaction.commit().await?;
            return Err(error);
        }
        if !pairing_provenance_matches(
            request_origin,
            native_proof_digest,
            &expected_origin,
            expected_native_proof.as_deref(),
        ) {
            sqlx::query(
                "UPDATE pairing_credentials SET failed_attempts = failed_attempts + 1 WHERE id = ?",
            )
            .bind(&pairing_id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Err(StorageError::PairingOriginMismatch);
        }
        let consumed = sqlx::query(
            "UPDATE pairing_credentials SET consumed_at_ms = ?
             WHERE id = ? AND consumed_at_ms IS NULL",
        )
        .bind(now_ms)
        .bind(&pairing_id)
        .execute(&mut *transaction)
        .await?;
        if consumed.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StorageError::PairingConsumed);
        }
        sqlx::query(
            "INSERT INTO device_sessions (
                id, owner_id, name, token_digest, endpoint_kind, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(device_id.to_string())
        .bind(owner_id.to_string())
        .bind(name)
        .bind(session_digest.as_slice())
        .bind(&endpoint_kind)
        .bind(now_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(DeviceSessionRecord {
            id: device_id,
            owner_id,
            name: name.to_owned(),
            endpoint_kind,
            created_at_ms: now_ms,
            last_seen_at_ms: None,
            revoked_at_ms: None,
        })
    }

    /// Authenticates an active device-session digest and coarsely updates last-seen time.
    ///
    /// # Errors
    /// Returns an integrity or database error when stored device metadata is invalid.
    pub async fn authenticate_device_session(
        &self,
        token_digest: &[u8; 32],
        now_ms: i64,
    ) -> Result<Option<DeviceSessionRecord>, StorageError> {
        let row = sqlx::query(
            "SELECT id, owner_id, name, endpoint_kind, created_at_ms, last_seen_at_ms, revoked_at_ms
             FROM device_sessions WHERE token_digest = ? AND revoked_at_ms IS NULL",
        )
        .bind(token_digest.as_slice())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let record = device_session_from_row(&row)?;
        if record
            .last_seen_at_ms
            .is_none_or(|last_seen| last_seen <= now_ms.saturating_sub(60_000))
        {
            sqlx::query(
                "UPDATE device_sessions SET last_seen_at_ms = ?
                 WHERE id = ? AND revoked_at_ms IS NULL",
            )
            .bind(now_ms)
            .bind(record.id.to_string())
            .execute(&self.pool)
            .await?;
        }
        Ok(Some(DeviceSessionRecord {
            last_seen_at_ms: Some(now_ms),
            ..record
        }))
    }

    /// Lists every active and revoked device session for owner management.
    ///
    /// # Errors
    /// Returns an integrity or database error when device metadata cannot be loaded.
    pub async fn device_sessions(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<DeviceSessionRecord>, StorageError> {
        sqlx::query(
            "SELECT id, owner_id, name, endpoint_kind, created_at_ms, last_seen_at_ms, revoked_at_ms
             FROM device_sessions WHERE owner_id = ? ORDER BY created_at_ms, id",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(device_session_from_row)
        .collect()
    }

    /// Checks whether a device session remains active for long-lived transports.
    ///
    /// # Errors
    /// Returns a database error when session state cannot be read.
    pub async fn device_session_is_active(
        &self,
        owner_id: Uuid,
        device_id: Uuid,
    ) -> Result<bool, StorageError> {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM device_sessions
                WHERE id = ? AND owner_id = ? AND revoked_at_ms IS NULL
             )",
        )
        .bind(device_id.to_string())
        .bind(owner_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Revokes a named device session idempotently.
    ///
    /// # Errors
    /// Returns not-found, integrity, or database errors.
    pub async fn revoke_device_session(
        &self,
        owner_id: Uuid,
        device_id: Uuid,
        now_ms: i64,
    ) -> Result<DeviceSessionRecord, StorageError> {
        let result = sqlx::query(
            "UPDATE device_sessions SET revoked_at_ms = COALESCE(revoked_at_ms, ?)
             WHERE id = ? AND owner_id = ?",
        )
        .bind(now_ms)
        .bind(device_id.to_string())
        .bind(owner_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::DeviceSessionNotFound);
        }
        let row = sqlx::query(
            "SELECT id, owner_id, name, endpoint_kind, created_at_ms, last_seen_at_ms, revoked_at_ms
             FROM device_sessions WHERE id = ? AND owner_id = ?",
        )
        .bind(device_id.to_string())
        .bind(owner_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        device_session_from_row(&row)
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

    /// Releases a just-claimed mutation that stopped at a structured approval boundary.
    ///
    /// Callers must only release keys they claimed in the same server operation before any
    /// external side effect. This permits an approval-bearing retry to claim the canonical
    /// mutation without treating the approval token itself as product input.
    ///
    /// # Errors
    /// Returns a database error if the release cannot be persisted.
    pub async fn release_idempotency(&self, key: Uuid) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM idempotency_records WHERE key = ?")
            .bind(key.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
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
        // Reserve the SQLite writer before taking the idempotency snapshot.
        // Without this harmless write, a concurrent scheduler commit can turn
        // the later insert into SQLITE_BUSY_SNAPSHOT instead of honoring the
        // configured busy timeout.
        sqlx::query(
            "UPDATE attachment_create_requests SET request_hash = request_hash
             WHERE idempotency_key = ?",
        )
        .bind(idempotency_key.to_string())
        .execute(&mut *transaction)
        .await?;
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

    /// Stores server-created artifact metadata after its content has been verified.
    ///
    /// # Errors
    ///
    /// Returns an ownership, range, or database error.
    pub async fn insert_artifact(&self, artifact: &ArtifactRecord) -> Result<(), StorageError> {
        self.require_owned_chat(artifact.owner_id, artifact.chat_id)
            .await?;
        let size_bytes = i64::try_from(artifact.size_bytes).map_err(|_| {
            StorageError::Integrity("artifact size exceeds SQLite range".to_owned())
        })?;
        sqlx::query(
            "INSERT INTO artifacts (
                id, owner_id, chat_id, message_id, activity_id, name, kind, media_type,
                size_bytes, sha256, storage_path, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(artifact.id.to_string())
        .bind(artifact.owner_id.to_string())
        .bind(artifact.chat_id.to_string())
        .bind(artifact.message_id.map(|id| id.to_string()))
        .bind(artifact.activity_id.map(|id| id.to_string()))
        .bind(&artifact.name)
        .bind(&artifact.kind)
        .bind(&artifact.media_type)
        .bind(size_bytes)
        .bind(&artifact.sha256)
        .bind(&artifact.storage_path)
        .bind(artifact.created_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns server-created artifact metadata visible to an owner.
    ///
    /// # Errors
    ///
    /// Returns a storage error if metadata cannot be decoded.
    pub async fn artifact(
        &self,
        owner_id: Uuid,
        artifact_id: Uuid,
    ) -> Result<Option<ArtifactRecord>, StorageError> {
        let row = sqlx::query(
            "SELECT id, owner_id, chat_id, message_id, activity_id, name, kind, media_type,
                    size_bytes, sha256, storage_path, created_at_ms
             FROM artifacts WHERE id = ? AND owner_id = ?",
        )
        .bind(artifact_id.to_string())
        .bind(owner_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| artifact_from_row(&row)).transpose()
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

async fn schema_version(pool: &SqlitePool) -> Result<u32, StorageError> {
    let has_migrations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await?;
    if has_migrations == 0 {
        return Ok(0);
    }
    let version: i64 = sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await?;
    u32::try_from(version)
        .map_err(|_| StorageError::Integrity("invalid migration version".to_owned()))
}

async fn verify_pool_integrity(pool: &SqlitePool) -> Result<(), StorageError> {
    let results: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(pool)
        .await?;
    if results.as_slice() != ["ok"] {
        return Err(StorageError::Integrity(results.join("; ")));
    }
    Ok(())
}

fn migration_backup_path(path: &Path, from_version: u32) -> Result<PathBuf, StorageError> {
    let filename = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or(StorageError::InvalidPath)?;
    Ok(path.with_file_name(format!(
        "{filename}.pre-migration-v{from_version}-to-v{SCHEMA_VERSION}.db"
    )))
}

async fn create_verified_migration_backup(
    pool: &SqlitePool,
    database: &Path,
    from_version: u32,
) -> Result<PathBuf, StorageError> {
    let backup = migration_backup_path(database, from_version)?;
    if let Ok(metadata) = backup.symlink_metadata()
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        return Err(StorageError::Integrity(
            "migration backup path is not a regular file".to_owned(),
        ));
    }
    if !backup.exists() {
        if let Err(error) = sqlx::query("VACUUM INTO ?")
            .bind(backup.to_str().ok_or(StorageError::InvalidPath)?)
            .execute(pool)
            .await
        {
            let _ = std::fs::remove_file(&backup);
            return Err(StorageError::Sql(error));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600))?;
    }
    let options =
        SqliteConnectOptions::from_str(backup.to_str().ok_or(StorageError::InvalidPath)?)?
            .read_only(true)
            .create_if_missing(false)
            .foreign_keys(true);
    let verification = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    verify_pool_integrity(&verification).await?;
    let backup_version = schema_version(&verification).await?;
    verification.close().await;
    if backup_version != from_version {
        return Err(StorageError::Integrity(format!(
            "migration backup schema version {backup_version} did not match source {from_version}"
        )));
    }
    Ok(backup)
}

async fn record_unknown_pairing_attempt(
    transaction: &mut Transaction<'_, Sqlite>,
    token_digest: &[u8; 32],
    source_digest: &[u8; 32],
    now_ms: i64,
) -> Result<bool, StorageError> {
    let source_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pairing_exchange_attempts WHERE source_digest = ?",
    )
    .bind(source_digest.as_slice())
    .fetch_one(&mut **transaction)
    .await?;
    if source_attempts >= 30 {
        return Ok(false);
    }
    sqlx::query(
        "DELETE FROM pairing_exchange_attempts WHERE id IN (
            SELECT id FROM pairing_exchange_attempts ORDER BY attempted_at_ms, id
            LIMIT max((SELECT count(*) FROM pairing_exchange_attempts) - 9999, 0)
         )",
    )
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO pairing_exchange_attempts (token_digest, source_digest, attempted_at_ms)
         VALUES (?, ?, ?)",
    )
    .bind(token_digest.as_slice())
    .bind(source_digest.as_slice())
    .bind(now_ms)
    .execute(&mut **transaction)
    .await?;
    Ok(true)
}

fn pairing_provenance_matches(
    request_origin: Option<&str>,
    native_proof_digest: Option<&[u8; 32]>,
    expected_origin: &str,
    expected_native_proof: Option<&[u8]>,
) -> bool {
    match (request_origin, native_proof_digest) {
        (Some(origin), None) => origin == expected_origin,
        (None, Some(proof)) => expected_native_proof.is_some_and(|expected| proof == expected),
        _ => false,
    }
}

fn pairing_credential_state(
    consumed_at_ms: Option<i64>,
    expires_at_ms: i64,
    failed_attempts: i64,
    now_ms: i64,
) -> Result<(), StorageError> {
    if consumed_at_ms.is_some() {
        Err(StorageError::PairingConsumed)
    } else if expires_at_ms <= now_ms {
        Err(StorageError::PairingExpired)
    } else if failed_attempts >= 5 {
        Err(StorageError::PairingRateLimited)
    } else {
        Ok(())
    }
}

fn validated_device_name(device_name: &str) -> Result<&str, StorageError> {
    let name = device_name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(StorageError::Integrity(
            "device name must contain 1 to 80 characters".to_owned(),
        ));
    }
    Ok(name)
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
        pinned_at_ms: row.try_get("pinned_at_ms")?,
        hidden_at_ms: row.try_get("hidden_at_ms")?,
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

fn group_chat_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<GroupChat, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    let ownership_bot_id: String = row.try_get("ownership_bot_id")?;
    let coordination_max_turns: i64 = row.try_get("coordination_max_turns")?;
    let coordination_turns_used: i64 = row.try_get("coordination_turns_used")?;
    let max_parallel_bots: i64 = row.try_get("max_parallel_bots")?;
    Ok(GroupChat {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        title: row.try_get("title")?,
        ownership_bot_id: parse_uuid(&ownership_bot_id)?,
        coordination_max_turns: u32::try_from(coordination_max_turns)
            .map_err(|_| StorageError::Integrity("invalid group turn limit".to_owned()))?,
        coordination_turns_used: u32::try_from(coordination_turns_used)
            .map_err(|_| StorageError::Integrity("invalid group turn count".to_owned()))?,
        max_parallel_bots: u32::try_from(max_parallel_bots)
            .map_err(|_| StorageError::Integrity("invalid group parallel limit".to_owned()))?,
        stop_requested: row.try_get("stop_requested")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn group_participant_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<GroupParticipant, StorageError> {
    let chat_id: String = row.try_get("chat_id")?;
    let bot_id: String = row.try_get("bot_id")?;
    let role: String = row.try_get("role")?;
    let status: String = row.try_get("status")?;
    let operation_id: Option<String> = row.try_get("active_operation_id")?;
    Ok(GroupParticipant {
        chat_id: parse_uuid(&chat_id)?,
        bot_id: parse_uuid(&bot_id)?,
        role: role.parse()?,
        status: status.parse()?,
        active_operation_id: operation_id.as_deref().map(parse_uuid).transpose()?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn group_handoff_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<OwnershipHandoff, StorageError> {
    let id: String = row.try_get("id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let from_bot_id: String = row.try_get("from_bot_id")?;
    let to_bot_id: String = row.try_get("to_bot_id")?;
    let message_id: Option<String> = row.try_get("message_id")?;
    Ok(OwnershipHandoff {
        id: parse_uuid(&id)?,
        chat_id: parse_uuid(&chat_id)?,
        from_bot_id: parse_uuid(&from_bot_id)?,
        to_bot_id: parse_uuid(&to_bot_id)?,
        message_id: message_id.as_deref().map(parse_uuid).transpose()?,
        reason: row.try_get("reason")?,
        created_at_ms: row.try_get("created_at_ms")?,
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
    let shared_context: Value = row.try_get("shared_context_message_ids_json")?;
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
        shared_context_message_ids: serde_json::from_value(shared_context)
            .map_err(|error| json_error(&error))?,
        created_at_ms: row.try_get("created_at_ms")?,
        completed_at_ms: row.try_get("completed_at_ms")?,
        error_json,
    })
}

async fn queue_position_for_insert(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    chat_id: Uuid,
    kind: QueuedPromptKind,
) -> Result<i64, StorageError> {
    if kind == QueuedPromptKind::FollowUp {
        return sqlx::query_scalar(
            "SELECT COALESCE(max(position) + 1, 0) FROM queued_prompts WHERE chat_id = ?",
        )
        .bind(chat_id.to_string())
        .fetch_one(&mut **transaction)
        .await
        .map_err(StorageError::from);
    }
    let insertion: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM queued_prompts WHERE chat_id = ? AND prompt_kind = 'steering'",
    )
    .bind(chat_id.to_string())
    .fetch_one(&mut **transaction)
    .await?;
    let queued_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM queued_prompts WHERE chat_id = ?")
            .bind(chat_id.to_string())
            .fetch_one(&mut **transaction)
            .await?;
    let offset = queued_count + 1;
    sqlx::query(
        "UPDATE queued_prompts SET position = position + ?
         WHERE chat_id = ? AND position >= ?",
    )
    .bind(offset)
    .bind(chat_id.to_string())
    .bind(insertion)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE queued_prompts SET position = position - ?
         WHERE chat_id = ? AND position >= ?",
    )
    .bind(offset - 1)
    .bind(chat_id.to_string())
    .bind(insertion + offset)
    .execute(&mut **transaction)
    .await?;
    Ok(insertion)
}

fn queued_prompt_result(
    prompt_id: Uuid,
    owner_id: Uuid,
    chat_id: Uuid,
    input: &QueuedPromptInput<'_>,
    position: i64,
    now_ms: i64,
) -> Result<QueuedPrompt, StorageError> {
    Ok(QueuedPrompt {
        id: prompt_id,
        owner_id,
        chat_id,
        content: input.content.trim().to_owned(),
        attachment_ids: input.attachment_ids.to_vec(),
        skill_ids: input
            .applied_skills
            .iter()
            .map(|skill| skill.skill_id)
            .collect(),
        skill_version_ids: input
            .applied_skills
            .iter()
            .map(|skill| skill.version_id)
            .collect(),
        kind: input.kind,
        position: u32::try_from(position)
            .map_err(|_| StorageError::Integrity("invalid queue position".to_owned()))?,
        created_at_ms: now_ms,
    })
}

fn queued_prompt_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<QueuedPrompt, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let attachment_ids: Value = row.try_get("attachment_ids_json")?;
    let skill_ids: Value = row.try_get("skill_ids_json")?;
    let skill_version_ids: Value = row.try_get("skill_version_ids_json")?;
    let position: i64 = row.try_get("position")?;
    let kind = match row.try_get::<String, _>("prompt_kind")?.as_str() {
        "follow_up" => QueuedPromptKind::FollowUp,
        "steering" => QueuedPromptKind::Steering,
        _ => {
            return Err(StorageError::Integrity(
                "invalid queued prompt kind".to_owned(),
            ));
        }
    };
    Ok(QueuedPrompt {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        chat_id: parse_uuid(&chat_id)?,
        content: row.try_get("content")?,
        attachment_ids: serde_json::from_value(attachment_ids)
            .map_err(|error| json_error(&error))?,
        skill_ids: serde_json::from_value(skill_ids).map_err(|error| json_error(&error))?,
        skill_version_ids: serde_json::from_value(skill_version_ids)
            .map_err(|error| json_error(&error))?,
        kind,
        position: u32::try_from(position)
            .map_err(|_| StorageError::Integrity("invalid queue position".to_owned()))?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

async fn insert_promoted_message(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: Uuid,
    prompt: &QueuedPrompt,
    now_ms: i64,
) -> Result<ChatMessage, StorageError> {
    let mut message = ChatMessage::user(
        prompt.chat_id,
        &prompt.content,
        &prompt.attachment_ids,
        None,
        Vec::new(),
        now_ms,
    )?;
    message.id = prompt.id;
    validate_message_references(transaction, owner_id, &message).await?;
    sqlx::query("INSERT INTO messages (id, chat_id, author_kind, status, mentioned_bot_ids_json, created_at_ms, completed_at_ms) VALUES (?, ?, 'user', 'completed', '[]', ?, ?)")
        .bind(message.id.to_string()).bind(prompt.chat_id.to_string()).bind(now_ms).bind(now_ms)
        .execute(&mut **transaction).await?;
    for part in &message.parts {
        let (part_id, ordinal) = message_part_identity(part);
        sqlx::query("INSERT INTO message_parts (id, message_id, ordinal, kind, content_json) VALUES (?, ?, ?, ?, ?)")
            .bind(part_id.to_string()).bind(message.id.to_string()).bind(i64::from(ordinal))
            .bind(message_part_kind(part)).bind(serde_json::to_value(part).map_err(|error| json_error(&error))?)
            .execute(&mut **transaction).await?;
    }
    for (ordinal, (skill_id, version_id)) in prompt
        .skill_ids
        .iter()
        .zip(&prompt.skill_version_ids)
        .enumerate()
    {
        let inserted = sqlx::query("INSERT INTO message_skill_versions (message_id, skill_id, skill_version_id, ordinal) SELECT ?, s.id, v.id, ? FROM skills s JOIN skill_versions v ON v.skill_id = s.id WHERE s.owner_id = ? AND s.id = ? AND v.id = ?")
            .bind(message.id.to_string()).bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
            .bind(owner_id.to_string()).bind(skill_id.to_string()).bind(version_id.to_string())
            .execute(&mut **transaction).await?;
        if inserted.rows_affected() != 1 {
            return Err(StorageError::SkillNotFound);
        }
    }
    sqlx::query("INSERT INTO message_references (message_id, ordinal, kind, target_id, target_version_id, label_snapshot) SELECT ?, ordinal, kind, target_id, target_version_id, label_snapshot FROM queued_prompt_references WHERE prompt_id = ? ORDER BY ordinal")
        .bind(message.id.to_string()).bind(prompt.id.to_string()).execute(&mut **transaction).await?;
    Ok(message)
}

async fn resolve_typed_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: Uuid,
    references: &[(MessageReferenceKind, Uuid)],
) -> Result<Vec<MessageReferenceRecord>, StorageError> {
    let mut resolved = Vec::with_capacity(references.len());
    for (kind, target_id) in references {
        let (label_snapshot, target_version_id) = match kind {
            MessageReferenceKind::Bot => (
                sqlx::query_scalar::<_, String>(
                    "SELECT name FROM bots WHERE owner_id = ? AND id = ?",
                )
                .bind(owner_id.to_string())
                .bind(target_id.to_string())
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(StorageError::BotNotFound)?,
                None,
            ),
            MessageReferenceKind::Group => (
                sqlx::query_scalar::<_, String>(
                    "SELECT title FROM chats WHERE owner_id = ? AND id = ? AND kind = 'group'",
                )
                .bind(owner_id.to_string())
                .bind(target_id.to_string())
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(StorageError::ChatNotFound)?,
                None,
            ),
            MessageReferenceKind::Routine => {
                let row = sqlx::query("SELECT name, active_version_id FROM routines WHERE owner_id = ? AND id = ? AND bot_id IS NOT NULL")
                    .bind(owner_id.to_string()).bind(target_id.to_string()).fetch_optional(&mut **transaction).await?.ok_or(StorageError::RoutineNotFound)?;
                (
                    row.try_get("name")?,
                    Some(parse_uuid(&row.try_get::<String, _>("active_version_id")?)?),
                )
            }
            MessageReferenceKind::Plugin => (
                sqlx::query_scalar::<_, String>(
                    "SELECT name FROM plugins WHERE owner_id = ? AND id = ?",
                )
                .bind(owner_id.to_string())
                .bind(target_id.to_string())
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(StorageError::PluginNotFound)?,
                None,
            ),
        };
        resolved.push(MessageReferenceRecord {
            kind: *kind,
            target_id: *target_id,
            target_version_id,
            label_snapshot,
        });
    }
    Ok(resolved)
}

async fn insert_queued_prompt_references(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    prompt_id: Uuid,
    references: &[MessageReferenceRecord],
) -> Result<(), StorageError> {
    for (ordinal, reference) in references.iter().enumerate() {
        sqlx::query("INSERT INTO queued_prompt_references (prompt_id, ordinal, kind, target_id, target_version_id, label_snapshot) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(prompt_id.to_string())
            .bind(i64::try_from(ordinal).map_err(|_| StorageError::Integrity("too many message references".to_owned()))?)
            .bind(reference.kind.as_str()).bind(reference.target_id.to_string())
            .bind(reference.target_version_id.map(|id| id.to_string())).bind(&reference.label_snapshot)
            .execute(&mut **transaction).await?;
    }
    Ok(())
}

async fn insert_message_reference_records(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message_id: Uuid,
    references: &[MessageReferenceRecord],
) -> Result<(), StorageError> {
    for (ordinal, reference) in references.iter().enumerate() {
        sqlx::query("INSERT INTO message_references (message_id, ordinal, kind, target_id, target_version_id, label_snapshot) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(message_id.to_string())
            .bind(i64::try_from(ordinal).map_err(|_| StorageError::Integrity("too many message references".to_owned()))?)
            .bind(reference.kind.as_str()).bind(reference.target_id.to_string())
            .bind(reference.target_version_id.map(|id| id.to_string())).bind(&reference.label_snapshot)
            .execute(&mut **transaction).await?;
    }
    Ok(())
}

fn validate_capability_rule(rule: &CapabilityRuleRecord) -> Result<(), StorageError> {
    const CAPABILITIES: &[&str] = &[
        "filesystem_read",
        "filesystem_write",
        "process_execute",
        "browser_observe",
        "browser_act",
        "git_read",
        "git_write",
        "git_remote",
        "plugin_read",
        "plugin_write",
        "external_communication",
        "external_mutation",
        "secret_use",
        "device_administration",
    ];
    if !CAPABILITIES.contains(&rule.capability.as_str())
        || !matches!(rule.effect.as_str(), "allow" | "require_approval" | "deny")
        || rule.action_prefix.as_ref().is_some_and(|prefix| {
            prefix.is_empty()
                || prefix.chars().count() > 120
                || prefix.chars().any(char::is_control)
        })
    {
        return Err(StorageError::Integrity(
            "invalid capability rule".to_owned(),
        ));
    }
    Ok(())
}

async fn validate_capability_rule_scopes(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rule: &CapabilityRuleRecord,
) -> Result<(), StorageError> {
    for (table, id) in [
        ("device_sessions", rule.device_id),
        ("bots", rule.bot_id),
        ("chats", rule.chat_id),
        ("repository_workspaces", rule.workspace_id),
    ] {
        let Some(id) = id else { continue };
        let query = format!("SELECT count(*) FROM {table} WHERE owner_id = ? AND id = ?");
        let count: i64 = sqlx::query_scalar(&query)
            .bind(rule.owner_id.to_string())
            .bind(id.to_string())
            .fetch_one(&mut **transaction)
            .await?;
        if count != 1 {
            return Err(StorageError::Integrity(
                "capability rule scope is not owner accessible".to_owned(),
            ));
        }
    }
    Ok(())
}

async fn insert_capability_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rule: &CapabilityRuleRecord,
    action: &str,
    now_ms: i64,
) -> Result<(), StorageError> {
    let snapshot = serde_json::json!({
        "capability": rule.capability, "effect": rule.effect,
        "device_id": rule.device_id, "bot_id": rule.bot_id, "chat_id": rule.chat_id,
        "workspace_id": rule.workspace_id, "action_prefix": rule.action_prefix,
    });
    sqlx::query("INSERT INTO capability_rule_audit (id, owner_id, rule_id, action, snapshot_json, created_at_ms) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(Uuid::now_v7().to_string()).bind(rule.owner_id.to_string()).bind(rule.id.to_string())
        .bind(action).bind(snapshot).bind(now_ms).execute(&mut **transaction).await?;
    Ok(())
}

fn capability_rule_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CapabilityRuleRecord, StorageError> {
    Ok(CapabilityRuleRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        capability: row.try_get("capability")?,
        effect: row.try_get("effect")?,
        device_id: row
            .try_get::<Option<String>, _>("device_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        bot_id: row
            .try_get::<Option<String>, _>("bot_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        chat_id: row
            .try_get::<Option<String>, _>("chat_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        workspace_id: row
            .try_get::<Option<String>, _>("workspace_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        action_prefix: row.try_get("action_prefix")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn capability_audit_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CapabilityRuleAuditRecord, StorageError> {
    Ok(CapabilityRuleAuditRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        rule_id: parse_uuid(row.try_get("rule_id")?)?,
        action: row.try_get("action")?,
        snapshot: row.try_get("snapshot_json")?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn validate_browser_state(controller: &str, status: &str) -> Result<(), StorageError> {
    if !matches!(controller, "bot" | "user")
        || !matches!(status, "active" | "awaiting_approval" | "closed" | "failed")
    {
        return Err(StorageError::Integrity(
            "invalid browser session state".to_owned(),
        ));
    }
    Ok(())
}

fn browser_session_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<BrowserSessionRecord, StorageError> {
    Ok(BrowserSessionRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        bot_id: parse_uuid(row.try_get("bot_id")?)?,
        profile_id: parse_uuid(row.try_get("profile_id")?)?,
        runtime_session_id: row
            .try_get::<Option<String>, _>("runtime_session_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        profile_name: row.try_get("profile_name")?,
        directory_ref: row.try_get("directory_ref")?,
        current_url: row.try_get("current_url")?,
        controller: row.try_get("controller")?,
        status: row.try_get("status")?,
        pending_approval_id: row
            .try_get::<Option<String>, _>("pending_approval_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        controlling_device_id: row
            .try_get::<Option<String>, _>("controlling_device_id")?
            .as_deref()
            .map(parse_uuid)
            .transpose()?,
        takeover_expires_at_ms: row.try_get("takeover_expires_at_ms")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

async fn reserve_promoted_chat(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_id: Uuid,
    prompt: &QueuedPrompt,
    now_ms: i64,
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM queued_prompts WHERE id = ? AND owner_id = ?")
        .bind(prompt.id.to_string())
        .bind(owner_id.to_string())
        .execute(&mut **transaction)
        .await?;
    sqlx::query("UPDATE queued_prompts SET position = position - 1 WHERE owner_id = ? AND chat_id = ? AND position > ?")
        .bind(owner_id.to_string()).bind(prompt.chat_id.to_string()).bind(i64::from(prompt.position))
        .execute(&mut **transaction).await?;
    let reserved = sqlx::query("UPDATE chats SET queued_count = queued_count - 1, running = 1, updated_at_ms = ? WHERE id = ? AND owner_id = ? AND running = 0 AND queued_count > 0")
        .bind(now_ms).bind(prompt.chat_id.to_string()).bind(owner_id.to_string())
        .execute(&mut **transaction).await?;
    if reserved.rows_affected() != 1 {
        return Err(StorageError::Integrity(
            "queued prompt could not reserve the idle chat".to_owned(),
        ));
    }
    Ok(())
}

fn working_context_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkingContextRecord, StorageError> {
    let used_tokens = row
        .try_get::<Option<i64>, _>("used_tokens")?
        .map(u64::try_from)
        .transpose()
        .map_err(|_| StorageError::Integrity("invalid working-context usage".to_owned()))?;
    let context_window_tokens = row
        .try_get::<Option<i64>, _>("context_window_tokens")?
        .map(u64::try_from)
        .transpose()
        .map_err(|_| StorageError::Integrity("invalid context window".to_owned()))?;
    Ok(WorkingContextRecord {
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        provider_profile_id: parse_uuid(row.try_get("provider_profile_id")?)?,
        interaction_mode: row.try_get("interaction_mode")?,
        used_tokens,
        context_window_tokens,
        compaction_status: row.try_get("compaction_status")?,
        generation: u32::try_from(row.try_get::<i64, _>("generation")?)
            .map_err(|_| StorageError::Integrity("invalid context generation".to_owned()))?,
        compacted_at_ms: row.try_get("compacted_at_ms")?,
        last_error: row.try_get("last_error")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn device_session_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DeviceSessionRecord, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    Ok(DeviceSessionRecord {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        name: row.try_get("name")?,
        endpoint_kind: row.try_get("endpoint_kind")?,
        created_at_ms: row.try_get("created_at_ms")?,
        last_seen_at_ms: row.try_get("last_seen_at_ms")?,
        revoked_at_ms: row.try_get("revoked_at_ms")?,
    })
}

fn activity_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ExecutionActivity, StorageError> {
    let id: String = row.try_get("id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let message_id: Option<String> = row.try_get("message_id")?;
    let status: String = row.try_get("status")?;
    Ok(ExecutionActivity {
        id: parse_uuid(&id)?,
        chat_id: parse_uuid(&chat_id)?,
        message_id: message_id.as_deref().map(parse_uuid).transpose()?,
        kind: row.try_get("kind")?,
        title: row.try_get("title")?,
        detail: row.try_get("detail")?,
        presentation_json: row.try_get("detail_json")?,
        status: status.parse()?,
        requires_attention: row.try_get("requires_attention")?,
        started_at_ms: row.try_get("started_at_ms")?,
        finished_at_ms: row.try_get("finished_at_ms")?,
    })
}

fn approval_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ChatApproval, StorageError> {
    let id: String = row.try_get("id")?;
    let owner_id: String = row.try_get("owner_id")?;
    let chat_id: String = row.try_get("chat_id")?;
    let message_id: Option<String> = row.try_get("message_id")?;
    let operation_id: String = row.try_get("operation_id")?;
    let status: String = row.try_get("status")?;
    Ok(ChatApproval {
        id: parse_uuid(&id)?,
        owner_id: parse_uuid(&owner_id)?,
        chat_id: parse_uuid(&chat_id)?,
        message_id: message_id.as_deref().map(parse_uuid).transpose()?,
        operation_id: parse_uuid(&operation_id)?,
        capability: row.try_get("capability")?,
        title: row.try_get("title")?,
        detail: row.try_get("detail")?,
        status: status.parse()?,
        created_at_ms: row.try_get("created_at_ms")?,
        decided_at_ms: row.try_get("decided_at_ms")?,
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

fn artifact_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ArtifactRecord, StorageError> {
    let size_bytes: i64 = row.try_get("size_bytes")?;
    Ok(ArtifactRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        message_id: row
            .try_get::<Option<&str>, _>("message_id")?
            .map(parse_uuid)
            .transpose()?,
        activity_id: row
            .try_get::<Option<&str>, _>("activity_id")?
            .map(parse_uuid)
            .transpose()?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        media_type: row.try_get("media_type")?,
        size_bytes: u64::try_from(size_bytes)
            .map_err(|_| StorageError::Integrity("negative artifact size".to_owned()))?,
        sha256: row.try_get("sha256")?,
        storage_path: row.try_get("storage_path")?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn secret_reference_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SecretReferenceRecord, StorageError> {
    Ok(SecretReferenceRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        locator: row.try_get("locator")?,
        label: row.try_get("label")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn plugin_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<PluginRecord, StorageError> {
    let configuration_json: String = row.try_get("configuration_json")?;
    Ok(PluginRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        kind: row.try_get("kind")?,
        configuration: serde_json::from_str(&configuration_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        enabled: row.try_get("enabled")?,
        connection_id: parse_uuid(row.try_get("connection_id")?)?,
        transport: row.try_get("transport")?,
        status: row.try_get("status")?,
        auth_status: row.try_get("auth_status")?,
        error_message: row.try_get("error_message")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn plugin_tool_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<PluginToolRecord, StorageError> {
    let input_schema_json: String = row.try_get("input_schema_json")?;
    Ok(PluginToolRecord {
        name: row.try_get("name")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        input_schema: serde_json::from_str(&input_schema_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
    })
}

fn normalize_skill_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn workspace_mode(mode: WorkspaceMode) -> &'static str {
    match mode {
        WorkspaceMode::Primary => "primary",
        WorkspaceMode::Isolated => "isolated",
    }
}

fn checkpoint_phase(phase: CheckpointPhase) -> &'static str {
    match phase {
        CheckpointPhase::BeforeTurn => "before_turn",
        CheckpointPhase::AfterTurn => "after_turn",
        CheckpointPhase::RestoreSafety => "restore_safety",
    }
}

fn conversation_reconciliation(value: ConversationReconciliation) -> &'static str {
    match value {
        ConversationReconciliation::Unchanged => "unchanged",
        ConversationReconciliation::Forked => "forked",
    }
}

fn repository_workspace_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RepositoryWorkspaceRecord, StorageError> {
    Ok(RepositoryWorkspaceRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        name: row.try_get("name")?,
        root_path: row.try_get("root_path")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn chat_workspace_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ChatWorkspaceRecord, StorageError> {
    let mode: String = row.try_get("mode")?;
    Ok(ChatWorkspaceRecord {
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        workspace_id: parse_uuid(row.try_get("workspace_id")?)?,
        mode: match mode.as_str() {
            "primary" => WorkspaceMode::Primary,
            "isolated" => WorkspaceMode::Isolated,
            _ => return Err(StorageError::Integrity("invalid workspace mode".to_owned())),
        },
        worktree_path: row.try_get("worktree_path")?,
        branch_name: row.try_get("branch_name")?,
        base_ref: row.try_get("base_ref")?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn turn_checkpoint_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<TurnCheckpointRecord, StorageError> {
    let phase: String = row.try_get("phase")?;
    Ok(TurnCheckpointRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        workspace_id: parse_uuid(row.try_get("workspace_id")?)?,
        message_id: row
            .try_get::<Option<String>, _>("message_id")?
            .map(|value| parse_uuid(&value))
            .transpose()?,
        phase: match phase.as_str() {
            "before_turn" => CheckpointPhase::BeforeTurn,
            "after_turn" => CheckpointPhase::AfterTurn,
            "restore_safety" => CheckpointPhase::RestoreSafety,
            _ => {
                return Err(StorageError::Integrity(
                    "invalid checkpoint phase".to_owned(),
                ));
            }
        },
        git_ref: row.try_get("git_ref")?,
        commit_oid: row.try_get("commit_oid")?,
        provider_profile_id: row
            .try_get::<Option<String>, _>("provider_profile_id")?
            .map(|value| parse_uuid(&value))
            .transpose()?,
        provider_conversation_id: row.try_get("provider_conversation_id")?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn checkpoint_restore_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CheckpointRestoreRecord, StorageError> {
    let reconciliation: String = row.try_get("reconciliation")?;
    Ok(CheckpointRestoreRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        checkpoint_id: parse_uuid(row.try_get("checkpoint_id")?)?,
        safety_checkpoint_id: parse_uuid(row.try_get("safety_checkpoint_id")?)?,
        reconciliation: match reconciliation.as_str() {
            "unchanged" => ConversationReconciliation::Unchanged,
            "forked" => ConversationReconciliation::Forked,
            _ => {
                return Err(StorageError::Integrity(
                    "invalid checkpoint reconciliation".to_owned(),
                ));
            }
        },
        previous_provider_conversation_id: row.try_get("previous_provider_conversation_id")?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn vcs_operation_result_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<VcsOperationResultRecord, StorageError> {
    let response: String = row.try_get("response_json")?;
    Ok(VcsOperationResultRecord {
        idempotency_key: parse_uuid(row.try_get("idempotency_key")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        chat_id: parse_uuid(row.try_get("chat_id")?)?,
        action: row.try_get("action")?,
        response: serde_json::from_str(&response)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        created_at_ms: row.try_get("created_at_ms")?,
    })
}

fn skill_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SkillRecord, StorageError> {
    let definition: String = row.try_get("definition_json")?;
    Ok(SkillRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        active_version_id: parse_uuid(row.try_get("active_version_id")?)?,
        version: u32::try_from(row.try_get::<i64, _>("version")?)
            .map_err(|_| StorageError::Integrity("invalid Skill version".to_owned()))?,
        definition: serde_json::from_str(&definition)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        bot_ids: Vec::new(),
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn routine_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RoutineRecord, StorageError> {
    routine_from_row_selected(row, "active_version_id")
}

fn routine_from_row_selected(
    row: &sqlx::sqlite::SqliteRow,
    version_id_column: &str,
) -> Result<RoutineRecord, StorageError> {
    let definition_json: String = row.try_get("definition_json")?;
    let version: i64 = row.try_get("version")?;
    Ok(RoutineRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        bot_id: parse_uuid(row.try_get("bot_id")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        enabled: row.try_get("enabled")?,
        draft: row.try_get("draft")?,
        active_version_id: parse_uuid(row.try_get(version_id_column)?)?,
        version: u32::try_from(version)
            .map_err(|_| StorageError::Integrity("invalid routine version".to_owned()))?,
        definition: serde_json::from_str(&definition_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn routine_recording_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RoutineRecordingRecord, StorageError> {
    let actions_json: String = row.try_get("actions_json")?;
    Ok(RoutineRecordingRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        bot_id: parse_uuid(row.try_get("bot_id")?)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        actions: serde_json::from_str(&actions_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn routine_run_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RoutineRunRecord, StorageError> {
    let trigger_json: String = row.try_get("trigger_json")?;
    let input_json: String = row.try_get("input_json")?;
    let result_json: Option<String> = row.try_get("result_json")?;
    Ok(RoutineRunRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        routine_id: parse_uuid(row.try_get("routine_id")?)?,
        routine_version_id: parse_uuid(row.try_get("routine_version_id")?)?,
        bot_id: parse_uuid(row.try_get("bot_id")?)?,
        status: row.try_get("status")?,
        trigger: serde_json::from_str(&trigger_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        dry_run: row.try_get("dry_run")?,
        inputs: serde_json::from_str(&input_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        results: result_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        error_message: row.try_get("error_message")?,
        attempt_count: u16::try_from(row.try_get::<i64, _>("attempt_count")?)
            .map_err(|_| StorageError::Integrity("invalid routine attempt count".to_owned()))?,
        scheduled_for_ms: row.try_get("scheduled_for_ms")?,
        started_at_ms: row.try_get("started_at_ms")?,
        finished_at_ms: row.try_get("finished_at_ms")?,
    })
}

fn routine_trigger_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RoutineTriggerRecord, StorageError> {
    let configuration: String = row.try_get("configuration_json")?;
    Ok(RoutineTriggerRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        routine_id: parse_uuid(row.try_get("routine_id")?)?,
        definition: serde_json::from_str(&configuration)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        enabled: row.try_get("enabled")?,
        last_evaluated_at_ms: row.try_get("last_evaluated_at_ms")?,
        next_fire_at_ms: row.try_get("next_fire_at_ms")?,
        last_event_sequence: u64::try_from(row.try_get::<i64, _>("last_event_sequence")?)
            .map_err(|_| StorageError::Integrity("invalid event cursor".to_owned()))?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}

fn routine_job_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RoutineJobRecord, StorageError> {
    let trigger_json: String = row.try_get("trigger_json")?;
    let input_json: String = row.try_get("input_json")?;
    Ok(RoutineJobRecord {
        id: parse_uuid(row.try_get("id")?)?,
        owner_id: parse_uuid(row.try_get("owner_id")?)?,
        trigger_id: parse_uuid(row.try_get("trigger_id")?)?,
        routine_id: parse_uuid(row.try_get("routine_id")?)?,
        routine_version_id: parse_uuid(row.try_get("routine_version_id")?)?,
        delivery_key: row.try_get("delivery_key")?,
        trigger: serde_json::from_str(&trigger_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        inputs: serde_json::from_str(&input_json)
            .map_err(|error| StorageError::Serialization(error.to_string()))?,
        status: row.try_get("status")?,
        attempt_count: u16::try_from(row.try_get::<i64, _>("attempt_count")?)
            .map_err(|_| StorageError::Integrity("invalid routine job attempt count".to_owned()))?,
        scheduled_for_ms: row.try_get("scheduled_for_ms")?,
        next_attempt_at_ms: row.try_get("next_attempt_at_ms")?,
        cancel_requested: row.try_get("cancel_requested")?,
        error_message: row.try_get("error_message")?,
        created_at_ms: row.try_get("created_at_ms")?,
        started_at_ms: row.try_get("started_at_ms")?,
        finished_at_ms: row.try_get("finished_at_ms")?,
    })
}

fn trigger_kind(definition: &RoutineTriggerDefinition) -> &'static str {
    match &definition.source {
        homebot_routines::RoutineTriggerSource::Schedule { .. } => "schedule",
        homebot_routines::RoutineTriggerSource::Webhook { .. } => "webhook",
        homebot_routines::RoutineTriggerSource::Event { .. } => "event",
        homebot_routines::RoutineTriggerSource::Plugin { .. } => "plugin",
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(value)
        .map_err(|_| StorageError::Integrity("database contains an invalid UUID".to_owned()))
}

impl MessageReferenceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bot => "bot",
            Self::Group => "group",
            Self::Routine => "routine",
            Self::Plugin => "plugin",
        }
    }

    fn from_str(value: &str) -> Result<Self, StorageError> {
        match value {
            "bot" => Ok(Self::Bot),
            "group" => Ok(Self::Group),
            "routine" => Ok(Self::Routine),
            "plugin" => Ok(Self::Plugin),
            _ => Err(StorageError::Integrity(
                "database contains an invalid message reference kind".to_owned(),
            )),
        }
    }
}

fn links_in(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split_whitespace().filter_map(|word| {
        let candidate = word.trim_matches(|character: char| {
            matches!(
                character,
                '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '!' | '?'
            )
        });
        (candidate.starts_with("https://") || candidate.starts_with("http://"))
            .then(|| candidate.trim_end_matches(['.', ':']).to_owned())
    })
}

fn search_snippet(text: &str, needle: &str) -> String {
    const LIMIT: usize = 180;
    let characters = text.chars().collect::<Vec<_>>();
    if characters.len() <= LIMIT {
        return text.to_owned();
    }
    let lower = text.to_lowercase();
    let byte_position = lower.find(&needle.to_lowercase()).unwrap_or(0);
    let character_position = text
        .char_indices()
        .take_while(|(index, _)| *index < byte_position)
        .count();
    let start = character_position.saturating_sub(LIMIT / 3);
    let end = (start + LIMIT).min(characters.len());
    format!(
        "{}{}{}",
        if start == 0 { "" } else { "…" },
        characters[start..end].iter().collect::<String>(),
        if end == characters.len() { "" } else { "…" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_domain::chat::{ActivityStatus, ApprovalStatus};
    use serde_json::json;
    use std::borrow::Cow;

    fn skill_definition(instructions: &str) -> SkillDefinition {
        SkillDefinition {
            instructions: instructions.to_owned(),
            context: vec![homebot_skills::SkillContext {
                label: "Guide".to_owned(),
                content: "Follow repository conventions.".to_owned(),
            }],
            tools: vec![homebot_skills::SkillToolReference {
                plugin_name: "repository".to_owned(),
                tool_name: "status".to_owned(),
            }],
        }
    }

    #[tokio::test]
    async fn skills_are_versioned_assigned_and_historical_messages_survive_restart()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let chat = storage
            .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
            .await?;
        let skill_id = Uuid::now_v7();
        let first_version_id = Uuid::now_v7();
        let first_definition = skill_definition("Use version one.");
        storage
            .create_skill(&SkillRecord {
                id: skill_id,
                owner_id: owner,
                name: "Repository reviewer".to_owned(),
                description: "Review source changes".to_owned(),
                active_version_id: first_version_id,
                version: 1,
                definition: first_definition.clone(),
                bot_ids: Vec::new(),
                created_at_ms: 3,
                updated_at_ms: 3,
            })
            .await?;
        storage
            .set_skill_assignment(owner, skill_id, bot.id.0, true, 4)
            .await?;
        let applied_v1 = storage.resolve_applied_skills(owner, bot.id.0, &[]).await?;
        assert_eq!(applied_v1[0].version_id, first_version_id);
        let message_id = Uuid::now_v7();
        storage
            .append_user_message(
                owner,
                chat.id,
                message_id,
                "Review this",
                &[],
                None,
                Vec::new(),
                &applied_v1,
                &[],
                5,
            )
            .await?;
        let second_version_id = Uuid::now_v7();
        storage
            .update_skill(
                owner,
                skill_id,
                "Repository reviewer",
                "Review source changes",
                &skill_definition("Use version two."),
                second_version_id,
                6,
            )
            .await?;
        assert_eq!(
            storage.resolve_applied_skills(owner, bot.id.0, &[]).await?[0].version_id,
            second_version_id
        );
        drop(storage);

        let reopened = Storage::open(&database).await?;
        let historical = reopened.message_applied_skills(owner, message_id).await?;
        assert_eq!(historical.len(), 1);
        assert_eq!(historical[0].version_id, first_version_id);
        assert_eq!(historical[0].definition, first_definition);
        reopened.delete_skill(owner, skill_id, 7).await?;
        assert_eq!(
            reopened.message_applied_skills(owner, message_id).await?[0].version_id,
            first_version_id
        );
        assert!(
            reopened
                .resolve_applied_skills(owner, bot.id.0, &[])
                .await?
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn repository_and_chat_workspaces_are_owner_scoped_and_restart_durable()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Coding")?, 1)
            .await?;
        let chat = storage
            .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
            .await?;
        let workspace = RepositoryWorkspaceRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            name: "HomeBot".to_owned(),
            root_path: "/fixture/HomeBot".to_owned(),
            created_at_ms: 3,
            updated_at_ms: 3,
        };
        storage.create_repository_workspace(&workspace).await?;
        let association = ChatWorkspaceRecord {
            owner_id: owner,
            chat_id: chat.id,
            workspace_id: workspace.id,
            mode: WorkspaceMode::Isolated,
            worktree_path: Some("/managed/chat".to_owned()),
            branch_name: Some("homebot/chat".to_owned()),
            base_ref: Some("main".to_owned()),
            created_at_ms: 4,
            updated_at_ms: 4,
        };
        storage.attach_chat_workspace(&association).await?;
        assert!(
            storage
                .list_repository_workspaces(Uuid::now_v7())
                .await?
                .is_empty()
        );
        drop(storage);

        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened.list_repository_workspaces(owner).await?,
            vec![workspace]
        );
        assert_eq!(
            reopened.chat_workspace(owner, chat.id).await?,
            Some(association)
        );
        reopened.detach_chat_workspace(owner, chat.id).await?;
        assert!(reopened.chat_workspace(owner, chat.id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn typed_message_references_survive_renames_versions_and_restart()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let first = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let second = storage
            .create_bot(owner, Bot::create("Patch", "Engineer")?, 2)
            .await?;
        let third = storage
            .create_bot(owner, Bot::create("Orbit", "Design")?, 3)
            .await?;
        let direct = storage
            .create_direct_chat(owner, first.id.0, Uuid::now_v7(), 4)
            .await?;
        let group = storage
            .create_group_chat(
                owner,
                Uuid::now_v7(),
                "Launch team",
                &[first.id.0, second.id.0, third.id.0],
                first.id.0,
                12,
                3,
                5,
            )
            .await?;
        let routine_id = Uuid::now_v7();
        let routine_version = Uuid::now_v7();
        storage
            .create_routine(&RoutineRecord {
                id: routine_id,
                owner_id: owner,
                bot_id: first.id.0,
                name: "Launch review".to_owned(),
                description: String::new(),
                enabled: false,
                draft: true,
                active_version_id: routine_version,
                version: 1,
                definition: RoutineDefinition {
                    inputs: Vec::new(),
                    steps: vec![homebot_routines::RoutineStep::BotPrompt {
                        bot_id: first.id.0,
                        prompt_template: "Review".to_owned(),
                        requires_approval: false,
                    }],
                    expected_outputs: Vec::new(),
                },
                created_at_ms: 6,
                updated_at_ms: 6,
            })
            .await?;
        let plugin_id = Uuid::now_v7();
        storage
            .create_plugin(&PluginRecord {
                id: plugin_id,
                owner_id: owner,
                name: "Repository".to_owned(),
                description: String::new(),
                kind: "local_mcp".to_owned(),
                configuration: json!({"program":"fixture","arguments":[]}),
                enabled: false,
                connection_id: Uuid::now_v7(),
                transport: "stdio".to_owned(),
                status: "connect".to_owned(),
                auth_status: "not_required".to_owned(),
                error_message: None,
                updated_at_ms: 7,
            })
            .await?;
        let message_id = Uuid::now_v7();
        let requested_references = [
            (MessageReferenceKind::Bot, first.id.0),
            (MessageReferenceKind::Group, group.id),
            (MessageReferenceKind::Routine, routine_id),
            (MessageReferenceKind::Plugin, plugin_id),
        ];
        storage
            .append_user_message(
                owner,
                direct.id,
                message_id,
                "@Nova ask @Launch team to run @Launch review with @Repository",
                &[],
                None,
                Vec::new(),
                &[],
                &requested_references,
                8,
            )
            .await?;
        let expected = storage.message_references(owner, message_id).await?;
        assert_eq!(expected[2].target_version_id, Some(routine_version));
        sqlx::query("UPDATE bots SET name = 'Nova renamed' WHERE id = ?")
            .bind(first.id.0.to_string())
            .execute(storage.pool())
            .await?;
        sqlx::query("UPDATE chats SET title = 'Team renamed' WHERE id = ?")
            .bind(group.id.to_string())
            .execute(storage.pool())
            .await?;
        drop(storage);
        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened.message_references(owner, message_id).await?,
            expected
        );
        assert_eq!(expected[0].label_snapshot, "Nova");
        assert_eq!(expected[1].label_snapshot, "Launch team");
        Ok(())
    }

    #[tokio::test]
    async fn plugins_are_owner_scoped_and_discovery_cascades() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let owner = Uuid::now_v7();
        let plugin_id = Uuid::now_v7();
        let record = PluginRecord {
            id: plugin_id,
            owner_id: owner,
            name: "Fixture MCP".to_owned(),
            description: String::new(),
            kind: "local_mcp".to_owned(),
            configuration: json!({"program":"/fixture","arguments":[]}),
            enabled: false,
            connection_id: Uuid::now_v7(),
            transport: "stdio".to_owned(),
            status: "connect".to_owned(),
            auth_status: "not_required".to_owned(),
            error_message: None,
            updated_at_ms: 1,
        };
        storage.create_plugin(&record).await?;
        assert_eq!(storage.list_plugins(owner).await?, vec![record]);
        assert!(storage.list_plugins(Uuid::now_v7()).await?.is_empty());
        let tools = vec![PluginToolRecord {
            name: "read".to_owned(),
            title: None,
            description: None,
            input_schema: json!({"type":"object"}),
        }];
        let updated = storage
            .update_plugin_connection(
                owner,
                plugin_id,
                PluginConnectionUpdate {
                    enabled: true,
                    status: "connected",
                    auth_status: "connected",
                    error_message: None,
                    tools: &tools,
                    updated_at_ms: 2,
                },
            )
            .await?;
        assert!(updated.enabled);
        assert_eq!(storage.plugin_tools(owner, plugin_id).await?, tools);
        storage.delete_plugin(owner, plugin_id).await?;
        assert!(matches!(
            storage.plugin(owner, plugin_id).await,
            Err(StorageError::PluginNotFound)
        ));
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn routine_versions_recordings_and_runs_are_restart_durable() -> Result<(), StorageError>
    {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let definition = homebot_routines::RoutineDefinition {
            inputs: Vec::new(),
            steps: vec![homebot_routines::RoutineStep::BotPrompt {
                bot_id: bot.id.0,
                prompt_template: "Summarise".to_owned(),
                requires_approval: false,
            }],
            expected_outputs: Vec::new(),
        };
        let record = RoutineRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            bot_id: bot.id.0,
            name: "Daily brief".to_owned(),
            description: String::new(),
            enabled: false,
            draft: true,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: definition.clone(),
            created_at_ms: 2,
            updated_at_ms: 2,
        };
        storage.create_routine(&record).await?;
        let edited = storage
            .update_routine(
                owner,
                record.id,
                RoutineUpdate {
                    name: "Morning brief",
                    description: "Edited",
                    definition: &definition,
                    draft: false,
                    updated_at_ms: 3,
                },
            )
            .await?;
        assert_eq!(edited.version, 2);
        let versions: i64 =
            sqlx::query_scalar("SELECT count(*) FROM routine_versions WHERE routine_id = ?")
                .bind(record.id.to_string())
                .fetch_one(storage.pool())
                .await?;
        assert_eq!(versions, 2);
        let recording = RoutineRecordingRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            bot_id: bot.id.0,
            name: "Recorded".to_owned(),
            description: String::new(),
            actions: Vec::new(),
            created_at_ms: 4,
            updated_at_ms: 4,
        };
        storage.create_routine_recording(&recording).await?;
        let action = homebot_routines::RecordedAction {
            actor: homebot_routines::RecordedActor::User,
            step: definition.steps[0].clone(),
        };
        assert_eq!(
            storage
                .append_routine_recording_action(owner, recording.id, &action, 5)
                .await?
                .actions,
            vec![action.clone()]
        );
        let run = RoutineRunRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            routine_id: record.id,
            routine_version_id: edited.active_version_id,
            bot_id: bot.id.0,
            status: "dry_run_succeeded".to_owned(),
            trigger: json!({"kind": "manual"}),
            dry_run: true,
            inputs: json!({}),
            results: Some(vec![]),
            error_message: None,
            attempt_count: 1,
            scheduled_for_ms: None,
            started_at_ms: 6,
            finished_at_ms: Some(7),
        };
        storage.create_routine_run(&run).await?;
        drop(storage);
        let reopened = Storage::open(&database).await?;
        assert_eq!(reopened.routine(owner, record.id).await?.version, 2);
        assert_eq!(
            reopened
                .routine_recording(owner, recording.id)
                .await?
                .actions,
            vec![action]
        );
        assert_eq!(reopened.routine_runs(owner, record.id).await?, vec![run]);
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn scheduler_jobs_deduplicate_survive_restart_and_enforce_overlap()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let routine = RoutineRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            bot_id: bot.id.0,
            name: "Scheduled brief".to_owned(),
            description: String::new(),
            enabled: true,
            draft: false,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: homebot_routines::RoutineDefinition {
                inputs: Vec::new(),
                steps: vec![homebot_routines::RoutineStep::BotPrompt {
                    bot_id: bot.id.0,
                    prompt_template: "Summarise".to_owned(),
                    requires_approval: false,
                }],
                expected_outputs: Vec::new(),
            },
            created_at_ms: 2,
            updated_at_ms: 2,
        };
        storage.create_routine(&routine).await?;
        let trigger = RoutineTriggerRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            routine_id: routine.id,
            definition: homebot_routines::RoutineTriggerDefinition {
                source: homebot_routines::RoutineTriggerSource::Schedule {
                    schedule: homebot_routines::RoutineSchedule::Interval {
                        anchor_unix_ms: 10,
                        every_seconds: 60,
                    },
                },
                missed_run_policy: homebot_routines::MissedRunPolicy::RunOnce,
                overlap_policy: homebot_routines::OverlapPolicy::Skip,
                retry_policy: homebot_routines::RetryPolicy {
                    maximum_attempts: 3,
                    initial_backoff_seconds: 1,
                    maximum_backoff_seconds: 4,
                },
                catch_up_limit: 1,
            },
            enabled: true,
            last_evaluated_at_ms: None,
            next_fire_at_ms: Some(10),
            last_event_sequence: 0,
            created_at_ms: 3,
            updated_at_ms: 3,
        };
        storage.create_routine_trigger(&trigger).await?;
        let job = RoutineJobRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            trigger_id: trigger.id,
            routine_id: routine.id,
            routine_version_id: routine.active_version_id,
            delivery_key: "schedule:10".to_owned(),
            trigger: json!({"kind":"schedule","scheduled_for_unix_ms":10}),
            inputs: json!({}),
            status: "queued".to_owned(),
            attempt_count: 0,
            scheduled_for_ms: 10,
            next_attempt_at_ms: 10,
            cancel_requested: false,
            error_message: None,
            created_at_ms: 3,
            started_at_ms: None,
            finished_at_ms: None,
        };
        assert_eq!(
            storage.enqueue_routine_job(&job).await?,
            RoutineJobClaim::Claimed
        );
        assert_eq!(
            storage.enqueue_routine_job(&job).await?,
            RoutineJobClaim::Replayed
        );
        drop(storage);

        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened.routine_triggers(owner, Some(routine.id)).await?,
            vec![trigger]
        );
        let claimed = reopened
            .claim_next_routine_job(owner, 10)
            .await?
            .ok_or(StorageError::RoutineJobNotFound)?;
        assert_eq!(claimed.id, job.id);
        assert_eq!(claimed.attempt_count, 1);

        let overlapping = RoutineJobRecord {
            id: Uuid::now_v7(),
            delivery_key: "schedule:60010".to_owned(),
            scheduled_for_ms: 60_010,
            next_attempt_at_ms: 60_010,
            created_at_ms: 4,
            ..job.clone()
        };
        assert_eq!(
            reopened.enqueue_routine_job(&overlapping).await?,
            RoutineJobClaim::Claimed
        );
        assert!(
            reopened
                .claim_next_routine_job(owner, 60_010)
                .await?
                .is_none()
        );
        assert_eq!(
            reopened
                .routine_jobs(owner, routine.id)
                .await?
                .into_iter()
                .find(|candidate| candidate.id == overlapping.id)
                .ok_or(StorageError::RoutineJobNotFound)?
                .status,
            "skipped"
        );
        reopened
            .finish_routine_job(owner, job.id, "succeeded", None, 60_011)
            .await?;
        assert_eq!(
            reopened.routine_jobs(owner, routine.id).await?[1].status,
            "succeeded"
        );

        let retrying = RoutineJobRecord {
            id: Uuid::now_v7(),
            delivery_key: "retry".to_owned(),
            scheduled_for_ms: 70_000,
            next_attempt_at_ms: 70_000,
            created_at_ms: 5,
            ..job.clone()
        };
        assert_eq!(
            reopened.enqueue_routine_job(&retrying).await?,
            RoutineJobClaim::Claimed
        );
        assert_eq!(
            reopened
                .claim_next_routine_job(owner, 70_000)
                .await?
                .ok_or(StorageError::RoutineJobNotFound)?
                .attempt_count,
            1
        );
        assert!(
            reopened
                .retry_or_fail_routine_job(owner, retrying.id, "temporary", 70_000)
                .await?
        );
        assert!(
            reopened
                .claim_next_routine_job(owner, 70_999)
                .await?
                .is_none()
        );
        assert_eq!(
            reopened
                .claim_next_routine_job(owner, 71_000)
                .await?
                .ok_or(StorageError::RoutineJobNotFound)?
                .attempt_count,
            2
        );
        assert!(
            reopened
                .retry_or_fail_routine_job(owner, retrying.id, "temporary", 71_000)
                .await?
        );
        assert_eq!(
            reopened
                .claim_next_routine_job(owner, 73_000)
                .await?
                .ok_or(StorageError::RoutineJobNotFound)?
                .attempt_count,
            3
        );
        assert!(
            !reopened
                .retry_or_fail_routine_job(owner, retrying.id, "terminal", 73_000)
                .await?
        );
        let cancelled = RoutineJobRecord {
            id: Uuid::now_v7(),
            delivery_key: "cancelled".to_owned(),
            scheduled_for_ms: 80_000,
            next_attempt_at_ms: 80_000,
            created_at_ms: 6,
            ..job
        };
        reopened.enqueue_routine_job(&cancelled).await?;
        reopened
            .cancel_routine_job(owner, cancelled.id, 79_000)
            .await?;
        let jobs = reopened.routine_jobs(owner, routine.id).await?;
        assert_eq!(
            jobs.iter()
                .find(|candidate| candidate.id == retrying.id)
                .ok_or(StorageError::RoutineJobNotFound)?
                .status,
            "failed"
        );
        let cancelled = jobs
            .iter()
            .find(|candidate| candidate.id == cancelled.id)
            .ok_or(StorageError::RoutineJobNotFound)?;
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.cancel_requested);
        let interrupted = RoutineJobRecord {
            id: Uuid::now_v7(),
            delivery_key: "interrupted".to_owned(),
            scheduled_for_ms: 90_000,
            next_attempt_at_ms: 90_000,
            created_at_ms: 7,
            ..retrying
        };
        reopened.enqueue_routine_job(&interrupted).await?;
        assert_eq!(
            reopened
                .claim_next_routine_job(owner, 90_000)
                .await?
                .ok_or(StorageError::RoutineJobNotFound)?
                .attempt_count,
            1
        );
        assert_eq!(
            reopened
                .recover_interrupted_routine_jobs(owner, 90_001)
                .await?,
            1
        );
        assert_eq!(
            reopened
                .claim_next_routine_job(owner, 90_001)
                .await?
                .ok_or(StorageError::RoutineJobNotFound)?
                .attempt_count,
            2
        );
        reopened
            .finish_routine_job(owner, interrupted.id, "succeeded", None, 90_002)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn secret_references_are_owner_scoped_and_never_store_values() -> Result<(), StorageError>
    {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let other_owner = Uuid::now_v7();
        let record = SecretReferenceRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            locator: format!("homebot:{}", Uuid::now_v7()),
            label: "OpenAI work".to_owned(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        storage.create_secret_reference(&record).await?;
        assert_eq!(
            storage.list_secret_references(owner).await?,
            vec![record.clone()]
        );
        assert!(
            storage
                .list_secret_references(other_owner)
                .await?
                .is_empty()
        );
        assert!(matches!(
            storage
                .create_secret_reference(&SecretReferenceRecord {
                    id: Uuid::now_v7(),
                    ..record.clone()
                })
                .await,
            Err(StorageError::DuplicateSecretLabel)
        ));
        let updated = storage
            .update_secret_reference(owner, record.id, "OpenAI personal", 2)
            .await?;
        assert_eq!(updated.label, "OpenAI personal");

        let bytes = std::fs::read(&database)?;
        assert!(
            !bytes
                .windows(b"canary-secret-value".len())
                .any(|window| window == b"canary-secret-value")
        );

        storage.delete_secret_reference(owner, record.id).await?;
        assert!(matches!(
            storage.secret_reference(owner, record.id).await,
            Err(StorageError::SecretNotFound)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn version_seven_secret_metadata_upgrades_without_inventing_values()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v7.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO secret_references (id, provider, locator, label, created_at_ms)
             VALUES (?, 'os_keyring', ?, 'Migrated key', 7)",
        )
        .bind(id.to_string())
        .bind(format!("homebot:{id}"))
        .execute(&pool)
        .await?;
        sqlx::raw_sql(include_str!("../migrations/0008_secret_references.sql"))
            .execute(&pool)
            .await?;
        let storage = Storage { pool };
        let migrated = storage.secret_reference(Uuid::nil(), id).await?;
        assert_eq!(migrated.label, "Migrated key");
        assert_eq!(migrated.updated_at_ms, 0);
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('secret_references')")
                .fetch_all(storage.pool())
                .await?;
        assert!(!columns.iter().any(|column| column == "value"));
        Ok(())
    }

    #[tokio::test]
    async fn version_nine_routine_upgrade_preserves_legacy_orphans_safely()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v9.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        let routine_id = Uuid::now_v7();
        let version_id = Uuid::now_v7();
        sqlx::query("INSERT INTO routines (id, name, active_version_id, enabled, created_at_ms, updated_at_ms) VALUES (?, 'Legacy', ?, 1, 1, 1)")
            .bind(routine_id.to_string()).bind(version_id.to_string()).execute(&pool).await?;
        sqlx::query("INSERT INTO routine_versions (id, routine_id, version, definition_json, created_at_ms) VALUES (?, ?, 1, '{}', 1)")
            .bind(version_id.to_string()).bind(routine_id.to_string()).execute(&pool).await?;
        sqlx::raw_sql(include_str!("../migrations/0010_routines.sql"))
            .execute(&pool)
            .await?;
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM routines WHERE id = ?")
            .bind(routine_id.to_string())
            .fetch_one(&pool)
            .await?;
        assert_eq!(retained, 1);
        let storage = Storage { pool };
        assert!(storage.list_routines(Uuid::nil()).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn version_eleven_skill_upgrade_preserves_legacy_versions_and_duplicate_names()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v11.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
            include_str!("../migrations/0010_routines.sql"),
            include_str!("../migrations/0011_routine_scheduler.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        let definition = serde_json::to_string(&skill_definition("Legacy instructions"))
            .map_err(|error| StorageError::Serialization(error.to_string()))?;
        for _ in 0..2 {
            let skill_id = Uuid::now_v7();
            let version_id = Uuid::now_v7();
            sqlx::query("INSERT INTO skills (id, name, active_version_id, created_at_ms) VALUES (?, 'Legacy', ?, 1)")
                .bind(skill_id.to_string()).bind(version_id.to_string()).execute(&pool).await?;
            sqlx::query("INSERT INTO skill_versions (id, skill_id, version, definition_json, created_at_ms) VALUES (?, ?, 1, ?, 1)")
                .bind(version_id.to_string()).bind(skill_id.to_string()).bind(&definition).execute(&pool).await?;
        }
        sqlx::raw_sql(include_str!("../migrations/0012_skills.sql"))
            .execute(&pool)
            .await?;
        let storage = Storage { pool };
        let skills = storage.list_skills(Uuid::nil()).await?;
        assert_eq!(skills.len(), 2);
        assert!(skills.iter().all(|skill| skill.version == 1));
        assert!(
            skills
                .iter()
                .all(|skill| skill.definition.instructions == "Legacy instructions")
        );
        let normalized: Vec<String> =
            sqlx::query_scalar("SELECT name_normalized FROM skills ORDER BY id")
                .fetch_all(storage.pool())
                .await?;
        assert_ne!(normalized[0], normalized[1]);
        Ok(())
    }

    #[tokio::test]
    async fn version_twelve_workspace_upgrade_preserves_existing_chats_and_accepts_associations()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v12.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
            include_str!("../migrations/0010_routines.sql"),
            include_str!("../migrations/0011_routine_scheduler.sql"),
            include_str!("../migrations/0012_skills.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        sqlx::raw_sql(include_str!("../migrations/0018_bot_parity.sql"))
            .execute(&pool)
            .await?;
        let legacy = Storage { pool: pool.clone() };
        let owner = Uuid::now_v7();
        let bot = legacy
            .create_bot(owner, Bot::create("Nova", "Coding")?, 1)
            .await?;
        let chat = legacy
            .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0013_workspaces.sql"))
            .execute(&pool)
            .await?;
        let upgraded = Storage { pool };
        let workspace = RepositoryWorkspaceRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            name: "Legacy project".to_owned(),
            root_path: "/fixture/legacy".to_owned(),
            created_at_ms: 3,
            updated_at_ms: 3,
        };
        upgraded.create_repository_workspace(&workspace).await?;
        let association = ChatWorkspaceRecord {
            owner_id: owner,
            chat_id: chat.id,
            workspace_id: workspace.id,
            mode: WorkspaceMode::Primary,
            worktree_path: None,
            branch_name: Some("main".to_owned()),
            base_ref: None,
            created_at_ms: 4,
            updated_at_ms: 4,
        };
        upgraded.attach_chat_workspace(&association).await?;
        assert_eq!(
            upgraded.chat_workspace(owner, chat.id).await?,
            Some(association)
        );
        assert_eq!(
            upgraded.list_repository_workspaces(owner).await?,
            vec![workspace]
        );
        assert_eq!(upgraded.list_direct_chats(owner).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn version_thirteen_checkpoint_upgrade_preserves_workspace_associations()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v13.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
            include_str!("../migrations/0010_routines.sql"),
            include_str!("../migrations/0011_routine_scheduler.sql"),
            include_str!("../migrations/0012_skills.sql"),
            include_str!("../migrations/0013_workspaces.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        sqlx::raw_sql(include_str!("../migrations/0018_bot_parity.sql"))
            .execute(&pool)
            .await?;
        let storage = Storage { pool: pool.clone() };
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Patch", "Coding")?, 1)
            .await?;
        let chat = storage
            .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
            .await?;
        let workspace = RepositoryWorkspaceRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            name: "Existing workspace".to_owned(),
            root_path: "/fixture/repository".to_owned(),
            created_at_ms: 3,
            updated_at_ms: 3,
        };
        storage.create_repository_workspace(&workspace).await?;
        storage
            .attach_chat_workspace(&ChatWorkspaceRecord {
                owner_id: owner,
                chat_id: chat.id,
                workspace_id: workspace.id,
                mode: WorkspaceMode::Primary,
                worktree_path: None,
                branch_name: Some("main".to_owned()),
                base_ref: None,
                created_at_ms: 4,
                updated_at_ms: 4,
            })
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0014_checkpoints.sql"))
            .execute(&pool)
            .await?;
        let checkpoint = TurnCheckpointRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            chat_id: chat.id,
            workspace_id: workspace.id,
            message_id: None,
            phase: CheckpointPhase::BeforeTurn,
            git_ref: format!("refs/homebot/checkpoints/{}/fixture", chat.id.simple()),
            commit_oid: "0123456789012345678901234567890123456789".to_owned(),
            provider_profile_id: None,
            provider_conversation_id: None,
            created_at_ms: 5,
        };
        storage.create_turn_checkpoint(&checkpoint).await?;
        assert_eq!(
            storage.turn_checkpoints(owner, chat.id).await?,
            vec![checkpoint]
        );
        assert_eq!(
            storage
                .chat_workspace(owner, chat.id)
                .await?
                .ok_or(StorageError::WorkspaceNotFound)?
                .workspace_id,
            workspace.id
        );
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn version_fourteen_vcs_and_context_upgrades_preserve_chats_and_exact_results()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v14.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
            include_str!("../migrations/0010_routines.sql"),
            include_str!("../migrations/0011_routine_scheduler.sql"),
            include_str!("../migrations/0012_skills.sql"),
            include_str!("../migrations/0013_workspaces.sql"),
            include_str!("../migrations/0014_checkpoints.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        sqlx::raw_sql(include_str!("../migrations/0018_bot_parity.sql"))
            .execute(&pool)
            .await?;
        let storage = Storage { pool: pool.clone() };
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Patch", "Coding")?, 1)
            .await?;
        let chat = storage
            .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0015_vcs_operations.sql"))
            .execute(&pool)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0016_working_context.sql"))
            .execute(&pool)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0017_device_pairing.sql"))
            .execute(&pool)
            .await?;
        sqlx::raw_sql(include_str!("../migrations/0024_pairing_provenance.sql"))
            .execute(&pool)
            .await?;
        let result = VcsOperationResultRecord {
            idempotency_key: Uuid::now_v7(),
            owner_id: owner,
            chat_id: chat.id,
            action: "commit".to_owned(),
            response: json!({"commit_oid":"0123456789012345678901234567890123456789"}),
            created_at_ms: 3,
        };
        storage.record_vcs_operation_result(&result).await?;
        assert_eq!(
            storage
                .vcs_operation_result(owner, chat.id, result.idempotency_key, "commit")
                .await?,
            Some(result)
        );
        assert_eq!(storage.list_direct_chats(owner).await?.len(), 1);
        let profile_id = Uuid::now_v7();
        sqlx::query("INSERT INTO provider_profiles (id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms) VALUES (?, 'fixture', 'Fixture', '{}', 4, 4)")
            .bind(profile_id.to_string()).execute(&pool).await?;
        let context = storage
            .working_context(owner, chat.id, profile_id, 5)
            .await?;
        assert_eq!(context.interaction_mode, "default");
        let context = storage
            .set_working_context_mode(owner, chat.id, "plan", 6)
            .await?;
        assert_eq!(context.interaction_mode, "plan");
        assert_eq!(
            storage
                .begin_working_context_compaction(owner, chat.id, 7)
                .await?
                .compaction_status,
            "running"
        );
        assert!(matches!(
            storage
                .begin_working_context_compaction(owner, chat.id, 8)
                .await,
            Err(StorageError::WorkingContextBusy)
        ));
        storage
            .set_working_context_compaction(owner, chat.id, "completed", true, true, None, 9)
            .await?;
        let pairing_id = Uuid::now_v7();
        storage
            .create_pairing_credential(
                owner,
                pairing_id,
                &[1; 32],
                &[8; 32],
                "http://127.0.0.1:7123",
                "http://127.0.0.1:7123",
                "loopback",
                10,
                1_000,
            )
            .await?;
        let device = storage
            .exchange_pairing_credential(
                owner,
                &[1; 32],
                Some(&[8; 32]),
                None,
                &[9; 32],
                Uuid::now_v7(),
                "Upgrade fixture",
                &[2; 32],
                11,
                60_000,
            )
            .await?;
        assert_eq!(storage.device_sessions(owner).await?, vec![device]);
        assert_eq!(storage.list_direct_chats(owner).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn version_ten_upgrade_preserves_routines_and_initializes_scheduler_state()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot-v10.db");
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_event_retention.sql"),
            include_str!("../migrations/0003_attachments.sql"),
            include_str!("../migrations/0004_bot_lifecycle.sql"),
            include_str!("../migrations/0005_direct_chat.sql"),
            include_str!("../migrations/0006_group_coordination.sql"),
            include_str!("../migrations/0007_activity_artifacts.sql"),
            include_str!("../migrations/0008_secret_references.sql"),
            include_str!("../migrations/0009_plugins.sql"),
            include_str!("../migrations/0010_routines.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await?;
        }
        let storage = Storage { pool: pool.clone() };
        let owner = Uuid::now_v7();
        let bot = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let routine = RoutineRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            bot_id: bot.id.0,
            name: "Existing routine".to_owned(),
            description: String::new(),
            enabled: true,
            draft: false,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: homebot_routines::RoutineDefinition {
                inputs: Vec::new(),
                steps: vec![homebot_routines::RoutineStep::BotPrompt {
                    bot_id: bot.id.0,
                    prompt_template: "Continue".to_owned(),
                    requires_approval: false,
                }],
                expected_outputs: Vec::new(),
            },
            created_at_ms: 2,
            updated_at_ms: 2,
        };
        storage.create_routine(&routine).await?;
        let trigger_id = Uuid::now_v7();
        let definition = homebot_routines::RoutineTriggerDefinition {
            source: homebot_routines::RoutineTriggerSource::Webhook {
                slug: "legacy".to_owned(),
            },
            missed_run_policy: homebot_routines::MissedRunPolicy::RunOnce,
            overlap_policy: homebot_routines::OverlapPolicy::Queue,
            retry_policy: homebot_routines::RetryPolicy::default(),
            catch_up_limit: 1,
        };
        sqlx::query("INSERT INTO routine_triggers (id, routine_id, kind, configuration_json, enabled) VALUES (?, ?, 'webhook', ?, 1)")
            .bind(trigger_id.to_string()).bind(routine.id.to_string()).bind(serde_json::to_string(&definition).map_err(|error| StorageError::Serialization(error.to_string()))?)
            .execute(&pool).await?;
        let run_id = Uuid::now_v7();
        sqlx::query("INSERT INTO routine_runs (id, owner_id, routine_id, routine_version_id, status, trigger_json, dry_run, input_json, started_at_ms) VALUES (?, ?, ?, ?, 'succeeded', '{\"kind\":\"manual\"}', 0, '{}', 3)")
            .bind(run_id.to_string()).bind(owner.to_string()).bind(routine.id.to_string()).bind(routine.active_version_id.to_string())
            .execute(&pool).await?;
        sqlx::raw_sql(include_str!("../migrations/0011_routine_scheduler.sql"))
            .execute(&pool)
            .await?;
        let storage = Storage { pool };
        let trigger = storage.routine_trigger(Uuid::nil(), trigger_id).await?;
        assert_eq!(trigger.definition, definition);
        assert_eq!(trigger.last_event_sequence, 0);
        let runs = storage.routine_runs(owner, routine.id).await?;
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].bot_id, bot.id.0);
        Ok(())
    }

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
        let pinned = reopened.set_bot_pinned(owner, nova.id.0, true, 17).await?;
        assert_eq!(pinned.pinned_at_ms, Some(17));
        let hidden = reopened.set_bot_hidden(owner, nova.id.0, true, 18).await?;
        assert_eq!(hidden.hidden_at_ms, Some(18));
        reopened.pool.close().await;
        let reopened = Storage::open(&database).await?;
        let durable = reopened.get_bot(owner, nova.id.0).await?;
        assert_eq!(
            (durable.pinned_at_ms, durable.hidden_at_ms),
            (Some(17), Some(18))
        );
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn bot_duplicate_copies_configuration_without_history_and_delete_cascades()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let source = storage
            .create_bot(owner, Bot::create("Nova", "Research")?, 1)
            .await?;
        let chat = storage
            .create_direct_chat(owner, source.id.0, Uuid::now_v7(), 2)
            .await?;
        storage
            .append_user_message(
                owner,
                chat.id,
                Uuid::now_v7(),
                "private history",
                &[],
                None,
                Vec::new(),
                &[],
                &[],
                3,
            )
            .await?;

        let skill_id = Uuid::now_v7();
        storage
            .create_skill(&SkillRecord {
                id: skill_id,
                owner_id: owner,
                name: "Reviewer".to_owned(),
                description: String::new(),
                active_version_id: Uuid::now_v7(),
                version: 1,
                definition: skill_definition("Review carefully."),
                bot_ids: Vec::new(),
                created_at_ms: 4,
                updated_at_ms: 4,
            })
            .await?;
        storage
            .set_skill_assignment(owner, skill_id, source.id.0, true, 5)
            .await?;
        let routine = RoutineRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            bot_id: source.id.0,
            name: "Daily brief".to_owned(),
            description: "Configured".to_owned(),
            enabled: true,
            draft: false,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: RoutineDefinition {
                inputs: Vec::new(),
                steps: Vec::new(),
                expected_outputs: Vec::new(),
            },
            created_at_ms: 6,
            updated_at_ms: 6,
        };
        storage.create_routine(&routine).await?;
        let trigger = RoutineTriggerRecord {
            id: Uuid::now_v7(),
            owner_id: owner,
            routine_id: routine.id,
            definition: RoutineTriggerDefinition {
                source: homebot_routines::RoutineTriggerSource::Webhook {
                    slug: "daily".to_owned(),
                },
                missed_run_policy: homebot_routines::MissedRunPolicy::RunOnce,
                overlap_policy: homebot_routines::OverlapPolicy::Queue,
                retry_policy: homebot_routines::RetryPolicy::default(),
                catch_up_limit: 1,
            },
            enabled: true,
            last_evaluated_at_ms: Some(7),
            next_fire_at_ms: Some(8),
            last_event_sequence: 9,
            created_at_ms: 7,
            updated_at_ms: 7,
        };
        storage.create_routine_trigger(&trigger).await?;

        let duplicate_id = Uuid::now_v7();
        let duplicate = storage
            .duplicate_bot_configuration(owner, source.id.0, duplicate_id, 10)
            .await?;
        assert_eq!(duplicate.name, "Nova copy");
        assert_eq!(
            storage.skill_bot_ids_for_bot(owner, duplicate_id).await?,
            vec![skill_id]
        );
        assert_eq!(
            storage.list_direct_chats(owner).await?.len(),
            1,
            "chat history is not copied"
        );
        let copied_routine = storage
            .list_routines(owner)
            .await?
            .into_iter()
            .find(|item| item.bot_id == duplicate_id)
            .ok_or(StorageError::RoutineNotFound)?;
        assert_eq!(
            (
                copied_routine.enabled,
                copied_routine.draft,
                copied_routine.version
            ),
            (true, false, 1)
        );
        let copied_triggers = storage
            .routine_triggers(owner, Some(copied_routine.id))
            .await?;
        assert_eq!(copied_triggers.len(), 1);
        assert_eq!(
            (
                copied_triggers[0].last_evaluated_at_ms,
                copied_triggers[0].last_event_sequence
            ),
            (None, 0)
        );
        assert!(
            storage
                .routine_runs(owner, copied_routine.id)
                .await?
                .is_empty()
        );

        storage.pool.close().await;
        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened.get_bot(owner, duplicate_id).await?.name,
            "Nova copy"
        );
        reopened.delete_bot(owner, source.id.0).await?;
        assert!(matches!(
            reopened.get_bot(owner, source.id.0).await,
            Err(StorageError::BotNotFound)
        ));
        assert!(reopened.list_direct_chats(owner).await?.is_empty());
        assert_eq!(
            reopened.list_routines(owner).await?.len(),
            1,
            "duplicate configuration survives source deletion"
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
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
                &[],
                &[],
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
                &[],
                &[],
                5,
            )
            .await?;
        assert_eq!(second.reply_to_message_id, Some(first_id));
        assert_eq!(
            storage
                .set_message_reaction(owner, first_id, "👍", true, 6)
                .await?,
            vec![MessageReactionRecord {
                emoji: "👍".to_owned(),
                count: 1,
                reacted_by_user: true
            }]
        );
        storage.set_chat_running(owner, chat_id, true, 6).await?;
        let queued_id = Uuid::now_v7();
        let queued = storage
            .enqueue_prompt(
                owner,
                chat_id,
                queued_id,
                QueuedPromptInput {
                    content: "Next",
                    attachment_ids: &[],
                    applied_skills: &[],
                    references: &[(MessageReferenceKind::Bot, bot.id.0)],
                    kind: QueuedPromptKind::FollowUp,
                },
                7,
            )
            .await?;
        assert_eq!(queued.position, 0);
        assert_eq!(storage.queued_prompts(owner, chat_id).await?.len(), 1);
        let activity = ExecutionActivity {
            id: Uuid::now_v7(),
            chat_id,
            message_id: Some(first_id),
            kind: "search".to_owned(),
            title: "Searching sources".to_owned(),
            detail: "Local index".to_owned(),
            presentation_json: serde_json::json!({
                "risk": "low",
                "detail": {"kind": "generic", "summary": "Local index"},
                "copy_text": null,
                "open_artifact_id": null
            }),
            status: ActivityStatus::Running,
            requires_attention: false,
            started_at_ms: 8,
            finished_at_ms: None,
        };
        storage.upsert_activity(owner, &activity).await?;
        assert_eq!(
            storage.chat_activities(owner, chat_id).await?,
            vec![activity]
        );
        let approval = ChatApproval {
            id: Uuid::now_v7(),
            owner_id: owner,
            chat_id,
            message_id: Some(first_id),
            operation_id: Uuid::now_v7(),
            capability: "filesystem.write".to_owned(),
            title: "Allow file change?".to_owned(),
            detail: "Nova wants to update README.md".to_owned(),
            status: ApprovalStatus::Pending,
            created_at_ms: 9,
            decided_at_ms: None,
        };
        storage.create_chat_approval(&approval).await?;
        let decided = storage
            .decide_chat_approval(owner, approval.id, false, 10)
            .await?;
        assert_eq!(decided.status, ApprovalStatus::Denied);
        assert!(matches!(
            storage
                .decide_chat_approval(owner, approval.id, true, 11)
                .await,
            Err(StorageError::ApprovalNotFound)
        ));
        assert!(matches!(
            storage.get_direct_chat(other_owner, chat_id).await,
            Err(StorageError::ChatNotFound)
        ));
        storage.pool.close().await;

        let reopened = Storage::open(&database).await?;
        let messages = reopened.chat_messages(owner, chat_id).await?;
        assert_eq!(messages.len(), 2);
        assert_eq!(reopened.queued_prompts(owner, chat_id).await?.len(), 1);
        assert_eq!(reopened.chat_activities(owner, chat_id).await?.len(), 1);
        assert_eq!(reopened.chat_approvals(owner, chat_id).await?.len(), 1);
        assert_eq!(reopened.message_reactions(owner, first_id).await?.len(), 1);
        reopened.set_chat_running(owner, chat_id, false, 12).await?;
        let Some(promoted) = reopened
            .promote_next_queued_prompt(owner, chat_id, 13)
            .await?
        else {
            return Err(StorageError::Integrity(
                "queued message did not promote after restart".to_owned(),
            ));
        };
        assert_eq!(promoted.message.id, queued_id);
        assert_eq!(
            reopened.message_references(owner, queued_id).await?,
            vec![MessageReferenceRecord {
                kind: MessageReferenceKind::Bot,
                target_id: bot.id.0,
                target_version_id: None,
                label_snapshot: "Nova".to_owned(),
            }]
        );
        assert!(
            reopened
                .set_message_reaction(owner, first_id, "👍", false, 14)
                .await?
                .is_empty()
        );
        assert!(matches!(
            &messages[0].parts[0],
            MessagePart::Text { text, .. } if text == "Hello"
        ));
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn three_bot_group_handoff_context_limits_and_restart_are_durable()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        let mut bots = Vec::new();
        for (index, name) in ["Nova", "Patch", "Scout"].into_iter().enumerate() {
            bots.push(
                storage
                    .create_bot(
                        owner,
                        Bot::create(name, "Group member")?,
                        i64::try_from(index).unwrap_or(i64::MAX),
                    )
                    .await?,
            );
        }
        let bot_ids = bots.iter().map(|bot| bot.id.0).collect::<Vec<_>>();
        assert!(matches!(
            storage
                .create_group_chat(
                    owner,
                    Uuid::now_v7(),
                    "Too small",
                    &bot_ids[..1],
                    bot_ids[0],
                    2,
                    2,
                    3,
                )
                .await,
            Err(StorageError::InvalidGroupParticipants)
        ));
        let chat_id = Uuid::now_v7();
        let group = storage
            .create_group_chat(
                owner,
                chat_id,
                "Release team",
                &bot_ids,
                bot_ids[0],
                2,
                2,
                4,
            )
            .await?;
        assert_eq!(group.ownership_bot_id, bot_ids[0]);
        assert_eq!(storage.group_participants(owner, chat_id).await?.len(), 3);

        storage
            .set_group_bot_status(
                owner,
                chat_id,
                bot_ids[0],
                GroupBotStatus::Running,
                Some(Uuid::now_v7()),
                5,
            )
            .await?;
        storage
            .set_group_bot_status(
                owner,
                chat_id,
                bot_ids[1],
                GroupBotStatus::Running,
                Some(Uuid::now_v7()),
                5,
            )
            .await?;
        assert!(matches!(
            storage
                .set_group_bot_status(
                    owner,
                    chat_id,
                    bot_ids[2],
                    GroupBotStatus::Running,
                    Some(Uuid::now_v7()),
                    5,
                )
                .await,
            Err(StorageError::CoordinationLimitReached)
        ));
        storage
            .record_group_coordination_turn(owner, chat_id, 6)
            .await?;
        storage
            .record_group_coordination_turn(owner, chat_id, 7)
            .await?;
        assert!(matches!(
            storage
                .record_group_coordination_turn(owner, chat_id, 8)
                .await,
            Err(StorageError::CoordinationLimitReached)
        ));

        let first_id = Uuid::now_v7();
        storage
            .append_group_bot_message(
                owner,
                chat_id,
                first_id,
                bot_ids[0],
                "Patch, inspect the tests.",
                &[bot_ids[1]],
                &[],
                9,
            )
            .await?;
        let second_id = Uuid::now_v7();
        let second = storage
            .append_group_bot_message(
                owner,
                chat_id,
                second_id,
                bot_ids[1],
                "Scout, verify this finding.",
                &[bot_ids[2]],
                &[first_id],
                10,
            )
            .await?;
        assert_eq!(second.shared_context_message_ids, vec![first_id]);
        storage
            .handoff_group_ownership(
                owner,
                chat_id,
                Uuid::now_v7(),
                bot_ids[0],
                bot_ids[2],
                Some(second_id),
                "Scout owns verification",
                11,
            )
            .await?;
        storage.pool().close().await;

        let reopened = Storage::open(&database).await?;
        assert_eq!(
            reopened
                .get_group_chat(owner, chat_id)
                .await?
                .ownership_bot_id,
            bot_ids[2]
        );
        let participants = reopened.group_participants(owner, chat_id).await?;
        assert_eq!(
            participants
                .iter()
                .find(|participant| participant.role == GroupParticipantRole::Owner)
                .map(|participant| participant.bot_id),
            Some(bot_ids[2])
        );
        assert_eq!(
            reopened.chat_messages(owner, chat_id).await?[1].shared_context_message_ids,
            vec![first_id]
        );
        assert!(
            reopened
                .stop_group_chat(owner, chat_id, 12)
                .await?
                .stop_requested
        );
        assert!(matches!(
            reopened
                .record_group_coordination_turn(owner, chat_id, 13)
                .await,
            Err(StorageError::CoordinationLimitReached)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn group_membership_concurrency_preserves_two_to_six_bounds() -> Result<(), StorageError>
    {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let owner = Uuid::now_v7();
        let mut bot_ids = Vec::new();
        for index in 0..7 {
            bot_ids.push(
                storage
                    .create_bot(
                        owner,
                        Bot::create(format!("Bot {index}"), "Member")?,
                        i64::from(index),
                    )
                    .await?
                    .id
                    .0,
            );
        }
        let chat_id = Uuid::now_v7();
        storage
            .create_group_chat(owner, chat_id, "Pair", &bot_ids[..2], bot_ids[0], 8, 2, 1)
            .await?;
        storage
            .rename_group_chat(owner, chat_id, "Renamed pair", 2)
            .await?;
        let additions = tokio::join!(
            storage.add_group_participant(owner, chat_id, bot_ids[2], 3),
            storage.add_group_participant(owner, chat_id, bot_ids[3], 4),
            storage.add_group_participant(owner, chat_id, bot_ids[4], 5),
            storage.add_group_participant(owner, chat_id, bot_ids[5], 6),
            storage.add_group_participant(owner, chat_id, bot_ids[6], 7),
        );
        assert_eq!(
            [
                additions.0,
                additions.1,
                additions.2,
                additions.3,
                additions.4
            ]
            .iter()
            .filter(|result| result.is_ok())
            .count(),
            4
        );
        assert_eq!(storage.group_participants(owner, chat_id).await?.len(), 6);

        let members = storage
            .group_participants(owner, chat_id)
            .await?
            .into_iter()
            .filter(|participant| participant.bot_id != bot_ids[0])
            .map(|participant| participant.bot_id)
            .collect::<Vec<_>>();
        for bot_id in &members[..3] {
            storage
                .remove_group_participant(owner, chat_id, *bot_id)
                .await?;
        }
        let (left, right) = tokio::join!(
            storage.remove_group_participant(owner, chat_id, members[3]),
            storage.remove_group_participant(owner, chat_id, members[4]),
        );
        assert_eq!(
            [left, right].iter().filter(|result| result.is_ok()).count(),
            1
        );
        assert_eq!(storage.group_participants(owner, chat_id).await?.len(), 2);
        assert_eq!(
            storage.get_group_chat(owner, chat_id).await?.title,
            "Renamed pair"
        );
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
            "chat_working_contexts",
            "chat_workspaces",
            "checkpoint_restores",
            "event_outbox",
            "event_retention_cursors",
            "messages",
            "paired_devices",
            "plugins",
            "provider_profiles",
            "repository_workspaces",
            "routine_runs",
            "routine_jobs",
            "routine_trigger_deliveries",
            "routine_triggers",
            "routines",
            "secret_references",
            "skills",
            "turn_checkpoints",
            "vcs_operation_results",
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

    async fn create_prior_schema(path: &Path, version: u32) -> Result<(), StorageError> {
        let options =
            SqliteConnectOptions::from_str(path.to_str().ok_or(StorageError::InvalidPath)?)?
                .create_if_missing(true)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let partial = sqlx::migrate::Migrator {
            migrations: Cow::Owned(
                MIGRATOR
                    .iter()
                    .filter(|migration| migration.version <= i64::from(version))
                    .cloned()
                    .collect(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        };
        partial.run(&pool).await?;
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn every_prior_schema_is_backed_up_verified_and_upgraded() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        for version in 1..SCHEMA_VERSION {
            let database = directory.path().join(format!("homebot-v{version}.db"));
            create_prior_schema(&database, version).await?;
            let storage = Storage::open(&database).await?;
            assert_eq!(schema_version(storage.pool()).await?, SCHEMA_VERSION);
            storage.pool.close().await;

            let backup = migration_backup_path(&database, version)?;
            assert!(backup.is_file(), "missing backup for schema {version}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(backup.metadata()?.permissions().mode() & 0o777, 0o600);
            }
            let options =
                SqliteConnectOptions::from_str(backup.to_str().ok_or(StorageError::InvalidPath)?)?
                    .read_only(true);
            let backup_pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await?;
            verify_pool_integrity(&backup_pool).await?;
            assert_eq!(schema_version(&backup_pool).await?, version);
            backup_pool.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn newer_schema_is_refused_without_mutation() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("future.db");
        let storage = Storage::open(&database).await?;
        sqlx::query("UPDATE _sqlx_migrations SET version = ? WHERE version = ?")
            .bind(i64::from(SCHEMA_VERSION + 1))
            .bind(i64::from(SCHEMA_VERSION))
            .execute(storage.pool())
            .await?;
        storage.pool.close().await;
        assert!(matches!(
            Storage::open(&database).await,
            Err(StorageError::SchemaTooNew {
                found,
                supported
            }) if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
        Ok(())
    }

    #[tokio::test]
    async fn failed_backup_prevents_migration_and_retry_recovers() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("interrupted.db");
        let prior = SCHEMA_VERSION - 1;
        create_prior_schema(&database, prior).await?;
        let backup = migration_backup_path(&database, prior)?;
        std::fs::create_dir(&backup)?;
        assert!(Storage::open(&database).await.is_err());

        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        assert_eq!(schema_version(&pool).await?, prior);
        pool.close().await;

        std::fs::remove_dir(backup)?;
        let recovered = Storage::open(&database).await?;
        assert_eq!(schema_version(recovered.pool()).await?, SCHEMA_VERSION);
        Ok(())
    }

    #[tokio::test]
    async fn verified_backup_is_reused_after_interrupted_launch() -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("restart.db");
        let prior = SCHEMA_VERSION - 1;
        create_prior_schema(&database, prior).await?;
        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let backup = create_verified_migration_backup(&pool, &database, prior).await?;
        let original_backup = std::fs::read(&backup)?;
        pool.close().await;

        let storage = Storage::open(&database).await?;
        assert_eq!(schema_version(storage.pool()).await?, SCHEMA_VERSION);
        assert_eq!(std::fs::read(backup)?, original_backup);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn migration_backup_symlink_is_rejected_before_schema_change() -> Result<(), StorageError>
    {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir()?;
        let database = directory.path().join("symlink.db");
        let prior = SCHEMA_VERSION - 1;
        create_prior_schema(&database, prior).await?;
        let backup = migration_backup_path(&database, prior)?;
        symlink(&database, &backup)?;
        assert!(Storage::open(&database).await.is_err());

        let options =
            SqliteConnectOptions::from_str(database.to_str().ok_or(StorageError::InvalidPath)?)?
                .read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        assert_eq!(schema_version(&pool).await?, prior);
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
    #[allow(clippy::too_many_lines)]
    async fn pairing_expiry_origin_limits_restart_and_revocation_are_durable()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("pairing.db");
        let owner = Uuid::now_v7();
        let storage = Storage::open(&database).await?;
        storage
            .create_pairing_credential(
                owner,
                Uuid::now_v7(),
                &[1; 32],
                &[8; 32],
                "https://expired.example",
                "https://expired.example",
                "custom_https",
                1,
                10,
            )
            .await?;
        assert!(matches!(
            storage
                .exchange_pairing_credential(
                    owner,
                    &[1; 32],
                    None,
                    Some("https://expired.example"),
                    &[9; 32],
                    Uuid::now_v7(),
                    "Expired",
                    &[2; 32],
                    10,
                    60_000,
                )
                .await,
            Err(StorageError::PairingExpired)
        ));

        storage
            .create_pairing_credential(
                owner,
                Uuid::now_v7(),
                &[3; 32],
                &[8; 32],
                "https://homebot.example",
                "https://homebot.example",
                "custom_https",
                20,
                10_000,
            )
            .await?;
        for _ in 0..5 {
            assert!(matches!(
                storage
                    .exchange_pairing_credential(
                        owner,
                        &[3; 32],
                        None,
                        Some("https://wrong.example"),
                        &[9; 32],
                        Uuid::now_v7(),
                        "Wrong origin",
                        &[4; 32],
                        21,
                        60_000,
                    )
                    .await,
                Err(StorageError::PairingOriginMismatch)
            ));
        }
        assert!(matches!(
            storage
                .exchange_pairing_credential(
                    owner,
                    &[3; 32],
                    None,
                    Some("https://homebot.example"),
                    &[9; 32],
                    Uuid::now_v7(),
                    "Rate limited",
                    &[5; 32],
                    22,
                    60_000,
                )
                .await,
            Err(StorageError::PairingRateLimited)
        ));

        storage
            .create_pairing_credential(
                owner,
                Uuid::now_v7(),
                &[6; 32],
                &[8; 32],
                "http://127.0.0.1:7123",
                "http://127.0.0.1:7123",
                "loopback",
                30,
                10_000,
            )
            .await?;
        let device = storage
            .exchange_pairing_credential(
                owner,
                &[6; 32],
                Some(&[8; 32]),
                None,
                &[9; 32],
                Uuid::now_v7(),
                "Persistent device",
                &[7; 32],
                31,
                60_000,
            )
            .await?;
        assert!(
            storage
                .authenticate_device_session(&[7; 32], 32)
                .await?
                .is_some()
        );
        storage.pool.close().await;
        let reopened = Storage::open(&database).await?;
        assert_eq!(reopened.device_sessions(owner).await?.len(), 1);
        let revoked = reopened.revoke_device_session(owner, device.id, 40).await?;
        assert_eq!(revoked.revoked_at_ms, Some(40));
        assert!(
            reopened
                .authenticate_device_session(&[7; 32], 41)
                .await?
                .is_none()
        );
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

    #[tokio::test]
    async fn capability_rules_reject_foreign_scopes_and_keep_immutable_audit_after_restart()
    -> Result<(), StorageError> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let storage = Storage::open(&database).await?;
        let owner = Uuid::now_v7();
        let other_owner = Uuid::now_v7();
        let foreign_bot = storage
            .create_bot(other_owner, Bot::create("Foreign", "Private")?, 1)
            .await?;
        let rule_id = Uuid::now_v7();
        let mut rule = CapabilityRuleRecord {
            id: rule_id,
            owner_id: owner,
            capability: "filesystem_write".to_owned(),
            effect: "deny".to_owned(),
            device_id: None,
            bot_id: Some(foreign_bot.id.0),
            chat_id: None,
            workspace_id: None,
            action_prefix: Some("filesystem.write".to_owned()),
            created_at_ms: 2,
            updated_at_ms: 2,
        };
        assert!(storage.upsert_capability_rule(&rule).await.is_err());
        rule.bot_id = None;
        storage.upsert_capability_rule(&rule).await?;
        rule.effect = "allow".to_owned();
        rule.updated_at_ms = 3;
        storage.upsert_capability_rule(&rule).await?;
        storage.delete_capability_rule(owner, rule_id, 4).await?;

        let reopened = Storage::open(&database).await?;
        assert!(reopened.capability_rules(owner).await?.is_empty());
        let audit = reopened.capability_rule_audit(owner).await?;
        assert_eq!(
            audit
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec!["created", "updated", "deleted"]
        );
        assert_eq!(audit[0].snapshot["effect"], "deny");
        assert_eq!(audit[2].snapshot["effect"], "allow");
        Ok(())
    }
}
