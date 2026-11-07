//! Host-bound identity, grants and asynchronous provider contracts.
//!
//! The provider is the authority adapter: opaque bindings are not credentials
//! supplied by guests. Implementations must live-check authorization, remain
//! cancellation-safe, bound their own allocations and never auto-retry effects.

pub use crate::bindings::yoi::host::{errors, http, logging};
pub use crate::ticket_bindings::yoi::host2_1_0::{extension_data, objects, ticket_read};
use async_trait::async_trait;
use std::{any::Any, collections::BTreeSet, sync::Arc};
use tokio::time::Instant;
pub use tokio_util::sync::CancellationToken;

/// Constructed by the authenticated Runtime integration, never from guest JSON.
/// All fields are a snapshot upper bound, not a cached live authorization.
#[derive(Clone, Debug)]
pub struct InvocationContext {
    pub workspace: String,
    pub installation: String,
    pub plan: String,
    pub subject: String,
    pub invocation_id: String,
    pub artifact: crate::ArtifactIdentity,
    pub tool_name: String,
    pub grants: Grants,
    pub deadline: Instant,
    pub cancellation: CancellationToken,
}

#[derive(Clone, Debug, Default)]
pub struct Grants {
    pub logging: bool,
    pub connection_slots: BTreeSet<String>,
    pub ticket_read: bool,
    pub ticket_events: bool,
    pub ticket_data_read: BTreeSet<(String, String, String)>,
    pub ticket_data_write: BTreeSet<(String, String, String)>,
}

/// Opaque Host-owned capability, stored only within one invocation's Store.
/// The provider must not interpret a guest resource representation as identity.
pub struct Connection {
    pub(crate) slot: String,
    pub(crate) binding: Arc<dyn Any + Send + Sync>,
}

/// Opaque Ticket capability stored only in one invocation's Store.
pub struct Ticket {
    pub(crate) binding: Arc<dyn Any + Send + Sync>,
}

/// Each operation requires a fresh owner check; admission covers pure Tools too.
pub enum Operation<'a> {
    Admit,
    Publish,
    Open {
        slot: &'a str,
    },
    Send {
        slot: &'a str,
        binding: &'a (dyn Any + Send + Sync),
        request: &'a http::Request,
    },
    Log,
}

/// Core operations require implementations. Additive APIs default to Unsupported
/// for old providers, never to permission or success. Test providers belong to tests.
///
/// `authorize` must check the current installation/artifact/plan/subject and
/// operation against live owner policy. `open`/`send` must also enforce concrete
/// binding policy, secret handling and effect receipts at dispatch time. The
/// executor only enforces static package and invocation ceilings. All waits are
/// dropped on cancellation/deadline; providers must not leave detached work.
#[async_trait]
pub trait Host: Send + Sync {
    /// Old providers fail closed, never infer permission from an old grant.
    async fn resolve_ticket(
        &self,
        _context: &InvocationContext,
        _ticket_ref: &str,
        _access: objects::Access,
    ) -> Result<Arc<dyn Any + Send + Sync>, errors::HostError> {
        Err(error(errors::Code::Unsupported, "Ticket API unavailable"))
    }
    async fn read_ticket(
        &self,
        _context: &InvocationContext,
        _binding: &(dyn Any + Send + Sync),
    ) -> Result<ticket_read::Snapshot, errors::HostError> {
        Err(error(errors::Code::Unsupported, "Ticket API unavailable"))
    }
    /// Providers live-authorize the concrete operation, Ticket and namespace.
    async fn ticket_data(
        &self,
        _context: &InvocationContext,
        _binding: &(dyn Any + Send + Sync),
        _operation: TicketDataOperation,
    ) -> Result<TicketDataResult, errors::HostError> {
        Err(error(
            errors::Code::Unsupported,
            "Ticket data API unavailable",
        ))
    }

    async fn authorize(
        &self,
        context: &InvocationContext,
        operation: Operation<'_>,
    ) -> Result<(), errors::HostError>;
    async fn open(
        &self,
        context: &InvocationContext,
        slot: &str,
    ) -> Result<Arc<dyn Any + Send + Sync>, errors::HostError>;
    async fn send(
        &self,
        context: &InvocationContext,
        binding: &(dyn Any + Send + Sync),
        request: http::Request,
    ) -> Result<http::Response, errors::HostError>;
    async fn log(
        &self,
        context: &InvocationContext,
        entry: logging::Entry,
    ) -> Result<(), errors::HostError>;
}

pub(crate) fn error(code: errors::Code, message: &str) -> errors::HostError {
    errors::HostError {
        code,
        message: message.into(),
        diagnostic_id: String::new(),
        effect: errors::EffectObservation::NotDispatched,
    }
}

/// Portable provider boundary shared by Remote and Embedded owners.
#[derive(Debug, Clone)]
pub enum TicketDataOperation {
    Get(server_api::PluginDataAddress),
    Mutate {
        address: server_api::PluginDataAddress,
        operation_key: String,
        mutation: server_api::PluginDataMutation,
    },
    List {
        collection: server_api::PluginDataCollection,
        cursor: Option<String>,
        limit: u32,
    },
    LookupMutation {
        address: server_api::PluginDataAddress,
        operation_key: String,
    },
}
#[derive(Debug)]
pub enum TicketDataResult {
    Cell(server_api::PluginDataCell),
    Mutated(server_api::PluginDataReceipt),
    Listed(server_api::PluginDataPage),
    MutationLookup(server_api::PluginDataMutationLookup),
}
