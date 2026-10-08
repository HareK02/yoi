//! Presentation-independent stateful runtime for WIP clients.
//!
//! [`Client`] separates observations by canonical WIP over HTTP endpoint and caller-
//! supplied security context. It reconciles requested observation coverage without
//! duplicating fresh or loading work, creates requests with the standard
//! [`wip_http`] adapter, correlates responses by [`RequestId`], and retains
//! bounded object, tree, interface, diagnostic, and operation state. The crate
//! contains no terminal, widget, or UI event types.

#![deny(missing_docs)]

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use wip_http::http::{Request, Response};
use wip_http::{
    ClientResponseError, DecodedResponse, Endpoint, Limits, decode_call_operation_response,
    decode_fetch_interface_response, decode_observe_response, encode_call_operation_request,
    encode_fetch_interface_request, encode_observe_request,
};
use wip_protocol::{
    CallOperationRequest, CallOperationResponse, FetchInterfaceRequest, InterfaceDescriptor,
    InterfaceReference, InterfaceTarget, Object, ObjectObservation as ProtocolObjectObservation,
    ObserveRequest, OperationDeclaration, ProtocolError, ProtocolErrorCode, Target, Value,
};

/// Monotonic identity assigned to one logical request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

impl RequestId {
    /// Returns the process-local numeric identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque identity for authentication and authorization state.
///
/// Applications must issue a different value whenever credentials, subject,
/// tenant, or other security-relevant context changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecurityContext(String);

impl SecurityContext {
    /// Creates a security-context identity.
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    /// Returns the application-defined identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of an isolated client session.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId {
    endpoint: String,
    security_context: SecurityContext,
}

impl SessionId {
    /// Returns the canonical endpoint URL, including its trailing slash.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the security context that partitions this session.
    #[must_use]
    pub fn security_context(&self) -> &SecurityContext {
        &self.security_context
    }
}

/// Bounded state limits for the client runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientLimits {
    /// Maximum simultaneously retained endpoint/security-context sessions.
    pub sessions: usize,
    /// Maximum object, path, and observed-children state retained by each session.
    pub objects_per_session: usize,
    /// Maximum interface observations retained by each session.
    pub interfaces_per_session: usize,
    /// Maximum requests awaiting a response.
    pub in_flight: usize,
    /// Maximum call results and diagnostics retained in each history.
    pub history: usize,
}

impl ClientLimits {
    /// Creates a limit set, rejecting every zero bound.
    pub fn new(
        sessions: usize,
        objects_per_session: usize,
        interfaces_per_session: usize,
        in_flight: usize,
        history: usize,
    ) -> Result<Self, ConfigurationError> {
        if sessions == 0
            || objects_per_session == 0
            || interfaces_per_session == 0
            || in_flight == 0
            || history == 0
        {
            return Err(ConfigurationError);
        }
        Ok(Self {
            sessions,
            objects_per_session,
            interfaces_per_session,
            in_flight,
            history,
        })
    }
}

/// A client bound must be nonzero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationError;

impl Display for ConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("client limits must be nonzero")
    }
}

impl Error for ConfigurationError {}

/// High-level category for a client-local failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientErrorKind {
    /// A peer response violated its interaction contract.
    InvalidResponse,
    /// A valid descriptor uses a format unsupported by this runtime.
    UnsupportedDescriptorFormat,
    /// The exchange failed outside a valid WIP response envelope.
    Transport,
    /// A valid Host protocol failure was applied to the observation.
    ProtocolFailure,
    /// A local request or operation selection was invalid.
    InvalidRequest,
    /// Required fresh object or interface state was unavailable.
    MissingObservation,
    /// A configured state bound prevented the requested work.
    Capacity,
    /// A response was superseded by a newer request for the same subject.
    StaleResponse,
}

/// Cloneable client-local failure suitable for model state and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    /// Stable local failure category.
    pub kind: ClientErrorKind,
    /// Human-readable diagnostic that callers must not branch on.
    pub detail: String,
}

impl ClientError {
    fn new(kind: ClientErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl Display for ClientError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "client failure ({:?}): {}",
            self.kind, self.detail
        )
    }
}

impl Error for ClientError {}

/// Lifecycle of one cached observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationState {
    /// A request is currently loading this subject.
    Loading(RequestId),
    /// The observation is the latest accepted value.
    Fresh,
    /// The value remains available but must be reobserved before use.
    Stale,
    /// The most recent observation attempt failed.
    Error(ClientError),
}

/// Object observation associated with a request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectObservation {
    /// Canonical path used to observe the object.
    pub path: String,
    /// Last accepted object, if one has been received.
    pub object: Option<Object>,
    /// Object validator copied from `object` for convenient inspection.
    pub validator: Option<Vec<u8>>,
    /// Current observation lifecycle.
    pub state: ObservationState,
}

/// Lazily materialized entry-tree node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeObservation {
    /// Canonical node path.
    pub path: String,
    /// Materialized direct child paths in Host order.
    pub children: Vec<String>,
    /// Request revision that most recently owned this tree subject.
    pub revision: RequestId,
    /// Current expansion lifecycle.
    pub state: ObservationState,
}

/// Cached interface descriptor observation for one structured `(scope, name)` pair.
///
/// A known lost scope binding retires the descriptor rather than replacing only
/// its [`Self::scope_ref`]. A direct scope `observe` returning `NotFound` may remove
/// this cache entry; edge omission and local Object eviction do not prove deletion.
/// Prepared calls retain their own snapshot until result validation completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceObservation {
    /// Structured interface reference.
    pub reference: InterfaceReference,
    /// Last accepted descriptor, if one has been received.
    pub descriptor: Option<InterfaceDescriptor>,
    /// Optional interface validator. Absence makes the descriptor immutable only
    /// while the scope binding remains valid.
    pub validator: Option<Vec<u8>>,
    /// Optional opaque identity of the interface scope Object.
    pub scope_ref: Option<String>,
    /// Current observation lifecycle.
    pub state: ObservationState,
}

/// Immutable operation inputs fixed before request construction.
#[derive(Debug, Clone, PartialEq)]
pub struct CallContext {
    session: SessionId,
    object_key: u64,
    object: ObjectObservation,
    interface: InterfaceObservation,
    interface_revision: RequestId,
    descriptor: InterfaceDescriptor,
    operation: OperationDeclaration,
    request: CallOperationRequest,
}

impl CallContext {
    /// Returns the isolated session used by this call.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Returns the object observation fixed at call construction.
    #[must_use]
    pub fn object(&self) -> &ObjectObservation {
        &self.object
    }

    /// Returns the interface observation fixed at call construction.
    #[must_use]
    pub fn interface(&self) -> &InterfaceObservation {
        &self.interface
    }

    /// Returns the descriptor used for both argument and result validation.
    #[must_use]
    pub fn descriptor(&self) -> &InterfaceDescriptor {
        &self.descriptor
    }

    /// Returns the exact operation declaration fixed for this call.
    #[must_use]
    pub fn operation(&self) -> &OperationDeclaration {
        &self.operation
    }

    /// Returns the fully constructed logical request.
    #[must_use]
    pub fn request(&self) -> &CallOperationRequest {
        &self.request
    }
}

/// Why a dispatched call has an unknown outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeUnknownReason {
    /// The Host explicitly returned `OperationOutcomeUnknown`.
    Protocol,
    /// The transport timed out after dispatch.
    Timeout,
    /// The connection was lost after dispatch.
    Disconnect,
    /// A response arrived but could not be decoded or validated.
    ResponseDecode,
}

/// Terminal state retained for an operation call.
#[derive(Debug, Clone, PartialEq)]
pub enum CallOutcome {
    /// A result validated with the context's fixed descriptor.
    Success(CallOperationResponse),
    /// A canonical Host failure whose non-execution guarantee is known.
    ProtocolFailure(ProtocolError),
    /// Dispatch occurred, but execution or commit cannot be ruled out.
    Unknown {
        /// Stable reason for the uncertainty.
        reason: OutcomeUnknownReason,
        /// Optional local or Host diagnostic.
        failure: Option<ClientError>,
    },
    /// The request failed before dispatch and therefore has no unknown effect.
    NotDispatched(ClientError),
}

