//! Operator-managed local Subject storage and explicit Host capabilities.
//! Profile policy, identifiers and repository files are never execution grants.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use feature_storage::FeatureStorage;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use server_api::*;
use subjektiv::{SubjectRecord, SubjectRole, SubjectSessionAttribution, SubjektivStore};
use worker::subjektiv::{
    SubjektivHost, SubjektivHostContext, SubjektivHostError, SubjektivHostSettings,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandaloneSubjectBinding {
    pub scope_id: String,
    pub subject_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SubjectError {
    #[error("local Subject storage: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Domain(#[from] subjektiv::SubjektivError),
    #[error(transparent)]
    Storage(#[from] feature_storage::FeatureStorageError),
    #[error("local Subject storage metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("local Subject storage permission or identity is invalid: {0}")]
    InvalidScope(String),
    #[error("Subject `{0}` is already active; its kernel lease cannot be replaced")]
    Active(String),
}

/// Explicit operator create/list surface, not a model-accessible database handle.
pub struct StandaloneSubjects {
    state_root: PathBuf,
    root: PathBuf,
    scope_id: String,
    memory_language: String,
    manager: FeatureStorage,
    store: SubjektivStore,
}

impl StandaloneSubjects {
    pub fn open(state_dir: impl AsRef<Path>) -> Result<Self, SubjectError> {
        Self::open_inner(state_dir.as_ref(), true)
    }

    pub(crate) fn open_existing(state_dir: &Path) -> Result<Self, SubjectError> {
        Self::open_inner(state_dir, false)
    }

    fn open_inner(state_dir: &Path, create: bool) -> Result<Self, SubjectError> {
        if create {
            private_dir(state_dir, true)?;
        }
        validate_directory(state_dir, true)?;
        let state_root = fs::canonicalize(state_dir)?;
        let root = state_root.join("subjektiv");
        if create {
            private_dir(&root, true)?;
        }
        validate_directory(&root, true)?;
        let init_lock = private_file(&root.join("scope.lock"), create)?;
        init_lock.lock_exclusive()?;
        let scope_path = root.join("scope.json");
        let scope_id: String = match fs::symlink_metadata(&scope_path) {
            Ok(_) => {
                validate_file(&scope_path)?;
                serde_json::from_slice(&fs::read(&scope_path)?)?
            }
            Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
                let id = uuid::Uuid::now_v7().to_string();
                let mut file = private_file(&scope_path, true)?;
                file.write_all(&serde_json::to_vec(&id)?)?;
                file.sync_all()?;
                File::open(&root)?.sync_all()?;
                id
            }
            Err(e) => return Err(e.into()),
        };
        if uuid::Uuid::parse_str(&scope_id).is_err() {
            return Err(SubjectError::InvalidScope(
                "storage scope id is invalid".into(),
            ));
        }
        let language_path = root.join("language.json");
        let memory_language: String = match fs::symlink_metadata(&language_path) {
            Ok(_) => {
                validate_file(&language_path)?;
                serde_json::from_slice(&fs::read(&language_path)?)?
            }
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                let language = "English".to_owned();
                let mut file = private_file(&language_path, true)?;
                file.write_all(&serde_json::to_vec(&language)?)?;
                file.sync_all()?;
                File::open(&root)?.sync_all()?;
                language
            }
            Err(error) => return Err(error.into()),
        };
        if !manifest::is_normalized_workspace_memory_language(&memory_language) {
            return Err(SubjectError::InvalidScope(
                "invalid local Memory language setting".into(),
            ));
        }
        let db_root = root.join("features");
        if create {
            private_dir(&db_root, true)?;
        }
        validate_directory(&db_root, true)?;
        // Validate every existing node before SQLite opens it. Private parents
        // prevent another user from replacing a checked node during the open.
        validate_tree(&db_root)?;
        let manager = FeatureStorage::new(&db_root);
        let scope = manager.scope(&scope_id)?;
        let registration = SubjektivStore::register(&scope)?;
        let store = SubjektivStore::open(&scope, &registration)?;
        protect_tree(&db_root)?;
        Ok(Self {
            state_root,
            root,
            scope_id,
            memory_language,
            manager,
            store,
        })
    }

    pub fn create(
        &self,
        role: &str,
        behavior_md: Option<&str>,
    ) -> Result<SubjectRecord, SubjectError> {
        Ok(self.store.create_subject_with_behavior(
            SubjectRole::new(role)?,
            behavior_md.unwrap_or_default().to_owned(),
        )?)
    }

    pub fn list(&self) -> Result<Vec<SubjectRecord>, SubjectError> {
        let mut items = Vec::new();
        let mut after: Option<(String, String)> = None;
        loop {
            let page = self.store.list_subjects_after(
                100,
                after
                    .as_ref()
                    .map(|(created, id)| (created.as_str(), id.as_str())),
            )?;
            if page.has_more {
                after = page
                    .items
                    .last()
                    .map(|item| (item.created_at.clone(), item.id.clone()));
            }
            items.extend(page.items);
            if !page.has_more {
                return Ok(items);
            }
        }
    }

    pub(crate) fn connect(
        self,
        subject_id: &str,
        worker_id: protocol::WorkerId,
    ) -> Result<SubjectConnection, SubjectError> {
        let subject = self.store.subject(subject_id)?.ok_or_else(|| {
            SubjectError::InvalidScope(
                "selected Subject does not exist in this storage scope".into(),
            )
        })?;
        if subject.state != subjektiv::SubjectState::Active {
            return Err(SubjectError::InvalidScope(
                "selected Subject is retired".into(),
            ));
        }
        // Canonical common-issued UUID is required before using it as a path.
        if !valid_subject_id(subject_id) || subject.id != subject_id {
            return Err(SubjectError::InvalidScope("invalid Subject id".into()));
        }
        let leases = self.root.join("leases");
        private_dir(&leases, true)?;
        let lease = private_file(&leases.join(format!("{subject_id}.lock")), true)?;
        if !lease.try_lock_exclusive()? {
            return Err(SubjectError::Active(subject_id.into()));
        }
        let jobs_root = self.root.join("jobs");
        private_dir(&jobs_root, true)?;
        let jobs_dir = jobs_root.join(subject_id);
        private_dir(&jobs_dir, true)?;
        validate_tree(&jobs_dir)?;
        let binding = StandaloneSubjectBinding {
            scope_id: self.scope_id.clone(),
            subject_id: subject_id.to_string(),
        };
        let host = Arc::new(LocalSubjectHost {
            language: self.memory_language.clone(),
            catalog: self,
            binding: binding.clone(),
            worker_id,
            worker_key: format!("standalone-{worker_id}"),
            live: AtomicBool::new(true),
            jobs: Mutex::new(None),
        });
        Ok(SubjectConnection {
            host,
            binding,
            lease: Some(lease),
            jobs_path: jobs_dir.join("jobs.sqlite3"),
            armed: false,
        })
    }
}

pub(crate) struct SubjectConnection {
    pub host: Arc<LocalSubjectHost>,
    pub binding: StandaloneSubjectBinding,
    lease: Option<File>,
    pub jobs_path: PathBuf,
    armed: bool,
}
impl SubjectConnection {
    pub fn arm(&mut self) {
        self.armed = true;
    }
    pub fn retain(&mut self) {
        if let Some(file) = self.lease.take() {
            std::mem::forget(file);
        }
    }
    pub fn close(&mut self) -> Result<(), SubjectError> {
        self.host.jobs.lock().expect("Subject Jobs poisoned").take();
        self.host.live.store(false, Ordering::Release);
        self.host.catalog.manager.shutdown()?;
        if let Some(file) = &self.lease {
            file.unlock()?;
        }
        self.lease.take();
        self.armed = false;
        Ok(())
    }
}

impl Drop for SubjectConnection {
    fn drop(&mut self) {
        if self.armed {
            self.retain();
        }
    }
}

pub(crate) struct LocalSubjectHost {
    catalog: StandaloneSubjects,
    binding: StandaloneSubjectBinding,
    worker_id: protocol::WorkerId,
    worker_key: String,
    language: String,
    live: AtomicBool,
    jobs: Mutex<Option<crate::jobs::StandaloneJobs>>,
}

impl LocalSubjectHost {
    fn ensure_live(&self) -> Result<(), SubjektivHostError> {
        if !self.live.load(Ordering::Acquire) {
            return Err(SubjektivHostError::Unavailable(
                "local Subject Host is closed".into(),
            ));
        }
        Ok(())
    }
    fn body(&self, context: &SubjektivHostContext) -> Result<(), SubjektivHostError> {
        self.ensure_live()?;
        if context.worker_id != self.worker_key && context.worker_id != self.worker_id.to_string() {
            return Err(SubjektivHostError::InvalidArgument(
                "Worker is not bound to this Subject capability".into(),
            ));
        }
        if context
            .session_id
            .parse::<session_store::SessionId>()
            .is_err()
        {
            return Err(SubjektivHostError::InvalidArgument(
                "invalid committed Session identity".into(),
            ));
        }
        Ok(())
    }
    fn attribution(
        &self,
        context: &SubjektivHostContext,
        create: bool,
    ) -> Result<SubjectSessionAttribution, SubjektivHostError> {
        self.body(context)?;
        let store = &self.catalog.store;
        if let Some(record) = store
            .session_attribution(&context.session_id)
            .map_err(domain_error)?
        {
            if record.subject_id != self.binding.subject_id
                || record.runtime_id != self.binding.scope_id
                || record.worker_id != self.worker_id.to_string()
            {
                return Err(SubjektivHostError::InvalidArgument(
                    "Session does not belong to this local Subject/Worker scope".into(),
                ));
            }
            return Ok(record);
        }
        if !create {
            return Err(SubjektivHostError::InvalidArgument(
                "Session attribution is unavailable".into(),
            ));
        }
        // The capability is bound before controller startup. The Worker supplies
        // the Session it owns; models cannot call this Host API or select one.
        let record = SubjectSessionAttribution::new(
            &self.binding.subject_id,
            &self.binding.scope_id,
            self.worker_id.to_string(),
            &context.session_id,
        )
        .map_err(domain_error)?;
        store
            .record_session_attribution(record)
            .map_err(domain_error)
    }
}

fn domain_error(error: subjektiv::SubjektivError) -> SubjektivHostError {
    operation_error(subjektiv::api::subjektiv_store_error(error))
}
fn operation_error(error: subjektiv::api::OperationError) -> SubjektivHostError {
    use subjektiv::api::OperationError::*;
    match error {
        Conflict(message) => SubjektivHostError::Conflict {
            code: if message.contains("stale_cursor") {
                "stale_cursor"
            } else if message.contains("candidate_decision_conflict") {
                "candidate_decision_conflict"
            } else {
                "revision_conflict"
            }
            .into(),
            message,
        },
        Store(message) => SubjektivHostError::Unavailable(message),
        other => SubjektivHostError::InvalidArgument(other.to_string()),
    }
}

fn private_dir(path: &Path, strict: bool) -> Result<(), SubjectError> {
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            match builder.create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
    }
    validate_directory(path, strict)
}
fn validate_directory(path: &Path, strict: bool) -> Result<(), SubjectError> {
    reject_symlink_components(path)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(SubjectError::InvalidScope(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    validate_owner_mode(path, &meta, if strict { 0o077 } else { 0o022 })
}
fn validate_file(path: &Path) -> Result<(), SubjectError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(SubjectError::InvalidScope(format!(
            "{} is not a regular private file",
            path.display()
        )));
    }
    validate_owner_mode(path, &meta, 0o077)
}
fn validate_owner_mode(path: &Path, meta: &fs::Metadata, mask: u32) -> Result<(), SubjectError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & mask != 0 {
            return Err(SubjectError::InvalidScope(format!(
                "{} must be owned by the operator and private/non-writable by others",
                path.display()
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, meta, mask);
        return Err(SubjectError::InvalidScope(
            "private local Subject storage is not supported on this platform".into(),
        ));
    }
    Ok(())
}
fn reject_symlink_components(path: &Path) -> Result<(), SubjectError> {
    let mut part = PathBuf::new();
    for component in path.components() {
        part.push(component);
        if fs::symlink_metadata(&part)?.file_type().is_symlink() {
            return Err(SubjectError::InvalidScope(
                "symlinked Subject storage is not authority".into(),
            ));
        }
    }
    Ok(())
}
fn private_file(path: &Path, create: bool) -> Result<File, SubjectError> {
    reject_symlink_components(
        path.parent()
            .ok_or_else(|| SubjectError::InvalidScope("missing parent".into()))?,
    )?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    validate_file(path)?;
    Ok(file)
}
fn validate_tree(root: &Path) -> Result<(), SubjectError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.is_dir() {
            validate_directory(&entry.path(), true)?;
            validate_tree(&entry.path())?;
        } else {
            validate_file(&entry.path())?;
        }
    }
    Ok(())
}
fn protect_tree(root: &Path) -> Result<(), SubjectError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                return Err(SubjectError::InvalidScope("symlinked database".into()));
            }
            fs::set_permissions(
                entry.path(),
                fs::Permissions::from_mode(if meta.is_dir() { 0o700 } else { 0o600 }),
            )?;
            if meta.is_dir() {
                protect_tree(&entry.path())?;
            }
        }
    }
    Ok(())
}

