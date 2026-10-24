//! Offline support for the approved, attachment-free stopped Worker fixture.
//!
//! Enabled only by `test-support`. This is not an import API: extraction accepts a
//! current-schema, settled, single-Segment aggregate; installation accepts only a
//! fresh Runtime directory containing its newly initialized `auth.toml`. No
//! Runtime or execution backend is constructed. Original Session bytes are never
//! rewritten. The caller must separately review/attest the artificial conversation;
//! structural checks and secret-marker checks cannot prove arbitrary prose safe.

use super::*;
use crate::catalog::ProfileSourceArchiveSource;
use crate::workspace_issuer::{
    WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState, validate_workspace_issuer_trust_records,
};
use session_store::{
    SegmentId, SessionId, WorkerActiveSegmentRef, WorkerMetadata, WorkerSessionStore,
};

#[cfg(test)]
#[path = "test_support_tests.rs"]
mod tests;

const TEMPLATE_VERSION: u32 = 1;
const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;

/// Portable fixture, intentionally without Debug (Session material is private).
/// Fields are private and every deserialized template is validated on install.
/// Serialize this value to retain the fixture; do not serialize its source aggregate.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoppedWorkerTemplate {
    template_version: u32,
    runtime_schema_version: u32,
    runtime_id: String,
    workspace_id: String,
    worker_id: WorkerId,
    created_at_ms: Option<u64>,
    session_id: SessionId,
    segment_id: SegmentId,
    session_manifest: Vec<u8>,
    segment_jsonl: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionManifest {
    schema_version: u32,
    session_id: SessionId,
}

// Read-only projection: never retain source key material in a template.
#[derive(Deserialize)]
struct AuthIdentity {
    identity_id: String,
}

#[derive(Deserialize)]
struct SourceAuth {
    identity: AuthIdentity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializedAuth {
    identity: crate::auth::RuntimeIdentityMaterial,
    workspace_issuers: Vec<WorkspaceIssuerTrustRecord>,
    #[serde(default)]
    archived_legacy_issuers: Vec<String>,
}

#[derive(Deserialize)]
struct GuardState {
    active_request_id: Option<String>,
    owners: BTreeMap<String, GuardOwner>,
}

#[derive(Deserialize)]
struct GuardOwner {
    receipt: Option<serde::de::IgnoredAny>,
}

fn reject(message: &'static str) -> RuntimeError {
    RuntimeError::InvalidRequest(format!("stopped Worker fixture: {message}"))
}

fn io<T>(result: std::io::Result<T>) -> Result<T, RuntimeError> {
    result.map_err(|_| reject("filesystem operation failed"))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, RuntimeError> {
    serde_json::from_slice(bytes).map_err(|_| reject("invalid typed document"))
}

fn no_symlink_components(path: &Path) -> Result<(), RuntimeError> {
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, std::path::Component::ParentDir) {
            return Err(reject("parent traversal is unsupported"));
        }
        current.push(part);
        let metadata = io(fs::symlink_metadata(&current))?;
        if metadata.file_type().is_symlink() {
            return Err(reject("symlinks are unsupported"));
        }
    }
    Ok(())
}

fn check_tree(path: &Path, budget: &mut usize) -> Result<(), RuntimeError> {
    if *budget == 0 {
        return Err(reject("file count exceeds fixture limit"));
    }
    *budget -= 1;
    let metadata = io(fs::symlink_metadata(path))?;
    if metadata.is_dir() {
        for entry in io(fs::read_dir(path))? {
            check_tree(&io(entry)?.path(), budget)?;
        }
    } else if !metadata.is_file() {
        return Err(reject("symlinks and special files are unsupported"));
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<(), RuntimeError> {
    no_symlink_components(path)?;
    if !io(fs::symlink_metadata(path))?.is_file() {
        return Err(reject("expected a regular file"));
    }
    Ok(())
}

fn bytes(path: &Path) -> Result<Vec<u8>, RuntimeError> {
    regular_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOATIME | libc::O_NOFOLLOW);
    }
    let file = io(options.open(path))?;
    let mut result = Vec::new();
    io(file
        .take(MAX_SESSION_BYTES as u64 + 1)
        .read_to_end(&mut result))?;
    if result.len() > MAX_SESSION_BYTES {
        return Err(reject("file exceeds fixture limit"));
    }
    Ok(result)
}