/// One bounded operation-history record.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    /// Request identity.
    pub request_id: RequestId,
    /// Immutable construction and validation context.
    pub context: CallContext,
    /// Terminal call outcome.
    pub outcome: CallOutcome,
}

/// Transport failure occurring after operation dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchedTransportFailure {
    /// The transport deadline elapsed.
    Timeout,
    /// The connection ended before a complete response arrived.
    Disconnect,
}

/// Kind of retained runtime diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// HTTP status disagreed with a valid protocol error envelope.
    HttpStatusMismatch,
    /// A response was rejected because a newer request owns the subject.
    StaleResponse,
    /// A client-local response or transport failure occurred.
    ClientFailure,
}

/// One bounded diagnostic-history entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Request associated with the diagnostic.
    pub request_id: RequestId,
    /// Stable diagnostic category.
    pub kind: DiagnosticKind,
    /// Human-readable bounded detail.
    pub detail: String,
}

/// Encoded HTTP request paired with its runtime identity.
#[derive(Debug)]
pub struct PreparedRequest {
    /// Identity required when completing or failing this request.
    pub id: RequestId,
    /// Standard WIP over HTTP request.
    pub request: Request<Vec<u8>>,
}

/// Result of applying a correlated response to the runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Completion {
    /// An object observation was updated.
    Object(ObjectObservation),
    /// An interface observation was updated.
    Interface(InterfaceObservation),
    /// An operation reached a terminal outcome.
    Call(Box<CallRecord>),
    /// A valid protocol failure was applied to a retrieval observation.
    ProtocolFailure(ProtocolError),
    /// A superseded response was rejected without mutating current state.
    StaleResponseRejected(RequestId),
}

#[derive(Debug, Clone)]
struct EntryRecord {
    object_key: Option<u64>,
    revision: RequestId,
    state: ObservationState,
    // Retrieval state can change without losing the last accepted binding.
    // Only confirmed unavailability ends that path's lifetime evidence.
    confirmed_missing: bool,
}

#[derive(Debug, Clone)]
struct ObjectRecord {
    object: Object,
    revision: RequestId,
    call_owners: usize,
    touched: u64,
}

#[derive(Debug, Clone)]
struct Session {
    entries: BTreeMap<String, EntryRecord>,
    objects: BTreeMap<u64, ObjectRecord>,
    refs: BTreeMap<String, u64>,
    trees: BTreeMap<String, TreeObservation>,
    interfaces: BTreeMap<InterfaceReference, InterfaceRecord>,
    calls: VecDeque<CallRecord>,
    diagnostics: VecDeque<Diagnostic>,
    next_object_key: u64,
    clock: u64,
}

#[derive(Debug, Clone)]
struct InterfaceRecord {
    observation: InterfaceObservation,
    // Ownership of this exact fetch/observation, not a Protocol identity.
    revision: RequestId,
    // Starting a refresh does not rebind its retained Descriptor. Scope evidence
    // must still be able to retire that old lifetime while the refresh loads.
    accepted_revision: RequestId,
    // Scope evidence is shared across names and can also come from Observe.
    // This private corroboration is never substituted for wire scope_ref and is
    // retained only with the bounded Interface record, not as a new identity/key.
    scope_binding: Option<String>,
    binding_revision: RequestId,
    touched: u64,
}

impl InterfaceRecord {
    fn retire(&mut self, revision: RequestId) {
        self.revision = self.revision.max(revision);
        self.observation.descriptor = None;
        self.observation.validator = None;
        self.observation.state = ObservationState::Stale;
    }
}

impl Session {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            objects: BTreeMap::new(),
            refs: BTreeMap::new(),
            trees: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            calls: VecDeque::new(),
            diagnostics: VecDeque::new(),
            next_object_key: 1,
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn object_observation(&self, path: &str) -> Option<ObjectObservation> {
        let entry = self.entries.get(path)?;
        let object = entry
            .object_key
            .and_then(|key| self.objects.get(&key))
            .map(|record| record.object.clone());
        let validator = object.as_ref().and_then(|value| value.validator.clone());
        Some(ObjectObservation {
            path: path.to_owned(),
            object,
            validator,
            state: entry.state.clone(),
        })
    }

    fn mark_object_stale(&mut self, key: u64, revision: RequestId) {
        if let Some(record) = self.objects.get_mut(&key) {
            record.revision = record.revision.max(revision);
        }
        for entry in self.entries.values_mut() {
            if entry.object_key == Some(key) {
                entry.state = ObservationState::Stale;
            }
        }
    }

    fn pin_call_object(&mut self, key: u64) {
        let record = self
            .objects
            .get_mut(&key)
            .expect("call context object remains observed");
        record.call_owners = record.call_owners.saturating_add(1);
    }

    fn release_call_object(&mut self, key: u64) {
        let Some(record) = self.objects.get_mut(&key) else {
            return;
        };
        record.call_owners = record.call_owners.saturating_sub(1);
        let remove = record.call_owners == 0
            && !self
                .entries
                .values()
                .any(|entry| entry.object_key == Some(key));
        if remove {
            remove_object(self, key);
        }
    }

    fn push_diagnostic(&mut self, value: Diagnostic, maximum: usize) {
        push_bounded(&mut self.diagnostics, value, maximum);
    }

    fn push_call(&mut self, value: CallRecord, maximum: usize) {
        push_bounded(&mut self.calls, value, maximum);
    }
}

#[derive(Debug)]
enum Pending {
    Observe {
        session: SessionId,
        request: ObserveRequest,
    },
    Interface {
        session: SessionId,
        request: FetchInterfaceRequest,
    },
    Call {
        context: Box<CallContext>,
        dispatched: bool,
    },
}

impl Pending {
    fn session(&self) -> &SessionId {
        match self {
            Self::Observe { session, .. } | Self::Interface { session, .. } => session,
            Self::Call { context, .. } => context.session(),
        }
    }
}

/// Stateful, presentation-independent WIP client runtime.
#[derive(Debug)]
pub struct Client {
    limits: ClientLimits,
    wire_limits: Limits,
    sessions: BTreeMap<SessionId, Session>,
    pending: BTreeMap<RequestId, Pending>,
    next_request_id: u64,
}

impl Client {
    /// Creates an empty runtime with explicit state and wire bounds.
    #[must_use]
    pub fn new(limits: ClientLimits, wire_limits: Limits) -> Self {
        Self {
            limits,
            wire_limits,
            sessions: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_request_id: 1,
        }
    }

    /// Opens or returns a session isolated by canonical endpoint and security context.
    pub fn open_session(
        &mut self,
        endpoint: &str,
        security_context: SecurityContext,
    ) -> Result<SessionId, ClientError> {
        let endpoint = Endpoint::parse(endpoint).map_err(|error| {
            ClientError::new(ClientErrorKind::InvalidRequest, error.to_string())
        })?;
        let id = SessionId {
            endpoint: endpoint.to_string(),
            security_context,
        };
        if !self.sessions.contains_key(&id) {
            if self.sessions.len() >= self.limits.sessions {
                return Err(ClientError::new(
                    ClientErrorKind::Capacity,
                    "session capacity reached",
                ));
            }
            self.sessions.insert(id.clone(), Session::new());
        }
        Ok(id)
    }

    /// Returns an object observation independently of any UI selection.
    #[must_use]
    pub fn object(&self, session: &SessionId, path: &str) -> Option<ObjectObservation> {
        self.sessions.get(session)?.object_observation(path)
    }

    /// Returns the bounded Known Space in canonical path order.
    #[must_use]
    pub fn known_space(&self, session: &SessionId) -> Option<Vec<ObjectObservation>> {
        let state = self.sessions.get(session)?;
        Some(
            state
                .entries
                .keys()
                .filter_map(|path| state.object_observation(path))
                .filter(|observation| observation.object.is_some())
                .collect(),
        )
    }

    /// Returns one materialized lazy-tree node.
    #[must_use]
    pub fn tree(&self, session: &SessionId, path: &str) -> Option<&TreeObservation> {
        self.sessions.get(session)?.trees.get(path)
    }

