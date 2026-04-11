//! Persistent session wrapper around [`Worker`].
//!
//! [`Session`] intercepts `Worker` operations and appends [`HashedEntry`] records
//! to a [`Store`]. It does not modify `Worker` internals — all persistence
//! happens by observing state before and after each operation.
//!
//! Each appended entry carries a hash that chains to the previous entry.
//! On append, the session checks whether the store's head still matches its
//! own `head_hash`; if not, it auto-forks into a new session.

use crate::session_log::{self, EntryHash, HashedEntry, LogEntry, Outcome};
use crate::store::{Store, StoreError};
use crate::SessionId;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::state::Mutable;
use llm_worker::{Worker, WorkerError, WorkerResult};

/// Configuration for session persistence.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Record raw stream events to a separate trace file.
    /// Default: `false`.
    pub record_event_trace: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            record_event_trace: false,
        }
    }
}

/// Errors from session operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Worker(#[from] WorkerError),

    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Persistent session wrapping a [`Worker`].
///
/// The `worker` field is public for direct access to Worker APIs
/// (tool registration, hook setup, subscriber management, etc.).
/// State-mutating operations (`run`, `resume`) should go through
/// Session methods to ensure proper logging.
pub struct Session<C: LlmClient, St: Store> {
    pub worker: Worker<C, Mutable>,
    store: St,
    session_id: SessionId,
    head_hash: Option<EntryHash>,
    _config: SessionConfig,
}

impl<C: LlmClient, St: Store> Session<C, St> {
    /// Create a new session, writing the initial `SessionStart` entry.
    pub async fn new(
        worker: Worker<C, Mutable>,
        store: St,
        config: SessionConfig,
    ) -> Result<Self, StoreError> {
        let session_id = crate::new_session_id();
        let entry = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: worker.get_system_prompt().map(String::from),
            config: worker.request_config().clone(),
            history: worker.history().to_vec(),
        };
        let hashed = session_log::compute_hash(None, &entry);
        let hashed_entry = HashedEntry {
            hash: hashed.clone(),
            prev_hash: None,
            entry,
        };
        store.append(session_id, &hashed_entry).await?;