fn text(path: &Path) -> Result<String, RuntimeError> {
    String::from_utf8(bytes(path)?).map_err(|_| reject("invalid UTF-8 document"))
}

fn portable_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
}

impl StoppedWorkerTemplate {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }
    pub fn worker_name(&self) -> String {
        runtime_worker_name(self.worker_id)
    }
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
    pub fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    /// Extract without migration, rewriting metadata, or copying credentials,
    /// resolved manifests, execution-operation receipts, or run directories.
    /// The source Runtime must be offline; its existing owner lock is held for
    /// the entire read. The path must be `<runtime>/workers/<WorkerId>`.
    pub fn extract(worker_dir: &Path) -> Result<Self, RuntimeError> {
        no_symlink_components(worker_dir)?;
        let workers = worker_dir
            .parent()
            .ok_or_else(|| reject("invalid aggregate path"))?;
        if workers.file_name().and_then(|s| s.to_str()) != Some(WORKERS_DIR) {
            return Err(reject("invalid aggregate path"));
        }
        let root = workers
            .parent()
            .ok_or_else(|| reject("invalid Runtime path"))?;
        // Require the existing lock, rather than creating a file in the source.
        let lock_path = root.with_file_name(format!(
            ".{}.runtime-owner.lock",
            root.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| reject("invalid Runtime path"))?
        ));
        regular_file(&lock_path)?;
        let lock = io(OpenOptions::new().read(true).open(lock_path))?;
        if !io(FileExt::try_lock_shared(&lock))? {
            return Err(reject("source Runtime is not offline"));
        }
        let _lock = RuntimeStoreOwnerLock { file: lock };
        check_tree(worker_dir, &mut 256)?;
        let snapshot: RuntimeSnapshot = decode(&bytes(&root.join(RUNTIME_FILE))?)?;
        snapshot
            .validate(&root.join(RUNTIME_FILE))
            .map_err(|_| reject("unsupported Runtime schema"))?;
        if !snapshot.subject_create_recovery_fences.is_empty()
            || !snapshot.subject_releases.is_empty()
        {
            return Err(reject("source has lifecycle operations"));
        }
        let record: WorkerAggregateRecord = decode(&bytes(&worker_dir.join(WORKER_FILE))?)?;
        let record = record
            .validate(&worker_dir.join(WORKER_FILE))
            .map_err(|_| reject("invalid Worker schema or identity"))?;
        let guard: GuardState = decode(
            &serde_json::to_vec(&record.restore_guard)
                .map_err(|_| reject("invalid restore guard"))?,
        )?;
        if record.status != WorkerStatus::Stopped
            || guard.active_request_id.is_some()
            || guard.owners.values().any(|owner| owner.receipt.is_none())
        {
            return Err(reject("Worker is not settled and stopped"));
        }
        let PersistedWorkerExecutionState::Available(execution) = &record.execution_state else {
            return Err(reject("pending or unavailable execution metadata"));
        };
        // A stopped aggregate may retain the empty historical binding marker.
        // It is deliberately not copied to the installed Worker.
        if execution.restore_intent != WorkerRestoreIntent::Explicit {
            return Err(reject("Worker requires explicit restore"));
        }
        if !record.workdir_attachments.is_empty() || !record.logical_workdir_attachments.is_empty()
        {
            return Err(reject("attachments are unsupported"));
        }
        check_request(&execution.request, false)?;
        let workspace_id = record
            .workspace_id
            .ok_or_else(|| reject("missing Workspace identity"))?;
        let metadata: WorkerMetadata = decode(&bytes(&worker_dir.join(WORKER_METADATA_FILE))?)?;
        crate::worker_backend::validate_fixture_restore_metadata(&metadata, &execution.request)
            .map_err(|_| reject("invalid source restore Manifest or settings"))?;
        if metadata.worker_name != runtime_worker_name(record.worker_id)
            || metadata.workspace_id.as_deref() != Some(&workspace_id)
            || !metadata.spawned_children.is_empty()
            || !metadata.reclaimed_children.is_empty()
            || !metadata.peers.is_empty()
            || metadata.subjektiv_session_attribution.is_some()
        {
            return Err(reject("unsupported or inconsistent Worker metadata"));
        }
        let active = metadata
            .active
            .ok_or_else(|| reject("missing active Session"))?;
        let segment_id = active
            .segment_id
            .ok_or_else(|| reject("unmaterialized Session"))?;
        let source_auth: SourceAuth = toml::from_str(&text(&root.join("auth.toml"))?)
            .map_err(|_| reject("missing Runtime identity"))?;
        let session = worker_dir.join("session");
        let fixture = Self {
            template_version: TEMPLATE_VERSION,
            runtime_schema_version: SCHEMA_VERSION,
            runtime_id: source_auth.identity.identity_id,
            workspace_id,
            worker_id: record.worker_id,
            created_at_ms: record.created_at_ms,
            session_id: active.session_id,
            segment_id,
            session_manifest: bytes(&session.join("session.json"))?,
            segment_jsonl: bytes(&session.join("segments").join(format!("{segment_id}.jsonl")))?,
        };
        fixture.validate_shape()?;
        fixture.validate_session(&session)?;
        Ok(fixture)
    }

    /// Install after normal identity/trust initialization, before Runtime start.
    /// `request` and `manifest` supply NEW backend/config/profile references and
    /// freshly resolved policy, not a launch. The saved source Manifest is never
    /// reused. `manifest.worker.name` must equal `worker_name()`.
    /// Only `auth.toml` may already exist. The owner lock fences live Runtimes;
    /// staged files are synced and published without overwriting any Worker.
    /// An interrupted publication fails closed as an incomplete Runtime layout.
    pub fn install(
        &self,
        options: &FsRuntimeStoreOptions,
        request: CreateWorkerRequest,
        manifest: &manifest::WorkerManifest,
    ) -> Result<(), RuntimeError> {
        self.validate_shape()?;
        check_request(&request, true)?;
        if manifest.worker.name != self.worker_name() {
            return Err(reject("resolved Manifest Worker name mismatch"));
        }
        let mut metadata = WorkerMetadata::new(
            self.worker_name(),
            Some(WorkerActiveSegmentRef {
                session_id: self.session_id,
                segment_id: Some(self.segment_id),
            }),
        )
        .with_workspace_id(&self.workspace_id);
        metadata.resolved_manifest_snapshot = Some(
            manifest::write_persisted_worker_manifest_snapshot(manifest)
                .map_err(|_| reject("invalid resolved Manifest"))?,
        );
        crate::worker_backend::validate_fixture_restore_metadata(&metadata, &request)
            .map_err(|_| reject("resolved Manifest does not match fresh request settings"))?;
        if options.runtime_id != self.runtime_id
            || request.worker_id != self.worker_id
            || request
                .workspace_api
                .as_ref()
                .map(|api| api.workspace_id.as_str())
                != Some(&self.workspace_id)
        {
            return Err(reject("installation identity mismatch"));
        }
        no_symlink_components(&options.root)?;
        check_tree(&options.root, &mut 256)?;
        // Validate the lock location before the ordinary owner-lock opener.
        let lock_path = options.root.with_file_name(format!(
            ".{}.runtime-owner.lock",
            options
                .root
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| reject("invalid Runtime path"))?
        ));
        if fs::symlink_metadata(&lock_path).is_ok() {
            regular_file(&lock_path)?;
        }
        let lock = acquire_runtime_store_owner_lock(&options.root)?;
        validate_fresh_runtime_store_layout(&options.root)
            .map_err(|_| reject("destination is not fresh; refusing overwrite"))?;
        let auth: InitializedAuth = toml::from_str(&text(&options.root.join("auth.toml"))?)
            .map_err(|_| reject("initialize Runtime identity and trust before install"))?;
        if auth.identity.identity_id != self.runtime_id
            || !auth.archived_legacy_issuers.is_empty()
            || auth.identity.signing_key().is_err()
        {
            return Err(reject("invalid destination Runtime identity"));
        }
        validate_workspace_issuer_trust_records(&auth.workspace_issuers)
            .map_err(|_| reject("invalid destination Workspace trust"))?;
        let api = request
            .workspace_api
            .as_ref()
            .expect("validated Workspace binding");
        if !auth.workspace_issuers.iter().any(|trust| {
            trust.workspace_id == self.workspace_id
                && trust.backend_url == api.base_url
                && trust.state == WorkspaceIssuerTrustState::Active
        }) {
            return Err(reject("request does not match initialized Workspace trust"));
        }
        let stage = options
            .root
            .join(format!(".fixture-{}", uuid::Uuid::now_v7()));
        io(fs::create_dir(&stage))?;
        let store = FsRuntimeStore {
            root: stage.clone(),
            _owner_lock: Some(lock),
            worker_write_fault: Arc::new(AtomicU8::new(AtomicWriteFault::None as u8)),
        };
        let result = (|| {
            let worker = PersistedWorkerRecord {
                restore_guard: crate::runtime::RestoreGuard::default(),
                worker_ref: WorkerRef::new(self.worker_id),
                worker_id: self.worker_id,
                profile: request.profile.clone(),
                display_name: request.display_name.clone(),
                profile_source: request.profile_source.reference(),
                config_bundle: request.config_bundle.clone(),
                created_at_ms: self.created_at_ms,
                status: WorkerStatus::Stopped,
                last_finished_submission_request_id: None,
                workspace_id: Some(self.workspace_id.clone()),
                workdir_attachments: Vec::new(),
                logical_workdir_attachments: Vec::new(),
                execution_state: PersistedWorkerExecutionState::Available(
                    PersistedWorkerExecution {
                        request,
                        binding: None,
                        restore_intent: WorkerRestoreIntent::Explicit,
                    },
                ),
            };
            WorkerAggregateRecord::from_persisted(&worker)
                .validate(&store.worker_dir(&self.worker_id).join(WORKER_FILE))
                .map_err(|_| reject("invalid installation request"))?;
            store.write_worker_record(&worker)?;
            let worker_dir = store.worker_dir(&self.worker_id);
            atomic_write_json(
                &worker_dir.join(WORKER_METADATA_FILE),
                &metadata,
                "write fixture metadata",
            )?;
            let session = worker_dir.join("session");
            io(fs::create_dir_all(session.join("segments")))?;
            write_new(&session.join("session.json"), &self.session_manifest)?;
            write_new(
                &session
                    .join("segments")
                    .join(format!("{}.jsonl", self.segment_id)),
                &self.segment_jsonl,
            )?;
            self.validate_session(&session)?;
            store.write_runtime_snapshot(&PersistedRuntimeState {
                subject_create_recovery_fences: BTreeMap::new(),
                subject_releases: Vec::new(),
                // Match normal store initialization: this is catalog availability,
                // not a process or Worker execution start.
                display_name: options.display_name.clone(),
                status: RuntimeStatus::Running,
                next_diagnostic_id: 1,
                workers: BTreeMap::new(),
                workspace_owners: BTreeMap::new(),
                diagnostics: Vec::new(),
            })?;
            let loaded = store.load_runtime_state()?;
            if loaded.workers.len() != 1 || !loaded.diagnostics.is_empty() {
                return Err(reject("staged aggregate failed ordinary store roundtrip"));
            }
            sync_runtime_tree(&stage)?;
            // No destination existed when checked under the owner lock. Hard-link
            // the snapshot last with create-new semantics; never replace a file.
            // Reserve the destination catalog with create-new semantics. Even
            // an empty preexisting workers directory must never be replaced.
            let destination_workers = options.root.join(WORKERS_DIR);
            io(fs::create_dir(&destination_workers))?;
            io(fs::rename(
                store.worker_dir(&self.worker_id),
                destination_workers.join(self.worker_id.to_string()),
            ))?;
            sync_directory(&destination_workers, "publish stopped Worker aggregate")?;
            io(fs::hard_link(
                stage.join(RUNTIME_FILE),
                options.root.join(RUNTIME_FILE),
            ))?;
            sync_directory(&options.root, "publish stopped fixture")?;
            Ok(())
        })();
        // Only remove our private staging directory, never existing destination data.
        let cleanup = fs::remove_dir_all(&stage);
        result?;
        io(cleanup)?;
        sync_directory(&options.root, "finish stopped fixture")
    }

    fn validate_shape(&self) -> Result<(), RuntimeError> {
        if self.template_version != TEMPLATE_VERSION
            || self.runtime_schema_version != SCHEMA_VERSION
        {
            return Err(reject("unsupported template schema"));
        }
        if !portable_id(&self.runtime_id)
            || !portable_id(&self.workspace_id)
            || self.session_id.get_version_num() != 7
            || self.segment_id.get_version_num() != 7
            || self.session_manifest.len() > MAX_SESSION_BYTES
            || self.segment_jsonl.len() > MAX_SESSION_BYTES
        {
            return Err(reject("invalid portable identity or size"));
        }
        let manifest: SessionManifest = decode(&self.session_manifest)?;
        if manifest.session_id != self.session_id
            || manifest.schema_version == 0
            || !self.segment_jsonl.ends_with(b"\n")
        {
            return Err(reject("Session identity or committed boundary mismatch"));
        }
        // Read-only JSON traversal, never mutation/reconstruction of Session bytes.
        // Reject known secret/host markers even in otherwise valid typed records.
        for line in self
            .segment_jsonl
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
        {
            let _: PortableJson = decode(line)?;
        }
        Ok(())
    }

    fn validate_history_entry(
        &self,
        entry: &session_store::LoggedHistoryEntry,
        ids: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), RuntimeError> {
        if !portable_id(&entry.metadata.entry_id.0)
            || !ids.insert(entry.metadata.entry_id.0.clone())
        {
            return Err(reject("duplicate or invalid entry identity"));
        }
        if !matches!(
            entry.item,
            session_store::LoggedItem::Message { .. }
                | session_store::LoggedItem::AssistantResponseBoundary { .. }
        ) {
            return Err(reject("only text conversation records are supported"));
        }
        if let session_store::LoggedSessionHistoryOrigin::ModelOutput { worker } =
            &entry.metadata.origin
        {
            // Session producer IDs identify the original execution, not the
            // Workspace catalog Worker. Preserve them; do not rebind/rewrite them.
            if !portable_id(&worker.worker_id)
                || worker
                    .workspace_id
                    .as_ref()
                    .is_some_and(|id| id != &self.workspace_id)
                || worker
                    .runtime_id
                    .as_ref()
                    .is_some_and(|id| id != &self.runtime_id)
            {
                return Err(reject("Session origin identity mismatch"));
            }
        }
        Ok(())
    }

    fn validate_session(&self, root: &Path) -> Result<(), RuntimeError> {
        for entry in io(fs::read_dir(root))? {
            let entry = io(entry)?;
            match entry.file_name().to_str() {
                Some("session.json" | "segments") => {}
                Some("artifacts") => check_empty_artifacts(&entry.path())?,
                _ => return Err(reject("unsupported Session files")),
            }
        }
        let entries = io(fs::read_dir(root.join("segments")))?.collect::<Result<Vec<_>, _>>();
        let entries = io(entries)?;
        if entries.len() != 1
            || entries[0].file_name() != format!("{}.jsonl", self.segment_id).as_str()
        {
            return Err(reject("only the fixed single-Segment fixture is supported"));
        }
        let store = WorkerSessionStore::open_read_only(root)
            .map_err(|_| reject("unsupported Session schema"))?;
        let logs = store
            .read_all_read_only_bounded(self.session_id, self.segment_id, MAX_SESSION_BYTES as u64)
            .map_err(|_| reject("invalid canonical Session records"))?
            .0;
        if !matches!(logs.first(), Some(session_store::LogEntry::AnnotatedSegmentStart {
            session_id, forked_from: None, compacted_from: None, ..
        }) if *session_id == self.session_id)
        {
            return Err(reject("invalid Segment seed or unsupported lineage"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for (index, log) in logs.iter().enumerate() {
            use session_store::LogEntry;
            match log {
                LogEntry::AnnotatedSegmentStart { history, .. }
                    if index == 0 && history.is_empty() => {}
                LogEntry::AnnotatedUserInput {
                    history,
                    extensions,
                    ..
                } => {
                    for extension in extensions {
                        validate_settled_activation(extension)?;
                    }
                    for entry in history {
                        self.validate_history_entry(entry, &mut ids)?;
                    }
                }
                LogEntry::AnnotatedAssistantItem { entry, .. } => {
                    self.validate_history_entry(entry, &mut ids)?
                }
                LogEntry::RunCompleted {
                    entry_id: Some(id), ..
                } => {
                    if !portable_id(&id.0) || !ids.insert(id.0.clone()) {
                        return Err(reject("duplicate or invalid entry identity"));
                    }
                }
                LogEntry::Invoke { .. } | LogEntry::LlmUsage { .. } | LogEntry::TurnEnd { .. } => {}
                _ => {
                    return Err(reject(
                        "unsupported conversation record or pending operation",
                    ));
                }
            }
        }
        let state = session_store::collect_state(&logs);
        if state.last_run_interrupted || state.active_run_turn_count.is_some() {
            return Err(reject("Session has pending execution"));
        }
        for segments in &state.user_segments {
            if segments
                .iter()
                .any(|segment| !matches!(segment, protocol::Segment::Text { .. }))
            {
                return Err(reject("Session attachments are unsupported"));
            }
        }
        Ok(())
    }
}

// The approved conversation has committed submission checkpoints in its original
// Session records. Keep their receipt bytes, but reject any resumable queue or
// Feature invocation history. This projection confers no execution authority.
fn validate_settled_activation(
    extension: &session_store::SessionExtension,
) -> Result<(), RuntimeError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Checkpoint {
        #[serde(rename = "next_activation_sequence")]
        _next_activation_sequence: u64,
        activating: Option<serde::de::IgnoredAny>,
        activating_notification: Vec<serde::de::IgnoredAny>,
        pending: Vec<serde::de::IgnoredAny>,
        pending_notifications: Vec<serde::de::IgnoredAny>,
        #[serde(rename = "receipts")]
        _receipts: Vec<serde::de::IgnoredAny>,
        #[serde(rename = "notification_receipts")]
        _notification_receipts: Vec<serde::de::IgnoredAny>,
        invocation_receipts: Vec<serde::de::IgnoredAny>,
        invocation_recovery_blocked: bool,
    }
    if extension.domain != "worker.pending_activations.v1" {
        return Err(reject("unsupported Session extension"));
    }
    let checkpoint: Checkpoint = serde_json::from_value(extension.payload.clone())
        .map_err(|_| reject("invalid Session activation checkpoint"))?;
    if checkpoint.activating.is_some()
        || !checkpoint.activating_notification.is_empty()
        || !checkpoint.pending.is_empty()
        || !checkpoint.pending_notifications.is_empty()
        || !checkpoint.invocation_receipts.is_empty()
        || checkpoint.invocation_recovery_blocked
    {
        return Err(reject(
            "Session has pending activation or invocation history",
        ));
    }
    Ok(())
}

fn check_request(request: &CreateWorkerRequest, install: bool) -> Result<(), RuntimeError> {
    if !request.workdir_attachment_requests.is_empty()
        || !request.workdir_attachments.is_empty()
        || request.backend_job.is_some()
        || request.subjektiv_attached
        || request.worker_observation_enabled
        || !request.worker_observation_grants.is_empty()
        || (install && request.initial_input.is_some())
    {
        return Err(reject(
            "unsupported capabilities, attachments, or initial input",
        ));
    }
    if install
        && (request.create_fingerprint.trim().is_empty()
            || request.config_bundle.is_none()
            || !matches!(
                request.profile_source,
                ProfileSourceArchiveSource::WorkspaceConfig { .. }
            ))
    {
        return Err(reject(
            "fresh config/profile references and create fingerprint are required",
        ));
    }
    Ok(())
}

fn check_empty_artifacts(root: &Path) -> Result<(), RuntimeError> {
    for entry in io(fs::read_dir(root))? {
        let entry = io(entry)?;
        let metadata = io(entry.metadata())?;
        if metadata.is_dir() {
            check_empty_artifacts(&entry.path())?;
        } else if entry.file_name() != ".aggregate.lock" || metadata.len() != 0 {
            return Err(reject("Session artifacts are unsupported"));
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let mut file = io(OpenOptions::new().write(true).create_new(true).open(path))?;
    io(file.write_all(bytes))?;
    io(file.sync_all())
}

// A streaming validation visitor, not a generic document available to callers.
struct PortableJson;
impl<'de> Deserialize<'de> for PortableJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PortableJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("portable fixture JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<PortableJson, E> {
                Ok(PortableJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<PortableJson, E> {
                Ok(PortableJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<PortableJson, E> {
                Ok(PortableJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<PortableJson, E> {
                Ok(PortableJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<PortableJson, E> {
                Ok(PortableJson)
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<PortableJson, E> {
                let lower = value.to_ascii_lowercase();
                if [
                    "://",
                    "private key",
                    "bearer ",
                    "sk-",
                    "api_key",
                    "access_token",
                    "refresh_token",
                    "private_key",
                    "authorization",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
                    || value
                        .split_whitespace()
                        .any(|word| word.starts_with('/') || word.contains(":\\"))
                {
                    return Err(E::custom("nonportable or sensitive fixture material"));
                }
                Ok(PortableJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<PortableJson, A::Error> {
                while seq.next_element::<PortableJson>()?.is_some() {}
                Ok(PortableJson)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<PortableJson, A::Error> {
                while map.next_entry::<PortableJson, PortableJson>()?.is_some() {}
                Ok(PortableJson)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