    /// Returns the current interface observation, isolated by session and the
    /// complete structured reference (never by display text or serialized JSON).
    #[must_use]
    pub fn interface(
        &self,
        session: &SessionId,
        reference: &InterfaceReference,
    ) -> Option<&InterfaceObservation> {
        self.sessions
            .get(session)?
            .interfaces
            .get(reference)
            .map(|record| &record.observation)
    }

    /// Returns operation history from oldest to newest.
    #[must_use]
    pub fn call_history(&self, session: &SessionId) -> Option<&VecDeque<CallRecord>> {
        Some(&self.sessions.get(session)?.calls)
    }

    /// Returns diagnostic history from oldest to newest.
    #[must_use]
    pub fn diagnostics(&self, session: &SessionId) -> Option<&VecDeque<Diagnostic>> {
        Some(&self.sessions.get(session)?.diagnostics)
    }

    /// Returns the number of requests currently in flight.
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.pending.len()
    }

    /// Returns the immutable context of an operation that is still in flight.
    #[must_use]
    pub fn pending_call_context(&self, id: RequestId) -> Option<&CallContext> {
        match self.pending.get(&id)? {
            Pending::Call { context, .. } => Some(context),
            _ => None,
        }
    }

    /// Ensures that an observation is available through `depth` without issuing
    /// duplicate work.
    ///
    /// Returns `None` when the requested coverage is already fresh, is currently
    /// loading, is blocked by a retained failure or stale object state that requires
    /// an explicit refresh, or when the bounded in-flight capacity is currently
    /// full. Callers may invoke this method again after applying another completion.
    pub fn ensure_observed(
        &mut self,
        session: &SessionId,
        path: impl Into<String>,
        depth: u32,
    ) -> Result<Option<PreparedRequest>, ClientError> {
        let path = path.into();
        wip_protocol::validate_path(&path).map_err(|error| {
            ClientError::new(ClientErrorKind::InvalidRequest, error.to_string())
        })?;
        let coverage = observation_coverage(self.session(session)?, &path, depth);
        if coverage != ObservationCoverage::Missing || self.pending.len() >= self.limits.in_flight {
            return Ok(None);
        }
        self.refresh_observed(session, path, depth).map(Some)
    }

    /// Ensures that an object's direct children have been observed without
    /// duplicating fresh or loading work.
    pub fn ensure_children_observed(
        &mut self,
        session: &SessionId,
        path: impl Into<String>,
    ) -> Result<Option<PreparedRequest>, ClientError> {
        self.ensure_observed(session, path, 1)
    }

    /// Explicitly refreshes only one object while preserving previously observed
    /// children in Known Space.
    pub fn refresh_object(
        &mut self,
        session: &SessionId,
        path: impl Into<String>,
    ) -> Result<PreparedRequest, ClientError> {
        self.refresh_observed(session, path, 0)
    }

    /// Explicitly refreshes one object and its direct children.
    pub fn refresh_children(
        &mut self,
        session: &SessionId,
        path: impl Into<String>,
    ) -> Result<PreparedRequest, ClientError> {
        self.refresh_observed(session, path, 1)
    }

    /// Explicitly starts or refreshes an object observation through `depth`.
    ///
    /// Unlike [`Client::ensure_observed`], this always creates a new request and
    /// supersedes older requests for the same root path.
    pub fn refresh_observed(
        &mut self,
        session: &SessionId,
        path: impl Into<String>,
        depth: u32,
    ) -> Result<PreparedRequest, ClientError> {
        self.ensure_capacity()?;
        let request = ObserveRequest {
            path: path.into(),
            depth,
        };
        let endpoint = session_endpoint(session)?;
        let encoded =
            encode_observe_request(&endpoint, &request, self.wire_limits).map_err(|error| {
                ClientError::new(ClientErrorKind::InvalidRequest, error.to_string())
            })?;
        let id = self.allocate_request_id();
        let maximum = self.limits.objects_per_session;
        let state = self.session_mut(session)?;
        ensure_entry_slot(state, &request.path, maximum)?;
        if request.depth > 0 {
            ensure_tree_slot(state, &request.path, maximum)?;
        }
        let entry = state
            .entries
            .entry(request.path.clone())
            .or_insert(EntryRecord {
                object_key: None,
                revision: id,
                state: ObservationState::Loading(id),
                confirmed_missing: false,
            });
        entry.revision = id;
        entry.state = ObservationState::Loading(id);
        if request.depth > 0 {
            let tree = state
                .trees
                .entry(request.path.clone())
                .or_insert(TreeObservation {
                    path: request.path.clone(),
                    children: Vec::new(),
                    revision: id,
                    state: ObservationState::Loading(id),
                });
            tree.revision = id;
            tree.state = ObservationState::Loading(id);
        } else if let Some(tree) = state.trees.get_mut(&request.path)
            && matches!(tree.state, ObservationState::Loading(_))
        {
            tree.state = ObservationState::Stale;
        }
        self.pending.insert(
            id,
            Pending::Observe {
                session: session.clone(),
                request,
            },
        );
        Ok(PreparedRequest {
            id,
            request: encoded,
        })
    }

    /// Ensures that an interface descriptor has been observed without issuing
    /// duplicate work or evicting another cached descriptor.
    ///
    /// Returns `None` when the descriptor is already fresh, is currently loading,
    /// is blocked by retained stale or failure state that requires an explicit
    /// refresh, or when either the interface cache or bounded in-flight capacity is
    /// currently full. Callers may invoke this method again after applying another
    /// completion.
    pub fn ensure_interface(
        &mut self,
        session: &SessionId,
        reference: InterfaceReference,
    ) -> Result<Option<PreparedRequest>, ClientError> {
        let state = self.session(session)?;
        if state.interfaces.contains_key(&reference)
            || state.interfaces.len() >= self.limits.interfaces_per_session
            || self.pending.len() >= self.limits.in_flight
        {
            return Ok(None);
        }
        self.prepare_interface(session, reference).map(Some)
    }

    /// Explicitly starts or refreshes an interface descriptor observation.
    ///
    /// Unlike [`Client::ensure_interface`], this always creates a new request and
    /// may evict an older non-loading descriptor when the interface cache is full.
    pub fn prepare_interface(
        &mut self,
        session: &SessionId,
        reference: InterfaceReference,
    ) -> Result<PreparedRequest, ClientError> {
        self.ensure_capacity()?;
        let request = FetchInterfaceRequest {
            interface: reference,
        };
        let endpoint = session_endpoint(session)?;
        let encoded = encode_fetch_interface_request(&endpoint, &request, self.wire_limits)
            .map_err(|error| {
                ClientError::new(ClientErrorKind::InvalidRequest, error.to_string())
            })?;
        let id = self.allocate_request_id();
        let maximum = self.limits.interfaces_per_session;
        let state = self.session_mut(session)?;
        ensure_interface_slot(state, &request.interface, maximum)?;
        let (scope_binding, binding_revision) =
            latest_scope_binding(state, &request.interface.scope);
        let touched = state.tick();
        let record = state
            .interfaces
            .entry(request.interface.clone())
            .or_insert(InterfaceRecord {
                observation: InterfaceObservation {
                    reference: request.interface.clone(),
                    scope_ref: None,
                    descriptor: None,
                    validator: None,
                    state: ObservationState::Loading(id),
                },
                revision: id,
                accepted_revision: RequestId(0),
                scope_binding,
                binding_revision,
                touched,
            });
        record.revision = id;
        record.observation.state = ObservationState::Loading(id);
        record.touched = touched;
        self.pending.insert(
            id,
            Pending::Interface {
                session: session.clone(),
                request,
            },
        );
        Ok(PreparedRequest {
            id,
            request: encoded,
        })
    }

    /// Constructs an operation from fresh object and interface observations.
    ///
    /// Membership, scope binding, both validators, the descriptor, and the operation
    /// declaration are fixed in the returned request's immutable [`CallContext`].
    /// The descriptor's optional `scope_ref` is copied unchanged, never substituted
    /// from a separate Object observation. No ancestor fetch is required. Without
    /// a scope ref, same-path replacement cannot always be detected.
    pub fn prepare_call(
        &mut self,
        session: &SessionId,
        path: &str,
        interface: &InterfaceReference,
        operation: impl Into<String>,
        arguments: BTreeMap<String, Value>,
    ) -> Result<PreparedRequest, ClientError> {
        self.ensure_capacity()?;
        let operation = operation.into();
        let context = {
            let state = self.session(session)?;
            let object_observation = state.object_observation(path).ok_or_else(|| {
                ClientError::new(
                    ClientErrorKind::MissingObservation,
                    "object is not observed",
                )
            })?;
            if object_observation.state != ObservationState::Fresh {
                return Err(ClientError::new(
                    ClientErrorKind::MissingObservation,
                    "object observation is not fresh",
                ));
            }
            let object = object_observation
                .object
                .as_ref()
                .expect("fresh object exists");
            if !object.interfaces.iter().any(|member| member == interface) {
                return Err(ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    "interface is not a member of the observed object",
                ));
            }
            let interface_observation = state
                .interfaces
                .get(interface)
                .map(|record| record.observation.clone())
                .ok_or_else(|| {
                    ClientError::new(
                        ClientErrorKind::MissingObservation,
                        "interface is not observed",
                    )
                })?;
            if interface_observation.state != ObservationState::Fresh {
                return Err(ClientError::new(
                    ClientErrorKind::MissingObservation,
                    "interface observation is not fresh",
                ));
            }
            let interface_revision = state.interfaces[interface].revision;
            let descriptor = interface_observation
                .descriptor
                .clone()
                .expect("fresh interface exists");
            descriptor
                .validate_arguments(&operation, &arguments)
                .map_err(|error| {
                    ClientError::new(ClientErrorKind::InvalidRequest, error.to_string())
                })?;
            let operation_declaration = descriptor
                .operations
                .iter()
                .find(|declaration| declaration.name == operation)
                .expect("validated operation exists")
                .clone();
            let request = CallOperationRequest {
                target: Target {
                    path: path.to_owned(),
                    validator: object.validator.clone(),
                },
                interface: InterfaceTarget {
                    reference: interface.clone(),
                    scope_ref: interface_observation.scope_ref.clone(),
                    validator: interface_observation.validator.clone(),
                },
                operation,
                arguments,
            };
            let object_key = state.entries[path]
                .object_key
                .expect("fresh entry has object key");
            CallContext {
                session: session.clone(),
                object_key,
                object: object_observation,
                interface: interface_observation,
                interface_revision,
                descriptor,
                operation: operation_declaration,
                request,
            }
        };
        let endpoint = session_endpoint(session)?;
        let encoded = encode_call_operation_request(
            &endpoint,
            &context.request,
            &context.descriptor,
            self.wire_limits,
        )
        .map_err(|error| ClientError::new(ClientErrorKind::InvalidRequest, error.to_string()))?;
        let id = self.allocate_request_id();
        self.session_mut(&context.session)?
            .pin_call_object(context.object_key);
        self.pending.insert(
            id,
            Pending::Call {
                context: Box::new(context),
                dispatched: false,
            },
        );
        Ok(PreparedRequest {
            id,
            request: encoded,
        })
    }

    /// Marks an operation request as handed to the transport.
    pub fn mark_dispatched(&mut self, id: RequestId) -> Result<(), ClientError> {
        match self.pending.get_mut(&id) {
            Some(Pending::Call { dispatched, .. }) => {
                *dispatched = true;
                Ok(())
            }
            Some(_) => Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "only operation calls have a dispatch boundary",
            )),
            None => Err(unknown_request()),
        }
    }

    /// Applies one HTTP response to the exact request identity that produced it.
    pub fn complete(
        &mut self,
        id: RequestId,
        response: Response<Vec<u8>>,
    ) -> Result<Completion, ClientError> {
        let pending = self.pending.remove(&id).ok_or_else(unknown_request)?;
        match pending {
            Pending::Observe { session, request } => {
                self.complete_observe(id, &session, request, response)
            }
            Pending::Interface { session, request } => {
                self.complete_interface(id, &session, request, response)
            }
            Pending::Call { context, .. } => {
                let session = context.session.clone();
                let object_key = context.object_key;
                let result = self.complete_call(id, *context, response);
                self.session_mut(&session)?.release_call_object(object_key);
                result
            }
        }
    }

    /// Records a transport failure for a request that produced no response.
    pub fn fail_transport(
        &mut self,
        id: RequestId,
        detail: impl Into<String>,
    ) -> Result<Completion, ClientError> {
        let detail = detail.into();
        let pending = self.pending.remove(&id).ok_or_else(unknown_request)?;
        match pending {
            Pending::Call {
                context,
                dispatched: true,
            } => {
                let session = context.session.clone();
                let object_key = context.object_key;
                let result = self.finish_unknown_call(
                    id,
                    *context,
                    OutcomeUnknownReason::Disconnect,
                    ClientError::new(ClientErrorKind::Transport, detail),
                );
                self.session_mut(&session)?.release_call_object(object_key);
                result
            }
            Pending::Call {
                context,
                dispatched: false,
            } => {
                let session = context.session.clone();
                let object_key = context.object_key;
                let record = CallRecord {
                    request_id: id,
                    context: *context,
                    outcome: CallOutcome::NotDispatched(ClientError::new(
                        ClientErrorKind::Transport,
                        detail,
                    )),
                };
                let maximum = self.limits.history;
                let state = self.session_mut(&session)?;
                state.push_call(record.clone(), maximum);
                state.release_call_object(object_key);
                Ok(Completion::Call(Box::new(record)))
            }
            other => {
                let session = other.session().clone();
                let is_current = match &other {
                    Pending::Observe { request, .. } => {
                        self.entry_request_is_current(&session, &request.path, id)
                    }
                    Pending::Interface { request, .. } => {
                        self.interface_request_is_current(&session, &request.interface, id)
                    }
                    Pending::Call { .. } => unreachable!("calls handled above"),
                };
                if !is_current {
                    return self.reject_stale(&session, id);
                }
                let error = ClientError::new(ClientErrorKind::Transport, detail);
                self.mark_retrieval_error(id, other, error.clone())?;
                self.push_failure_diagnostic(&session, id, &error)?;
                Err(error)
            }
        }
    }

    /// Records a timeout or disconnect after an operation was dispatched.
    pub fn fail_dispatched_call(
        &mut self,
        id: RequestId,
        failure: DispatchedTransportFailure,
        detail: impl Into<String>,
    ) -> Result<Completion, ClientError> {
        match self.pending.get(&id) {
            Some(Pending::Call {
                dispatched: true, ..
            }) => {}
            Some(_) => {
                return Err(ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    "request was not a dispatched operation call",
                ));
            }
            None => return Err(unknown_request()),
        }
        let Some(Pending::Call { context, .. }) = self.pending.remove(&id) else {
            unreachable!("validated dispatched call remains pending")
        };
        let reason = match failure {
            DispatchedTransportFailure::Timeout => OutcomeUnknownReason::Timeout,
            DispatchedTransportFailure::Disconnect => OutcomeUnknownReason::Disconnect,
        };
        let session = context.session.clone();
        let object_key = context.object_key;
        let result = self.finish_unknown_call(
            id,
            *context,
            reason,
            ClientError::new(ClientErrorKind::Transport, detail),
        );
        self.session_mut(&session)?.release_call_object(object_key);
        result
    }

    fn complete_observe(
        &mut self,
        id: RequestId,
        session: &SessionId,
        request: ObserveRequest,
        response: Response<Vec<u8>>,
    ) -> Result<Completion, ClientError> {
        if !self.entry_request_is_current(session, &request.path, id) {
            return self.reject_stale(session, id);
        }
        match decode_observe_response(&request, &response, self.wire_limits) {
            Ok(DecodedResponse::Success(value)) => {
                let maximum = self.limits.objects_per_session;
                let mut candidate = self.session(session)?.clone();
                if let Err(error) =
                    apply_observation(&mut candidate, &request.path, value, maximum, id)
                {
                    set_observe_subject_state(
                        self.session_mut(session)?,
                        &request,
                        id,
                        ObservationState::Error(error.clone()),
                    );
                    self.push_failure_diagnostic(session, id, &error)?;
                    return Err(error);
                }
                let observation = candidate
                    .object_observation(&request.path)
                    .expect("inserted observation exists");
                self.sessions.insert(session.clone(), candidate);
                Ok(Completion::Object(observation))
            }
            Ok(DecodedResponse::ProtocolFailure {
                error,
                status_mismatch,
            }) => {
                self.record_status_mismatch(session, id, status_mismatch)?;
                let observation_state = if error.code == ProtocolErrorCode::NotFound {
                    ObservationState::Stale
                } else {
                    ObservationState::Error(ClientError::new(
                        ClientErrorKind::ProtocolFailure,
                        error.to_string(),
                    ))
                };
                if error.code == ProtocolErrorCode::NotFound {
                    let state = self.session_mut(session)?;
                    retire_scope_subtree(state, &request.path, id);
                    mark_subtree_stale(state, &request.path, id);
                }
                set_observe_subject_state(
                    self.session_mut(session)?,
                    &request,
                    id,
                    observation_state,
                );
                Ok(Completion::ProtocolFailure(error))
            }
            Err(error) => {
                self.retrieval_decode_error(id, session, RetrievalSubject::Observe(request), error)
            }
        }
    }

    fn complete_interface(
        &mut self,
        id: RequestId,
        session: &SessionId,
        request: FetchInterfaceRequest,
        response: Response<Vec<u8>>,
    ) -> Result<Completion, ClientError> {
        if !self.interface_request_is_current(session, &request.interface, id) {
            return self.reject_stale(session, id);
        }
        match decode_fetch_interface_response(&request, &response, self.wire_limits) {
            Ok(DecodedResponse::Success(value)) => {
                let state = self.session_mut(session)?;
                // Newer scope evidence (Observe or another Interface name) can
                // supersede even the first fetch for this pair.
                let (scope_binding, binding_revision) =
                    latest_scope_binding(state, &request.interface.scope);
                if binding_revision > id
                    && value.scope_ref.is_some()
                    && value.scope_ref != scope_binding
                {
                    state
                        .interfaces
                        .get_mut(&request.interface)
                        .expect("interface exists")
                        .retire(binding_revision);
                    return self.reject_stale(session, id);
                }
                let existing_record = &state.interfaces[&request.interface];
                let existing = &existing_record.observation;
                let lost_wire_binding = existing.scope_ref.is_some() && value.scope_ref.is_none();
                // Omission stays omission in the public observation and call. A
                // known Object/sibling binding can still supply private lifetime
                // evidence unless this response explicitly lost its prior ref.
                let current_binding = if lost_wire_binding || value.scope_ref.is_some() {
                    value.scope_ref.clone()
                } else {
                    scope_binding
                };
                let same_lifetime = existing.scope_ref == value.scope_ref
                    || (current_binding.is_some()
                        && existing_record.scope_binding == current_binding);
                if let Some(descriptor) = &existing.descriptor
                    && same_lifetime
                    && ((existing.validator.is_none()
                        && (value.validator.is_some() || descriptor != &value.descriptor))
                        || (existing.validator.is_some()
                            && existing.validator == value.validator
                            && descriptor != &value.descriptor))
                {
                    let error = ClientError::new(
                        ClientErrorKind::InvalidResponse,
                        "interface representation changed for an immutable observation identity",
                    );
                    state
                        .interfaces
                        .get_mut(&request.interface)
                        .expect("interface exists")
                        .observation
                        .state = ObservationState::Error(error.clone());
                    self.push_failure_diagnostic(session, id, &error)?;
                    return Err(error);
                }
                update_scope_binding(
                    state,
                    &request.interface.scope,
                    current_binding.clone(),
                    id,
                    false,
                    Some(&request.interface),
                );
                let touched = state.tick();
                let observation = InterfaceObservation {
                    reference: value.interface,
                    scope_ref: value.scope_ref,
                    descriptor: Some(value.descriptor),
                    validator: value.validator,
                    state: ObservationState::Fresh,
                };
                let record = state
                    .interfaces
                    .get_mut(&request.interface)
                    .expect("interface exists");
                record.observation = observation.clone();
                record.accepted_revision = id;
                record.scope_binding = current_binding;
                record.binding_revision = id;
                record.touched = touched;
                Ok(Completion::Interface(observation))
            }
            Ok(DecodedResponse::ProtocolFailure {
                error,
                status_mismatch,
            }) => {
                self.record_status_mismatch(session, id, status_mismatch)?;
                self.session_mut(session)?
                    .interfaces
                    .get_mut(&request.interface)
                    .expect("interface exists")
                    .observation
                    .state = ObservationState::Error(ClientError::new(
                    ClientErrorKind::ProtocolFailure,
                    error.to_string(),
                ));
                Ok(Completion::ProtocolFailure(error))
            }
            Err(error) => self.retrieval_decode_error(
                id,
                session,
                RetrievalSubject::Interface(request.interface),
                error,
            ),
        }
    }

    fn complete_call(
        &mut self,
        id: RequestId,
        context: CallContext,
        response: Response<Vec<u8>>,
    ) -> Result<Completion, ClientError> {
        match decode_call_operation_response(
            &context.request,
            &context.descriptor,
            &response,
            self.wire_limits,
        ) {
            Ok(DecodedResponse::Success(value)) => {
                let session_id = context.session.clone();
                let maximum = self.limits.history;
                let state = self.session_mut(&session_id)?;
                if let Some(object) = state.objects.get_mut(&context.object_key)
                    && object.revision <= id
                {
                    if let Some(validator) = value.validator.clone() {
                        object.object.validator = Some(validator);
                    }
                    object.revision = id;
                }
                let record = CallRecord {
                    request_id: id,
                    context,
                    outcome: CallOutcome::Success(value),
                };
                state.push_call(record.clone(), maximum);
                Ok(Completion::Call(Box::new(record)))
            }
            Ok(DecodedResponse::ProtocolFailure {
                error,
                status_mismatch,
            }) => {
                let session_id = context.session.clone();
                self.record_status_mismatch(&session_id, id, status_mismatch)?;
                if error.code == ProtocolErrorCode::OperationOutcomeUnknown {
                    let maximum = self.limits.history;
                    let state = self.session_mut(&session_id)?;
                    state.mark_object_stale(context.object_key, id);
                    let record = CallRecord {
                        request_id: id,
                        context,
                        outcome: CallOutcome::Unknown {
                            reason: OutcomeUnknownReason::Protocol,
                            failure: None,
                        },
                    };
                    state.push_call(record.clone(), maximum);
                    return Ok(Completion::Call(Box::new(record)));
                }
                let maximum = self.limits.history;
                let state = self.session_mut(&session_id)?;
                match error.code {
                    ProtocolErrorCode::ValidatorRequired
                    | ProtocolErrorCode::ValidatorMismatch
                    | ProtocolErrorCode::NotFound => {
                        state.mark_object_stale(context.object_key, id);
                    }
                    ProtocolErrorCode::InterfaceMismatch => {
                        state.mark_object_stale(context.object_key, id);
                        if let Some(interface) =
                            state.interfaces.get_mut(&context.interface.reference)
                            && interface.revision == context.interface_revision
                        {
                            // The call cannot reuse this observation, but mismatch
                            // alone does not prove the scope lifetime ended. Retain
                            // its Descriptor for same-binding consistency checks.
                            interface.revision = interface.revision.max(id);
                            interface.observation.state = ObservationState::Stale;
                        }
                    }
                    ProtocolErrorCode::InterfaceValidatorRequired
                    | ProtocolErrorCode::InterfaceValidatorMismatch => {
                        if let Some(interface) = state
                            .interfaces
                            .get_mut(&context.request.interface.reference)
                            && interface.revision == context.interface_revision
                        {
                            interface.observation.state = ObservationState::Stale;
                        }
                    }
                    _ => {}
                }
                let record = CallRecord {
                    request_id: id,
                    context,
                    outcome: CallOutcome::ProtocolFailure(error),
                };
                state.push_call(record.clone(), maximum);
                Ok(Completion::Call(Box::new(record)))
            }
            Err(error) => {
                let failure = classify_response_error(error);
                self.finish_unknown_call(id, context, OutcomeUnknownReason::ResponseDecode, failure)
            }
        }
    }

    fn finish_unknown_call(
        &mut self,
        id: RequestId,
        context: CallContext,
        reason: OutcomeUnknownReason,
        failure: ClientError,
    ) -> Result<Completion, ClientError> {
        let session_id = context.session.clone();
        let maximum = self.limits.history;
        let state = self.session_mut(&session_id)?;
        state.mark_object_stale(context.object_key, id);
        state.push_diagnostic(
            Diagnostic {
                request_id: id,
                kind: DiagnosticKind::ClientFailure,
                detail: failure.to_string(),
            },
            maximum,
        );
        let record = CallRecord {
            request_id: id,
            context,
            outcome: CallOutcome::Unknown {
                reason,
                failure: Some(failure),
            },
        };
        state.push_call(record.clone(), maximum);
        Ok(Completion::Call(Box::new(record)))
    }

    fn retrieval_decode_error(
        &mut self,
        id: RequestId,
        session: &SessionId,
        subject: RetrievalSubject,
        error: ClientResponseError,
    ) -> Result<Completion, ClientError> {
        let error = classify_response_error(error);
        match subject {
            RetrievalSubject::Observe(request) => {
                set_observe_subject_state(
                    self.session_mut(session)?,
                    &request,
                    id,
                    ObservationState::Error(error.clone()),
                );
            }
            RetrievalSubject::Interface(reference) => {
                self.session_mut(session)?
                    .interfaces
                    .get_mut(&reference)
                    .expect("interface exists")
                    .observation
                    .state = ObservationState::Error(error.clone());
            }
        }
        self.push_failure_diagnostic(session, id, &error)?;
        Err(error)
    }

    fn mark_retrieval_error(
        &mut self,
        id: RequestId,
        pending: Pending,
        error: ClientError,
    ) -> Result<(), ClientError> {
        match pending {
            Pending::Observe { session, request } => {
                set_observe_subject_state(
                    self.session_mut(&session)?,
                    &request,
                    id,
                    ObservationState::Error(error),
                );
            }
            Pending::Interface { session, request } => {
                self.session_mut(&session)?
                    .interfaces
                    .get_mut(&request.interface)
                    .expect("interface exists")
                    .observation
                    .state = ObservationState::Error(error);
            }
            Pending::Call { .. } => unreachable!("handled above"),
        }
        Ok(())
    }

    fn record_status_mismatch(
        &mut self,
        session: &SessionId,
        id: RequestId,
        mismatch: Option<wip_http::StatusMismatch>,
    ) -> Result<(), ClientError> {
        if let Some(mismatch) = mismatch {
            let maximum = self.limits.history;
            self.session_mut(session)?.push_diagnostic(
                Diagnostic {
                    request_id: id,
                    kind: DiagnosticKind::HttpStatusMismatch,
                    detail: format!(
                        "received HTTP {}; canonical status is {}",
                        mismatch.actual, mismatch.expected
                    ),
                },
                maximum,
            );
        }
        Ok(())
    }

    fn push_failure_diagnostic(
        &mut self,
        session: &SessionId,
        id: RequestId,
        error: &ClientError,
    ) -> Result<(), ClientError> {
        let maximum = self.limits.history;
        self.session_mut(session)?.push_diagnostic(
            Diagnostic {
                request_id: id,
                kind: DiagnosticKind::ClientFailure,
                detail: error.to_string(),
            },
            maximum,
        );
        Ok(())
    }

    fn reject_stale(
        &mut self,
        session: &SessionId,
        id: RequestId,
    ) -> Result<Completion, ClientError> {
        let maximum = self.limits.history;
        self.session_mut(session)?.push_diagnostic(
            Diagnostic {
                request_id: id,
                kind: DiagnosticKind::StaleResponse,
                detail: "response superseded by a newer request".into(),
            },
            maximum,
        );
        Ok(Completion::StaleResponseRejected(id))
    }

    fn entry_request_is_current(&self, session: &SessionId, path: &str, id: RequestId) -> bool {
        matches!(
            self.sessions
                .get(session)
                .and_then(|state| state.entries.get(path))
                .map(|entry| &entry.state),
            Some(ObservationState::Loading(current)) if *current == id
        )
    }

    fn interface_request_is_current(
        &self,
        session: &SessionId,
        reference: &InterfaceReference,
        id: RequestId,
    ) -> bool {
        matches!(
            self.sessions
                .get(session)
                .and_then(|state| state.interfaces.get(reference))
                .map(|interface| &interface.observation.state),
            Some(ObservationState::Loading(current)) if *current == id
        )
    }

    fn ensure_capacity(&self) -> Result<(), ClientError> {
        if self.pending.len() >= self.limits.in_flight {
            Err(ClientError::new(
                ClientErrorKind::Capacity,
                "in-flight request capacity reached",
            ))
        } else {
            Ok(())
        }
    }

    fn allocate_request_id(&mut self) -> RequestId {
        let id = RequestId(self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        id
    }

    fn session(&self, id: &SessionId) -> Result<&Session, ClientError> {
        self.sessions.get(id).ok_or_else(|| {
            ClientError::new(ClientErrorKind::InvalidRequest, "unknown client session")
        })
    }

    fn session_mut(&mut self, id: &SessionId) -> Result<&mut Session, ClientError> {
        self.sessions.get_mut(id).ok_or_else(|| {
            ClientError::new(ClientErrorKind::InvalidRequest, "unknown client session")
        })
    }
}

