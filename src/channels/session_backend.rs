//! Trait abstraction for session persistence backends.
//!
//! Backends store per-sender conversation histories. The trait is intentionally
//! minimal — load, append, remove_last, list — so that JSONL and SQLite (and
//! future backends) share a common interface.

use super::session_sqlite::SqliteSessionBackend;
use crate::providers::traits::ChatMessage;
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;

/// Metadata about a persisted session.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// Session key (e.g. `telegram_user123`).
    pub key: String,
    /// Optional human-readable name (e.g. `eyrie-commander-briefing`).
    pub name: Option<String>,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the last message was appended.
    pub last_activity: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
}

/// Query parameters for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionQuery {
    /// Keyword to search in session messages (FTS5 if available).
    pub keyword: Option<String>,
    /// Maximum number of sessions to return.
    pub limit: Option<usize>,
}

/// Open the configured session backend for channel session persistence and
/// session-inspection tools.
///
/// SQLite is the only live runtime backend. Legacy `jsonl` configs are accepted
/// for compatibility, but they are treated as `sqlite` and any old JSONL files
/// are migrated into the SQLite store.
pub fn open_session_backend(
    workspace_dir: &Path,
    backend_name: &str,
) -> Result<Arc<dyn SessionBackend>> {
    let normalized = backend_name.trim().to_ascii_lowercase();
    if !normalized.is_empty() && normalized != "sqlite" && normalized != "jsonl" {
        bail!("Unsupported session backend '{normalized}'. Expected 'sqlite' or legacy 'jsonl'.");
    }

    if normalized == "jsonl" {
        tracing::warn!(
            "Session backend 'jsonl' is deprecated and now treated as 'sqlite'; migrating any legacy JSONL sessions"
        );
    }

    let backend = SqliteSessionBackend::new(workspace_dir)?;
    match backend.migrate_from_jsonl(workspace_dir) {
        Ok(migrated) if migrated > 0 => {
            tracing::info!("Migrated {migrated} JSONL session file(s) into SQLite");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!("Failed to migrate legacy JSONL sessions into SQLite: {error}");
        }
    }

    Ok(Arc::new(backend))
}

/// Trait for session persistence backends.
///
/// Implementations must be `Send + Sync` for sharing across async tasks.
pub trait SessionBackend: Send + Sync {
    /// Load all messages for a session. Returns empty vec if session doesn't exist.
    fn load(&self, session_key: &str) -> Vec<ChatMessage>;

    /// Append a single message to a session.
    fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()>;

    /// Remove the last message from a session. Returns `true` if a message was removed.
    fn remove_last(&self, session_key: &str) -> std::io::Result<bool>;

    /// List all session keys.
    fn list_sessions(&self) -> Vec<String>;

    /// List sessions with metadata.
    fn list_sessions_with_metadata(&self) -> Vec<SessionMetadata> {
        // Default: construct metadata from messages (backends can override for efficiency)
        self.list_sessions()
            .into_iter()
            .map(|key| {
                let messages = self.load(&key);
                SessionMetadata {
                    key,
                    name: None,
                    created_at: Utc::now(),
                    last_activity: Utc::now(),
                    message_count: messages.len(),
                }
            })
            .collect()
    }

    /// Compact a session file (remove duplicates/corruption). No-op by default.
    fn compact(&self, _session_key: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Remove sessions that haven't been active within the given TTL hours.
    fn cleanup_stale(&self, _ttl_hours: u32) -> std::io::Result<usize> {
        Ok(0)
    }

    /// Search sessions by keyword. Default returns empty (backends with FTS override).
    fn search(&self, _query: &SessionQuery) -> Vec<SessionMetadata> {
        Vec::new()
    }

    /// Delete all messages for a session. Returns `true` if the session existed.
    fn delete_session(&self, _session_key: &str) -> std::io::Result<bool> {
        Ok(false)
    }

    /// Set or update the human-readable name for a session.
    fn set_session_name(&self, _session_key: &str, _name: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Get the human-readable name for a session (if set).
    fn get_session_name(&self, _session_key: &str) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    /// Set the session state (e.g. "idle", "running", "error").
    /// `turn_id` identifies the current turn (set when running, cleared on idle).
    fn set_session_state(
        &self,
        _session_key: &str,
        _state: &str,
        _turn_id: Option<&str>,
    ) -> std::io::Result<()> {
        Ok(())
    }

    /// Get the current session state. Returns `None` if the backend doesn't track state.
    fn get_session_state(&self, _session_key: &str) -> std::io::Result<Option<SessionState>> {
        Ok(None)
    }

    /// List sessions currently in "running" state.
    fn list_running_sessions(&self) -> Vec<SessionMetadata> {
        Vec::new()
    }

    /// List sessions stuck in "running" state longer than `threshold_secs`.
    fn list_stuck_sessions(&self, _threshold_secs: u64) -> Vec<SessionMetadata> {
        Vec::new()
    }
}

/// Session state information.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Current state: "idle", "running", or "error".
    pub state: String,
    /// Turn ID of the active or last turn.
    pub turn_id: Option<String>,
    /// When the current state was entered.
    pub turn_started_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::session_store::SessionStore;

    #[test]
    fn open_jsonl_setting_is_promoted_to_sqlite() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = open_session_backend(tmp.path(), "jsonl").unwrap();

        backend
            .append("telegram_room_alice", &ChatMessage::user("hello"))
            .unwrap();

        let messages = backend.load("telegram_room_alice");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hello");
        assert!(tmp.path().join("sessions/sessions.db").exists());
        assert!(
            !tmp.path()
                .join("sessions/telegram_room_alice.jsonl")
                .exists()
        );
    }

    #[test]
    fn open_sqlite_backend_migrates_legacy_jsonl_sessions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let legacy_store = SessionStore::new(tmp.path()).unwrap();
        legacy_store
            .append("telegram_room_alice", &ChatMessage::user("hello"))
            .unwrap();

        let backend = open_session_backend(tmp.path(), "sqlite").unwrap();

        let messages = backend.load("telegram_room_alice");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hello");
        assert!(tmp.path().join("sessions/sessions.db").exists());
        assert!(
            tmp.path()
                .join("sessions/telegram_room_alice.jsonl.migrated")
                .exists()
        );
    }

    #[test]
    fn session_metadata_is_constructible() {
        let meta = SessionMetadata {
            key: "test".into(),
            name: None,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 5,
        };
        assert_eq!(meta.key, "test");
        assert_eq!(meta.message_count, 5);
    }

    #[test]
    fn session_query_defaults() {
        let q = SessionQuery::default();
        assert!(q.keyword.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn open_session_backend_rejects_unknown_backend() {
        let tmp = tempfile::TempDir::new().unwrap();
        let error = open_session_backend(tmp.path(), "bogus")
            .err()
            .expect("unknown backend should fail");
        assert!(error.to_string().contains("Unsupported session backend"));
    }
}
