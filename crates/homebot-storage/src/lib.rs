//! Durable `SQLite` persistence and resumable event outbox.

use serde_json::Value;
use sqlx::{
    Row, SqlitePool,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, str::FromStr, time::Duration};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
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
        transaction.commit().await?;
        Ok(OutboxEvent {
            sequence,
            event_id,
            owner_id,
            event_kind: event_kind.to_owned(),
            payload: payload.clone(),
            created_at_ms,
        })
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

fn parse_uuid(value: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(value)
        .map_err(|_| StorageError::Integrity("database contains an invalid UUID".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            "bots",
            "chats",
            "event_outbox",
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
}