impl StandaloneSubjects {
    pub(crate) fn scope_id(&self) -> &str {
        &self.scope_id
    }
}
impl LocalSubjectHost {
    pub(crate) fn bind_jobs(
        self: &Arc<Self>,
        jobs: crate::jobs::StandaloneJobs,
    ) -> Result<(), crate::StandaloneStartupError> {
        jobs.bind_domain(Arc::new(LocalDomainProvider(Arc::downgrade(self))))
            .map_err(|e| crate::StandaloneStartupError::Subject(e.to_string()))?;
        *self.jobs.lock().expect("Subject Job connection poisoned") = Some(jobs);
        Ok(())
    }
    fn jobs(&self) -> Result<crate::jobs::StandaloneJobs, SubjektivHostError> {
        self.ensure_live()?;
        self.jobs
            .lock()
            .expect("Subject Job connection poisoned")
            .clone()
            .ok_or_else(|| SubjektivHostError::Unavailable("Subject Job Host unavailable".into()))
    }
    pub(crate) fn recover_jobs(&self) {
        let Ok(jobs) = self.jobs() else {
            return;
        };
        if let Ok(results) = jobs.unacknowledged_results() {
            for result in results {
                if result
                    .domain_grant
                    .as_ref()
                    .and_then(|v| serde_json::from_value::<LocalConsolidationGrant>(v.clone()).ok())
                    .is_some_and(|g| g.binding == self.binding)
                {
                    // Acceptance already verified the immutable attempt/receipts.
                    // Delivery acknowledgement never replays the successful model.
                    let _ = jobs.acknowledge(&result.request.job_id);
                }
            }
        }
        if let Ok(pending) = jobs.pending() {
            for job in pending {
                if job
                    .domain_grant
                    .as_ref()
                    .and_then(|v| serde_json::from_value::<LocalConsolidationGrant>(v.clone()).ok())
                    .is_some_and(|g| g.binding == self.binding)
                {
                    schedule_job(jobs.clone(), job.request.job_id);
                }
            }
        }
    }
    fn validate_evidence(
        &self,
        session: &str,
        evidence: &mut [memory::extract::StagingEvidence],
        sources: &mut [memory::schema::SourceEvidenceRef],
    ) -> Result<(), SubjektivHostError> {
        if evidence.is_empty()
            || sources.is_empty()
            || evidence.len() > subjektiv::MAX_STAGING_ANCHORS
            || sources.len() > subjektiv::MAX_STAGING_ANCHORS
        {
            return Err(SubjektivHostError::InvalidArgument(
                "staging requires bounded committed public evidence".into(),
            ));
        }
        let index = crate::subjektiv_sessions::authorized_index(
            &self.catalog.state_root,
            &self.binding.scope_id,
            &self.binding.subject_id,
            &self.catalog.store,
            session,
        )?;
        let metadata = session_store::public_index::read_fs_session_evidence_index(
            &self
                .catalog
                .state_root
                .join(self.worker_id.to_string())
                .join("sessions"),
            session
                .parse()
                .map_err(|_| SubjektivHostError::InvalidArgument("invalid Session id".into()))?,
            session_store::SessionPublicIndexLimits {
                max_bytes: 8 * 1024 * 1024,
                max_entries: 20_000,
                max_segments: 64,
            },
        )
        .map_err(|e| {
            SubjektivHostError::Unavailable(format!("committed evidence metadata unavailable: {e}"))
        })?;
        if metadata.index.generation != index.generation {
            return Err(SubjektivHostError::Unavailable("committed Session changed while resolving evidence; retry from the committed capture".into()));
        }
        for anchor in evidence {
            let source = sources
                .iter_mut()
                .find(|source| source.evidence_id.as_deref() == Some(&anchor.id))
                .ok_or_else(|| {
                    SubjektivHostError::InvalidArgument("evidence pointer is missing".into())
                })?;
            if source.session_id.as_deref().is_some_and(|id| id != session) {
                return Err(SubjektivHostError::InvalidArgument(
                    "foreign Session evidence".into(),
                ));
            }
            let entry = index.segments.iter().filter(|seg|source.segment_id.as_deref() == Some(seg.segment_id.as_str()))
                .flat_map(|seg|&seg.entries).find(|entry|entry.entry_ref == anchor.id)
                .ok_or_else(|| if metadata.has_open_run {
                    SubjektivHostError::Conflict { code:"pending_commit".into(),message:"explicit evidence remains inside an open logical Run; retry after the append-only Run commit".into() }
                } else { SubjektivHostError::InvalidArgument("evidence is not an accessible committed public entry".into()) })?;
            if source.entry_range != anchor.entry_range
                || anchor.entry_range.is_none_or(|[a, b]| a > b)
            {
                return Err(SubjektivHostError::InvalidArgument(
                    "invalid committed evidence range".into(),
                ));
            }
            if !metadata
                .ranges
                .get(&(entry.segment_id.clone(), anchor.id.clone()))
                .is_some_and(|ranges| {
                    anchor
                        .entry_range
                        .is_some_and(|range| ranges.contains(&range))
                })
            {
                return Err(SubjektivHostError::InvalidArgument("evidence numeric range does not match either common committed producer projection".into()));
            }
            let kind = match entry.provenance {
                protocol::SessionEntryProvenance::HumanInput => {
                    memory::schema::EvidenceOriginKind::HumanInput
                }
                protocol::SessionEntryProvenance::WorkerInput => {
                    memory::schema::EvidenceOriginKind::WorkerInput
                }
                protocol::SessionEntryProvenance::FlowInstruction => {
                    memory::schema::EvidenceOriginKind::FlowInstruction
                }
                protocol::SessionEntryProvenance::BackendInstruction => {
                    memory::schema::EvidenceOriginKind::BackendInstruction
                }
                protocol::SessionEntryProvenance::ModelOutput => {
                    memory::schema::EvidenceOriginKind::ModelOutput
                }
                protocol::SessionEntryProvenance::ToolOutput => {
                    memory::schema::EvidenceOriginKind::ToolOutput
                }
                protocol::SessionEntryProvenance::DerivedSummary => {
                    memory::schema::EvidenceOriginKind::DerivedSummary
                }
                protocol::SessionEntryProvenance::LegacyUnknown => {
                    memory::schema::EvidenceOriginKind::LegacyUnknown
                }
            };
            let origin = memory::schema::EvidenceOrigin {
                kind,
                account_id: None,
                workspace_id: None,
                // Local storage scope is not a Runtime/control-plane identity.
                runtime_id: None,
                worker_id: Some(self.worker_id.to_string()),
                flow_selector: None,
                flow_definition_id: None,
                flow_definition_revision: None,
            };
            anchor.origin = Some(origin.clone());
            source.origin = Some(origin);
            anchor.excerpt = Some(entry.compact_text.clone());
            anchor.summary = Some(entry.compact_text.clone());
            source.session_id = Some(session.into());
            source.summary = Some(entry.compact_text.clone());
        }
        if sources
            .iter()
            .any(|source| source.session_id.as_deref() != Some(session))
        {
            return Err(SubjektivHostError::InvalidArgument(
                "unresolved evidence source".into(),
            ));
        }
        Ok(())
    }
}
impl SubjektivHost for LocalSubjectHost {
    fn settings(&self) -> SubjektivHostSettings {
        SubjektivHostSettings {
            language: self.language.clone(),
        }
    }
    fn memory(
        &self,
        context: &SubjektivHostContext,
        mut operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError> {
        self.body(context)?;
        let attribution =
            if let SubjektivMemoryBackendOperation::StageExplicit(input) = &mut operation {
                if input.session_id != context.session_id {
                    return Err(SubjektivHostError::InvalidArgument(
                        "explicit staging must use the connected committed Session".into(),
                    ));
                }
                let attribution = self.attribution(context, false)?;
                self.validate_evidence(
                    &context.session_id,
                    &mut input.evidence,
                    &mut input.source_refs,
                )?;
                Some(attribution)
            } else {
                None
            };
        let authority = subjektiv::api::HostOperationContext::validated_body(
            &self.catalog.store,
            &self.binding.subject_id,
            attribution,
        )
        .map_err(operation_error)?;
        subjektiv::api::execute(&self.catalog.store, &authority, operation).map_err(operation_error)
    }
    fn session(
        &self,
        context: &SubjektivHostContext,
        operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
        self.body(context)?;
        crate::subjektiv_sessions::execute(
            &self.catalog.state_root,
            &self.binding.scope_id,
            &self.binding.subject_id,
            &self.catalog.store,
            operation,
        )
    }
    fn stage_candidate(
        &self,
        context: &SubjektivHostContext,
        mut operation: memory::backend::MemoryStageCandidateOperation,
    ) -> Result<SubjektivStageCandidateResponse, SubjektivHostError> {
        let attribution = self.attribution(context, false)?;
        self.validate_evidence(
            &context.session_id,
            &mut operation.evidence,
            &mut operation.source_refs,
        )?;
        if operation.candidate.kind == memory::extract::CandidateKind::Preference
            && operation.evidence.iter().any(|e| {
                e.origin
                    .as_ref()
                    .is_none_or(|o| o.kind != memory::schema::EvidenceOriginKind::HumanInput)
            })
        {
            return Err(SubjektivHostError::InvalidArgument(
                "preference requires HumanInput evidence".into(),
            ));
        }
        let record = memory::extract::StagingRecord::from_candidate(
            uuid::Uuid::now_v7().to_string(),
            operation.extract_run_id,
            operation.source,
            operation.candidate,
            operation.evidence,
            operation.source_refs,
        );
        let (staged, _) = self
            .catalog
            .store
            .stage_candidate_with_attribution(
                subjektiv::SubjectStagingRecord::attach(&self.binding.subject_id, record),
                attribution,
            )
            .map_err(domain_error)?;
        Ok(SubjektivStageCandidateResponse {
            staging_id: staged.id,
        })
    }
    fn record_session(
        &self,
        context: &SubjektivHostContext,
        create_if_missing: bool,
    ) -> Result<SubjektivRecordSessionResponse, SubjektivHostError> {
        let attribution = self.attribution(context, create_if_missing)?;
        Ok(SubjektivRecordSessionResponse {
            subject_id: attribution.subject_id,
            session_id: attribution.session_id,
        })
    }
    fn request_consolidation(
        &self,
        context: &SubjektivHostContext,
        operation: memory::backend::MemoryConsolidateStagingOperation,
    ) -> Result<memory::backend::MemoryConsolidationOutput, SubjektivHostError> {
        self.body(context)?;
        let jobs = self.jobs()?;
        if let Some(pending) = jobs
            .pending()
            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?
            .into_iter()
            .find(|job| {
                job.domain_grant
                    .as_ref()
                    .and_then(|v| serde_json::from_value::<LocalConsolidationGrant>(v.clone()).ok())
                    .is_some_and(|g| g.binding == self.binding)
            })
        {
            schedule_job(jobs, pending.request.job_id);
            let (count, bytes) = self
                .catalog
                .store
                .pending_staging_backlog(&self.binding.subject_id)
                .map_err(domain_error)?;
            return Ok(memory::backend::MemoryConsolidationOutput { status:"resumed_pending".into(),summary:"Resumed the existing immutable reserved Subject Job; unknown attempts are never replayed".into(),candidate_count:count,total_bytes:bytes });
        }
        let (count, bytes) = self
            .catalog
            .store
            .pending_staging_backlog(&self.binding.subject_id)
            .map_err(domain_error)?;
        let surface = self
            .catalog
            .store
            .resident_surface(&self.binding.subject_id)
            .map_err(domain_error)?
            .availability;
        if !subjektiv::job::consolidation_required(count, bytes, operation.force, surface) {
            return Ok(memory::backend::MemoryConsolidationOutput {
                status: "skipped_below_threshold".into(),
                summary: "Subject backlog/surface is below the common consolidation threshold"
                    .into(),
                candidate_count: count,
                total_bytes: bytes,
            });
        }
        let ids = subjektiv::job::bounded_candidate_batch(
            self.catalog
                .store
                .pending_staging_candidates(&self.binding.subject_id, 100)
                .map_err(domain_error)?
                .into_iter()
                .map(|c| c.id),
            job::DEFAULT_MAX_RESULT_BYTES,
        )
        .map_err(operation_error)?;
        let subject = self
            .catalog
            .store
            .subject(&self.binding.subject_id)
            .map_err(domain_error)?
            .ok_or_else(|| SubjektivHostError::Unavailable("Subject missing".into()))?;
        let grant = LocalConsolidationGrant {
            binding: self.binding.clone(),
            candidate_ids: ids,
        };
        let request=job::JobRequest {
            job_id:format!("subjektiv-consolidation:{}",uuid::Uuid::now_v7()),purpose:"subjektiv_consolidation".into(),
            input_revision:subject.store_revision.to_string(),input_ref:format!("subjektiv://{}/consolidation",self.binding.subject_id),
            input:serde_json::json!({"subject_id":self.binding.subject_id,"candidate_ids":grant.candidate_ids}),
            instruction:"Resolve ONLY candidate_ids with MemoryStagingList/Read, recall and MemoryApplyCandidate. Already resolved IDs are durable receipts; do not repeat them. Leave new candidates for a later Job. When the entire immutable batch has a disposition, call SubmitJobResult with result: {subject_id, candidate_ids}. The Host awaits clean-context surface generation and verifies actual receipts/outcome before accepting success; never supply surface claims.".into(),
            profile:"builtin:standalone-subjektiv-consolidation".into(),serialization_key:Some(format!("subjektiv:{}",self.binding.subject_id)),
            limits:job::JobLimits { timeout_seconds:600,..Default::default() },
        };
        let jobs = self.jobs()?;
        let snapshot = jobs
            .request_granted(
                request,
                serde_json::to_value(grant)
                    .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?,
            )
            .map_err(|e| SubjektivHostError::Unavailable(e.to_string()))?;
        schedule_job(jobs, snapshot.request.job_id);
        Ok(memory::backend::MemoryConsolidationOutput { status:"started".into(),summary:"Requested a durable generic standalone consolidation Job; processing stops on Host shutdown".into(),candidate_count:count,total_bytes:bytes })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalConsolidationGrant {
    binding: StandaloneSubjectBinding,
    candidate_ids: Vec<String>,
}
struct LocalDomainProvider(std::sync::Weak<LocalSubjectHost>);
impl crate::jobs::JobDomainProvider for LocalDomainProvider {
    fn grant(
        &self,
        snapshot: &crate::jobs::JobSnapshot,
        fence: Arc<dyn crate::jobs::JobAttemptFence>,
    ) -> Result<worker::job::JobFeatureGrant, String> {
        let host = self.0.upgrade().ok_or("Subject Host unavailable")?;
        host.ensure_live().map_err(|e| e.to_string())?;
        let grant: LocalConsolidationGrant = serde_json::from_value(
            snapshot
                .domain_grant
                .clone()
                .ok_or("missing explicit Subject delegation")?,
        )
        .map_err(|e| e.to_string())?;
        if grant.binding != host.binding
            || snapshot.request.purpose != "subjektiv_consolidation"
            || snapshot.request.input
                != serde_json::json!({"subject_id":host.binding.subject_id,"candidate_ids":grant.candidate_ids})
        {
            return Err("persisted Job grant/input does not match local Subject capability".into());
        }
        let adapter = Arc::new(LocalConsolidationHost {
            host,
            grant,
            job_id: snapshot.request.job_id.clone(),
            attempt_id: snapshot.attempt.attempt_id.clone(),
            fence,
        });
        let connection = worker::subjektiv::SubjektivHostConnection::new(
            adapter.clone(),
            SubjektivHostContext {
                worker_id: format!("job:{}", snapshot.attempt.attempt_id),
                session_id: format!("job:{}", snapshot.attempt.attempt_id),
            },
        )
        .map_err(|e| e.to_string())?;
        let feature = worker::feature::builtin::memory::SubjektivConsolidationToolsFeature::new(
            connection.clone(),
        )
        .map_err(|e| e.to_string())?;
        Ok(worker::job::JobFeatureGrant {
            features: worker::feature::FeatureRegistryBuilder::new().with_module(feature),
            satisfied_requirements: vec!["feature.subjektiv"],
            finalizer: Some(Arc::new(LocalConsolidationFinalizer {
                adapter,
                connection,
            })),
        })
    }
}
struct LocalConsolidationHost {
    host: Arc<LocalSubjectHost>,
    grant: LocalConsolidationGrant,
    job_id: String,
    attempt_id: String,
    fence: Arc<dyn crate::jobs::JobAttemptFence>,
}
impl LocalConsolidationHost {
    fn live(&self) -> Result<(), SubjektivHostError> {
        self.host.ensure_live()?;
        self.fence
            .ensure_live()
            .map_err(SubjektivHostError::Unavailable)
    }
    fn denied<T>(&self) -> Result<T, SubjektivHostError> {
        Err(SubjektivHostError::InvalidArgument(
            "consolidation Job has no Subject body/extraction authority".into(),
        ))
    }
}
impl SubjektivHost for LocalConsolidationHost {
    fn settings(&self) -> SubjektivHostSettings {
        self.host.settings()
    }
    fn consolidation_granted(&self) -> bool {
        true
    }
    fn memory(
        &self,
        _context: &SubjektivHostContext,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError> {
        self.live()?;
        let authority = subjektiv::api::HostOperationContext::validated_consolidation(
            &self.host.catalog.store,
            &self.grant.binding.subject_id,
            Some(self.grant.candidate_ids.clone()),
            Some(subjektiv::api::JobAttemptBinding {
                job_id: self.job_id.clone(),
                attempt_id: self.attempt_id.clone(),
            }),
        )
        .map_err(operation_error)?;
        subjektiv::api::execute(&self.host.catalog.store, &authority, operation)
            .map_err(operation_error)
    }
    fn session(
        &self,
        _context: &SubjektivHostContext,
        _operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
        self.denied()
    }
    fn stage_candidate(
        &self,
        _: &SubjektivHostContext,
        _: memory::backend::MemoryStageCandidateOperation,
    ) -> Result<SubjektivStageCandidateResponse, SubjektivHostError> {
        self.denied()
    }
    fn record_session(
        &self,
        _: &SubjektivHostContext,
        _: bool,
    ) -> Result<SubjektivRecordSessionResponse, SubjektivHostError> {
        self.denied()
    }
    fn request_consolidation(
        &self,
        _: &SubjektivHostContext,
        _: memory::backend::MemoryConsolidateStagingOperation,
    ) -> Result<memory::backend::MemoryConsolidationOutput, SubjektivHostError> {
        self.denied()
    }
}
struct LocalConsolidationFinalizer {
    adapter: Arc<LocalConsolidationHost>,
    connection: worker::subjektiv::SubjektivHostConnection,
}
#[async_trait::async_trait]
impl worker::job::JobResultFinalizer for LocalConsolidationFinalizer {
    fn validate_profile(&self, manifest: &manifest::WorkerManifest) -> Result<(), String> {
        let config = &manifest.feature.subjektiv;
        if !config.profile.enabled
            || !config.profile.consolidation_tools
            || config.profile.extraction.enabled
            || config.workspace_settings.is_some()
        {
            return Err("local consolidation requires enabled decision policy without extraction or Backend settings".into());
        }
        Ok(())
    }
    async fn finalize(
        &self,
        result: serde_json::Value,
        manifest: manifest::WorkerManifest,
        client: Box<dyn agen::llm_client::LlmClient>,
        cancellation: worker::feature::background::BackgroundTaskCancellation,
    ) -> Result<serde_json::Value, agen::tool::ToolError> {
        use agen::tool::ToolError;
        self.adapter.live().map_err(ToolError::from)?;
        if result
            != serde_json::json!({"subject_id":self.adapter.grant.binding.subject_id,"candidate_ids":self.adapter.grant.candidate_ids})
        {
            return Err(ToolError::InvalidArgument("result must claim exactly the bound Subject/batch, without model surface/disposition assertions".into()));
        }
        for id in &self.adapter.grant.candidate_ids {
            if self
                .adapter
                .host
                .catalog
                .store
                .staging_resolution(&self.adapter.grant.binding.subject_id, id)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
                .is_none()
            {
                return Err(ToolError::InvalidArgument(format!(
                    "candidate {id} is unresolved"
                )));
            }
        }
        let prompts = worker::PromptCatalog::builtins_only()
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let feature = worker::subjektiv::SubjektivSurfaceLifecycleFeature::for_host(
            self.connection.clone(),
            manifest,
            client,
            Arc::new(prompts.into()),
            worker::WorkerWorkspaceContext::no_workspace(),
        )
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let surface = feature
            .complete_for_job(cancellation, None)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        self.adapter.live().map_err(ToolError::from)?;
        let mut value = result;
        value
            .as_object_mut()
            .expect("checked object")
            .insert("surface".into(), surface);
        subjektiv::job::validate_result(
            &self.adapter.host.catalog.store,
            &self.adapter.grant.binding.subject_id,
            &self.adapter.grant.candidate_ids,
            &self.adapter.job_id,
            &self.adapter.attempt_id,
            &value,
        )
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        subjektiv::job::with_dispositions(
            &self.adapter.host.catalog.store,
            &self.adapter.grant.binding.subject_id,
            &self.adapter.grant.candidate_ids,
            &value,
        )
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

fn schedule_job(jobs: crate::jobs::StandaloneJobs, id: String) {
    // Scheduling/consumer acknowledgement has no Internal Worker ownership.
    // The generic service fences and retains every execution until cleanup.
    tokio::spawn(async move {
        if let Err(error) = jobs.start(&id).await {
            eprintln!("standalone Subject Job {id} remains pending: {error}");
            return;
        }
        match jobs.wait(&id).await {
            Ok(result) if result.state == crate::jobs::JobState::Completed => {
                let _ = jobs.acknowledge(&id);
            }
            Ok(_) => {}
            Err(error) => eprintln!(
                "standalone Subject Job {id} cleanup/acknowledgement is unconfirmed: {error}"
            ),
        }
    });
}

#[cfg(test)]
mod tests;

pub(crate) fn valid_subject_id(value: &str) -> bool {
    value
        .strip_prefix("subject-")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .is_some_and(|id| format!("subject-{id}") == value)
}
