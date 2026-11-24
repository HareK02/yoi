//! Finite recording bytes belong to the common Host Job, not to a second lifecycle.
//! Presence of this extension selects Host execution; purpose/profile never do.
//! Capture and current authorization are the caller's responsibility. Retained bytes
//! neither require a live source Worker nor attest its termination or Journal reflection.
use super::*;
use feature_storage::DatabaseIdentity;
use serde_json::Value;
use session_store::capture::{
    CaptureBody, CaptureDto, CaptureEntryDto, CaptureLimits, RecordingPosition,
};

pub(super) const MIGRATION_NAME: &str = "Finite recording input and prefix execution owned by Jobs";
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 139 {
        return Err(Error::Store(
            "Recording input migration requires schema 139".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_SQL)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(140,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) const SCHEMA_SQL: &str = r#"
CREATE TABLE backend_job_recording_inputs (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    scope_json TEXT NOT NULL CHECK(json_valid(scope_json)),
    scope_digest TEXT NOT NULL CHECK(length(scope_digest)=64),
    chunk_count INTEGER NOT NULL CHECK(chunk_count BETWEEN 0 AND 4096),
    total_bytes INTEGER NOT NULL CHECK(total_bytes BETWEEN 0 AND 67108864),
    current_child_job_id TEXT,
    PRIMARY KEY(workspace_id,job_id),
    FOREIGN KEY(workspace_id,job_id) REFERENCES backend_jobs(workspace_id,job_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id,job_id,current_child_job_id)
        REFERENCES backend_job_recording_children(workspace_id,host_job_id,child_job_id)
);
CREATE TABLE backend_job_recording_chunks (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL CHECK(chunk_index BETWEEN 0 AND 4095),
    capture_json TEXT NOT NULL CHECK(json_valid(capture_json) AND length(CAST(capture_json AS BLOB))<=131072),
    capture_digest TEXT NOT NULL CHECK(length(capture_digest)=64),
    PRIMARY KEY(workspace_id,job_id,chunk_index),
    FOREIGN KEY(workspace_id,job_id) REFERENCES backend_job_recording_inputs(workspace_id,job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_recording_children (
    workspace_id TEXT NOT NULL,
    host_job_id TEXT NOT NULL,
    child_job_id TEXT NOT NULL CHECK(child_job_id!=host_job_id),
    PRIMARY KEY(workspace_id,host_job_id,child_job_id),
    UNIQUE(workspace_id,child_job_id),
    FOREIGN KEY(workspace_id,host_job_id) REFERENCES backend_job_recording_inputs(workspace_id,job_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id,child_job_id) REFERENCES backend_jobs(workspace_id,job_id)
);
CREATE TRIGGER backend_job_recording_scope_immutable BEFORE UPDATE ON backend_job_recording_inputs
WHEN NEW.workspace_id!=OLD.workspace_id OR NEW.job_id!=OLD.job_id
 OR NEW.scope_json!=OLD.scope_json OR NEW.scope_digest!=OLD.scope_digest
 OR NEW.chunk_count!=OLD.chunk_count OR NEW.total_bytes!=OLD.total_bytes
BEGIN SELECT RAISE(ABORT,'Job recording scope is immutable'); END;
CREATE TRIGGER backend_job_recording_chunk_immutable BEFORE UPDATE ON backend_job_recording_chunks
BEGIN SELECT RAISE(ABORT,'Job recording chunks are immutable'); END;
CREATE TRIGGER backend_job_recording_child_immutable BEFORE UPDATE ON backend_job_recording_children
BEGIN SELECT RAISE(ABORT,'Job recording child links are immutable'); END;
"#;

pub const MAX_RECORDING_CHUNK_BYTES: usize = 128 * 1024;
pub const MAX_RECORDING_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RECORDING_CHUNKS: usize = 4096;
pub const MAX_RECORDING_CHUNK_PAGE: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendJobRecordingScope {
    pub subject_id: String,
    pub session_id: String,
    pub worker: RuntimeWorkerRef,
    pub binding_id: String,
    #[serde(with = "database_identity::Identity")]
    pub database: DatabaseIdentity,
    pub start: RecordingPosition,
    pub end: RecordingPosition,
}

mod database_identity {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(remote = "DatabaseIdentity", deny_unknown_fields)]
    pub struct Identity {
        pub scope: String,
        pub feature: String,
        pub physical: String,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendJobRecordingInput {
    pub workspace_id: String,
    pub job_id: String,
    pub scope: BackendJobRecordingScope,
    /// Digest of the complete scope and ordered capture values, not just descriptors.
    pub scope_digest: String,
    pub chunk_count: usize,
    pub total_bytes: usize,
    pub current_child_job_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendJobRecordingChunk {
    pub index: usize,
    pub capture: CaptureDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendJobRecordingChunkPage {
    pub chunks: Vec<BackendJobRecordingChunk>,
    /// Inclusive start index for the next page; None means the finite end was reached.
    pub next_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackendJobRecordingReservation {
    pub job: BackendJobRecord,
    pub input: BackendJobRecordingInput,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackendJobRecordingOutcome {
    Completed { result: Value },
    Failed { category: String, detail: String },
}

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.into())
}
fn conflict(message: &str) -> Error {
    Error::RepositoryConflict(message.into())
}
fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes))
}
fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|e| invalid(&format!("Invalid recording input: {e}")))
}
fn position(p: RecordingPosition) -> (u64, u64) {
    (p.canonical_order, p.byte_offset)
}
fn identity(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(invalid("Invalid recording scope identity"));
    }
    Ok(())
}

struct PreparedInput {
    scope_json: String,
    digest: String,
    chunks: Vec<String>,
    total_bytes: usize,
}

/// Validate a fixed, complete interval. Fragment joins must retain *all* source
/// metadata. Full descriptors spanning several chunks are hashed incrementally.
fn prepare(scope: &BackendJobRecordingScope, chunks: &[CaptureDto]) -> Result<PreparedInput> {
    for id in [
        &scope.subject_id,
        &scope.session_id,
        &scope.binding_id,
        &scope.worker.runtime_id,
        &scope.worker.worker_id,
        &scope.database.scope,
        &scope.database.feature,
        &scope.database.physical,
    ] {
        identity(id)?;
    }
    if chunks.len() > MAX_RECORDING_CHUNKS || position(scope.start) > position(scope.end) {
        return Err(invalid("Invalid finite recording scope"));
    }
    let scope_json = json(scope)?;
    let mut hash = Sha256::new();
    hash.update(b"backend-job-recording-input-v1\0");
    hash.update((scope_json.len() as u64).to_le_bytes());
    hash.update(scope_json.as_bytes());
    hash.update((chunks.len() as u64).to_le_bytes());
    let mut next = scope.start;
    let mut previous: Option<&CaptureEntryDto> = None;
    let mut body_hash: Option<Sha256> = None;
    let mut seen = HashSet::new();
    let mut source_floors = HashMap::<&str, u64>::new();
    let mut header: Option<(&str, u64)> = None;
    let mut total_bytes = 0usize;
    let mut encoded = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        chunk
            .validate(CaptureLimits {
                max_bytes: MAX_RECORDING_CHUNK_BYTES,
                max_text_bytes: MAX_RECORDING_CHUNK_BYTES,
                ..Default::default()
            })
            .map_err(|e| invalid(&format!("Invalid recording capture: {e}")))?;
        if chunk.session_id != scope.session_id
            || chunk.entries.is_empty()
            || header.is_some_and(|h| h != (chunk.segment_id.as_str(), chunk.view_entry_count))
            || scope.end.canonical_order > chunk.view_entry_count
            || (scope.end.canonical_order == chunk.view_entry_count && scope.end.byte_offset != 0)
        {
            return Err(invalid("Recording capture Session or fixed view mismatch"));
        }
        header = Some((&chunk.segment_id, chunk.view_entry_count));
        for entry in &chunk.entries {
            let CaptureBody::CanonicalPublic {
                descriptor,
                byte_offset,
                text,
                boundary,
            } = &entry.body
            else {
                return Err(invalid(
                    "Recording input must contain canonical public bytes",
                ));
            };
            if entry.order != next.canonical_order || *byte_offset != next.byte_offset {
                return Err(invalid("Recording chunks are not contiguous"));
            }
            if let Some(prior) = previous.filter(|p| p.order == entry.order) {
                let CaptureBody::CanonicalPublic {
                    descriptor: old,
                    boundary: old_boundary,
                    ..
                } = &prior.body
                else {
                    unreachable!()
                };
                if old != descriptor
                    || old_boundary != boundary
                    || prior.source != entry.source
                    || prior.policy != entry.policy
                    || prior.entry_id != entry.entry_id
                {
                    return Err(invalid("Recording fragment descriptor changed"));
                }
            } else {
                if !seen.insert((
                    descriptor.identity.segment_id,
                    descriptor.identity.entry_ref.as_str(),
                )) {
                    return Err(invalid("Recording descriptor identity reused"));
                }
                let floor = source_floors.entry(&entry.source.segment_id).or_default();
                if entry.source.entry_index < *floor {
                    return Err(invalid("Recording source order reversed"));
                }
                *floor = entry
                    .source
                    .entry_index
                    .checked_add(u64::from(boundary.is_none()))
                    .ok_or_else(|| invalid("Recording source position overflow"))?;
                body_hash = (*byte_offset == 0).then(Sha256::new);
            }
            if let Some(hash) = &mut body_hash {
                hash.update(text.as_bytes());
            }
            let end = byte_offset
                .checked_add(text.len() as u64)
                .ok_or_else(|| invalid("Recording byte position overflow"))?;
            next = if end == descriptor.total_bytes {
                if let Some(hash) = body_hash.take() {
                    if hex(hash.finalize()) != descriptor.sha256 {
                        return Err(invalid("Recording complete body digest mismatch"));
                    }
                }
                RecordingPosition {
                    canonical_order: entry
                        .order
                        .checked_add(1)
                        .ok_or_else(|| invalid("Recording order overflow"))?,
                    byte_offset: 0,
                }
            } else {
                RecordingPosition {
                    canonical_order: entry.order,
                    byte_offset: end,
                }
            };
            if position(next) > position(scope.end) {
                return Err(invalid("Recording capture exceeds fixed end"));
            }
            previous = Some(entry);
        }
        let value = json(chunk)?;
        total_bytes = total_bytes
            .checked_add(value.len())
            .ok_or_else(|| invalid("Recording input too large"))?;
        if total_bytes > MAX_RECORDING_INPUT_BYTES {
            return Err(invalid("Recording input exceeds finite aggregate limit"));
        }
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
        encoded.push(value);
    }
    if next != scope.end {
        return Err(invalid("Recording capture does not cover fixed scope"));
    }
    Ok(PreparedInput {
        scope_json,
        digest: hex(hash.finalize()),
        chunks: encoded,
        total_bytes,
    })
}

