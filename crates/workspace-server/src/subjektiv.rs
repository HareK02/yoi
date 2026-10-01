//! Subject-scoped, revisioned Memory storage for the `subjektiv` Feature.
//!
//! This module owns subjektiv's domain schema and typed repository. Physical
//! SQLite placement, connection configuration, migration execution, backup,
//! restore, and teardown remain owned by [`crate::feature_storage`]. A store is
//! already bound to one Workspace; a subject identifier is therefore a lookup
//! key, not an authorization credential.

use std::collections::HashSet;

use chrono::{SecondsFormat, Utc};
use memory::extract::{CandidateKind, STAGING_SCHEMA_VERSION, StagingEvidence, StagingRecord};
use memory::schema::{SourceEvidenceRef, SourceRef};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::feature_storage::{
    FeatureDatabase, FeatureMigration, FeatureRegistration, FeatureStorageError, RegisteredFeature,
    WorkspaceFeatureStorage,
};

pub const SUBJEKTIV_SCHEMA_VERSION: u32 = 1;
pub const SUBJEKTIV_FEATURE_ID: &str = "subjektiv";

pub type Result<T> = std::result::Result<T, SubjektivError>;

#[derive(Debug, thiserror::Error)]
pub enum SubjektivError {
    #[error(transparent)]
    Storage(#[from] FeatureStorageError),
    #[error("invalid subjektiv record: {0}")]
    InvalidRecord(String),
    #[error("subject `{0}` was not found in this Workspace")]
    SubjectNotFound(String),
    #[error("subject `{0}` is retired")]
    SubjectRetired(String),
    #[error("staging candidate `{0}` was not found for this subject")]
    CandidateNotFound(String),
    #[error("staging candidate `{0}` already exists with different content")]
    CandidateConflict(String),
    #[error("staging candidate `{0}` is already resolved")]
    CandidateResolved(String),
    #[error(
        "session `{session_id}` is already attributed to subject `{existing_subject_id}` and cannot be attributed to `{requested_subject_id}`"
    )]
    SessionSubjectConflict {
        session_id: String,
        existing_subject_id: String,
        requested_subject_id: String,
    },
    #[error("session `{0}` already has different historical attribution")]
    SessionAttributionConflict(String),
    #[error("memory `{0}` was not found for this subject")]
    MemoryNotFound(String),
    #[error("memory `{memory_id}` revision conflict: expected {expected}, current {actual}")]
    RevisionConflict {
        memory_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("memory `{memory_id}` cannot transition from {from:?} to {to:?}")]
    InvalidStateTransition {
        memory_id: String,
        from: MemoryState,
        to: MemoryState,
    },
    #[error("reference `{reference}` does not belong to subject `{subject_id}`")]
    SubjectScopeMismatch {
        subject_id: String,
        reference: String,
    },
    #[error("subjektiv JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<rusqlite::Error> for SubjektivError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(FeatureStorageError::from(error))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SubjectRole(String);

