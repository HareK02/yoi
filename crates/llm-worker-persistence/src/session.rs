//! Persistent session wrapper around [`Worker`].
//!
//! [`Session`] intercepts `Worker` operations and appends [`LogEntry`] records
//! to a [`Store`]. It does not modify `Worker` internals — all persistence
//! happens by observing state before and after each operation.

use crate::session_log::{self, LogEntry, Outcome};
use crate::store::{Store, StoreError};
use crate::SessionId;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::llm_client::types::Item;
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
        let start = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: worker.get_system_prompt().map(String::from),
            config: worker.request_config().clone(),
            history: worker.history().to_vec(),
        };
        store.append(session_id, &start).await?;

        Ok(Self {
            worker,
            store,
            session_id,
            _config: config,
        })
    }

    /// Restore a session from a stored log.
    ///
    /// Reads all log entries, replays them to reconstruct state,
    /// and returns a `Session` ready for `resume()`.
    pub async fn restore(
        client: C,
        store: St,
        session_id: SessionId,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let entries = store.read_all(session_id).await?;
        let state = session_log::replay_entries(&entries);

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
            _config: config,
        })
    }

    /// The session ID.
    pub fn session_id(&self) -> SessionId {
        self.session_id
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
        let input = user_input.into();
        let user_item = Item::user_message(&input);
        let history_before = self.worker.history().len();

        let result = self.worker.run(input).await;

        self.log_history_delta(history_before, Some(&user_item))
            .await?;
        self.log_turn_end().await?;
        self.log_outcome(&result).await?;

        result.map_err(SessionError::Worker)
    }

    /// Resume from a paused state, logging all state changes.
    pub async fn resume(&mut self) -> Result<WorkerResult, SessionError> {
        let history_before = self.worker.history().len();

        let result = self.worker.resume().await;

        self.log_history_delta(history_before, None).await?;
        self.log_turn_end().await?;
        self.log_outcome(&result).await?;

        result.map_err(SessionError::Worker)
    }

    /// Fork this session at its current state.
    /// Returns the new session ID. The new log contains a `SessionStart`
    /// seeded with the current history.
    pub async fn fork(&self) -> Result<SessionId, StoreError> {
        let fork_id = crate::new_session_id();
        let start = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: self.worker.get_system_prompt().map(String::from),
            config: self.worker.request_config().clone(),
            history: self.worker.history().to_vec(),
        };
        self.store.create_session(fork_id, &[start]).await?;
        Ok(fork_id)
    }

    /// Fork from an arbitrary point in a stored session's log.
    /// Replays entries up to `up_to_entry` and creates a new session
    /// with that reconstructed state.
    pub async fn fork_at(
        store: &St,
        source_id: SessionId,
        up_to_entry: usize,
    ) -> Result<SessionId, StoreError> {
        let entries = store.read_all(source_id).await?;
        let truncated = &entries[..up_to_entry.min(entries.len())];
        let state = session_log::replay_entries(truncated);

        let fork_id = crate::new_session_id();
        let start = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: state.system_prompt,
            config: state.config,
            history: state.history,
        };
        store.create_session(fork_id, &[start]).await?;
        Ok(fork_id)
    }

    /// Log a `CacheLocked` entry.
    pub async fn log_cache_locked(
        &self,
        locked_prefix_len: usize,
    ) -> Result<(), StoreError> {
        self.store
            .append(
                self.session_id,
                &LogEntry::CacheLocked {
                    ts: session_log::now_millis(),
                    locked_prefix_len,
                },
            )
            .await
    }

    /// Log a `CacheUnlocked` entry.
    pub async fn log_cache_unlocked(&self) -> Result<(), StoreError> {
        self.store
            .append(
                self.session_id,
                &LogEntry::CacheUnlocked {
                    ts: session_log::now_millis(),
                },
            )
            .await
    }

    /// Log a `ConfigChanged` entry.
    pub async fn log_config_changed(&self) -> Result<(), StoreError> {
        self.store
            .append(
                self.session_id,
                &LogEntry::ConfigChanged {
                    ts: session_log::now_millis(),
                    config: self.worker.request_config().clone(),
                },
            )
            .await
    }

    // ── Private helpers ──────────────────────────────────────────────────

    async fn log_history_delta(
        &self,
        before_len: usize,
        user_item: Option<&Item>,
    ) -> Result<(), StoreError> {
        let history = self.worker.history();
        if history.len() <= before_len {
            return Ok(());
        }

        let ts = session_log::now_millis();
        let new_items = &history[before_len..];
        let mut i = 0;

        // If we have a user_item, the first new item should be the user input
        if let Some(item) = user_item {
            self.store
                .append(
                    self.session_id,
                    &LogEntry::UserInput {
                        ts,
                        item: item.clone(),
                    },
                )
                .await?;
            i = 1;
        }

        // Classify and group remaining items
        while i < new_items.len() {
            let item = &new_items[i];
            if item.is_tool_result() {
                let start = i;
                while i < new_items.len() && new_items[i].is_tool_result() {
                    i += 1;
                }
                self.store
                    .append(
                        self.session_id,
                        &LogEntry::ToolResults {
                            ts,
                            items: new_items[start..i].to_vec(),
                        },
                    )
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
                self.store
                    .append(
                        self.session_id,
                        &LogEntry::AssistantItems {
                            ts,
                            items: new_items[start..i].to_vec(),
                        },
                    )
                    .await?;
            } else {
                self.store
                    .append(
                        self.session_id,
                        &LogEntry::HookInjectedItems {
                            ts,
                            items: vec![new_items[i].clone()],
                        },
                    )
                    .await?;
                i += 1;
            }
        }
        Ok(())
    }

    async fn log_turn_end(&self) -> Result<(), StoreError> {
        self.store
            .append(
                self.session_id,
                &LogEntry::TurnEnd {
                    ts: session_log::now_millis(),
                    turn_count: self.worker.turn_count(),
                },
            )
            .await
    }

    async fn log_outcome(
        &self,
        result: &Result<WorkerResult, WorkerError>,
    ) -> Result<(), StoreError> {
        let outcome = match result {
            Ok(WorkerResult::Finished) => Outcome::Finished,
            Ok(WorkerResult::Paused) => Outcome::Paused,
            Err(e) => Outcome::Error {
                message: e.to_string(),
            },
        };
        self.store
            .append(
                self.session_id,
                &LogEntry::RunOutcome {
                    ts: session_log::now_millis(),
                    outcome,
                    interrupted: self.worker.last_run_interrupted(),
                },
            )
            .await
    }
}