        Ok(Self {
            worker,
            store,
            session_id,
            head_hash: Some(hashed),
            _config: config,
        })
    }

    /// Restore a session from a stored log.
    ///
    /// Reads all log entries, collects state from them,
    /// and returns a `Session` ready for `resume()`.
    pub async fn restore(
        client: C,
        store: St,
        session_id: SessionId,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let entries = store.read_all(session_id).await?;
        let state = session_log::collect_state(&entries);

        let mut worker = Worker::new(client);
        if let Some(ref prompt) = state.system_prompt {
            worker.set_system_prompt(prompt);
        }
        worker.set_history(state.history);
        worker.set_request_config(state.config);
        worker.set_turn_count(state.turn_count);
        worker.set_last_run_interrupted(state.last_run_interrupted);

        Ok(Self {
            worker,
            store,
            session_id,
            head_hash: state.head_hash,
            _config: config,
        })
    }

    /// The session ID.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The current head hash of the session log chain.
    pub fn head_hash(&self) -> Option<&EntryHash> {
        self.head_hash.as_ref()
    }

    /// Reference to the underlying store.
    pub fn store(&self) -> &St {
        &self.store
    }

    /// Run a user turn, logging all state changes.
    pub async fn run(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<WorkerResult, SessionError> {
        self.ensure_head_or_fork().await?;

        let history_before = self.worker.history().len();

        let result = self.worker.run(user_input).await;

        self.log_history_delta(history_before).await?;
        self.log_turn_end().await?;
        self.log_outcome(&result).await?;

        result.map_err(SessionError::Worker)
    }

    /// Resume from a paused state, logging all state changes.
    pub async fn resume(&mut self) -> Result<WorkerResult, SessionError> {
        self.ensure_head_or_fork().await?;

        let history_before = self.worker.history().len();

        let result = self.worker.resume().await;

        self.log_history_delta(history_before).await?;
        self.log_turn_end().await?;
        self.log_outcome(&result).await?;

        result.map_err(SessionError::Worker)
    }

    /// Fork this session at its current state.
    /// Returns the new session ID. The new log contains a `SessionStart`
    /// seeded with the current history.
    pub async fn fork(&self) -> Result<SessionId, StoreError> {
        let fork_id = crate::new_session_id();
        let entry = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: self.worker.get_system_prompt().map(String::from),
            config: self.worker.request_config().clone(),
            history: self.worker.history().to_vec(),
        };
        let hashed = session_log::compute_hash(None, &entry);
        let hashed_entry = HashedEntry {
            hash: hashed,
            prev_hash: None,
            entry,
        };
        self.store
            .create_session(fork_id, &[hashed_entry])
            .await?;
        Ok(fork_id)
    }

    /// Fork from an arbitrary point in a stored session's log.
    /// Finds the entry matching `at_hash` and creates a new session
    /// with state reconstructed up to that point.
    pub async fn fork_at(
        store: &St,
        source_id: SessionId,
        at_hash: &EntryHash,
    ) -> Result<SessionId, StoreError> {
        let entries = store.read_all(source_id).await?;
        let cut = entries
            .iter()
            .position(|e| &e.hash == at_hash)
            .map(|i| i + 1)
            .unwrap_or(entries.len());
        let state = session_log::collect_state(&entries[..cut]);

        let fork_id = crate::new_session_id();
        let entry = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: state.system_prompt,
            config: state.config,
            history: state.history,
        };
        let hashed = session_log::compute_hash(None, &entry);
        let hashed_entry = HashedEntry {
            hash: hashed,
            prev_hash: None,
            entry,
        };
        store.create_session(fork_id, &[hashed_entry]).await?;
        Ok(fork_id)
    }

    /// Log a `CacheLocked` entry.
    pub async fn log_cache_locked(
        &mut self,
        locked_prefix_len: usize,
    ) -> Result<(), StoreError> {
        let entry = LogEntry::CacheLocked {
            ts: session_log::now_millis(),
            locked_prefix_len,
        };
        self.append_entry(entry).await
    }

    /// Log a `CacheUnlocked` entry.
    pub async fn log_cache_unlocked(&mut self) -> Result<(), StoreError> {
        let entry = LogEntry::CacheUnlocked {
            ts: session_log::now_millis(),
        };
        self.append_entry(entry).await
    }

    /// Log a `ConfigChanged` entry.
    pub async fn log_config_changed(&mut self) -> Result<(), StoreError> {
        let entry = LogEntry::ConfigChanged {
            ts: session_log::now_millis(),
            config: self.worker.request_config().clone(),
        };
        self.append_entry(entry).await
    }

    // ── Private helpers ──────────────────────────────────────────────────

    /// Append a `LogEntry`, computing its hash and updating `head_hash`.
    async fn append_entry(&mut self, entry: LogEntry) -> Result<(), StoreError> {
        let hash = session_log::compute_hash(self.head_hash.as_ref(), &entry);
        let hashed_entry = HashedEntry {
            hash: hash.clone(),
            prev_hash: self.head_hash.clone(),
            entry,
        };
        self.store
            .append(self.session_id, &hashed_entry)
            .await?;
        self.head_hash = Some(hash);
        Ok(())
    }

    /// Check that the store's head still matches ours. If not, auto-fork.
    async fn ensure_head_or_fork(&mut self) -> Result<(), StoreError> {
        let store_head = self.store.read_head_hash(self.session_id).await?;
        if store_head == self.head_hash {
            return Ok(());
        }
        // Another writer advanced this session — fork from our known state.
        let fork_id = crate::new_session_id();
        let entry = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: self.worker.get_system_prompt().map(String::from),
            config: self.worker.request_config().clone(),
            history: self.worker.history().to_vec(),
        };
        let hash = session_log::compute_hash(None, &entry);
        let hashed_entry = HashedEntry {
            hash: hash.clone(),
            prev_hash: None,
            entry,
        };
        self.store
            .create_session(fork_id, &[hashed_entry])
            .await?;
        self.session_id = fork_id;
        self.head_hash = Some(hash);
        Ok(())
    }

    async fn log_history_delta(&mut self, before_len: usize) -> Result<(), StoreError> {
        let history = self.worker.history();
        if history.len() <= before_len {
            return Ok(());
        }

        let ts = session_log::now_millis();
        let new_items = history[before_len..].to_vec();
        let mut i = 0;

        while i < new_items.len() {
            let item = &new_items[i];
            if item.is_user_message() {
                self.append_entry(LogEntry::UserInput {
                    ts,
                    item: new_items[i].clone(),
                })
                .await?;
                i += 1;
            } else if item.is_tool_result() {
                let start = i;
                while i < new_items.len() && new_items[i].is_tool_result() {
                    i += 1;
                }
                self.append_entry(LogEntry::ToolResults {
                    ts,
                    items: new_items[start..i].to_vec(),
                })
                .await?;
            } else if item.is_assistant_message()
                || item.is_tool_call()
                || item.is_reasoning()
            {
                let start = i;
                while i < new_items.len()
                    && (new_items[i].is_assistant_message()
                        || new_items[i].is_tool_call()
                        || new_items[i].is_reasoning())
                {
                    i += 1;
                }
                self.append_entry(LogEntry::AssistantItems {
                    ts,
                    items: new_items[start..i].to_vec(),
                })
                .await?;
            } else {
                self.append_entry(LogEntry::HookInjectedItems {
                    ts,
                    items: vec![new_items[i].clone()],
                })
                .await?;
                i += 1;
            }
        }
        Ok(())
    }

    async fn log_turn_end(&mut self) -> Result<(), StoreError> {
        self.append_entry(LogEntry::TurnEnd {
            ts: session_log::now_millis(),
            turn_count: self.worker.turn_count(),
        })
        .await
    }

    async fn log_outcome(
        &mut self,
        result: &Result<WorkerResult, WorkerError>,
    ) -> Result<(), StoreError> {
        let outcome = match result {
            Ok(WorkerResult::Finished) => Outcome::Finished,
            Ok(WorkerResult::Paused) => Outcome::Paused,
            Ok(WorkerResult::LimitReached) => Outcome::LimitReached,
            Err(e) => Outcome::Error {
                message: e.to_string(),
            },
        };
        self.append_entry(LogEntry::RunOutcome {
            ts: session_log::now_millis(),
            outcome,
            interrupted: self.worker.last_run_interrupted(),
        })
        .await
    }
}