impl SubjectRole {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_label("subject role", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SubjectRole {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        SubjectRole::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectState {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectRecord {
    pub schema_version: u32,
    pub id: String,
    pub role: SubjectRole,
    pub state: SubjectState,
    /// Monotonic generation incremented exactly once for every committed Memory
    /// creation or revision for this subject.
    pub store_revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Immutable host-recorded history that attributes one durable Session to the
/// subject and Worker that produced it. This is archival provenance only: it is
/// deliberately not a current-Worker link or execution ownership record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectSessionAttribution {
    pub schema_version: u32,
    pub subject_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub session_id: String,
    pub attributed_at: String,
}

impl SubjectSessionAttribution {
    pub fn new(
        subject_id: impl Into<String>,
        runtime_id: impl Into<String>,
        worker_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Result<Self> {
        let record = Self {
            schema_version: SUBJEKTIV_SCHEMA_VERSION,
            subject_id: subject_id.into(),
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
            session_id: session_id.into(),
            attributed_at: now(),
        };
        validate_session_attribution(&record)?;
        Ok(record)
    }
}

/// The existing extraction schema with only host-owned subject scope and
/// persistence time added. Model input still contains neither field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectStagingRecord {
    pub schema_version: u32,
    pub id: String,
    pub subject_id: String,
    pub extract_run_id: String,
    pub source: SourceRef,
    pub kind: CandidateKind,
    pub claim: String,
    pub why_useful: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staleness: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StagingEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceEvidenceRef>,
    pub created_at: String,
}

impl SubjectStagingRecord {
    /// Attaches host-resolved subject scope and time to an existing v2 staging
    /// record without changing its taxonomy, evidence, or source anchors.
    pub fn attach(subject_id: impl Into<String>, record: StagingRecord) -> Self {
        Self::attach_at(subject_id, record, now())
    }

    fn attach_at(subject_id: impl Into<String>, record: StagingRecord, created_at: String) -> Self {
        Self {
            schema_version: record.schema_version,
            id: record.id,
            subject_id: subject_id.into(),
            extract_run_id: record.extract_run_id,
            source: record.source,
            kind: record.kind,
            claim: record.claim,
            why_useful: record.why_useful,
            staleness: record.staleness,
            evidence: record.evidence,
            source_refs: record.source_refs,
            created_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    /// Current and eligible for ordinary recall. This does not assert truth and
    /// grants no authority.
    Active,
    /// No longer current because its question, condition, or applicability has
    /// ended. A later revision may reopen it when circumstances change.
    Resolved,
    /// Invalidated and no longer eligible for recall. Retraction is terminal;
    /// corrected follow-up experience is a separate Memory linked by derivation.
    Retracted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRevisionRef {
    pub memory_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    pub schema_version: u32,
    pub id: String,
    pub subject_id: String,
    pub revision: u64,
    pub kind: CandidateKind,
    pub state: MemoryState,
    pub claim: String,
    pub body_md: String,
    pub why_useful: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staleness: Option<String>,
    #[serde(default)]
    pub source_candidate_ids: Vec<String>,
    #[serde(default)]
    pub derived_from: Vec<MemoryRevisionRef>,
    pub change_reason: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct MemoryDraft {
    pub kind: CandidateKind,
    pub state: MemoryState,
    pub claim: String,
    pub body_md: String,
    pub why_useful: String,
    pub staleness: Option<String>,
    pub source_candidate_ids: Vec<String>,
    pub derived_from: Vec<MemoryRevisionRef>,
    pub change_reason: String,
}

impl MemoryDraft {
    pub fn active(
        kind: CandidateKind,
        claim: impl Into<String>,
        body_md: impl Into<String>,
        why_useful: impl Into<String>,
        change_reason: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            state: MemoryState::Active,
            claim: claim.into(),
            body_md: body_md.into(),
            why_useful: why_useful.into(),
            staleness: None,
            source_candidate_ids: Vec::new(),
            derived_from: Vec::new(),
            change_reason: change_reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingResolutionAction {
    Applied,
    Discarded,
    Invalid,
    Duplicate,
    AlreadyCovered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingResolution {
    pub schema_version: u32,
    pub id: String,
    pub subject_id: String,
    pub candidate_id: String,
    pub action: StagingResolutionAction,
    pub reason: String,
    #[serde(default)]
    pub affected_memory: Vec<MemoryRevisionRef>,
    /// Immutable copy of the candidate as it was staged. The database also
    /// retains the exact serialized bytes independently of this projection.
    pub candidate: SubjectStagingRecord,
    pub resolved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSnapshot {
    pub schema_version: u32,
    pub id: String,
    pub subject_id: String,
    pub body_md: String,
    #[serde(default)]
    pub memory_refs: Vec<MemoryRevisionRef>,
    pub built_from_store_revision: u64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub enum MemoryRevisionTarget {
    Create,
    Revise {
        memory_id: String,
        expected_revision: u64,
    },
}

fn create_schema(transaction: &Transaction<'_>) -> crate::feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE store_scope (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    workspace_id TEXT NOT NULL UNIQUE
);

CREATE TABLE subjects (
    subject_id TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'retired')),
    store_revision INTEGER NOT NULL CHECK (store_revision >= 0),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE staging_records (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE memory_records (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    current_revision INTEGER NOT NULL CHECK (current_revision > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, current_revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE memory_revisions (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision),
    FOREIGN KEY (subject_id, memory_id)
        REFERENCES memory_records(subject_id, memory_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE memory_revision_candidates (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision, candidate_id),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE memory_revision_derivations (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    source_memory_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    PRIMARY KEY (
        subject_id, memory_id, revision, source_memory_id, source_revision
    ),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, source_memory_id, source_revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE staging_resolutions (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    resolution_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (
        action IN ('applied', 'discarded', 'invalid', 'duplicate', 'already_covered')
    ),
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    resolution_json TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    UNIQUE (resolution_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_targets (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, candidate_id, memory_id, revision),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshots (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    built_from_store_revision INTEGER NOT NULL CHECK (built_from_store_revision >= 0),
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_refs (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id, memory_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE memory_revision_seals (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_seals (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_seals (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT
);

CREATE TRIGGER memory_revision_candidates_no_late_insert
BEFORE INSERT ON memory_revision_candidates
WHEN EXISTS (
    SELECT 1 FROM memory_revision_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND revision = NEW.revision
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv revision evidence is sealed');
END;
CREATE TRIGGER memory_revision_derivations_no_late_insert
BEFORE INSERT ON memory_revision_derivations
WHEN EXISTS (
    SELECT 1 FROM memory_revision_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND revision = NEW.revision
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are sealed');
END;
CREATE TRIGGER staging_resolution_targets_no_late_insert
BEFORE INSERT ON staging_resolution_targets
WHEN EXISTS (
    SELECT 1 FROM staging_resolution_seals
    WHERE subject_id = NEW.subject_id
      AND candidate_id = NEW.candidate_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are sealed');
END;
CREATE TRIGGER surface_snapshot_refs_no_late_insert
BEFORE INSERT ON surface_snapshot_refs
WHEN EXISTS (
    SELECT 1 FROM surface_snapshot_seals
    WHERE subject_id = NEW.subject_id
      AND snapshot_id = NEW.snapshot_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are sealed');
END;

CREATE TRIGGER memory_revision_seals_no_update
BEFORE UPDATE ON memory_revision_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv revision seals are immutable');
END;
CREATE TRIGGER memory_revision_seals_no_delete
BEFORE DELETE ON memory_revision_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv revision seals are retained');
END;
CREATE TRIGGER staging_resolution_seals_no_update
BEFORE UPDATE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are immutable');
END;
CREATE TRIGGER staging_resolution_seals_no_delete
BEFORE DELETE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are retained');
END;
CREATE TRIGGER surface_snapshot_seals_no_update
BEFORE UPDATE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are immutable');
END;
CREATE TRIGGER surface_snapshot_seals_no_delete
BEFORE DELETE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are retained');
END;

CREATE TRIGGER memory_revisions_no_update
BEFORE UPDATE ON memory_revisions BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory revisions are immutable');
END;
CREATE TRIGGER memory_revisions_no_delete
BEFORE DELETE ON memory_revisions BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory revisions are retained');
END;
CREATE TRIGGER memory_revision_candidates_no_update
BEFORE UPDATE ON memory_revision_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv revision evidence is immutable');
END;
CREATE TRIGGER memory_revision_candidates_no_delete
BEFORE DELETE ON memory_revision_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv revision evidence is retained');
END;
CREATE TRIGGER memory_revision_derivations_no_update
BEFORE UPDATE ON memory_revision_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are immutable');
END;
CREATE TRIGGER memory_revision_derivations_no_delete
BEFORE DELETE ON memory_revision_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are retained');
END;
CREATE TRIGGER staging_records_no_update
BEFORE UPDATE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are immutable');
END;
CREATE TRIGGER staging_records_no_delete
BEFORE DELETE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are retained');
END;
CREATE TRIGGER staging_resolutions_no_update
BEFORE UPDATE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are immutable');
END;
CREATE TRIGGER staging_resolutions_no_delete
BEFORE DELETE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are retained');
END;
CREATE TRIGGER staging_resolution_targets_no_update
BEFORE UPDATE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are immutable');
END;
CREATE TRIGGER staging_resolution_targets_no_delete
BEFORE DELETE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are retained');
END;
CREATE TRIGGER surface_snapshots_no_update
BEFORE UPDATE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are immutable');
END;
CREATE TRIGGER surface_snapshots_no_delete
BEFORE DELETE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are retained');
END;
CREATE TRIGGER surface_snapshot_refs_no_update
BEFORE UPDATE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are immutable');
END;
CREATE TRIGGER surface_snapshot_refs_no_delete
BEFORE DELETE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are retained');
END;
"#,
    )?;
    Ok(())
}

fn add_subject_session_attribution(
    transaction: &Transaction<'_>,
) -> crate::feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE subject_session_attributions (
    session_id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    attributed_at TEXT NOT NULL,
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE INDEX subject_session_attributions_by_subject
ON subject_session_attributions(subject_id, attributed_at, session_id);

CREATE TRIGGER subject_session_attributions_no_update
BEFORE UPDATE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is immutable');
END;
CREATE TRIGGER subject_session_attributions_no_delete
BEFORE DELETE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is retained');
END;
"#,
    )?;
    Ok(())
}

static MIGRATIONS: &[FeatureMigration] = &[
    FeatureMigration::new(1, "create subjektiv subject memory store", create_schema),
    FeatureMigration::new(
        2,
        "add immutable subject session attribution",
        add_subject_session_attribution,
    ),
];

pub const REGISTRATION: FeatureRegistration =
    FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, MIGRATIONS);

#[derive(Clone)]
pub struct SubjektivStore {
    database: FeatureDatabase,
}

impl SubjektivStore {
    /// Registers the trusted subjektiv schema with the Server storage manager.
    /// Registration is intentionally separate so one registration can be reused
    /// by every Workspace served by that manager.
    pub fn register(storage: &WorkspaceFeatureStorage) -> Result<RegisteredFeature> {
        storage.register(REGISTRATION).map_err(Into::into)
    }

    pub fn open(
        storage: &WorkspaceFeatureStorage,
        registration: &RegisteredFeature,
    ) -> Result<Self> {
        if registration.feature_id() != SUBJEKTIV_FEATURE_ID {
            return Err(SubjektivError::InvalidRecord(format!(
                "registration is for Feature `{}` rather than `{SUBJEKTIV_FEATURE_ID}`",
                registration.feature_id()
            )));
        }
        let database = storage.open(registration)?;
        let workspace_id = database.workspace_id().to_string();
        database.try_transaction::<_, SubjektivError>(|transaction| {
            let stored = transaction
                .query_row(
                    "SELECT workspace_id FROM store_scope WHERE singleton = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            match stored {
                Some(stored) if stored != workspace_id => {
                    Err(SubjektivError::SubjectScopeMismatch {
                        subject_id: "<store>".to_string(),
                        reference: format!(
                            "database is bound to Workspace `{stored}`, not `{workspace_id}`"
                        ),
                    })
                }
                Some(_) => Ok(()),
                None => {
                    transaction.execute(
                        "INSERT INTO store_scope (singleton, workspace_id) VALUES (1, ?1)",
                        [&workspace_id],
                    )?;
                    Ok(())
                }
            }
        })?;
        Ok(Self { database })
    }

    pub fn workspace_id(&self) -> &str {
        self.database.workspace_id()
    }

    /// Issues a random host-owned subject id. It is never derived from Worker,
    /// Profile, Session, or Memory content.
    pub fn create_subject(&self, role: SubjectRole) -> Result<SubjectRecord> {
        let timestamp = now();
        let record = SubjectRecord {
            schema_version: SUBJEKTIV_SCHEMA_VERSION,
            id: issued_id("subject"),
            role,
            state: SubjectState::Active,
            store_revision: 0,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        let raw = serde_json::to_string(&record)?;
        self.database.try_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO subjects (
                    subject_id, role, state, store_revision, record_json, created_at, updated_at
                 ) VALUES (?1, ?2, 'active', 0, ?3, ?4, ?5)",
                params![
                    record.id,
                    record.role.as_str(),
                    raw,
                    record.created_at,
                    record.updated_at
                ],
            )?;
            Ok(record)
        })
    }

    pub fn subject(&self, subject_id: &str) -> Result<Option<SubjectRecord>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT record_json FROM subjects WHERE subject_id = ?1",
                    [subject_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_subject(&raw)).transpose()
        })
    }

    /// Subject retirement is distinct from stopping any current Worker. It does
    /// not edit or delete Memory and cannot be reversed by this baseline schema.
    pub fn retire_subject(&self, subject_id: &str) -> Result<SubjectRecord> {
        self.database.try_transaction(|transaction| {
            let mut subject = require_subject(transaction, subject_id)?;
            if subject.state == SubjectState::Retired {
                return Ok(subject);
            }
            subject.state = SubjectState::Retired;
            subject.updated_at = now();
            let raw = serde_json::to_string(&subject)?;
            transaction.execute(
                "UPDATE subjects
                 SET state = 'retired', record_json = ?2, updated_at = ?3
                 WHERE subject_id = ?1",
                params![subject_id, raw, subject.updated_at],
            )?;
            Ok(subject)
        })
    }

    pub fn stage_candidate(&self, record: SubjectStagingRecord) -> Result<SubjectStagingRecord> {
        validate_staging_record(self.workspace_id(), &record)?;
        let raw = serde_json::to_string(&record)?;
        self.database.try_transaction(|transaction| {
            require_active_subject(transaction, &record.subject_id)?;
            write_staging_candidate(transaction, self.workspace_id(), record, raw)
        })
    }

    /// Atomically records immutable host attribution for the committed Session
    /// and stages its extracted candidate. Exact retries return the first stored
    /// records, including their original host timestamps.
    pub fn stage_candidate_with_attribution(
        &self,
        mut record: SubjectStagingRecord,
        attribution: SubjectSessionAttribution,
    ) -> Result<(SubjectStagingRecord, SubjectSessionAttribution)> {
        validate_session_attribution(&attribution)?;
        if record.subject_id != attribution.subject_id {
            return Err(SubjektivError::SubjectScopeMismatch {
                subject_id: record.subject_id,
                reference: format!(
                    "session {} is attributed to subject {}",
                    attribution.session_id, attribution.subject_id
                ),
            });
        }
        attach_session_to_candidate(&mut record, &attribution.session_id)?;
        validate_staging_record(self.workspace_id(), &record)?;
        let candidate_raw = serde_json::to_string(&record)?;
        let attribution_raw = serde_json::to_string(&attribution)?;

        self.database.try_transaction(|transaction| {
            require_active_subject(transaction, &record.subject_id)?;
            let attribution = write_session_attribution(transaction, attribution, attribution_raw)?;
            let candidate =
                write_staging_candidate(transaction, self.workspace_id(), record, candidate_raw)?;
            Ok((candidate, attribution))
        })
    }

    pub fn record_session_attribution(
        &self,
        attribution: SubjectSessionAttribution,
    ) -> Result<SubjectSessionAttribution> {
        validate_session_attribution(&attribution)?;
        let raw = serde_json::to_string(&attribution)?;
        self.database.try_transaction(|transaction| {
            require_active_subject(transaction, &attribution.subject_id)?;
            write_session_attribution(transaction, attribution, raw)
        })
    }

    pub fn session_attribution(
        &self,
        session_id: &str,
    ) -> Result<Option<SubjectSessionAttribution>> {
        validate_label("session id", session_id)?;
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT record_json FROM subject_session_attributions
                     WHERE session_id = ?1",
                    [session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_session_attribution(&raw)).transpose()
        })
    }

    pub fn subject_session_attributions(
        &self,
        subject_id: &str,
    ) -> Result<Vec<SubjectSessionAttribution>> {
        validate_label("subject id", subject_id)?;
        self.database.try_with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT record_json FROM subject_session_attributions
                 WHERE subject_id = ?1
                 ORDER BY attributed_at ASC, session_id ASC",
            )?;
            let rows = statement.query_map([subject_id], |row| row.get::<_, String>(0))?;
            let mut records = Vec::new();
            for row in rows {
                records.push(parse_session_attribution(&row?)?);
            }
            Ok(records)
        })
    }

    pub fn staging_candidate(
        &self,
        subject_id: &str,
        candidate_id: &str,
    ) -> Result<Option<SubjectStagingRecord>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT record_json FROM staging_records
                     WHERE subject_id = ?1 AND candidate_id = ?2",
                    params![subject_id, candidate_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_staging(self.workspace_id(), &raw))
                .transpose()
        })
    }

    pub fn create_memory(&self, subject_id: &str, draft: MemoryDraft) -> Result<MemoryRecord> {
        validate_direct_memory_draft(&draft)?;
        if draft.state != MemoryState::Active {
            return Err(SubjektivError::InvalidRecord(
                "a new Memory must start in active state".to_string(),
            ));
        }
        let memory_id = issued_id("memory");
        self.database.try_transaction(|transaction| {
            write_memory_revision(transaction, subject_id, &memory_id, None, draft)
        })
    }

    pub fn revise_memory(
        &self,
        subject_id: &str,
        memory_id: &str,
        expected_revision: u64,
        draft: MemoryDraft,
    ) -> Result<MemoryRecord> {
        validate_direct_memory_draft(&draft)?;
        self.database.try_transaction(|transaction| {
            write_memory_revision(
                transaction,
                subject_id,
                memory_id,
                Some(expected_revision),
                draft,
            )
        })
    }

    /// Atomically writes a Memory revision and one candidate's immutable
    /// resolution. Any conflict or invalid reference rolls both changes back.
    pub fn apply_candidate(
        &self,
        subject_id: &str,
        candidate_id: &str,
        target: MemoryRevisionTarget,
        draft: MemoryDraft,
        reason: impl Into<String>,
    ) -> Result<(MemoryRecord, StagingResolution)> {
        let candidate_ids = vec![candidate_id.to_string()];
        let (memory, mut resolutions) =
            self.apply_candidates(subject_id, &candidate_ids, target, draft, reason)?;
        Ok((memory, resolutions.remove(0)))
    }

    /// Atomically applies one or more candidates to exactly one new Memory
    /// revision. The repository derives `source_candidate_ids` from this list so
    /// every applied resolution has a reciprocal immutable provenance edge.
    pub fn apply_candidates(
        &self,
        subject_id: &str,
        candidate_ids: &[String],
        target: MemoryRevisionTarget,
        mut draft: MemoryDraft,
        reason: impl Into<String>,
    ) -> Result<(MemoryRecord, Vec<StagingResolution>)> {
        if candidate_ids.is_empty() {
            return Err(SubjektivError::InvalidRecord(
                "candidate application requires at least one candidate".into(),
            ));
        }
        reject_duplicates("applied candidate", candidate_ids)?;
        for candidate_id in candidate_ids {
            validate_label("candidate id", candidate_id)?;
        }
        draft.source_candidate_ids = candidate_ids.to_vec();
        let reason = reason.into();
        validate_nonempty("resolution reason", &reason)?;
        let new_memory_id = issued_id("memory");
        self.database.try_transaction(|transaction| {
            for candidate_id in candidate_ids {
                require_unresolved_candidate(transaction, subject_id, candidate_id)?;
            }
            let memory = match &target {
                MemoryRevisionTarget::Create => {
                    if draft.state != MemoryState::Active {
                        return Err(SubjektivError::InvalidRecord(
                            "a new Memory must start in active state".to_string(),
                        ));
                    }
                    write_memory_revision(transaction, subject_id, &new_memory_id, None, draft)?
                }
                MemoryRevisionTarget::Revise {
                    memory_id,
                    expected_revision,
                } => write_memory_revision(
                    transaction,
                    subject_id,
                    memory_id,
                    Some(*expected_revision),
                    draft,
                )?,
            };
            let reference = MemoryRevisionRef {
                memory_id: memory.id.clone(),
                revision: memory.revision,
            };
            let resolutions = candidate_ids
                .iter()
                .map(|candidate_id| {
                    insert_resolution(
                        transaction,
                        subject_id,
                        candidate_id,
                        StagingResolutionAction::Applied,
                        &reason,
                        std::slice::from_ref(&reference),
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((memory, resolutions))
        })
    }

    pub fn resolve_candidate(
        &self,
        subject_id: &str,
        candidate_id: &str,
        action: StagingResolutionAction,
        reason: impl Into<String>,
        affected_memory: Vec<MemoryRevisionRef>,
    ) -> Result<StagingResolution> {
        let reason = reason.into();
        validate_nonempty("resolution reason", &reason)?;
        if action == StagingResolutionAction::Applied {
            return Err(SubjektivError::InvalidRecord(
                "applied candidates must use apply_candidate(s) so Memory and provenance commit atomically"
                    .to_string(),
            ));
        }
        self.database.try_transaction(|transaction| {
            insert_resolution(
                transaction,
                subject_id,
                candidate_id,
                action,
                &reason,
                &affected_memory,
            )
        })
    }

    pub fn staging_resolution(
        &self,
        subject_id: &str,
        candidate_id: &str,
    ) -> Result<Option<StagingResolution>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT resolution_json FROM staging_resolutions
                     WHERE subject_id = ?1 AND candidate_id = ?2",
                    params![subject_id, candidate_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_resolution(&raw)).transpose()
        })
    }

    pub fn memory(&self, subject_id: &str, memory_id: &str) -> Result<Option<MemoryRecord>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT record_json FROM memory_records
                     WHERE subject_id = ?1 AND memory_id = ?2",
                    params![subject_id, memory_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_memory(&raw)).transpose()
        })
    }

    pub fn memory_revision(
        &self,
        subject_id: &str,
        memory_id: &str,
        revision: u64,
    ) -> Result<Option<MemoryRecord>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT record_json FROM memory_revisions
                     WHERE subject_id = ?1 AND memory_id = ?2 AND revision = ?3",
                    params![subject_id, memory_id, to_i64(revision)?],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_memory(&raw)).transpose()
        })
    }

    pub fn create_surface_snapshot(
        &self,
        subject_id: &str,
        body_md: impl Into<String>,
        memory_refs: Vec<MemoryRevisionRef>,
        built_from_store_revision: u64,
    ) -> Result<SurfaceSnapshot> {
        reject_duplicate_refs("surface snapshot", &memory_refs)?;
        reject_duplicate_memory_ids("surface snapshot", &memory_refs)?;
        let snapshot = SurfaceSnapshot {
            schema_version: SUBJEKTIV_SCHEMA_VERSION,
            id: issued_id("surface"),
            subject_id: subject_id.to_string(),
            body_md: body_md.into(),
            memory_refs,
            built_from_store_revision,
            created_at: now(),
        };
        let raw = serde_json::to_string(&snapshot)?;
        self.database.try_transaction(|transaction| {
            let subject = require_active_subject(transaction, subject_id)?;
            if subject.store_revision != built_from_store_revision {
                return Err(SubjektivError::InvalidRecord(format!(
                    "surface snapshot generation {built_from_store_revision} does not match current subject store revision {}",
                    subject.store_revision
                )));
            }
            validate_memory_refs(transaction, subject_id, &snapshot.memory_refs)?;
            transaction.execute(
                "INSERT INTO surface_snapshots (
                    subject_id, snapshot_id, built_from_store_revision, snapshot_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    subject_id,
                    snapshot.id,
                    to_i64(snapshot.built_from_store_revision)?,
                    raw,
                    snapshot.created_at
                ],
            )?;
            for reference in &snapshot.memory_refs {
                transaction.execute(
                    "INSERT INTO surface_snapshot_refs (
                        subject_id, snapshot_id, memory_id, revision
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        subject_id,
                        snapshot.id,
                        reference.memory_id,
                        to_i64(reference.revision)?
                    ],
                )?;
            }
            transaction.execute(
                "INSERT INTO surface_snapshot_seals (subject_id, snapshot_id)
                 VALUES (?1, ?2)",
                params![subject_id, snapshot.id],
            )?;
            Ok(snapshot)
        })
    }

    pub fn surface_snapshot(
        &self,
        subject_id: &str,
        snapshot_id: &str,
    ) -> Result<Option<SurfaceSnapshot>> {
        self.database.try_with_connection(|connection| {
            let raw = connection
                .query_row(
                    "SELECT snapshot_json FROM surface_snapshots
                     WHERE subject_id = ?1 AND snapshot_id = ?2",
                    params![subject_id, snapshot_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            raw.map(|raw| parse_surface_snapshot(&raw)).transpose()
        })
    }
}

fn write_memory_revision(
    transaction: &Transaction<'_>,
    subject_id: &str,
    memory_id: &str,
    expected_revision: Option<u64>,
    draft: MemoryDraft,
) -> Result<MemoryRecord> {
    validate_memory_draft(&draft)?;
    let mut subject = require_active_subject(transaction, subject_id)?;
    let current_raw = transaction
        .query_row(
            "SELECT record_json FROM memory_records
             WHERE subject_id = ?1 AND memory_id = ?2",
            params![subject_id, memory_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let current = current_raw.map(|raw| parse_memory(&raw)).transpose()?;

    let (revision, created_at) = match (current.as_ref(), expected_revision) {
        (None, None) => (1, now()),
        (None, Some(_)) => return Err(SubjektivError::MemoryNotFound(memory_id.to_string())),
        (Some(_), None) => {
            return Err(SubjektivError::InvalidRecord(format!(
                "memory `{memory_id}` already exists"
            )));
        }
        (Some(current), Some(expected)) if current.revision != expected => {
            return Err(SubjektivError::RevisionConflict {
                memory_id: memory_id.to_string(),
                expected,
                actual: current.revision,
            });
        }
        (Some(current), Some(_)) => {
            validate_state_transition(memory_id, current.state, draft.state)?;
            (current.revision + 1, current.created_at.clone())
        }
    };

    reject_duplicates("source candidate", &draft.source_candidate_ids)?;
    reject_duplicate_refs("derived Memory", &draft.derived_from)?;
    validate_candidate_refs(transaction, subject_id, &draft.source_candidate_ids)?;
    validate_memory_refs(transaction, subject_id, &draft.derived_from)?;

    let timestamp = now();
    let record = MemoryRecord {
        schema_version: SUBJEKTIV_SCHEMA_VERSION,
        id: memory_id.to_string(),
        subject_id: subject_id.to_string(),
        revision,
        kind: draft.kind,
        state: draft.state,
        claim: draft.claim,
        body_md: draft.body_md,
        why_useful: draft.why_useful,
        staleness: draft.staleness,
        source_candidate_ids: draft.source_candidate_ids,
        derived_from: draft.derived_from,
        change_reason: draft.change_reason,
        created_at,
        updated_at: timestamp,
    };
    let raw = serde_json::to_string(&record)?;

    if current.is_none() {
        transaction.execute(
            "INSERT INTO memory_records (
                subject_id, memory_id, current_revision, kind, state,
                record_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                subject_id,
                memory_id,
                to_i64(revision)?,
                candidate_kind_name(&record.kind),
                memory_state_name(record.state),
                raw,
                record.created_at,
                record.updated_at
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO memory_revisions (
            subject_id, memory_id, revision, kind, state, record_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            subject_id,
            memory_id,
            to_i64(revision)?,
            candidate_kind_name(&record.kind),
            memory_state_name(record.state),
            raw,
            record.updated_at
        ],
    )?;
    if current.is_some() {
        transaction.execute(
            "UPDATE memory_records
             SET current_revision = ?3, kind = ?4, state = ?5,
                 record_json = ?6, updated_at = ?7
             WHERE subject_id = ?1 AND memory_id = ?2",
            params![
                subject_id,
                memory_id,
                to_i64(revision)?,
                candidate_kind_name(&record.kind),
                memory_state_name(record.state),
                raw,
                record.updated_at
            ],
        )?;
    }
    for candidate_id in &record.source_candidate_ids {
        transaction.execute(
            "INSERT INTO memory_revision_candidates (
                subject_id, memory_id, revision, candidate_id
             ) VALUES (?1, ?2, ?3, ?4)",
            params![subject_id, memory_id, to_i64(revision)?, candidate_id],
        )?;
    }
    for source in &record.derived_from {
        transaction.execute(
            "INSERT INTO memory_revision_derivations (
                subject_id, memory_id, revision, source_memory_id, source_revision
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                subject_id,
                memory_id,
                to_i64(revision)?,
                source.memory_id,
                to_i64(source.revision)?
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO memory_revision_seals (subject_id, memory_id, revision)
         VALUES (?1, ?2, ?3)",
        params![subject_id, memory_id, to_i64(revision)?],
    )?;

    subject.store_revision = subject
        .store_revision
        .checked_add(1)
        .ok_or_else(|| SubjektivError::InvalidRecord("subject store revision overflow".into()))?;
    subject.updated_at = record.updated_at.clone();
    let subject_raw = serde_json::to_string(&subject)?;
    transaction.execute(
        "UPDATE subjects
         SET store_revision = ?2, record_json = ?3, updated_at = ?4
         WHERE subject_id = ?1",
        params![
            subject_id,
            to_i64(subject.store_revision)?,
            subject_raw,
            subject.updated_at
        ],
    )?;
    Ok(record)
}

fn write_staging_candidate(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    record: SubjectStagingRecord,
    raw: String,
) -> Result<SubjectStagingRecord> {
    let existing = transaction
        .query_row(
            "SELECT record_json FROM staging_records
             WHERE subject_id = ?1 AND candidate_id = ?2",
            params![record.subject_id, record.id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if equivalent_staging_payload(&existing, &raw)? {
            return parse_staging(workspace_id, &existing);
        }
        return Err(SubjektivError::CandidateConflict(record.id.clone()));
    }
    transaction.execute(
        "INSERT INTO staging_records (
            subject_id, candidate_id, kind, record_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            record.subject_id,
            record.id,
            candidate_kind_name(&record.kind),
            raw,
            record.created_at
        ],
    )?;
    Ok(record)
}

fn write_session_attribution(
    transaction: &Transaction<'_>,
    record: SubjectSessionAttribution,
    raw: String,
) -> Result<SubjectSessionAttribution> {
    let existing = transaction
        .query_row(
            "SELECT record_json FROM subject_session_attributions WHERE session_id = ?1",
            [&record.session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        let existing = parse_session_attribution(&existing)?;
        if existing.subject_id != record.subject_id {
            return Err(SubjektivError::SessionSubjectConflict {
                session_id: record.session_id,
                existing_subject_id: existing.subject_id,
                requested_subject_id: record.subject_id,
            });
        }
        if equivalent_session_attribution(&existing, &record) {
            return Ok(existing);
        }
        return Err(SubjektivError::SessionAttributionConflict(
            record.session_id,
        ));
    }
    transaction.execute(
        "INSERT INTO subject_session_attributions (
            session_id, subject_id, runtime_id, worker_id, record_json, attributed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            record.session_id,
            record.subject_id,
            record.runtime_id,
            record.worker_id,
            raw,
            record.attributed_at
        ],
    )?;
    Ok(record)
}

fn insert_resolution(
    transaction: &Transaction<'_>,
    subject_id: &str,
    candidate_id: &str,
    action: StagingResolutionAction,
    reason: &str,
    affected_memory: &[MemoryRevisionRef],
) -> Result<StagingResolution> {
    require_active_subject(transaction, subject_id)?;
    reject_duplicate_refs("staging resolution", affected_memory)?;
    validate_memory_refs(transaction, subject_id, affected_memory)?;
    if action == StagingResolutionAction::Applied {
        if affected_memory.is_empty() {
            return Err(SubjektivError::InvalidRecord(
                "an applied candidate requires an affected Memory revision".into(),
            ));
        }
        for reference in affected_memory {
            let reciprocal = transaction
                .query_row(
                    "SELECT 1 FROM memory_revision_candidates
                     WHERE subject_id = ?1 AND memory_id = ?2
                       AND revision = ?3 AND candidate_id = ?4",
                    params![
                        subject_id,
                        reference.memory_id,
                        to_i64(reference.revision)?,
                        candidate_id
                    ],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !reciprocal {
                return Err(SubjektivError::InvalidRecord(format!(
                    "applied candidate `{candidate_id}` has no reciprocal provenance edge to `{}@{}`",
                    reference.memory_id, reference.revision
                )));
            }
        }
    }
    let candidate_raw = require_unresolved_candidate(transaction, subject_id, candidate_id)?;
    let candidate = parse_staging_without_workspace(&candidate_raw)?;
    if candidate.subject_id != subject_id {
        return Err(SubjektivError::SubjectScopeMismatch {
            subject_id: subject_id.to_string(),
            reference: candidate_id.to_string(),
        });
    }
    let resolution = StagingResolution {
        schema_version: SUBJEKTIV_SCHEMA_VERSION,
        id: issued_id("resolution"),
        subject_id: subject_id.to_string(),
        candidate_id: candidate_id.to_string(),
        action,
        reason: reason.to_string(),
        affected_memory: affected_memory.to_vec(),
        candidate,
        resolved_at: now(),
    };
    let affected_raw = serde_json::to_string(affected_memory)?;
    let resolution_raw = serde_json::to_string(&resolution)?;
    transaction.execute(
        "INSERT INTO staging_resolutions (
            subject_id, candidate_id, resolution_id, action, reason,
            affected_refs_json, staging_raw_json, resolution_json, resolved_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            subject_id,
            candidate_id,
            resolution.id,
            resolution_action_name(action),
            reason,
            affected_raw,
            candidate_raw,
            resolution_raw,
            resolution.resolved_at
        ],
    )?;
    for reference in affected_memory {
        transaction.execute(
            "INSERT INTO staging_resolution_targets (
                subject_id, candidate_id, memory_id, revision
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                subject_id,
                candidate_id,
                reference.memory_id,
                to_i64(reference.revision)?
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO staging_resolution_seals (subject_id, candidate_id)
         VALUES (?1, ?2)",
        params![subject_id, candidate_id],
    )?;
    Ok(resolution)
}

fn require_unresolved_candidate(
    transaction: &Transaction<'_>,
    subject_id: &str,
    candidate_id: &str,
) -> Result<String> {
    let raw = transaction
        .query_row(
            "SELECT record_json FROM staging_records
             WHERE subject_id = ?1 AND candidate_id = ?2",
            params![subject_id, candidate_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| SubjektivError::CandidateNotFound(candidate_id.to_string()))?;
    let resolved = transaction
        .query_row(
            "SELECT 1 FROM staging_resolutions
             WHERE subject_id = ?1 AND candidate_id = ?2",
            params![subject_id, candidate_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if resolved {
        return Err(SubjektivError::CandidateResolved(candidate_id.to_string()));
    }
    Ok(raw)
}

fn require_subject(transaction: &Transaction<'_>, subject_id: &str) -> Result<SubjectRecord> {
    let raw = transaction
        .query_row(
            "SELECT record_json FROM subjects WHERE subject_id = ?1",
            [subject_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| SubjektivError::SubjectNotFound(subject_id.to_string()))?;
    parse_subject(&raw)
}

fn require_active_subject(
    transaction: &Transaction<'_>,
    subject_id: &str,
) -> Result<SubjectRecord> {
    let subject = require_subject(transaction, subject_id)?;
    if subject.state == SubjectState::Retired {
        return Err(SubjektivError::SubjectRetired(subject_id.to_string()));
    }
    Ok(subject)
}

fn validate_candidate_refs(
    transaction: &Transaction<'_>,
    subject_id: &str,
    candidate_ids: &[String],
) -> Result<()> {
    for candidate_id in candidate_ids {
        let exists = transaction
            .query_row(
                "SELECT 1 FROM staging_records
                 WHERE subject_id = ?1 AND candidate_id = ?2",
                params![subject_id, candidate_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(SubjektivError::SubjectScopeMismatch {
                subject_id: subject_id.to_string(),
                reference: candidate_id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_memory_refs(
    transaction: &Transaction<'_>,
    subject_id: &str,
    references: &[MemoryRevisionRef],
) -> Result<()> {
    for reference in references {
        if reference.revision == 0 {
            return Err(SubjektivError::InvalidRecord(format!(
                "Memory `{}` revision must be greater than zero",
                reference.memory_id
            )));
        }
        let exists = transaction
            .query_row(
                "SELECT 1 FROM memory_revisions
                 WHERE subject_id = ?1 AND memory_id = ?2 AND revision = ?3",
                params![subject_id, reference.memory_id, to_i64(reference.revision)?],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(SubjektivError::SubjectScopeMismatch {
                subject_id: subject_id.to_string(),
                reference: format!("{}@{}", reference.memory_id, reference.revision),
            });
        }
    }
    Ok(())
}

fn validate_session_attribution(record: &SubjectSessionAttribution) -> Result<()> {
    validate_schema_version(record.schema_version, "subject Session attribution")?;
    validate_label("subject id", &record.subject_id)?;
    validate_label("runtime id", &record.runtime_id)?;
    validate_label("worker id", &record.worker_id)?;
    validate_label("session id", &record.session_id)?;
    validate_label("Session attribution timestamp", &record.attributed_at)?;
    chrono::DateTime::parse_from_rfc3339(&record.attributed_at).map_err(|error| {
        SubjektivError::InvalidRecord(format!(
            "Session attribution timestamp is not RFC 3339: {error}"
        ))
    })?;
    Ok(())
}

fn attach_session_to_candidate(record: &mut SubjectStagingRecord, session_id: &str) -> Result<()> {
    if record.source_refs.is_empty() {
        record.source_refs.push(SourceEvidenceRef {
            session_id: Some(session_id.to_string()),
            segment_id: Some(record.source.segment_id.clone()),
            entry_range: Some(record.source.range),
            ..SourceEvidenceRef::default()
        });
        return Ok(());
    }
    for source in &mut record.source_refs {
        match source.session_id.as_deref() {
            Some(existing) if existing != session_id => {
                return Err(SubjektivError::SubjectScopeMismatch {
                    subject_id: record.subject_id.clone(),
                    reference: format!(
                        "candidate source Session `{existing}` does not match attributed Session `{session_id}`"
                    ),
                });
            }
            Some(_) => {}
            None => source.session_id = Some(session_id.to_string()),
        }
    }
    Ok(())
}

fn validate_staging_record(workspace_id: &str, record: &SubjectStagingRecord) -> Result<()> {
    if record.schema_version != STAGING_SCHEMA_VERSION {
        return Err(SubjektivError::InvalidRecord(format!(
            "staging schema version {} is unsupported",
            record.schema_version
        )));
    }
    validate_label("candidate id", &record.id)?;
    validate_label("subject id", &record.subject_id)?;
    validate_label("extract run id", &record.extract_run_id)?;
    validate_label("source segment id", &record.source.segment_id)?;
    validate_nonempty("candidate claim", &record.claim)?;
    validate_nonempty("candidate why_useful", &record.why_useful)?;
    validate_range("candidate source", record.source.range)?;
    if matches!(record.kind, CandidateKind::Preference)
        && record.evidence.iter().any(|evidence| {
            !matches!(
                evidence.origin.as_ref().map(|origin| &origin.kind),
                Some(memory::schema::EvidenceOriginKind::HumanInput)
            )
        })
    {
        return Err(SubjektivError::InvalidRecord(
            "preference candidates require exclusively HumanInput evidence".to_string(),
        ));
    }

    let mut evidence_ids = HashSet::new();
    for evidence in &record.evidence {
        validate_label("evidence id", &evidence.id)?;
        if !evidence_ids.insert(evidence.id.as_str()) {
            return Err(SubjektivError::InvalidRecord(format!(
                "duplicate evidence id `{}`",
                evidence.id
            )));
        }
        validate_nonempty("evidence kind", evidence.kind.as_str())?;
        if let Some(range) = evidence.entry_range {
            validate_range("evidence entry", range)?;
        }
    }
    for source in &record.source_refs {
        if let Some(session_id) = source.session_id.as_deref() {
            validate_label("source session id", session_id)?;
        }
        if let Some(segment_id) = source.segment_id.as_deref() {
            validate_label("source segment id", segment_id)?;
        }
        if let Some(range) = source.entry_range {
            validate_range("source evidence entry", range)?;
        }
        if let Some(evidence_id) = source.evidence_id.as_deref() {
            validate_label("source evidence id", evidence_id)?;
            if !evidence_ids.contains(evidence_id) {
                return Err(SubjektivError::InvalidRecord(format!(
                    "source evidence id `{evidence_id}` is not present in evidence"
                )));
            }
        }
        if let Some(kind) = source.evidence_kind.as_ref() {
            validate_nonempty("source evidence kind", kind.as_str())?;
        }
    }
    for origin in record
        .evidence
        .iter()
        .filter_map(|evidence| evidence.origin.as_ref())
        .chain(
            record
                .source_refs
                .iter()
                .filter_map(|source| source.origin.as_ref()),
        )
    {
        if let Some(origin_workspace) = origin.workspace_id.as_deref()
            && origin_workspace != workspace_id
        {
            return Err(SubjektivError::SubjectScopeMismatch {
                subject_id: record.subject_id.clone(),
                reference: format!("evidence Workspace {origin_workspace}"),
            });
        }
    }
    Ok(())
}

fn validate_memory_draft(draft: &MemoryDraft) -> Result<()> {
    validate_nonempty("Memory claim", &draft.claim)?;
    validate_nonempty("Memory why_useful", &draft.why_useful)?;
    validate_nonempty("Memory change_reason", &draft.change_reason)?;
    for candidate_id in &draft.source_candidate_ids {
        validate_label("source candidate id", candidate_id)?;
    }
    for reference in &draft.derived_from {
        validate_label("derived Memory id", &reference.memory_id)?;
    }
    Ok(())
}

fn validate_direct_memory_draft(draft: &MemoryDraft) -> Result<()> {
    if draft.source_candidate_ids.is_empty() {
        Ok(())
    } else {
        Err(SubjektivError::InvalidRecord(
            "candidate provenance must be committed through apply_candidate(s)".into(),
        ))
    }
}

fn validate_state_transition(memory_id: &str, from: MemoryState, to: MemoryState) -> Result<()> {
    let allowed = from != MemoryState::Retracted;
    if allowed {
        Ok(())
    } else {
        Err(SubjektivError::InvalidStateTransition {
            memory_id: memory_id.to_string(),
            from,
            to,
        })
    }
}

fn parse_subject(raw: &str) -> Result<SubjectRecord> {
    let record: SubjectRecord = serde_json::from_str(raw)?;
    validate_schema_version(record.schema_version, "subject")?;
    Ok(record)
}

fn equivalent_session_attribution(
    left: &SubjectSessionAttribution,
    right: &SubjectSessionAttribution,
) -> bool {
    left.schema_version == right.schema_version
        && left.subject_id == right.subject_id
        && left.runtime_id == right.runtime_id
        && left.worker_id == right.worker_id
        && left.session_id == right.session_id
}

fn equivalent_staging_payload(left: &str, right: &str) -> Result<bool> {
    let mut left = serde_json::from_str::<serde_json::Value>(left)?;
    let mut right = serde_json::from_str::<serde_json::Value>(right)?;
    let left_object = left.as_object_mut().ok_or_else(|| {
        SubjektivError::InvalidRecord("stored staging record is not a JSON object".into())
    })?;
    let right_object = right.as_object_mut().ok_or_else(|| {
        SubjektivError::InvalidRecord("new staging record is not a JSON object".into())
    })?;
    // A retry may reconstruct the host envelope after an ambiguous response.
    // Identity and extracted content must match, while the first persisted
    // host-issued staging timestamp remains authoritative.
    left_object.remove("created_at");
    right_object.remove("created_at");
    Ok(left == right)
}

fn parse_session_attribution(raw: &str) -> Result<SubjectSessionAttribution> {
    let record: SubjectSessionAttribution = serde_json::from_str(raw)?;
    validate_session_attribution(&record)?;
    Ok(record)
}

fn parse_staging(workspace_id: &str, raw: &str) -> Result<SubjectStagingRecord> {
    let record = parse_staging_without_workspace(raw)?;
    validate_staging_record(workspace_id, &record)?;
    Ok(record)
}

fn parse_staging_without_workspace(raw: &str) -> Result<SubjectStagingRecord> {
    let record: SubjectStagingRecord = serde_json::from_str(raw)?;
    if record.schema_version != STAGING_SCHEMA_VERSION {
        return Err(SubjektivError::InvalidRecord(format!(
            "staging schema version {} is unsupported",
            record.schema_version
        )));
    }
    Ok(record)
}

fn parse_memory(raw: &str) -> Result<MemoryRecord> {
    let record: MemoryRecord = serde_json::from_str(raw)?;
    validate_schema_version(record.schema_version, "Memory")?;
    if record.revision == 0 {
        return Err(SubjektivError::InvalidRecord(
            "Memory revision must be greater than zero".to_string(),
        ));
    }
    Ok(record)
}

fn parse_resolution(raw: &str) -> Result<StagingResolution> {
    let record: StagingResolution = serde_json::from_str(raw)?;
    validate_schema_version(record.schema_version, "staging resolution")?;
    Ok(record)
}

fn parse_surface_snapshot(raw: &str) -> Result<SurfaceSnapshot> {
    let record: SurfaceSnapshot = serde_json::from_str(raw)?;
    validate_schema_version(record.schema_version, "surface snapshot")?;
    Ok(record)
}

fn validate_schema_version(version: u32, kind: &str) -> Result<()> {
    if version == SUBJEKTIV_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(SubjektivError::InvalidRecord(format!(
            "{kind} schema version {version} is unsupported"
        )))
    }
}

fn validate_range(kind: &str, range: [u64; 2]) -> Result<()> {
    if range[0] <= range[1] {
        Ok(())
    } else {
        Err(SubjektivError::InvalidRecord(format!(
            "{kind} range is reversed"
        )))
    }
}

fn validate_label(kind: &str, value: &str) -> Result<()> {
    validate_nonempty(kind, value)?;
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(SubjektivError::InvalidRecord(format!(
            "{kind} must be at most 256 bytes and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_nonempty(kind: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(SubjektivError::InvalidRecord(format!(
            "{kind} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn reject_duplicates(kind: &str, values: &[String]) -> Result<()> {
    let mut seen = HashSet::new();
    if let Some(duplicate) = values.iter().find(|value| !seen.insert(value.as_str())) {
        return Err(SubjektivError::InvalidRecord(format!(
            "duplicate {kind} `{duplicate}`"
        )));
    }
    Ok(())
}

fn reject_duplicate_refs(kind: &str, values: &[MemoryRevisionRef]) -> Result<()> {
    let mut seen = HashSet::new();
    if let Some(duplicate) = values
        .iter()
        .find(|value| !seen.insert((value.memory_id.as_str(), value.revision)))
    {
        return Err(SubjektivError::InvalidRecord(format!(
            "duplicate {kind} `{}@{}`",
            duplicate.memory_id, duplicate.revision
        )));
    }
    Ok(())
}

fn reject_duplicate_memory_ids(kind: &str, values: &[MemoryRevisionRef]) -> Result<()> {
    let mut seen = HashSet::new();
    if let Some(duplicate) = values
        .iter()
        .find(|value| !seen.insert(value.memory_id.as_str()))
    {
        return Err(SubjektivError::InvalidRecord(format!(
            "duplicate {kind} Memory `{}`",
            duplicate.memory_id
        )));
    }
    Ok(())
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        SubjektivError::InvalidRecord(format!("revision value {value} exceeds SQLite range"))
    })
}

fn issued_id(kind: &str) -> String {
    format!("{kind}-{}", Uuid::now_v7())
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn candidate_kind_name(kind: &CandidateKind) -> &'static str {
    kind.as_str()
}

fn memory_state_name(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Active => "active",
        MemoryState::Resolved => "resolved",
        MemoryState::Retracted => "retracted",
    }
}

fn resolution_action_name(action: StagingResolutionAction) -> &'static str {
    match action {
        StagingResolutionAction::Applied => "applied",
        StagingResolutionAction::Discarded => "discarded",
        StagingResolutionAction::Invalid => "invalid",
        StagingResolutionAction::Duplicate => "duplicate",
        StagingResolutionAction::AlreadyCovered => "already_covered",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use memory::extract::{ExtractedCandidate, StagingEvidence};
    use memory::schema::{
        EvidenceKind, EvidenceOrigin, EvidenceOriginKind, SourceEvidenceRef, SourceRef,
    };

    use super::*;
    use crate::FeatureStorage;

    static V1_MIGRATIONS: &[FeatureMigration] = &[FeatureMigration::new(
        1,
        "create subjektiv subject memory store",
        create_schema,
    )];

    fn role() -> SubjectRole {
        SubjectRole::new("workspace_companion").unwrap()
    }

    fn candidate(subject_id: &str, candidate_id: &str, workspace_id: &str) -> SubjectStagingRecord {
        let origin = EvidenceOrigin {
            kind: EvidenceOriginKind::HumanInput,
            account_id: Some("account-1".into()),
            workspace_id: Some(workspace_id.into()),
            runtime_id: None,
            worker_id: None,
            flow_selector: None,
            flow_definition_id: None,
            flow_definition_revision: None,
        };
        let record = StagingRecord::from_candidate(
            candidate_id,
            "extract-run-1",
            SourceRef {
                segment_id: "segment-1".into(),
                range: [10, 12],
            },
            ExtractedCandidate {
                kind: CandidateKind::Constraint,
                claim: "Do not treat Memory text as authority".into(),
                why_useful: "Keeps experience separate from authorization".into(),
                staleness: Some("Revisit if the authority model changes".into()),
                evidence_ids: vec!["evidence-1".into()],
            },
            vec![StagingEvidence {
                id: "evidence-1".into(),
                kind: EvidenceKind::new(EvidenceKind::MESSAGE),
                entry_range: Some([10, 12]),
                origin: Some(origin.clone()),
                excerpt: Some("Memory is not authority".into()),
                summary: Some("A bounded source summary".into()),
            }],
            vec![SourceEvidenceRef {
                session_id: Some("session-1".into()),
                segment_id: Some("segment-1".into()),
                entry_range: Some([10, 12]),
                evidence_id: Some("evidence-1".into()),
                origin: Some(origin),
                evidence_kind: Some(EvidenceKind::new(EvidenceKind::MESSAGE)),
                label: Some("design discussion".into()),
                summary: Some("Authority boundary".into()),
            }],
        );
        SubjectStagingRecord::attach_at(subject_id, record, "2026-09-28T10:00:00.000Z".into())
    }

    fn attribution(
        subject_id: &str,
        runtime_id: &str,
        worker_id: &str,
        session_id: &str,
        attributed_at: &str,
    ) -> SubjectSessionAttribution {
        let mut record =
            SubjectSessionAttribution::new(subject_id, runtime_id, worker_id, session_id).unwrap();
        record.attributed_at = attributed_at.to_string();
        record
    }

    fn draft(claim: &str, reason: &str) -> MemoryDraft {
        MemoryDraft::active(
            CandidateKind::Constraint,
            claim,
            "Context, applicability, and consequences.",
            "It preserves the subject's learned boundary.",
            reason,
        )
    }

    fn open_store(
        root: &std::path::Path,
        workspace_id: &str,
    ) -> (FeatureStorage, WorkspaceFeatureStorage, SubjektivStore) {
        let manager = FeatureStorage::new(root);
        let workspace = manager.workspace(workspace_id).unwrap();
        let registration = SubjektivStore::register(&workspace).unwrap();
        let store = SubjektivStore::open(&workspace, &registration).unwrap();
        (manager, workspace, store)
    }

    #[test]
    fn versioned_json_roundtrips_and_rejects_invalid_shapes() {
        let staging = candidate("subject-1", "candidate-1", "workspace-a");
        let raw = serde_json::to_string_pretty(&staging).unwrap();
        let decoded: SubjectStagingRecord = serde_json::from_str(&raw).unwrap();
        assert_eq!(decoded.schema_version, STAGING_SCHEMA_VERSION);
        assert_eq!(decoded.subject_id, "subject-1");
        assert_eq!(
            decoded.source_refs[0].session_id.as_deref(),
            Some("session-1")
        );
        assert!(raw.contains("human_input"));

        let mut value = serde_json::to_value(&staging).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("invented".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<SubjectStagingRecord>(value).is_err());

        let memory = MemoryRecord {
            schema_version: SUBJEKTIV_SCHEMA_VERSION,
            id: "memory-1".into(),
            subject_id: "subject-1".into(),
            revision: 1,
            kind: CandidateKind::Constraint,
            state: MemoryState::Active,
            claim: "Memory is not authority".into(),
            body_md: "Applies to every tool call.".into(),
            why_useful: "Avoids privilege confusion".into(),
            staleness: None,
            source_candidate_ids: vec!["candidate-1".into()],
            derived_from: vec![],
            change_reason: "Accepted the staged constraint".into(),
            created_at: "2026-09-28T10:00:00.000Z".into(),
            updated_at: "2026-09-28T10:00:00.000Z".into(),
        };
        let raw = serde_json::to_string(&memory).unwrap();
        let decoded: MemoryRecord = serde_json::from_str(&raw).unwrap();
        assert_eq!(decoded.revision, 1);
        assert!(serde_json::from_str::<MemoryRecord>(&raw.replace("active", "unknown")).is_err());
    }

    #[test]
    fn v1_store_migrates_before_attributed_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let subject_id = {
            let manager = FeatureStorage::new(&root);
            let workspace = manager.workspace("workspace-a").unwrap();
            let registration = workspace
                .register(FeatureRegistration::new(
                    SUBJEKTIV_FEATURE_ID,
                    V1_MIGRATIONS,
                ))
                .unwrap();
            let store = SubjektivStore::open(&workspace, &registration).unwrap();
            store.create_subject(role()).unwrap().id
        };

        let (_manager, _workspace, store) = open_store(&root, "workspace-a");
        let (staged, stored_attribution) = store
            .stage_candidate_with_attribution(
                candidate(&subject_id, "candidate-after-migration", "workspace-a"),
                attribution(
                    &subject_id,
                    "runtime-1",
                    "worker-1",
                    "session-1",
                    "2026-09-28T10:00:00.000Z",
                ),
            )
            .unwrap();

        assert_eq!(staged.subject_id, subject_id);
        assert_eq!(stored_attribution.session_id, "session-1");
        assert_eq!(
            store.session_attribution("session-1").unwrap().unwrap(),
            stored_attribution
        );
    }

    #[test]
    fn attributed_staging_is_atomic_and_retry_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let first_attribution = attribution(
            &subject.id,
            "runtime-1",
            "worker-1",
            "session-1",
            "2026-09-28T10:00:00.000Z",
        );
        let (first_candidate, first_attribution) = store
            .stage_candidate_with_attribution(
                candidate(&subject.id, "candidate-1", "workspace-a"),
                first_attribution,
            )
            .unwrap();

        let mut retried_candidate = first_candidate.clone();
        retried_candidate.created_at = "2026-09-28T11:00:00.000Z".into();
        let retried_attribution = attribution(
            &subject.id,
            "runtime-1",
            "worker-1",
            "session-1",
            "2026-09-28T11:00:00.000Z",
        );
        let (retried_candidate, retried_attribution) = store
            .stage_candidate_with_attribution(retried_candidate, retried_attribution)
            .unwrap();
        assert_eq!(retried_candidate.created_at, first_candidate.created_at);
        assert_eq!(retried_attribution, first_attribution);

        let mut conflicting_candidate = candidate(&subject.id, "candidate-1", "workspace-a");
        conflicting_candidate.claim = "different content".into();
        conflicting_candidate.source_refs[0].session_id = Some("session-2".into());
        let result = store.stage_candidate_with_attribution(
            conflicting_candidate,
            attribution(
                &subject.id,
                "runtime-1",
                "worker-1",
                "session-2",
                "2026-09-28T12:00:00.000Z",
            ),
        );
        assert!(matches!(
            result,
            Err(SubjektivError::CandidateConflict(candidate_id))
                if candidate_id == "candidate-1"
        ));
        assert!(store.session_attribution("session-2").unwrap().is_none());
    }

    #[test]
    fn worker_replacement_preserves_subject_history_and_rejects_cross_subject_session() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let other_subject = store.create_subject(role()).unwrap();

        store
            .stage_candidate_with_attribution(
                candidate(&subject.id, "candidate-1", "workspace-a"),
                attribution(
                    &subject.id,
                    "runtime-1",
                    "worker-1",
                    "session-1",
                    "2026-09-28T10:00:00.000Z",
                ),
            )
            .unwrap();
        let mut replacement_candidate = candidate(&subject.id, "candidate-2", "workspace-a");
        replacement_candidate.source_refs[0].session_id = None;
        let (replacement_candidate, _) = store
            .stage_candidate_with_attribution(
                replacement_candidate,
                attribution(
                    &subject.id,
                    "runtime-2",
                    "worker-2",
                    "session-2",
                    "2026-09-28T11:00:00.000Z",
                ),
            )
            .unwrap();
        assert_eq!(
            replacement_candidate.source_refs[0].session_id.as_deref(),
            Some("session-2")
        );

        let history = store.subject_session_attributions(&subject.id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].worker_id, "worker-1");
        assert_eq!(history[1].worker_id, "worker-2");
        assert!(history.iter().all(|entry| entry.subject_id == subject.id));

        let update = store
            .database
            .try_with_connection::<_, SubjektivError>(|connection| {
                connection.execute(
                    "UPDATE subject_session_attributions SET worker_id = 'worker-other'
                     WHERE session_id = 'session-1'",
                    [],
                )?;
                Ok(())
            });
        assert!(update.is_err());
        let delete = store
            .database
            .try_with_connection::<_, SubjektivError>(|connection| {
                connection.execute(
                    "DELETE FROM subject_session_attributions WHERE session_id = 'session-1'",
                    [],
                )?;
                Ok(())
            });
        assert!(delete.is_err());

        let mut foreign_candidate =
            candidate(&other_subject.id, "candidate-foreign", "workspace-a");
        foreign_candidate.source_refs[0].session_id = Some("session-1".into());
        let result = store.stage_candidate_with_attribution(
            foreign_candidate,
            attribution(
                &other_subject.id,
                "runtime-1",
                "worker-1",
                "session-1",
                "2026-09-28T12:00:00.000Z",
            ),
        );
        assert!(matches!(
            result,
            Err(SubjektivError::SessionSubjectConflict {
                session_id,
                existing_subject_id,
                requested_subject_id,
            }) if session_id == "session-1"
                && existing_subject_id == subject.id
                && requested_subject_id == other_subject.id
        ));
        assert!(
            store
                .staging_candidate(&other_subject.id, "candidate-foreign")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn malformed_nested_provenance_is_rejected_without_staging() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();

        let mut non_human_preference =
            candidate(&subject.id, "candidate-preference", "workspace-a");
        non_human_preference.kind = CandidateKind::Preference;
        non_human_preference.evidence[0]
            .origin
            .as_mut()
            .unwrap()
            .kind = EvidenceOriginKind::ModelOutput;
        assert!(matches!(
            store.stage_candidate(non_human_preference),
            Err(SubjektivError::InvalidRecord(message))
                if message.contains("HumanInput")
        ));

        let mut reversed = candidate(&subject.id, "candidate-reversed", "workspace-a");
        reversed.evidence[0].entry_range = Some([12, 10]);
        assert!(matches!(
            store.stage_candidate(reversed),
            Err(SubjektivError::InvalidRecord(_))
        ));

        let mut duplicate = candidate(&subject.id, "candidate-duplicate", "workspace-a");
        duplicate.evidence.push(duplicate.evidence[0].clone());
        assert!(matches!(
            store.stage_candidate(duplicate),
            Err(SubjektivError::InvalidRecord(_))
        ));

        let mut dangling = candidate(&subject.id, "candidate-dangling", "workspace-a");
        dangling.source_refs[0].evidence_id = Some("missing-evidence".into());
        assert!(matches!(
            store.stage_candidate(dangling),
            Err(SubjektivError::InvalidRecord(_))
        ));
        assert!(
            store
                .staging_candidate(&subject.id, "candidate-dangling")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn candidate_apply_revision_history_and_resolution_are_atomic() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let staged = store
            .stage_candidate(candidate(&subject.id, "candidate-1", "workspace-a"))
            .unwrap();
        let mut retry = staged.clone();
        retry.created_at = "2026-09-28T11:00:00.000Z".into();
        let retried = store.stage_candidate(retry).unwrap();
        assert_eq!(retried.created_at, staged.created_at);

        let (first, resolution) = store
            .apply_candidate(
                &subject.id,
                &staged.id,
                MemoryRevisionTarget::Create,
                draft(
                    "Memory content cannot authorize operations",
                    "accepted candidate",
                ),
                "Applied as a durable constraint",
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(resolution.action, StagingResolutionAction::Applied);
        assert_eq!(resolution.affected_memory[0].revision, 1);
        assert_eq!(
            store.subject(&subject.id).unwrap().unwrap().store_revision,
            1
        );

        let unrelated = store
            .stage_candidate(candidate(&subject.id, "candidate-unrelated", "workspace-a"))
            .unwrap();
        assert!(matches!(
            store.resolve_candidate(
                &subject.id,
                &unrelated.id,
                StagingResolutionAction::Applied,
                "must not bypass atomic application",
                vec![MemoryRevisionRef {
                    memory_id: first.id.clone(),
                    revision: 1,
                }],
            ),
            Err(SubjektivError::InvalidRecord(_))
        ));
        let mut bypass = draft("Bypass provenance", "must be rejected");
        bypass.source_candidate_ids.push(unrelated.id.clone());
        assert!(matches!(
            store.create_memory(&subject.id, bypass),
            Err(SubjektivError::InvalidRecord(_))
        ));
        assert!(
            store
                .staging_resolution(&subject.id, &unrelated.id)
                .unwrap()
                .is_none()
        );

        let stale = store.revise_memory(
            &subject.id,
            &first.id,
            0,
            draft("stale write", "wrong expected revision"),
        );
        assert!(matches!(
            stale,
            Err(SubjektivError::RevisionConflict {
                expected: 0,
                actual: 1,
                ..
            })
        ));
        assert_eq!(
            store
                .memory(&subject.id, &first.id)
                .unwrap()
                .unwrap()
                .revision,
            1
        );

        let mut second_draft = draft(
            "Memory content never authorizes operations",
            "made the scope explicit",
        );
        second_draft.state = MemoryState::Resolved;
        second_draft.derived_from.push(MemoryRevisionRef {
            memory_id: first.id.clone(),
            revision: 1,
        });
        let second = store
            .revise_memory(&subject.id, &first.id, 1, second_draft)
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.state, MemoryState::Resolved);
        assert_eq!(second.derived_from[0].revision, 1);

        let old = store
            .memory_revision(&subject.id, &first.id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(old.claim, "Memory content cannot authorize operations");
        assert_eq!(old.source_candidate_ids, ["candidate-1"]);
        assert_eq!(
            store.subject(&subject.id).unwrap().unwrap().store_revision,
            2
        );

        let repeat = store.apply_candidate(
            &subject.id,
            &staged.id,
            MemoryRevisionTarget::Revise {
                memory_id: first.id.clone(),
                expected_revision: 2,
            },
            draft("must roll back", "candidate already resolved"),
            "cannot apply twice",
        );
        assert!(matches!(repeat, Err(SubjektivError::CandidateResolved(_))));
        assert_eq!(
            store
                .memory(&subject.id, &first.id)
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        assert_eq!(
            store.subject(&subject.id).unwrap().unwrap().store_revision,
            2
        );
    }

    #[test]
    fn concurrent_writers_get_one_typed_revision_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let first_manager = FeatureStorage::new(&root);
        let first_registration = first_manager.register(REGISTRATION).unwrap();
        let first_workspace = first_manager.workspace("workspace-a").unwrap();
        let first_store = SubjektivStore::open(&first_workspace, &first_registration).unwrap();
        let subject = first_store.create_subject(role()).unwrap();
        let memory = first_store
            .create_memory(&subject.id, draft("Revision basis", "initial"))
            .unwrap();

        let second_manager = FeatureStorage::new(&root);
        let second_registration = second_manager.register(REGISTRATION).unwrap();
        let second_workspace = second_manager.workspace("workspace-a").unwrap();
        let second_store = SubjektivStore::open(&second_workspace, &second_registration).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let revise = |store: SubjektivStore, claim: &'static str, barrier: Arc<Barrier>| {
            let subject_id = subject.id.clone();
            let memory_id = memory.id.clone();
            thread::spawn(move || {
                barrier.wait();
                store.revise_memory(
                    &subject_id,
                    &memory_id,
                    1,
                    draft(claim, "concurrent refinement"),
                )
            })
        };
        let first = revise(first_store.clone(), "first writer", barrier.clone());
        let second = revise(second_store, "second writer", barrier.clone());
        barrier.wait();
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(SubjektivError::RevisionConflict { .. })))
                .count(),
            1
        );
        assert_eq!(
            first_store
                .subject(&subject.id)
                .unwrap()
                .unwrap()
                .store_revision,
            2
        );
    }

    #[test]
    fn scope_and_fixed_revision_references_are_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let registration = manager.register(REGISTRATION).unwrap();
        let workspace_a = manager.workspace("workspace-a").unwrap();
        let workspace_b = manager.workspace("workspace-b").unwrap();
        let store_a = SubjektivStore::open(&workspace_a, &registration).unwrap();
        let store_b = SubjektivStore::open(&workspace_b, &registration).unwrap();
        let subject_a = store_a.create_subject(role()).unwrap();
        let other_subject_a = store_a.create_subject(role()).unwrap();
        let subject_b = store_b.create_subject(role()).unwrap();
        let memory_a = store_a
            .create_memory(
                &subject_a.id,
                draft("Workspace A experience", "direct host record"),
            )
            .unwrap();

        assert!(matches!(
            store_b.create_memory(&subject_a.id, draft("foreign subject", "must be rejected")),
            Err(SubjektivError::SubjectNotFound(_))
        ));
        let mut foreign_derived = draft("Workspace B experience", "invalid derivation");
        foreign_derived.derived_from.push(MemoryRevisionRef {
            memory_id: memory_a.id.clone(),
            revision: 1,
        });
        assert!(matches!(
            store_b.create_memory(&subject_b.id, foreign_derived),
            Err(SubjektivError::SubjectScopeMismatch { .. })
        ));
        assert_eq!(
            store_b
                .subject(&subject_b.id)
                .unwrap()
                .unwrap()
                .store_revision,
            0
        );

        let mut other_subject_derived = draft("Same Workspace, other subject", "invalid scope");
        other_subject_derived.derived_from.push(MemoryRevisionRef {
            memory_id: memory_a.id.clone(),
            revision: 1,
        });
        assert!(matches!(
            store_a.create_memory(&other_subject_a.id, other_subject_derived),
            Err(SubjektivError::SubjectScopeMismatch { .. })
        ));
        let mut wrong_revision = draft("Missing fixed revision", "invalid revision");
        wrong_revision.derived_from.push(MemoryRevisionRef {
            memory_id: memory_a.id.clone(),
            revision: 2,
        });
        assert!(matches!(
            store_a.create_memory(&subject_a.id, wrong_revision),
            Err(SubjektivError::SubjectScopeMismatch { .. })
        ));
        assert_eq!(
            store_a
                .subject(&other_subject_a.id)
                .unwrap()
                .unwrap()
                .store_revision,
            0
        );

        let foreign_origin = candidate(&subject_a.id, "candidate-foreign", "workspace-b");
        assert!(matches!(
            store_a.stage_candidate(foreign_origin),
            Err(SubjektivError::SubjectScopeMismatch { .. })
        ));
    }

    #[test]
    fn memory_states_snapshots_and_retirement_have_explicit_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let first = store
            .create_memory(&subject.id, draft("A constraint", "initial observation"))
            .unwrap();
        let snapshot = store
            .create_surface_snapshot(
                &subject.id,
                "Bounded resident summary",
                vec![MemoryRevisionRef {
                    memory_id: first.id.clone(),
                    revision: 1,
                }],
                1,
            )
            .unwrap();
        assert_eq!(
            store
                .surface_snapshot(&subject.id, &snapshot.id)
                .unwrap()
                .unwrap()
                .memory_refs[0]
                .revision,
            1
        );
        assert!(
            store
                .create_surface_snapshot(&subject.id, "stale", vec![], 0)
                .is_err()
        );

        let mut retract = draft("Retracted constraint", "evidence invalidated it");
        retract.state = MemoryState::Retracted;
        let retracted = store
            .revise_memory(&subject.id, &first.id, 1, retract)
            .unwrap();
        assert_eq!(retracted.state, MemoryState::Retracted);
        assert!(matches!(
            store.revise_memory(
                &subject.id,
                &first.id,
                2,
                draft("cannot reactivate", "retraction is terminal")
            ),
            Err(SubjektivError::InvalidStateTransition { .. })
        ));
        let mut retract_again = draft("still retracted", "no post-retraction revisions");
        retract_again.state = MemoryState::Retracted;
        assert!(matches!(
            store.revise_memory(&subject.id, &first.id, 2, retract_again),
            Err(SubjektivError::InvalidStateTransition { .. })
        ));

        let retired = store.retire_subject(&subject.id).unwrap();
        assert_eq!(retired.state, SubjectState::Retired);
        assert!(matches!(
            store.create_memory(&subject.id, draft("after retirement", "invalid")),
            Err(SubjektivError::SubjectRetired(_))
        ));
        assert!(store.memory(&subject.id, &first.id).unwrap().is_some());
    }

    #[test]
    fn history_rows_reject_update_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let (_manager, _workspace, store) = open_store(&temp.path().join("storage"), "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let staged = store
            .stage_candidate(candidate(&subject.id, "candidate-1", "workspace-a"))
            .unwrap();
        let (memory, _) = store
            .apply_candidate(
                &subject.id,
                &staged.id,
                MemoryRevisionTarget::Create,
                draft("Immutable history", "accept"),
                "applied",
            )
            .unwrap();
        let extra_candidate = store
            .stage_candidate(candidate(&subject.id, "candidate-extra", "workspace-a"))
            .unwrap();
        let other_memory = store
            .create_memory(&subject.id, draft("Other memory", "snapshot seal target"))
            .unwrap();
        let snapshot = store
            .create_surface_snapshot(
                &subject.id,
                "sealed snapshot",
                vec![MemoryRevisionRef {
                    memory_id: memory.id.clone(),
                    revision: 1,
                }],
                2,
            )
            .unwrap();

        let update = store
            .database
            .try_with_connection::<_, SubjektivError>(|connection| {
                connection.execute(
                    "UPDATE memory_revisions SET record_json = '{}' \
                 WHERE subject_id = ?1 AND memory_id = ?2 AND revision = 1",
                    params![subject.id, memory.id],
                )?;
                Ok(())
            });
        assert!(update.is_err());
        let delete = store
            .database
            .try_with_connection::<_, SubjektivError>(|connection| {
                connection.execute(
                    "DELETE FROM staging_records WHERE subject_id = ?1 AND candidate_id = ?2",
                    params![subject.id, staged.id],
                )?;
                Ok(())
            });
        assert!(delete.is_err());

        let late_evidence = store
            .database
            .try_with_connection::<_, SubjektivError>(|connection| {
                connection.execute(
                    "INSERT INTO memory_revision_candidates (
                        subject_id, memory_id, revision, candidate_id
                     ) VALUES (?1, ?2, 1, ?3)",
                    params![subject.id, memory.id, extra_candidate.id],
                )?;
                Ok(())
            });
        assert!(late_evidence.is_err());
        let late_derivation =
            store
                .database
                .try_with_connection::<_, SubjektivError>(|connection| {
                    connection.execute(
                        "INSERT INTO memory_revision_derivations (
                        subject_id, memory_id, revision, source_memory_id, source_revision
                     ) VALUES (?1, ?2, 1, ?3, 1)",
                        params![subject.id, memory.id, other_memory.id],
                    )?;
                    Ok(())
                });
        assert!(late_derivation.is_err());
        let late_resolution_target =
            store
                .database
                .try_with_connection::<_, SubjektivError>(|connection| {
                    connection.execute(
                        "INSERT INTO staging_resolution_targets (
                        subject_id, candidate_id, memory_id, revision
                     ) VALUES (?1, ?2, ?3, 1)",
                        params![subject.id, staged.id, other_memory.id],
                    )?;
                    Ok(())
                });
        assert!(late_resolution_target.is_err());
        let late_snapshot_ref =
            store
                .database
                .try_with_connection::<_, SubjektivError>(|connection| {
                    connection.execute(
                        "INSERT INTO surface_snapshot_refs (
                        subject_id, snapshot_id, memory_id, revision
                     ) VALUES (?1, ?2, ?3, 1)",
                        params![subject.id, snapshot.id, other_memory.id],
                    )?;
                    Ok(())
                });
        assert!(late_snapshot_ref.is_err());

        assert!(
            store
                .memory_revision(&subject.id, &memory.id, 1)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .staging_candidate(&subject.id, &staged.id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn reopen_and_backup_restore_preserve_exact_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let snapshot_path = temp.path().join("snapshot");
        let (manager, _workspace, store) = open_store(&root, "workspace-a");
        let subject = store.create_subject(role()).unwrap();
        let candidate = store
            .stage_candidate(candidate(&subject.id, "candidate-1", "workspace-a"))
            .unwrap();
        let (memory, _) = store
            .apply_candidate(
                &subject.id,
                &candidate.id,
                MemoryRevisionTarget::Create,
                draft("Persist this revision", "accepted before restart"),
                "applied",
            )
            .unwrap();
        drop(store);
        manager.shutdown().unwrap();
        drop(manager);

        let (manager, workspace, reopened) = open_store(&root, "workspace-a");
        assert_eq!(
            reopened
                .memory_revision(&subject.id, &memory.id, 1)
                .unwrap()
                .unwrap()
                .claim,
            "Persist this revision"
        );
        assert_eq!(
            reopened
                .staging_resolution(&subject.id, &candidate.id)
                .unwrap()
                .unwrap()
                .candidate
                .source_refs[0]
                .session_id
                .as_deref(),
            Some("session-1")
        );
        let surface = reopened
            .create_surface_snapshot(
                &subject.id,
                "Persisted resident surface",
                vec![MemoryRevisionRef {
                    memory_id: memory.id.clone(),
                    revision: 1,
                }],
                1,
            )
            .unwrap();
        workspace.backup(&snapshot_path).unwrap();
        let mut post_backup = draft("Post-backup revision", "not in snapshot");
        post_backup.derived_from.push(MemoryRevisionRef {
            memory_id: memory.id.clone(),
            revision: 1,
        });
        let second = reopened
            .revise_memory(&subject.id, &memory.id, 1, post_backup)
            .unwrap();
        assert_eq!(second.revision, 2);
        workspace.delete().unwrap();
        drop(reopened);
        drop(workspace);
        drop(manager);

        let manager = FeatureStorage::new(&root);
        let workspace = manager.workspace("workspace-a").unwrap();
        workspace.restore(&snapshot_path).unwrap();
        let registration = SubjektivStore::register(&workspace).unwrap();
        let restored = SubjektivStore::open(&workspace, &registration).unwrap();
        assert_eq!(
            restored
                .subject(&subject.id)
                .unwrap()
                .unwrap()
                .store_revision,
            1
        );
        assert_eq!(
            restored
                .memory(&subject.id, &memory.id)
                .unwrap()
                .unwrap()
                .claim,
            "Persist this revision"
        );
        assert!(
            restored
                .memory_revision(&subject.id, &memory.id, 2)
                .unwrap()
                .is_none()
        );
        assert!(
            restored
                .staging_resolution(&subject.id, &candidate.id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            restored
                .surface_snapshot(&subject.id, &surface.id)
                .unwrap()
                .unwrap()
                .memory_refs,
            vec![MemoryRevisionRef {
                memory_id: memory.id,
                revision: 1,
            }]
        );
    }
}