impl BackendJobRecordingScope {
    /// Bind the small common request input to the complete immutable body store.
    pub fn recording_scope_digest(&self, chunks: &[CaptureDto]) -> Result<String> {
        Ok(prepare(self, chunks)?.digest)
    }
    pub fn request_input(&self, chunks: &[CaptureDto]) -> Result<Value> {
        Ok(serde_json::json!({"recording_scope_digest":self.recording_scope_digest(chunks)?}))
    }
}

fn load(conn: &Connection, workspace: &str, id: &str) -> Result<Option<BackendJobRecordingInput>> {
    let row = conn.query_row("SELECT scope_json,scope_digest,chunk_count,total_bytes,current_child_job_id FROM backend_job_recording_inputs WHERE workspace_id=?1 AND job_id=?2",params![workspace,id],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,usize>(2)?,r.get::<_,usize>(3)?,r.get::<_,Option<String>>(4)?))).optional()?;
    row.map(
        |(scope, digest, chunk_count, total_bytes, current_child_job_id)| {
            Ok(BackendJobRecordingInput {
                workspace_id: workspace.into(),
                job_id: id.into(),
                scope: serde_json::from_str(&scope)
                    .map_err(|_| Error::Store("Invalid saved recording scope".into()))?,
                scope_digest: digest,
                chunk_count,
                total_bytes,
                current_child_job_id,
            })
        },
    )
    .transpose()
}
fn require_input(conn: &Connection, workspace: &str, id: &str) -> Result<BackendJobRecordingInput> {
    load(conn, workspace, id)?.ok_or_else(|| invalid("Host recording Job input is missing"))
}
fn require_host(conn: &Connection, workspace: &str, id: &str) -> Result<BackendJobRecord> {
    let job = read_backend_job(conn, workspace, id)?
        .ok_or_else(|| invalid("Host recording Job is missing"))?;
    let has_attempt: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM backend_job_attempts WHERE workspace_id=?1 AND job_id=?2)",
        params![workspace, id],
        |r| r.get(0),
    )?;
    if has_attempt
        || !job.request.grants.is_empty()
        || job.request.source_worker.is_some()
        || job.request.notification_target.is_some()
    {
        return Err(conflict(
            "Host recording Job cannot own a Worker attempt or grants",
        ));
    }
    Ok(job)
}
fn require_admission(
    conn: &Connection,
    workspace: &str,
    input: &BackendJobRecordingInput,
) -> Result<()> {
    let deleted:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND subject_id=?2) OR EXISTS(SELECT 1 FROM subject_deletion_jobs WHERE workspace_id=?1 AND job_id=?3)",params![workspace,input.scope.subject_id,input.job_id],|r|r.get(0))?;
    if deleted {
        return Err(conflict("subject_deleted: recording Job admission fenced"));
    }
    Ok(())
}

