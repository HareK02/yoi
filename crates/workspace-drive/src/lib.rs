//! Workspace-owned latest-version storage. Call these blocking operations on a
//! blocking thread, never on an async executor. SQL IMMEDIATE transactions are
//! the cross-process authority for uploads, tree changes, reads, and collection.
mod blob;
mod migrations;
use migrations::MIGRATIONS;

use feature_storage::{
    FeatureDatabase, FeatureRegistration, FeatureStorageError, RegisteredFeature,
    ScopedFeatureStorage,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_READ_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE: usize = 200;
pub const MAX_SEARCH_NODES: usize = 128;
pub const MAX_SEARCH_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_NAME_BYTES: usize = 255;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Drive request: {0}")]
    Invalid(String),
    #[error("Drive node no longer exists")]
    NotFound,
    #[error("Drive node changed, name is taken, or request identity conflicts")]
    Conflict,
    #[error("Drive access denied")]
    Denied,
    #[error("Workspace Drive is fenced for deletion")]
    Fenced,
    #[error("Drive storage failure: {0}")]
    Storage(#[from] FeatureStorageError),
    #[error("Drive metadata failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Drive blob failure: {0}")]
    Blob(#[from] blob::BlobError),
    #[error("Drive result encoding failure: {0}")]
    Encoding(#[from] serde_json::Error),
}

// Decimal strings at the external boundary avoid JavaScript's 53-bit limit.
macro_rules! decimal_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(i64);
        impl $name {
            pub fn get(self) -> i64 {
                self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> std::result::Result<Self, String> {
                let n: i64 = value.parse().map_err(|_| "invalid decimal identifier")?;
                if n <= 0 || n.to_string() != value {
                    return Err("invalid decimal identifier".into());
                }
                Ok(Self(n))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0.to_string()
            }
        }
    };
}
decimal_id!(NodeId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub workspace_id: String,
    pub parent_id: Option<NodeId>,
    pub name: String,
    pub kind: Kind,
    /// Request ID of the last committed mutation; empty only for the immutable root.
    pub last_mutation_id: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub updated_by: String,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
    CreateFolder {
        parent: NodeId,
        name: String,
    },
    CreateFile {
        parent: NodeId,
        name: String,
        content_type: String,
        bytes: Vec<u8>,
    },
    Update {
        id: NodeId,
        expected_mutation_id: String,
        content_type: String,
        bytes: Vec<u8>,
    },
    Relocate {
        id: NodeId,
        expected_mutation_id: String,
        parent: NodeId,
        name: String,
    },
    Delete {
        id: NodeId,
        expected_mutation_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationResult {
    pub node: Node,
    pub deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RequestStatus {
    /// No committed result in this authority snapshot. A concurrently running
    /// request may still commit; retry with the SAME identity and fingerprint.
    Uncommitted,
    Committed {
        result: MutationResult,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Page {
    pub nodes: Vec<Node>,
    pub next_after: Option<NodeId>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadChunk {
    pub id: NodeId,
    pub last_mutation_id: String,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub eof: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct TextChunk {
    pub id: NodeId,
    pub last_mutation_id: String,
    pub offset: u64,
    pub text: String,
    pub eof: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchPage {
    pub nodes: Vec<Node>,
    pub next_after: Option<NodeId>,
    pub examined: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CollectionPage {
    pub removed: usize,
    pub next_after: Option<String>,
}

type Authorizer = dyn Fn(&Connection, bool) -> Result<()> + Send + Sync;

#[derive(Clone)]
pub struct Drive {
    authorizer: Option<Arc<Authorizer>>,
    database: FeatureDatabase,
    blobs: Arc<blob::BlobStore>,
    workspace_id: String,
    _storage: ScopedFeatureStorage,
}

impl Drive {
    pub fn register(storage: &ScopedFeatureStorage) -> feature_storage::Result<RegisteredFeature> {
        storage.register(FeatureRegistration::new("workspace-drive", MIGRATIONS))
    }
    pub fn open(
        storage: &ScopedFeatureStorage,
        registration: &RegisteredFeature,
        root: &Path,
    ) -> Result<Self> {
        let database = storage.open(registration)?;
        let workspace_id = storage.workspace_id().to_owned();
        let blobs = Arc::new(blob::BlobStore::open(root)?);
        database.try_transaction::<_,Error>(|tx| {
            tx.execute("INSERT OR IGNORE INTO drive_state(singleton,workspace_id) VALUES (1,?1)", [&workspace_id])?;
            let stored: String = tx.query_row("SELECT workspace_id FROM drive_state WHERE singleton=1", [], |r| r.get(0))?;
            if stored != workspace_id { return Err(Error::Invalid("metadata belongs to another Workspace".into())); }
            tx.execute("INSERT OR IGNORE INTO drive_nodes(workspace_id,parent_id,name,kind,last_mutation_id,size,updated_by,updated_at_ms)
                VALUES (?1,NULL,'','directory','',0,'server',?2)", params![workspace_id, now_ms()])?;
            Ok(())
        })?;
        Ok(Self {
            authorizer: None,
            database,
            blobs,
            workspace_id,
            _storage: storage.clone(),
        })
    }
    /// Trusted Server attachment: SQLite IMMEDIATE also locks this authority,
    /// so a cached Drive handle cannot race deletion reservation in server.db.
    /// This is Host configuration, never a model-selected database path. The
    /// attached authority is read-only by convention; Drive never writes it.
    pub fn open_with_workspace_authority(
        storage: &ScopedFeatureStorage,
        registration: &RegisteredFeature,
        root: &Path,
        authority_database: &Path,
    ) -> Result<Self> {
        let drive = Self::open(storage, registration, root)?;
        let path = authority_database
            .to_str()
            .ok_or_else(|| Error::Invalid("authority database path must be UTF-8".into()))?;
        if !authority_database.is_file() {
            return Err(Error::Invalid(
                "Workspace authority database is missing".into(),
            ));
        }
        drive.database.try_with_connection::<_, Error>(|c| {
            let existing: Option<String> = c
                .query_row(
                    "SELECT file FROM pragma_database_list WHERE name='yoi_workspace_authority'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(old) if Path::new(&old) != authority_database => Err(Error::Invalid(
                    "Drive already bound to a different Workspace authority".into(),
                )),
                Some(_) => Ok(()),
                None => {
                    c.execute("ATTACH DATABASE ?1 AS yoi_workspace_authority", [path])?;
                    Ok(())
                }
            }
        })?;
        Ok(drive)
    }
    /// Bind immutable caller authority to this handle and its clones. The callback
    /// runs within every client operation's IMMEDIATE transaction, never at bind
    /// time; attached authority stays locked through blob I/O and publication.
    /// Host maintenance must use an unbound handle.
    pub fn with_authorizer<F>(&self, authorizer: F) -> Self
    where
        F: Fn(&Connection, bool) -> Result<()> + Send + Sync + 'static,
    {
        Self {
            authorizer: Some(Arc::new(authorizer)),
            ..self.clone()
        }
    }
    fn authorize(&self, connection: &Connection, write: bool) -> Result<()> {
        match &self.authorizer {
            Some(authorizer) => authorizer(connection, write),
            None => Ok(()),
        }
    }
    /// Early admission check, not a lease: every subsequent operation still
    /// invokes its authorizer in that operation's own transaction.
    pub fn check_authorized(&self, write: bool) -> Result<()> {
        self.database.try_transaction(|tx| {
            self.authorize(tx, write)?;
            accepting(tx)
        })
    }
    pub fn root(&self) -> Result<Node> {
        self.database.try_transaction(|c| {
            self.authorize(c, false)?;
            let id = c.query_row(
                "SELECT id FROM drive_nodes WHERE parent_id IS NULL",
                [],
                |r| r.get(0),
            )?;
            node(c, NodeId(id))
        })
    }
    pub fn metadata(&self, id: NodeId) -> Result<Node> {
        self.database.try_transaction(|c| {
            self.authorize(c, false)?;
            node(c, id)
        })
    }

    /// Keyset pages are ordered by stable ID, not name. Concurrent mutations may
    /// appear/disappear between pages. There is no snapshot promise across calls.
    pub fn list(&self, parent: NodeId, after: Option<NodeId>, limit: usize) -> Result<Page> {
        validate_limit(limit, MAX_PAGE)?;
        self.database.try_transaction(|tx| {
            self.authorize(tx, false)?;
            directory(tx, parent)?;
            let mut stmt = tx.prepare(
                "SELECT id FROM drive_nodes WHERE parent_id=?1 AND id>?2 ORDER BY id LIMIT ?3",
            )?;
            let ids = stmt
                .query_map(
                    params![parent.0, after.map_or(0, |v| v.0), limit + 1],
                    |r| r.get::<_, i64>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let has_more = ids.len() > limit;
            let nodes = ids
                .into_iter()
                .take(limit)
                .map(|id| node(tx, NodeId(id)))
                .collect::<Result<Vec<_>>>()?;
            let next_after = has_more.then(|| nodes.last().unwrap().id);
            Ok(Page { nodes, next_after })
        })
    }

    /// Recover the committed result by request identity, including pre-cutover
    /// receipts whose payload cannot be verified for replay. Committed status is
    /// not proof that a different actor/payload matches that request.
    pub fn request_status(&self, request_id: &str) -> Result<RequestStatus> {
        validate_identity(request_id, "request_id", 128)?;
        self.database.try_transaction(|c| {
            self.authorize(c, false)?;
            let result = c
                .query_row(
                    "SELECT result_json FROM drive_receipts WHERE request_id=?1",
                    [request_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            match result {
                Some(json) => Ok(RequestStatus::Committed {
                    result: serde_json::from_str(&json)?,
                }),
                None => Ok(RequestStatus::Uncommitted),
            }
        })
    }

    /// DB commit is publication. The blob and receipt are never independently
    /// treated as a successful mutation. Successful request IDs are retained for
    /// the Workspace lifetime; reuse with a different actor/payload is conflict.
    /// Pre-cutover replay checks the original typed-payload digest using proven
    /// receipt correspondence. If that evidence is unavailable, returns Conflict
    /// without executing; recover the committed result through request_status.
    pub fn mutate(
        &self,
        request_id: &str,
        actor: &str,
        mutation: Mutation,
    ) -> Result<MutationResult> {
        validate_identity(request_id, "request_id", 128)?;
        validate_identity(actor, "actor", 256)?;
        validate_mutation(&mutation)?;
        let fingerprint: String = Sha256::digest(serde_json::to_vec(&(actor, &mutation))?)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.database.try_transaction(|tx| {
            self.authorize(tx, true)?;
            if let Some((old_fingerprint,json)) = tx.query_row("SELECT fingerprint,result_json FROM drive_receipts WHERE request_id=?1",
                [request_id], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()? {
                if old_fingerprint != fingerprint && !migrations::matches_legacy_replay(tx, request_id, actor, &mutation, &old_fingerprint)? {
                    return Err(Error::Conflict);
                }
                return Ok(serde_json::from_str(&json)?);
            }
            accepting(tx)?;
            let at = now_ms();
            let result = match mutation {
                Mutation::CreateFolder { parent,name } => {
                    directory(tx,parent)?;
                    insert(tx,&self.workspace_id,parent,&name,Kind::Directory,None,0,None,actor,at,request_id)?
                }
                Mutation::CreateFile { parent,name,content_type,bytes } => {
                    directory(tx,parent)?;
                    let key = Uuid::now_v7().to_string();
                    self.blobs.put(&key,&bytes)?;
                    insert(tx,&self.workspace_id,parent,&name,Kind::File,Some(&key),bytes.len(),Some(&content_type),actor,at,request_id)?
                }
                Mutation::Update { id,expected_mutation_id,content_type,bytes } => {
                    let old = changeable(tx,id,&expected_mutation_id)?;
                    if old.kind != Kind::File { return Err(Error::Invalid("cannot write a directory".into())); }
                    let key = Uuid::now_v7().to_string();
                    self.blobs.put(&key,&bytes)?;
                    cas(tx.execute("UPDATE drive_nodes SET blob_key=?1,size=?2,content_type=?3,last_mutation_id=?7,updated_by=?4,updated_at_ms=?5
                        WHERE id=?6", params![key,bytes.len(),content_type,actor,at,id.0,request_id])?)?;
                    MutationResult { node: node(tx,id)?, deleted:false }
                }
                Mutation::Relocate { id,expected_mutation_id,parent,name } => {
                    changeable(tx,id,&expected_mutation_id)?;
                    directory(tx,parent)?;
                    let cyclic: bool = tx.query_row("WITH RECURSIVE ancestors(id,parent_id) AS (
                        SELECT id,parent_id FROM drive_nodes WHERE id=?1 UNION ALL
                        SELECT n.id,n.parent_id FROM drive_nodes n JOIN ancestors a ON n.id=a.parent_id)
                        SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=?2)",params![parent.0,id.0],|r|r.get(0))?;
                    if cyclic { return Err(Error::Invalid("move would create a cycle".into())); }
                    unique(tx.execute("UPDATE drive_nodes SET parent_id=?1,name=?2,last_mutation_id=?6,updated_by=?3,updated_at_ms=?4
                        WHERE id=?5",params![parent.0,name,actor,at,id.0,request_id]))?;
                    MutationResult { node:node(tx,id)?,deleted:false }
                }
                Mutation::Delete { id,expected_mutation_id } => {
                    let old = changeable(tx,id,&expected_mutation_id)?;
                    let children: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM drive_nodes WHERE parent_id=?1)",[id.0],|r|r.get(0))?;
                    if children { return Err(Error::Conflict); }
                    cas(tx.execute("DELETE FROM drive_nodes WHERE id=?1",params![id.0])?)?;
                    MutationResult { node:old,deleted:true }
                }
            };
            tx.execute("INSERT INTO drive_receipts(request_id,fingerprint,result_json,committed_at_ms) VALUES (?1,?2,?3,?4)",
                params![request_id,fingerprint,serde_json::to_string(&result)?,at])?;
            Ok(result)
        })
    }

    /// Each chunk requires the originally observed committed mutation. Old blobs are
    /// not retained: after update/delete return conflict/not-found, never switch
    /// silently to the latest blob. GC cannot run while this read holds SQL lock.
    pub fn read(
        &self,
        id: NodeId,
        expected_mutation_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<ReadChunk> {
        validate_limit(length, MAX_READ_BYTES)?;
        self.database.try_transaction(|tx| {
            self.authorize(tx, false)?;
            accepting(tx)?;
            let selected = node(tx, id)?;
            if selected.last_mutation_id != expected_mutation_id {
                return Err(Error::Conflict);
            }
            if selected.kind != Kind::File {
                return Err(Error::Invalid("cannot read a directory".into()));
            }
            if offset > selected.size {
                return Err(Error::Invalid("offset beyond end of file".into()));
            }
            let end = offset.saturating_add(length as u64).min(selected.size);
            let key: String = tx.query_row(
                "SELECT blob_key FROM drive_nodes WHERE id=?1",
                [id.0],
                |r| r.get(0),
            )?;
            if self.blobs.size(&key)? != selected.size {
                return Err(Error::Invalid("referenced blob length mismatch".into()));
            }
            let bytes = self.blobs.read(&key, offset as usize..end as usize)?;
            if bytes.len() != (end - offset) as usize {
                return Err(Error::Invalid("referenced blob length mismatch".into()));
            }
            Ok(ReadChunk {
                id,
                last_mutation_id: selected.last_mutation_id,
                offset,
                bytes,
                eof: end == selected.size,
            })
        })
    }

    /// UTF-8 text over byte offsets. Callers must select character-aligned
    /// ranges; invalid UTF-8 (including a split character) is an explicit error.
    pub fn read_text(
        &self,
        id: NodeId,
        expected_mutation_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<TextChunk> {
        let chunk = self.read(id, expected_mutation_id, offset, length)?;
        let text = String::from_utf8(chunk.bytes).map_err(|_| {
            Error::Invalid("range is not valid UTF-8; select character-aligned offsets".into())
        })?;
        Ok(TextChunk {
            id,
            last_mutation_id: chunk.last_mutation_id,
            offset,
            text,
            eof: chunk.eof,
        })
    }

    /// Search examines at most `budget` nodes after the cursor. Text inspection
    /// is limited to UTF-8 text/* files <=64KiB, and matches are bounded by budget.
    /// An empty matching page may still carry a continuation cursor.
    pub fn search(
        &self,
        query: &str,
        text: bool,
        after: Option<NodeId>,
        budget: usize,
    ) -> Result<SearchPage> {
        validate_identity(query, "query", 256)?;
        validate_limit(budget, MAX_SEARCH_NODES)?;
        self.database.try_transaction(|tx| {
            self.authorize(tx, false)?;
            accepting(tx)?;
            let mut stmt = tx.prepare("SELECT id FROM drive_nodes WHERE parent_id IS NOT NULL AND id>?1 ORDER BY id LIMIT ?2")?;
            let ids = stmt.query_map(params![after.map_or(0,|v|v.0),budget+1],|r|r.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let more = ids.len()>budget;
            let mut nodes = Vec::new();
            let mut last = None;
            let mut examined = 0;
            for id in ids.into_iter().take(budget) {
                let n = node(tx,NodeId(id))?;
                last = Some(n.id); examined += 1;
                let mut matched = n.name.contains(query);
                if !matched && text && n.kind==Kind::File && n.size<=MAX_SEARCH_TEXT_BYTES as u64 && n.content_type.as_deref().is_some_and(|t|t.starts_with("text/")) {
                    let key: String = tx.query_row("SELECT blob_key FROM drive_nodes WHERE id=?1",[id],|r|r.get(0))?;
                    if self.blobs.size(&key)? != n.size { return Err(Error::Invalid("referenced blob length mismatch".into())); }
                    let bytes = self.blobs.read(&key,0..n.size as usize)?;
                    if bytes.len()!=n.size as usize { return Err(Error::Invalid("referenced blob length mismatch".into())); }
                    matched = std::str::from_utf8(&bytes).is_ok_and(|s|s.contains(query));
                }
                if matched { nodes.push(n); }
            }
            Ok(SearchPage { nodes, next_after:if more {last} else {None}, examined })
        })
    }

    /// Collection shares the exact database lock with uploads/readers. No grace
    /// period or process-local mutex is used as a safety proof. Repeat full sweeps
    /// from None, since new random keys can sort before a previous cursor.
    pub fn collect(&self, after: Option<&str>, limit: usize) -> Result<CollectionPage> {
        validate_limit(limit, MAX_PAGE)?;
        self.database.try_transaction(|tx| {
            accepting(tx)?;
            let keys = self.blobs.list(after, limit + 1)?;
            let more = keys.len() > limit;
            let mut removed = self.blobs.cleanup_staging(limit)?;
            let mut last = None;
            for key in keys.into_iter().take(limit) {
                let referenced: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM drive_nodes WHERE blob_key=?1)",
                    [&key],
                    |r| r.get(0),
                )?;
                if !referenced {
                    self.blobs.delete(&key)?;
                    removed += 1;
                }
                last = Some(key);
            }
            Ok(CollectionPage {
                removed,
                next_after: if more { last } else { None },
            })
        })
    }
    pub fn fence(&self) -> Result<()> {
        self.database.try_transaction(|tx| {
            tx.execute("UPDATE drive_state SET deleting=1 WHERE singleton=1", [])?;
            Ok(())
        })
    }
    /// Retryable purge. Fence is committed first. Root and deletion tombstone
    /// remain until Host FeatureStorage deletes the metadata scope.
    pub fn purge(&self) -> Result<()> {
        self.fence()?;
        self.database.try_transaction(|tx| {
            while self.blobs.cleanup_staging(MAX_PAGE)? > 0 {}
            loop {
                let keys = self.blobs.list(None, MAX_PAGE)?;
                if keys.is_empty() {
                    break;
                }
                for key in keys {
                    self.blobs.delete(&key)?;
                }
            }
            tx.execute("DELETE FROM drive_nodes WHERE parent_id IS NOT NULL", [])?;
            tx.execute("DELETE FROM drive_receipts", [])?;
            Ok(())
        })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn validate_limit(n: usize, max: usize) -> Result<()> {
    if n == 0 || n > max {
        return Err(Error::Invalid(format!("limit must be 1..={max}")));
    }
    Ok(())
}
fn validate_identity(s: &str, what: &str, max: usize) -> Result<()> {
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(Error::Invalid(format!("invalid {what}")));
    }
    Ok(())
}
fn validate_name(s: &str) -> Result<()> {
    validate_identity(s, "name", MAX_NAME_BYTES)?;
    if s == "." || s == ".." || s.contains(['/', '\\']) {
        return Err(Error::Invalid("name is not a path".into()));
    }
    Ok(())
}
fn validate_content(content_type: &str, bytes: &[u8]) -> Result<()> {
    validate_identity(content_type, "content_type", 128)?;
    if !content_type.is_ascii() || !content_type.contains('/') {
        return Err(Error::Invalid("invalid content_type".into()));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Invalid(format!(
            "file exceeds {MAX_FILE_BYTES} bytes"
        )));
    }
    Ok(())
}
fn validate_mutation(m: &Mutation) -> Result<()> {
    match m {
        Mutation::CreateFolder { name, .. } | Mutation::Relocate { name, .. } => {
            validate_name(name)
        }
        Mutation::CreateFile {
            name,
            content_type,
            bytes,
            ..
        } => {
            validate_name(name)?;
            validate_content(content_type, bytes)
        }
        Mutation::Update {
            content_type,
            bytes,
            ..
        } => validate_content(content_type, bytes),
        Mutation::Delete { .. } => Ok(()),
    }
}
fn accepting(c: &Connection) -> Result<()> {
    if c.query_row(
        "SELECT deleting FROM drive_state WHERE singleton=1",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::Fenced);
    }
    let attached: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_database_list WHERE name='yoi_workspace_authority')",
        [],
        |r| r.get(0),
    )?;
    if attached {
        let state:Option<String>=c.query_row("SELECT state FROM yoi_workspace_authority.workspaces WHERE workspace_id=(SELECT workspace_id FROM drive_state WHERE singleton=1)",[],|r|r.get(0)).optional()?;
        if state.as_deref() != Some("active") {
            return Err(Error::Fenced);
        }
    }
    Ok(())
}
fn node(c: &Connection, id: NodeId) -> Result<Node> {
    c.query_row("SELECT id,workspace_id,parent_id,name,kind,last_mutation_id,size,content_type,updated_by,updated_at_ms FROM drive_nodes WHERE id=?1",[id.0],|r| {
        Ok(Node {id:NodeId(r.get(0)?),workspace_id:r.get(1)?,parent_id:r.get::<_,Option<i64>>(2)?.map(NodeId),name:r.get(3)?,
            kind:if r.get::<_,String>(4)?=="file" {Kind::File} else {Kind::Directory},last_mutation_id:r.get(5)?,
            size:r.get::<_,i64>(6)? as u64,content_type:r.get(7)?,updated_by:r.get(8)?,updated_at_ms:r.get(9)?})
    }).optional()?.ok_or(Error::NotFound)
}
fn directory(c: &Connection, id: NodeId) -> Result<()> {
    if node(c, id)?.kind != Kind::Directory {
        Err(Error::Invalid("parent must be a directory".into()))
    } else {
        Ok(())
    }
}
// Called only inside the same IMMEDIATE transaction that publishes the mutation.
// Compare the last successful request, not time: even identical bytes or a move
// away and back cannot revive an old request. Receipts prevent request ID reuse.
fn changeable(c: &Transaction<'_>, id: NodeId, expected_mutation_id: &str) -> Result<Node> {
    let n = node(c, id)?;
    if n.parent_id.is_none() {
        return Err(Error::Invalid("root is immutable".into()));
    }
    validate_identity(expected_mutation_id, "expected_mutation_id", 128)?;
    if n.last_mutation_id != expected_mutation_id {
        return Err(Error::Conflict);
    }
    Ok(n)
}
fn unique(result: rusqlite::Result<usize>) -> Result<usize> {
    match result {
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            Err(Error::Conflict)
        }
        other => Ok(other?),
    }
}
fn cas(changed: usize) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(Error::Conflict)
    }
}
#[allow(clippy::too_many_arguments)]
fn insert(
    tx: &Transaction<'_>,
    workspace: &str,
    parent: NodeId,
    name: &str,
    kind: Kind,
    key: Option<&str>,
    size: usize,
    content_type: Option<&str>,
    actor: &str,
    at: i64,
    request_id: &str,
) -> Result<MutationResult> {
    unique(tx.execute("INSERT INTO drive_nodes(workspace_id,parent_id,name,kind,last_mutation_id,blob_key,size,content_type,updated_by,updated_at_ms)
        VALUES (?1,?2,?3,?4,?10,?5,?6,?7,?8,?9)",params![workspace,parent.0,name,if kind==Kind::File {"file"} else {"directory"},key,size,content_type,actor,at,request_id]))?;
    Ok(MutationResult {
        node: node(tx, NodeId(tx.last_insert_rowid()))?,
        deleted: false,
    })
}

#[cfg(test)]
mod tests;