#[derive(Debug)]
enum RetrievalSubject {
    Observe(ObserveRequest),
    Interface(InterfaceReference),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationCoverage {
    Missing,
    Loading,
    Covered,
    Blocked,
}

fn observation_coverage(state: &Session, path: &str, depth: u32) -> ObservationCoverage {
    let Some(entry) = state.entries.get(path) else {
        return ObservationCoverage::Missing;
    };
    match &entry.state {
        ObservationState::Loading(_) => return ObservationCoverage::Loading,
        ObservationState::Stale | ObservationState::Error(_) => {
            return ObservationCoverage::Blocked;
        }
        ObservationState::Fresh => {}
    }
    if entry
        .object_key
        .is_none_or(|key| !state.objects.contains_key(&key))
    {
        return ObservationCoverage::Missing;
    }
    if depth == 0 {
        return ObservationCoverage::Covered;
    }

    let Some(tree) = state.trees.get(path) else {
        return ObservationCoverage::Missing;
    };
    match &tree.state {
        ObservationState::Loading(_) => return ObservationCoverage::Loading,
        ObservationState::Stale => return ObservationCoverage::Missing,
        ObservationState::Error(_) => return ObservationCoverage::Blocked,
        ObservationState::Fresh => {}
    }

    let mut saw_loading = false;
    let mut saw_missing = false;
    for child in &tree.children {
        match observation_coverage(state, child, depth - 1) {
            ObservationCoverage::Blocked => return ObservationCoverage::Blocked,
            ObservationCoverage::Loading => saw_loading = true,
            ObservationCoverage::Missing => saw_missing = true,
            ObservationCoverage::Covered => {}
        }
    }
    if saw_loading {
        ObservationCoverage::Loading
    } else if saw_missing {
        ObservationCoverage::Missing
    } else {
        ObservationCoverage::Covered
    }
}

fn session_endpoint(session: &SessionId) -> Result<Endpoint, ClientError> {
    Endpoint::parse(&session.endpoint)
        .map_err(|error| ClientError::new(ClientErrorKind::InvalidRequest, error.to_string()))
}

fn unknown_request() -> ClientError {
    ClientError::new(ClientErrorKind::InvalidRequest, "unknown request identity")
}

fn classify_response_error(error: ClientResponseError) -> ClientError {
    match error {
        ClientResponseError::InvalidResponse { .. } => {
            ClientError::new(ClientErrorKind::InvalidResponse, error.to_string())
        }
        ClientResponseError::UnsupportedDescriptorFormat { .. } => ClientError::new(
            ClientErrorKind::UnsupportedDescriptorFormat,
            error.to_string(),
        ),
        ClientResponseError::TransportBinding(_) => {
            ClientError::new(ClientErrorKind::Transport, error.to_string())
        }
    }
}

fn set_observe_subject_state(
    state: &mut Session,
    request: &ObserveRequest,
    revision: RequestId,
    observation_state: ObservationState,
) {
    if request.depth > 0
        && let Some(tree) = state.trees.get_mut(&request.path)
        && tree.revision == revision
    {
        tree.state = observation_state.clone();
    }
    if let Some(entry) = state.entries.get_mut(&request.path)
        && entry.revision == revision
    {
        entry.state = observation_state;
    }
}

fn ensure_interface_slot(
    state: &mut Session,
    reference: &InterfaceReference,
    maximum: usize,
) -> Result<(), ClientError> {
    if state.interfaces.contains_key(reference) || state.interfaces.len() < maximum {
        return Ok(());
    }
    let candidate = state
        .interfaces
        .iter()
        .filter(|(_, record)| !matches!(record.observation.state, ObservationState::Loading(_)))
        .min_by_key(|(_, record)| record.touched)
        .map(|(reference, _)| reference.clone())
        .ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Capacity,
                "all interface cache entries are loading",
            )
        })?;
    state.interfaces.remove(&candidate);
    Ok(())
}