impl SqliteWorkspaceStore {
    /// Atomic admission of the Host Job and its complete captured interval. No
    /// Worker attempt, worker lookup, or worker authority is manufactured here.
    pub fn reserve_backend_job_recording_input(
        &self,
        workspace: &str,
        request: &BackendJobRequest,
        scope: &BackendJobRecordingScope,
        chunks: &[CaptureDto],
        now: &str,
    ) -> Result<BackendJobRecordingReservation> {
        request.validate()?;
        if !request.grants.is_empty()
            || request.source_worker.is_some()
            || request.notification_target.is_some()
        {
            return Err(invalid(
                "Host recording request must not carry Worker references or grants",
            ));
        }
        let prepared = prepare(scope, chunks)?;
        if request.input != serde_json::json!({"recording_scope_digest":prepared.digest}) {
            return Err(invalid(
                "Host recording request must bind the complete recording scope digest",
            ));
        }
        let fingerprint = request.fingerprint()?;
        let request_json = json(request)?;
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            subject_deletion_store::require_job_admission(&tx,workspace,request)?;
            let candidate=BackendJobRecordingInput { workspace_id:workspace.into(),job_id:request.job_id.clone(),scope:scope.clone(),scope_digest:prepared.digest.clone(),chunk_count:chunks.len(),total_bytes:prepared.total_bytes,current_child_job_id:None };
            require_admission(&tx,workspace,&candidate)?;
            if let Some(job)=read_backend_job(&tx,workspace,&request.job_id)? {
                let input=require_input(&tx,workspace,&request.job_id)?;
                require_host(&tx,workspace,&request.job_id)?;
                if job.intent_fingerprint!=fingerprint || input.scope!=*scope || input.scope_digest!=prepared.digest
                    || input.chunk_count!=chunks.len() || input.total_bytes!=prepared.total_bytes {
                    return Err(invalid("Recording Job id reused with different immutable intent"));
                }
                tx.commit()?;
                return Ok(BackendJobRecordingReservation {job,input,replayed:true});
            }
            if let Some(key)=request.resource_key() {
                if read_active_backend_job_for_resource(&tx,workspace,&key)?.is_some() {
                    return Err(conflict("Recording Host resource already owns an active Job"));
                }
            }
            tx.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,resource_key,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',1,?8,?9,?9)",params![workspace,request.job_id,request.purpose,request.input_digest()?,request.input_ref,request_json,fingerprint,request.resource_key(),now])?;
            tx.execute("INSERT INTO backend_job_recording_inputs(workspace_id,job_id,scope_json,scope_digest,chunk_count,total_bytes) VALUES(?1,?2,?3,?4,?5,?6)",params![workspace,request.job_id,prepared.scope_json,prepared.digest,chunks.len(),prepared.total_bytes])?;
            for (index,value) in prepared.chunks.iter().enumerate() {
                tx.execute("INSERT INTO backend_job_recording_chunks(workspace_id,job_id,chunk_index,capture_json,capture_digest) VALUES(?1,?2,?3,?4,?5)",params![workspace,request.job_id,index,value,digest(value.as_bytes())])?;
            }
            let job=require_host(&tx,workspace,&request.job_id)?;
            tx.commit()?;
            Ok(BackendJobRecordingReservation {job,input:candidate,replayed:false})
        })
    }

    pub fn get_backend_job_recording_input(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<BackendJobRecordingInput>> {
        self.with_conn(|conn| load(conn, workspace, id))
    }

    pub(crate) fn find_backend_job_recording_prefix(
        &self,
        workspace: &str,
        scope: &BackendJobRecordingScope,
        cursor: RecordingPosition,
    ) -> Result<Option<(String, BackendJobState)>> {
        self.with_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT i.job_id,i.scope_json,j.state FROM backend_job_recording_inputs i
                 JOIN backend_jobs j USING(workspace_id,job_id)
                 WHERE i.workspace_id=?1 AND json_extract(i.scope_json,'$.subject_id')=?2
                 AND json_extract(i.scope_json,'$.session_id')=?3
                 AND json_extract(i.scope_json,'$.binding_id')=?4
                 AND j.state IN ('pending','finalizing','failed') ORDER BY i.job_id",
            )?;
            let mut rows = statement.query(params![
                workspace,
                scope.subject_id,
                scope.session_id,
                scope.binding_id
            ])?;
            let mut failed = None;
            while let Some(row) = rows.next()? {
                let saved: BackendJobRecordingScope =
                    serde_json::from_str(&row.get::<_, String>(1)?)
                        .map_err(|error| Error::Store(error.to_string()))?;
                if saved.worker != scope.worker
                    || saved.database != scope.database
                    || position(saved.start) > position(cursor)
                    || position(saved.end) <= position(cursor)
                {
                    continue;
                }
                let id = row.get::<_, String>(0)?;
                let state = BackendJobState::parse(&row.get::<_, String>(2)?)?;
                match state {
                    BackendJobState::Pending | BackendJobState::Finalizing => {
                        return Ok(Some((id, state)));
                    }
                    BackendJobState::Failed => {
                        failed = Some((id, state));
                    }
                    BackendJobState::Completed => {}
                }
            }
            Ok(failed)
        })
    }

    pub fn list_active_backend_job_recording_ids(
        &self,
        workspace: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("Recording Job page limit must be 1..=100"));
        }
        self.with_conn(|conn| Ok(conn.prepare("SELECT i.job_id FROM backend_job_recording_inputs i JOIN backend_jobs j USING(workspace_id,job_id) WHERE i.workspace_id=?1 AND j.state IN ('pending','finalizing') AND (?2 IS NULL OR i.job_id>?2) ORDER BY i.job_id LIMIT ?3")?.query_map(params![workspace,after,limit],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?))
    }

    pub fn get_backend_job_recording_chunks(
        &self,
        workspace: &str,
        id: &str,
        start_index: usize,
        limit: usize,
    ) -> Result<BackendJobRecordingChunkPage> {
        if !(1..=MAX_RECORDING_CHUNK_PAGE).contains(&limit) {
            return Err(invalid("Recording chunk page limit must be 1..=16"));
        }
        self.with_conn(|conn| {
            let input=require_input(conn,workspace,id)?;
            let job=require_host(conn,workspace,id)?;
            job.load_executable_request()?;
            require_admission(conn,workspace,&input)?;
            if start_index>input.chunk_count { return Err(invalid("Recording chunk page starts past finite end")); }
            let rows=conn.prepare("SELECT chunk_index,capture_json,capture_digest FROM backend_job_recording_chunks WHERE workspace_id=?1 AND job_id=?2 AND chunk_index>=?3 ORDER BY chunk_index LIMIT ?4")?.query_map(params![workspace,id,start_index,limit],|r| Ok((r.get::<_,usize>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len()!=limit.min(input.chunk_count-start_index) { return Err(Error::Store("Recording chunks are missing".into())); }
            let mut chunks=Vec::with_capacity(rows.len());
            for (offset,(index,value,hash)) in rows.into_iter().enumerate() {
                if index!=start_index+offset || value.len()>MAX_RECORDING_CHUNK_BYTES || digest(value.as_bytes())!=hash {
                    return Err(Error::Store("Recording chunk integrity mismatch".into()));
                }
                chunks.push(BackendJobRecordingChunk {index,capture:serde_json::from_str(&value).map_err(|_| Error::Store("Invalid saved recording capture".into()))?});
            }
            let next=start_index+chunks.len();
            Ok(BackendJobRecordingChunkPage {chunks,next_index:(next<input.chunk_count).then_some(next)})
        })
    }

    pub fn set_backend_job_recording_child(
        &self,
        workspace: &str,
        id: &str,
        expected: Option<&str>,
        new: Option<&str>,
    ) -> Result<BackendJobRecordingInput> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let input = bind_child(&tx, workspace, id, expected, new)?;
            tx.commit()?;
            Ok(input)
        })
    }

    /// The reservation callback must use this transaction, not reenter the store.
    /// This permits the common worker-reservation helper to create a bounded child
    /// and bind it atomically, without widening the WorkspaceStore trait.
    pub(crate) fn reserve_backend_job_recording_child_with(
        &self,
        workspace: &str,
        id: &str,
        expected: Option<&str>,
        reserve: impl FnOnce(&Connection) -> Result<BackendJobReservation>,
    ) -> Result<(BackendJobRecordingInput, BackendJobReservation)> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let reservation = reserve(&tx)?;
            if reservation.job.workspace_id != workspace || reservation.resource_reused {
                return Err(conflict(
                    "Recording child reservation reused a different owner",
                ));
            }
            let input = bind_child(
                &tx,
                workspace,
                id,
                expected,
                Some(&reservation.job.request.job_id),
            )?;
            tx.commit()?;
            Ok((input, reservation))
        })
    }

    pub fn list_backend_job_recording_child_ids(
        &self,
        workspace: &str,
        id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("Recording child page limit must be 1..=100"));
        }
        self.with_conn(|conn| {
            require_input(conn,workspace,id)?;
            Ok(conn.prepare("SELECT child_job_id FROM backend_job_recording_children WHERE workspace_id=?1 AND host_job_id=?2 AND (?3 IS NULL OR child_job_id>?3) ORDER BY child_job_id LIMIT ?4")?.query_map(params![workspace,id,after,limit],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Accept the Host outcome only after every linked child's canonical Job and
    /// attempt cleanup is terminal. This writes no Worker termination evidence.
    pub fn finish_backend_job_recording(
        &self,
        workspace: &str,
        id: &str,
        outcome: &BackendJobRecordingOutcome,
        now: &str,
    ) -> Result<BackendJobRecord> {
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_input(&tx,workspace,id)?;
            let job=require_host(&tx,workspace,id)?;
            let (state,result,result_digest,category,detail)=match outcome {
                BackendJobRecordingOutcome::Completed {result} => {
                    let value=json(result)?;
                    if value.len()>job.request.limits.max_result_bytes as usize { return Err(invalid("Host recording result exceeds Job result limit")); }
                    (BackendJobState::Completed,Some(value.clone()),Some(digest(value.as_bytes())),None,None)
                },
                BackendJobRecordingOutcome::Failed {category,detail} => {
                    identity(category)?;
                    if detail.len()>4096 { return Err(invalid("Host recording failure detail too large")); }
                    (BackendJobState::Failed,None,None,Some(category.clone()),Some(detail.clone()))
                }
            };
            if matches!(job.state,BackendJobState::Completed|BackendJobState::Failed) {
                if job.state!=state || job.result_digest!=result_digest || job.failure_category!=category || job.failure_detail!=detail { return Err(conflict("Host recording terminal outcome is immutable")); }
                tx.commit()?;
                return Ok(job);
            }
            if job.state!=BackendJobState::Pending { return Err(conflict("Unexpected Host recording lifecycle state")); }
            let children=tx.prepare("SELECT child_job_id FROM backend_job_recording_children WHERE workspace_id=?1 AND host_job_id=?2")?.query_map(params![workspace,id],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for child in children { require_settled_child(&tx,workspace,&child)?; }
            tx.execute("UPDATE backend_jobs SET state=?3,result_json=?4,result_digest=?5,failure_category=?6,failure_detail=?7,completed_at=?8,updated_at=?8 WHERE workspace_id=?1 AND job_id=?2 AND state='pending'",params![workspace,id,state.as_str(),result,result_digest,category,detail,now])?;
            let saved=require_host(&tx,workspace,id)?;
            tx.commit()?;
            Ok(saved)
        })
    }
}

fn require_settled_child(conn: &Connection, workspace: &str, id: &str) -> Result<()> {
    let job = read_backend_job(conn, workspace, id)?
        .ok_or_else(|| conflict("Linked recording child is missing"))?;
    let attempts = backend_job_recovery_store::history(conn, workspace, id)?;
    if !matches!(
        job.state,
        BackendJobState::Completed | BackendJobState::Failed
    ) || attempts.is_empty()
        || attempts.iter().any(|a| {
            !a.state.terminal()
                || a.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed)
        })
    {
        return Err(conflict(
            "Recording child execution or cleanup still owns resources",
        ));
    }
    Ok(())
}

pub(super) fn bind_child(
    conn: &Connection,
    workspace: &str,
    id: &str,
    expected: Option<&str>,
    new: Option<&str>,
) -> Result<BackendJobRecordingInput> {
    let mut input = require_input(conn, workspace, id)?;
    let host = require_host(conn, workspace, id)?;
    require_admission(conn, workspace, &input)?;
    if input.current_child_job_id.as_deref() == new {
        return Ok(input);
    }
    if input.current_child_job_id.as_deref() != expected {
        return Err(conflict("Recording child CAS conflict"));
    }
    if host.state != BackendJobState::Pending {
        return Err(conflict("Terminal recording Host cannot bind a new child"));
    }
    if let Some(old) = expected {
        require_settled_child(conn, workspace, old)?;
    }
    if let Some(child) = new {
        let job = read_backend_job(conn, workspace, child)?
            .ok_or_else(|| invalid("Recording child Job does not exist in this Workspace"))?;
        let grant = job
            .request
            .grants
            .subjektiv_journal
            .as_ref()
            .ok_or_else(|| invalid("Recording child requires a Journal grant"))?;
        if child == id
            || load(conn, workspace, child)?.is_some()
            || grant.subject_id != input.scope.subject_id
            || grant.session_id != input.scope.session_id
            || grant.binding_id != input.scope.binding_id
            || job
                .request
                .source_worker
                .as_ref()
                .is_some_and(|w| w != &input.scope.worker)
        {
            return Err(invalid("Recording child Journal source identity mismatch"));
        }
        job.load_executable_request()?;
        let prior=conn.prepare("SELECT child_job_id FROM backend_job_recording_children WHERE workspace_id=?1 AND host_job_id=?2")?.query_map(params![workspace,id],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if prior.len() >= MAX_RECORDING_CHUNKS {
            return Err(invalid("Recording child history exceeds finite limit"));
        }
        for old in prior {
            if old == child {
                return Err(conflict(
                    "A previous recording child cannot become current again",
                ));
            }
            let old = read_backend_job(conn, workspace, &old)?
                .ok_or_else(|| conflict("Previous recording child missing"))?;
            if old
                .request
                .grants
                .subjektiv_journal
                .as_ref()
                .is_none_or(|g| g.journal_id != grant.journal_id)
            {
                return Err(invalid("Recording child Journal identity changed"));
            }
        }
        if backend_job_recovery_store::history(conn, workspace, child)?.is_empty() {
            return Err(invalid("Recording child has no worker Job attempt"));
        }
        conn.execute("INSERT INTO backend_job_recording_children(workspace_id,host_job_id,child_job_id) VALUES(?1,?2,?3)",params![workspace,id,child])?;
    }
    conn.execute("UPDATE backend_job_recording_inputs SET current_child_job_id=?3 WHERE workspace_id=?1 AND job_id=?2 AND current_child_job_id IS ?4",params![workspace,id,new,expected])?;
    input.current_child_job_id = new.map(str::to_owned);
    Ok(input)
}

impl SqliteWorkspaceStore {
    pub fn accepted_recording_end(
        &self,
        workspace: &str,
        subject: &str,
        session: &str,
        binding: &str,
    ) -> Result<Option<RecordingPosition>> {
        self.with_conn(|conn| {
            let values=conn.prepare("SELECT scope_json FROM backend_job_recording_inputs WHERE workspace_id=?1 AND json_extract(scope_json,'$.subject_id')=?2 AND json_extract(scope_json,'$.session_id')=?3 AND json_extract(scope_json,'$.binding_id')=?4")?
                .query_map(params![workspace,subject,session,binding],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut end=None;
            for value in values {
                let scope:BackendJobRecordingScope=serde_json::from_str(&value).map_err(|e|Error::Store(e.to_string()))?;
                if end.is_none_or(|old|position(scope.end)>position(old)) {end=Some(scope.end);}
            }
            Ok(end)
        })
    }

    pub fn erase_backend_job_recording_inputs_for_session(
        &self,
        workspace: &str,
        subject: &str,
        session: &str,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let ids=tx.prepare("SELECT job_id FROM backend_job_recording_inputs WHERE workspace_id=?1 AND json_extract(scope_json,'$.subject_id')=?2 AND json_extract(scope_json,'$.session_id')=?3")?
                .query_map(params![workspace,subject,session],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for id in ids {
                let job=require_host(&tx,workspace,&id)?;
                let mut request=job.request;request.input=Value::Null;
                tx.execute("UPDATE backend_jobs SET request_json=?3,input_disposition='erased' WHERE workspace_id=?1 AND job_id=?2",params![workspace,id,json(&request)?])?;
                tx.execute("DELETE FROM backend_job_recording_chunks WHERE workspace_id=?1 AND job_id=?2",params![workspace,id])?;
            }
            tx.commit()?;Ok(())
        })
    }
}

#[cfg(test)]
#[path = "backend_job_recording_input_tests.rs"]
mod tests;
