//! Finite Drop cleanup. A cancel acknowledgement is not termination evidence; only a
//! validated terminal fence confirms the process result. No provider text is logged.

use super::*;
use crate::bash_output::{
    CleanupCancel, CleanupDiagnostics, CleanupDrain, CleanupFailure, CleanupProviderFailure,
    CleanupRegistration,
};
use std::time::Duration;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
// Reserve time for independent terminal observation even if cancel never responds.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct Cleanup {
    pub session: WorkdirSessionHandle,
    pub state: Arc<Mutex<BashExecutionState>>,
    pub execution_id: String,
    pub execution_nonce: u64,
    pub handle: CommandHandle,
    pub cursor: u64,
    pub retained: BashOutputWriter,
}

impl Cleanup {
    pub async fn run(mut self) {
        let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
        let cancel = match tokio::time::timeout(
            CANCEL_TIMEOUT,
            self.session.cancel_command(self.handle.clone()),
        )
        .await
        {
            Ok(Ok(())) => CleanupCancel::Acknowledged,
            Ok(Err(error)) => CleanupCancel::Failed {
                reason: CleanupProviderFailure::from(&error),
            },
            Err(_) => CleanupCancel::DeadlineExceeded,
        };
        let drain = match tokio::time::timeout_at(deadline, self.drain()).await {
            Ok(Ok(())) => CleanupDrain::TerminalConfirmed,
            Ok(Err(reason)) => CleanupDrain::Failed { reason },
            Err(_) => CleanupDrain::DeadlineExceeded,
        };
        let deadline_expired = matches!(drain, CleanupDrain::DeadlineExceeded);
        // Finalize evidence before exposing an empty registry. Keep the lock through
        // publication so an older cleanup can never remove a newer generation.
        let mut state = self.state.lock().unwrap();
        let registration = if state
            .active
            .get(&self.execution_id)
            .is_some_and(|active| active.execution_nonce == self.execution_nonce)
        {
            state.active.remove(&self.execution_id);
            state.cancellation_requested.remove(&self.execution_id);
            CleanupRegistration::Released
        } else {
            CleanupRegistration::Superseded
        };
        let diagnostics = CleanupDiagnostics {
            execution_nonce: self.execution_nonce,
            cancel,
            drain,
            deadline_expired,
            registration,
        };
        tracing::info!(
            output_id = %self.retained.id(),
            cleanup = ?diagnostics,
            "Bash Drop cleanup finished; cancellation and terminal observation are independent"
        );
        self.retained.finish_cleanup(diagnostics);
        drop(self.retained);
    }

    async fn drain(&mut self) -> Result<(), CleanupFailure> {
        loop {
            let page = command_page(
                &self.session,
                CommandOutputRequest {
                    handle: self.handle.clone(),
                    cursor: self.cursor,
                    limit: workdir::MAX_COMMAND_READ_BYTES,
                    wait: true,
                },
            )
            .await
            .map_err(|error| CleanupFailure::Provider {
                reason: CleanupProviderFailure::from(&error),
            })?;
            let mut next = self.cursor;
            for chunk in &page.chunks {
                if chunk.sequence != next {
                    return Err(CleanupFailure::SequenceGap);
                }
                next = next
                    .checked_add(chunk.bytes.len() as u64)
                    .filter(|next| *next <= crate::bash_output::COMMAND_BYTES as u64)
                    .ok_or(CleanupFailure::DeliveryBudgetExceeded)?;
                self.retained
                    .append_chunk(chunk)
                    .map_err(|_| CleanupFailure::RetainedChunkInvalid)?;
            }
            if page.next_cursor != next {
                return Err(CleanupFailure::CursorGap);
            }
            self.cursor = next;
            if let Some(terminal) = page.terminal {
                self.retained
                    .finish_transport(&terminal)
                    .map_err(|_| CleanupFailure::TerminalMetadataInvalid)?;
                return Ok(());
            }
            // An immediately-ready provider must not starve the cleanup deadline.
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
mod tests;