fn ensure_entry_slot(state: &mut Session, path: &str, maximum: usize) -> Result<(), ClientError> {
    if state.entries.contains_key(path) || state.entries.len() < maximum {
        return Ok(());
    }
    let candidate = state
        .entries
        .iter()
        .find(|(path, entry)| {
            !matches!(entry.state, ObservationState::Loading(_))
                && !entry.object_key.is_some_and(|key| {
                    state
                        .objects
                        .get(&key)
                        .is_some_and(|record| record.call_owners > 0)
                })
                && !state
                    .trees
                    .get(*path)
                    .is_some_and(|tree| matches!(tree.state, ObservationState::Loading(_)))
        })
        .map(|(path, _)| path.clone())
        .ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Capacity,
                "all entry observations are owned by in-flight work",
            )
        })?;
    remove_entry(state, &candidate);
    Ok(())
}

fn ensure_tree_slot(state: &mut Session, path: &str, maximum: usize) -> Result<(), ClientError> {
    if state.trees.contains_key(path) || state.trees.len() < maximum {
        return Ok(());
    }
    let candidate = state
        .trees
        .iter()
        .find(|(_, tree)| !matches!(tree.state, ObservationState::Loading(_)))
        .map(|(path, _)| path.clone())
        .ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Capacity,
                "all tree observations are loading",
            )
        })?;
    state.trees.remove(&candidate);
    Ok(())
}

