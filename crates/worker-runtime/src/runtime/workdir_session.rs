//! Retain a Runtime active-use reservation for a live provider session.
//!
//! Cloned handles share the reservation. A successful provider close releases
//! it even while closed handles remain; a failed or cancelled close retains it
//! until a later successful close or the final wrapper is dropped. Dropping the
//! wrapper does not close the provider or wait for outstanding commands, so
//! callers must keep a session handle alive while using provider-owned work.

use super::WorkdirOperationGuard;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use workdir::{
    CheckoutObservation, CheckoutRequest, CheckoutResult, CheckoutSearchRequest,
    CheckoutSearchResult, CommandEvent, CommandHandle, CommandOutput, CommandOutputRequest,
    CommandRequest, CommandSnapshot, CommandStatus, EditRequest, EditResult, GlobRequest,
    GlobResult, GrepRequest, GrepResult, ListRequest, ListResult, ReadBytesRequest,
    ReadBytesResult, ReadRequest, ReadResult, StatRequest, StatResult, Workdir, WorkdirError,
    WorkdirPath, WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionHandle, WriteRequest, WriteResult,
};

pub(super) fn retain_operation(
    session: WorkdirSessionHandle,
    operation: WorkdirOperationGuard,
) -> WorkdirSessionHandle {
    Arc::new(RetainedWorkdirSession {
        inner: session,
        operation: Mutex::new(Some(operation)),
    })
}

struct RetainedWorkdirSession {
    inner: WorkdirSessionHandle,
    operation: Mutex<Option<WorkdirOperationGuard>>,
}

impl std::fmt::Debug for RetainedWorkdirSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not expose provider internals or reservation state containing host paths.
        f.debug_struct("RetainedWorkdirSession")
            .field("workdir", self.inner.workdir())
            .field("capabilities", &self.inner.capabilities())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl WorkdirSession for RetainedWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.inner.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.inner.capabilities()
    }

    async fn authorize_scope_path(
        &self,
        request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        self.inner.authorize_scope_path(request).await
    }

    async fn scope_rules_overlap(
        &self,
        request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        self.inner.scope_rules_overlap(request).await
    }

    async fn checkout_search(
        &self,
        request: CheckoutSearchRequest,
    ) -> Result<CheckoutSearchResult, WorkdirError> {
        self.inner.checkout_search(request).await
    }

    async fn checkout_observe(
        &self,
        path: WorkdirPath,
    ) -> Result<CheckoutObservation, WorkdirError> {
        self.inner.checkout_observe(path).await
    }

    async fn checkout_execute(
        &self,
        request: CheckoutRequest,
    ) -> Result<CheckoutResult, WorkdirError> {
        self.inner.checkout_execute(request).await
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.inner.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.inner.read(request).await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<ReadBytesResult, WorkdirError> {
        self.inner.read_bytes(request).await
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.inner.write(request).await
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        self.inner.edit(request).await
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.inner.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.inner.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.inner.grep(request).await
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.inner.start_command(request).await
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.inner.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.inner.command_output(request).await
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.inner.cancel_command(handle).await
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        self.inner.subscribe_command_events()
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        self.inner.command_snapshot()
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        // Provider close must succeed before releasing the active-use reservation.
        // No wrapper-local terminal checks may bypass the provider's own checks.
        self.inner.close().await?;
        let operation = self
            .operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        // Release outside the mutex: guard Drop may acquire Runtime state locks.
        drop(operation);
        Ok(())
    }
}