fn remove_entry(state: &mut Session, path: &str) {
    let object_key = state
        .entries
        .remove(path)
        .and_then(|entry| entry.object_key);
    state.trees.remove(path);
    for tree in state.trees.values_mut() {
        tree.children.retain(|child| child != path);
    }
    if let Some(key) = object_key
        && !state
            .entries
            .values()
            .any(|entry| entry.object_key == Some(key))
    {
        remove_object(state, key);
    }
}

fn object_ownership_revision(state: &Session, key: u64) -> Option<RequestId> {
    state
        .entries
        .values()
        .filter(|entry| entry.object_key == Some(key))
        .map(|entry| entry.revision)
        .chain(state.objects.get(&key).map(|record| record.revision))
        .max()
}

fn validate_object_consistency(state: &Session, object: &Object) -> Result<(), ClientError> {
    let Some(reference) = &object.r#ref else {
        return Ok(());
    };
    let Some(existing) = state
        .refs
        .get(reference)
        .and_then(|key| state.objects.get(key))
    else {
        return Ok(());
    };
    if existing.object.validator.is_some()
        && existing.object.validator == object.validator
        && existing.object != *object
    {
        return Err(ClientError::new(
            ClientErrorKind::InvalidResponse,
            "object representation changed for the same ref and validator",
        ));
    }
    Ok(())
}

fn insert_object(
    state: &mut Session,
    path: &str,
    object: Object,
    maximum: usize,
    revision: RequestId,
) -> Result<u64, ClientError> {
    validate_object_consistency(state, &object)?;
    ensure_entry_slot(state, path, maximum)?;
    let existing_path_entry = state.entries.get(path).cloned();
    let existing_path_key = existing_path_entry
        .as_ref()
        .and_then(|entry| entry.object_key);
    let previous_scope_ref = existing_path_entry
        .as_ref()
        .filter(|entry| !entry.confirmed_missing)
        .and_then(|entry| entry.object_key)
        .and_then(|key| state.objects.get(&key))
        .and_then(|record| record.object.r#ref.clone());
    let replaceable_key = existing_path_key.filter(|key| {
        state
            .objects
            .get(key)
            .is_none_or(|record| record.call_owners == 0)
            && !state
                .entries
                .iter()
                .any(|(entry_path, entry)| entry_path != path && entry.object_key == Some(*key))
    });
    let key = if let Some(reference) = &object.r#ref {
        if let Some(key) = state.refs.get(reference).copied() {
            key
        } else {
            make_object_slot(state, maximum, replaceable_key)?
        }
    } else if let Some(key) = existing_path_key
        && state
            .objects
            .get(&key)
            .is_some_and(|record| record.object.r#ref.is_none())
    {
        key
    } else {
        make_object_slot(state, maximum, replaceable_key)?
    };

    if let Some(old_key) = existing_path_key
        && old_key != key
        && !state
            .entries
            .iter()
            .any(|(entry_path, entry)| entry_path != path && entry.object_key == Some(old_key))
    {
        remove_object(state, old_key);
    }
    if let Some(reference) = &object.r#ref {
        state.refs.insert(reference.clone(), key);
    }
    let touched = state.tick();
    let ownership_revision = object_ownership_revision(state, key).unwrap_or(revision);
    let suppressed = ownership_revision > revision;
    let call_owners = state
        .objects
        .get(&key)
        .map_or(0, |record| record.call_owners);
    match state.objects.get_mut(&key) {
        Some(existing) if suppressed => {
            existing.revision = ownership_revision;
            existing.touched = touched;
        }
        _ => {
            state.objects.insert(
                key,
                ObjectRecord {
                    object,
                    revision,
                    call_owners,
                    touched,
                },
            );
        }
    }
    let entry = if suppressed {
        match existing_path_entry {
            Some(mut entry) if entry.object_key == Some(key) => {
                entry.revision = ownership_revision;
                if matches!(entry.state, ObservationState::Loading(current) if current == revision)
                {
                    entry.state = ObservationState::Fresh;
                }
                entry
            }
            _ => EntryRecord {
                object_key: Some(key),
                revision: ownership_revision,
                state: ObservationState::Stale,
                confirmed_missing: false,
            },
        }
    } else {
        EntryRecord {
            object_key: Some(key),
            revision,
            state: ObservationState::Fresh,
            confirmed_missing: false,
        }
    };
    state.entries.insert(path.to_owned(), entry);
    if !suppressed {
        let scope_ref = state.objects[&key].object.r#ref.clone();
        let changed = previous_scope_ref.is_some() && previous_scope_ref != scope_ref;
        update_scope_binding(state, path, scope_ref, revision, changed, None);
    }
    Ok(key)
}

fn make_object_slot(
    state: &mut Session,
    maximum: usize,
    replaceable: Option<u64>,
) -> Result<u64, ClientError> {
    if state.objects.len() >= maximum {
        let candidate = replaceable.or_else(|| {
            state
                .objects
                .iter()
                .filter(|(key, record)| {
                    record.call_owners == 0
                        && !state.entries.iter().any(|(path, entry)| {
                            entry.object_key == Some(**key)
                                && (matches!(entry.state, ObservationState::Loading(_))
                                    || state.trees.get(path).is_some_and(|tree| {
                                        matches!(tree.state, ObservationState::Loading(_))
                                    }))
                        })
                })
                .min_by_key(|(_, record)| record.touched)
                .map(|(key, _)| *key)
        });
        let candidate = candidate.ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Capacity,
                "all object cache entries are owned by in-flight work",
            )
        })?;
        remove_object(state, candidate);
    }
    let key = state.next_object_key;
    state.next_object_key = state.next_object_key.saturating_add(1);
    Ok(key)
}

fn remove_object(state: &mut Session, key: u64) {
    if state
        .objects
        .get(&key)
        .is_some_and(|record| record.call_owners > 0)
    {
        return;
    }
    if let Some(record) = state.objects.remove(&key)
        && let Some(reference) = record.object.r#ref
        && state.refs.get(&reference) == Some(&key)
    {
        state.refs.remove(&reference);
    }
    let removed_paths: Vec<String> = state
        .entries
        .iter()
        .filter(|(_, entry)| entry.object_key == Some(key))
        .map(|(path, _)| path.clone())
        .collect();
    state
        .entries
        .retain(|_, entry| entry.object_key != Some(key));
    let removable_paths: Vec<&String> = removed_paths
        .iter()
        .filter(|path| {
            !state
                .trees
                .get(*path)
                .is_some_and(|tree| matches!(tree.state, ObservationState::Loading(_)))
        })
        .collect();
    for path in &removable_paths {
        state.trees.remove(*path);
    }
    for tree in state.trees.values_mut() {
        tree.children.retain(|path| {
            !removable_paths
                .iter()
                .any(|removed| removed.as_str() == path)
        });
    }
}

fn reserve_observation_metadata(
    state: &mut Session,
    paths: &[String],
    observed_children_paths: &[String],
    maximum: usize,
) -> Result<(), ClientError> {
    while state.entries.len()
        + paths
            .iter()
            .filter(|path| !state.entries.contains_key(*path))
            .count()
        > maximum
    {
        let candidate = state
            .entries
            .iter()
            .find(|(path, entry)| {
                !paths.contains(path)
                    && !matches!(entry.state, ObservationState::Loading(_))
                    && !entry.object_key.is_some_and(|key| {
                        state
                            .objects
                            .get(&key)
                            .is_some_and(|record| record.call_owners > 0)
                    })
                    && !state
                        .trees
                        .get(*path)
                        .is_some_and(|tree| matches!(tree.state, ObservationState::Loading(_)))
            })
            .map(|(path, _)| path.clone())
            .ok_or_else(|| {
                ClientError::new(
                    ClientErrorKind::Capacity,
                    "tree response cannot evict entries owned by in-flight work",
                )
            })?;
        remove_entry(state, &candidate);
    }
    while state.trees.len()
        + observed_children_paths
            .iter()
            .filter(|path| !state.trees.contains_key(*path))
            .count()
        > maximum
    {
        let candidate = state
            .trees
            .iter()
            .find(|(path, tree)| {
                !observed_children_paths.contains(path)
                    && !matches!(tree.state, ObservationState::Loading(_))
            })
            .map(|(path, _)| path.clone())
            .ok_or_else(|| {
                ClientError::new(
                    ClientErrorKind::Capacity,
                    "tree response cannot evict loading tree observations",
                )
            })?;
        state.trees.remove(&candidate);
    }
    Ok(())
}

// Only retained observations contribute scope evidence. No unbounded scope
// tombstones or synthetic identity are needed: evidence is kept with bounded
// Interface records, survives Object eviction, and disappears with cache cleanup.
fn latest_scope_binding(state: &Session, scope: &str) -> (Option<String>, RequestId) {
    let object_binding = state
        .entries
        .get(scope)
        .filter(|entry| !entry.confirmed_missing)
        .and_then(|entry| entry.object_key)
        .and_then(|key| state.objects.get(&key))
        .map(|record| (record.object.r#ref.clone(), record.revision));
    state
        .interfaces
        .iter()
        .filter(|(reference, _)| reference.scope == scope)
        .map(|(_, record)| (record.scope_binding.clone(), record.binding_revision))
        .chain(object_binding)
        .max_by_key(|(_, revision)| *revision)
        .unwrap_or((None, RequestId(0)))
}

fn update_scope_binding(
    state: &mut Session,
    scope: &str,
    scope_ref: Option<String>,
    revision: RequestId,
    confirmed_change: bool,
    except: Option<&InterfaceReference>,
) {
    for (reference, interface) in &mut state.interfaces {
        if reference.scope != scope
            || except == Some(reference)
            || interface.accepted_revision > revision
            || interface.binding_revision > revision
        {
            continue;
        }
        if confirmed_change
            || (interface.scope_binding.is_some() && interface.scope_binding != scope_ref)
        {
            interface.retire(revision);
        }
        // Corroboration is not wire metadata: never update observation.scope_ref
        // on a retained Descriptor, even when the first known ref is learned.
        interface.scope_binding = scope_ref.clone();
        interface.binding_revision = revision;
    }
}

fn is_subtree_path(root: &str, path: &str) -> bool {
    root == "/"
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

// Direct NotFound confirms that this scope path is no longer available. Edge
// omission and local eviction deliberately do not call this function.
fn retire_scope_subtree(state: &mut Session, root: &str, revision: RequestId) {
    for (path, entry) in &mut state.entries {
        if is_subtree_path(root, path) && entry.revision <= revision {
            entry.confirmed_missing = true;
        }
    }
    state.interfaces.retain(|reference, record| {
        !is_subtree_path(root, &reference.scope) || record.accepted_revision > revision
    });
}

fn mark_subtree_stale(state: &mut Session, root: &str, request_id: RequestId) {
    for (path, entry) in &mut state.entries {
        if is_subtree_path(root, path) && entry.revision <= request_id {
            entry.revision = request_id;
            entry.state = ObservationState::Stale;
        }
    }
    for (path, tree) in &mut state.trees {
        if is_subtree_path(root, path) && tree.revision <= request_id {
            tree.revision = request_id;
            tree.state = ObservationState::Stale;
        }
    }
}

fn apply_observation(
    state: &mut Session,
    root_path: &str,
    response: ProtocolObjectObservation,
    maximum: usize,
    request_id: RequestId,
) -> Result<(), ClientError> {
    let mut nodes = Vec::new();
    flatten_observation(root_path, &response, &mut nodes);
    if nodes.len() > maximum {
        return Err(ClientError::new(
            ClientErrorKind::Capacity,
            "observation response exceeds the local observation bound",
        ));
    }
    let paths: Vec<String> = nodes.iter().map(|(path, _, _)| path.clone()).collect();
    let observed_children_paths: Vec<String> = nodes
        .iter()
        .filter_map(|(path, _, children)| children.as_ref().map(|_| path.clone()))
        .collect();
    reserve_observation_metadata(state, &paths, &observed_children_paths, maximum)?;

    for (path, _, children) in &nodes {
        if children.is_none()
            && let Some(tree) = state.trees.get_mut(path)
            && tree.revision < request_id
            && matches!(tree.state, ObservationState::Loading(_))
        {
            tree.state = ObservationState::Stale;
        }
    }

    let mut omitted = Vec::new();
    for (path, _, children) in &nodes {
        let Some(children) = children else {
            continue;
        };
        if let Some(previous) = state
            .trees
            .get(path)
            .filter(|previous| previous.revision <= request_id)
        {
            omitted.extend(
                previous
                    .children
                    .iter()
                    .filter(|child| !children.contains(child))
                    .cloned(),
            );
        }
    }
    for path in omitted {
        mark_subtree_stale(state, &path, request_id);
    }

    for (path, object, _) in &nodes {
        validate_object_consistency(state, object)?;
        let newer_owner = state
            .entries
            .get(path)
            .is_some_and(|entry| entry.revision > request_id);
        if !newer_owner {
            insert_object(state, path, object.clone(), maximum, request_id)?;
        }
    }
    for (path, _, children) in nodes {
        let Some(children) = children else {
            continue;
        };
        let newer_owner = state
            .trees
            .get(&path)
            .is_some_and(|tree| tree.revision > request_id);
        if newer_owner {
            continue;
        }
        state.trees.insert(
            path.clone(),
            TreeObservation {
                path,
                children,
                revision: request_id,
                state: ObservationState::Fresh,
            },
        );
    }
    Ok(())
}

fn flatten_observation(
    path: &str,
    observation: &ProtocolObjectObservation,
    output: &mut Vec<(String, Object, Option<Vec<String>>)>,
) {
    let children = observation.children.as_ref().map(|children| {
        children
            .iter()
            .map(|child| child_path(path, &child.object.name))
            .collect::<Vec<_>>()
    });
    output.push((
        path.to_owned(),
        observation.object.clone(),
        children.clone(),
    ));
    if let (Some(observations), Some(paths)) = (&observation.children, children) {
        for (child, child_path) in observations.iter().zip(paths) {
            flatten_observation(&child_path, child, output);
        }
    }
}

fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn push_bounded<T>(history: &mut VecDeque<T>, value: T, maximum: usize) {
    if history.len() == maximum {
        history.pop_front();
    }
    history.push_back(value);
}
