#![doc = include_str!("../README.md")]

use std::collections::BTreeMap;

use content_history::{History, HistoryId, Object, ObjectId, TreeId, verify};
use rusqlite::{OptionalExtension, Transaction, params};

/// New domain tables, deliberately not an alternate Server migration runner.
/// The adopting domain installs these through its ordered schema migration and fresh schema.
pub const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("content access denied")]
    Denied,
    #[error("content unavailable in authorized scope")]
    Unavailable,
    #[error("content scope was explicitly erased")]
    Erased,
    #[error("observed history head is stale")]
    Conflict,
    #[error("invalid content: {0}")]
    Model(#[from] content_history::Error),
    #[error("invalid stored history reference")]
    InvalidReference,
    #[error("immutable content differs from stored bytes")]
    ImmutableMismatch,
    #[error("sqlite content store: {0}")]
    Sql(#[from] rusqlite::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// The business owner supplies exact identities, not path containment rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeKey {
    pub workspace: String,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Erase,
}

/// Existing caller authority is checked at each operation, before querying objects.
/// This is a trusted Host integration seam, not a grant constructor or authentication system.
pub trait Authority {
    fn allows(&self, key: &ScopeKey, access: Access) -> bool;
}

/// An adapter borrows the caller's real transaction; it never commits business adoption.
/// Do not expose this type or its authority implementation directly to untrusted code.
pub struct ScopedStore<'a, 'conn> {
    tx: &'a Transaction<'conn>,
    key: ScopeKey,
    authority: &'a dyn Authority,
}

impl<'a, 'conn> ScopedStore<'a, 'conn> {
    pub fn new(tx: &'a Transaction<'conn>, key: ScopeKey, authority: &'a dyn Authority) -> Self {
        Self { tx, key, authority }
    }

    fn authorize(&self, access: Access) -> Result<()> {
        if self.key.workspace.is_empty()
            || self.key.target.is_empty()
            || !self.authority.allows(&self.key, access)
        {
            return Err(Error::Denied);
        }
        let erased: Option<bool> = self
            .tx
            .query_row(
                "SELECT erased FROM content_scopes WHERE workspace=?1 AND target=?2",
                params![self.key.workspace, self.key.target],
                |r| r.get(0),
            )
            .optional()?;
        if erased == Some(true) {
            return Err(Error::Erased);
        }
        Ok(())
    }

    fn ensure_scope(&self) -> Result<()> {
        self.tx.execute(
            "INSERT OR IGNORE INTO content_scopes(workspace,target) VALUES(?1,?2)",
            params![self.key.workspace, self.key.target],
        )?;
        Ok(())
    }

    pub fn head(&self) -> Result<Option<HistoryId>> {
        self.authorize(Access::Read)?;
        self.head_inner()
    }

    fn head_inner(&self) -> Result<Option<HistoryId>> {
        let raw: Option<Vec<u8>> = self
            .tx
            .query_row(
                "SELECT head FROM content_scopes WHERE workspace=?1 AND target=?2",
                params![self.key.workspace, self.key.target],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        raw.map(|v| {
            Ok(HistoryId::from_bytes(
                v.try_into().map_err(|_| Error::InvalidReference)?,
            ))
        })
        .transpose()
    }

    /// Preparation requires the full canonical bytes, not possession of another scope's hash.
    /// Prepared objects are not readable snapshots until adoption succeeds.
    pub fn put(&self, claimed: ObjectId, bytes: &[u8]) -> Result<()> {
        self.authorize(Access::Write)?;
        verify(claimed, bytes)?;
        self.atomic(|| self.put_inner(claimed, bytes))
    }

    fn put_inner(&self, id: ObjectId, bytes: &[u8]) -> Result<()> {
        self.ensure_scope()?;
        let (kind, hash) = parts(id);
        self.tx.execute(
            "INSERT OR IGNORE INTO content_objects(kind,hash,bytes) VALUES(?1,?2,?3)",
            params![kind, hash, bytes],
        )?;
        let stored: Vec<u8> = self.tx.query_row(
            "SELECT bytes FROM content_objects WHERE kind=?1 AND hash=?2",
            params![kind, hash],
            |r| r.get(0),
        )?;
        if stored != bytes {
            return Err(Error::ImmutableMismatch);
        }
        self.tx.execute("INSERT OR IGNORE INTO content_scope_objects(workspace,target,kind,hash) VALUES(?1,?2,?3,?4)",
            params![self.key.workspace, self.key.target, kind, hash])?;
        Ok(())
    }

    /// A hash read is confined to objects reachable from this target's adopted history.
    /// Unknown and cross-target hashes have the same result, regardless of physical deduplication.
    pub fn read(&self, id: ObjectId) -> Result<Object> {
        self.authorize(Access::Read)?;
        let (kind, hash) = parts(id);
        let allowed: bool = self.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM content_reachable WHERE workspace=?1 AND target=?2 AND kind=?3 AND hash=?4)",
            params![self.key.workspace, self.key.target, kind, hash], |r| r.get(0))?;
        if !allowed {
            return Err(Error::Unavailable);
        }
        self.load_prepared(id)
    }

    fn load_prepared(&self, id: ObjectId) -> Result<Object> {
        let (kind, hash) = parts(id);
        let bytes: Option<Vec<u8>> = self.tx.query_row(
            "SELECT o.bytes FROM content_objects o JOIN content_scope_objects s ON s.kind=o.kind AND s.hash=o.hash WHERE s.workspace=?1 AND s.target=?2 AND s.kind=?3 AND s.hash=?4",
            params![self.key.workspace, self.key.target, kind, hash], |r| r.get(0)).optional()?;
        Ok(verify(id, &bytes.ok_or(Error::Unavailable)?)?)
    }

    /// Restore uses a retained history, not a chain of text deltas. The returned graph includes
    /// every full blob and structural node and fails on any missing or corrupt descendant.
    pub fn snapshot(&self, history: HistoryId) -> Result<BTreeMap<ObjectId, Object>> {
        let Object::History(record) = self.read(ObjectId::History(history))? else {
            return Err(Error::InvalidReference);
        };
        self.walk([ObjectId::Tree(record.root)], true)
    }

    fn walk(
        &self,
        roots: impl IntoIterator<Item = ObjectId>,
        public: bool,
    ) -> Result<BTreeMap<ObjectId, Object>> {
        let mut pending: Vec<_> = roots.into_iter().collect();
        let mut objects = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if objects.contains_key(&id) {
                continue;
            }
            let object = if public {
                self.read(id)?
            } else {
                self.load_prepared(id)?
            };
            match &object {
                Object::Blob(_) => {}
                Object::Tree(tree) => {
                    pending.extend(tree.entries().iter().map(|e| ObjectId::from(e.target)))
                }
                Object::History(history) => {
                    pending.push(ObjectId::Tree(history.root));
                    if let Some(parent) = history.parent {
                        pending.push(ObjectId::History(parent));
                    }
                }
            }
            objects.insert(id, object);
        }
        Ok(objects)
    }

    /// Authenticated actor and clock facts belong to the caller, not restored post authors.
    /// Restoring an old root appends a history with the current parent; no rewind operation exists.
    /// The private savepoint prevents even a caught error from publishing a partial adoption.
    pub fn adopt(
        &self,
        root: TreeId,
        expected: Option<HistoryId>,
        actor: String,
        timestamp_micros: i64,
    ) -> Result<HistoryId> {
        self.authorize(Access::Write)?;
        self.atomic(|| {
            if self.head_inner()? != expected { return Err(Error::Conflict); }
            let mut graph = self.walk([ObjectId::Tree(root)], false)?;
            if let Some(parent) = expected {
                // Validate all retained ancestors too; corruption cannot become a successful new head.
                graph.extend(self.walk([ObjectId::History(parent)], false)?);
            }
            let history = Object::History(History { root, parent: expected, actor, timestamp_micros });
            let ObjectId::History(id) = history.id() else { unreachable!() };
            self.put_inner(history.id(), &history.encode())?;
            graph.insert(history.id(), history);
            for object in graph.keys() {
                let (kind, hash) = parts(*object);
                self.tx.execute("INSERT OR IGNORE INTO content_reachable(workspace,target,kind,hash) VALUES(?1,?2,?3,?4)",
                    params![self.key.workspace, self.key.target, kind, hash])?;
            }
            let changed = self.tx.execute(
                "UPDATE content_scopes SET head=?3 WHERE workspace=?1 AND target=?2 AND erased=0 AND head IS ?4",
                params![self.key.workspace, self.key.target, id.as_bytes().as_slice(), expected.map(|h| h.as_bytes().to_vec())])?;
            if changed != 1 { return Err(Error::Conflict); }
            Ok(id)
        })
    }

    /// Retains every adopted history. Only unadopted preparations are reclaimed.
    /// Physical rows survive while *any* target has prepared or retained ownership.
    pub fn collect_unadopted(&self) -> Result<()> {
        self.authorize(Access::Write)?;
        self.atomic(|| {
            self.tx.execute("DELETE FROM content_scope_objects AS s WHERE workspace=?1 AND target=?2 AND NOT EXISTS(SELECT 1 FROM content_reachable r WHERE r.workspace=s.workspace AND r.target=s.target AND r.kind=s.kind AND r.hash=s.hash)",
                params![self.key.workspace, self.key.target])?;
            self.collect_physical()
        })
    }

    /// Erasure is permanent for this exact business identity, including its old roots.
    /// Shared bytes retained by other authorized targets are not destroyed. Backups and bytes
    /// already exported by callers are outside this adapter's logical erasure guarantee.
    pub fn erase(&self) -> Result<()> {
        self.authorize(Access::Erase)?;
        self.atomic(|| {
            self.ensure_scope()?;
            self.tx.execute(
                "DELETE FROM content_reachable WHERE workspace=?1 AND target=?2",
                params![self.key.workspace, self.key.target],
            )?;
            self.tx.execute(
                "DELETE FROM content_scope_objects WHERE workspace=?1 AND target=?2",
                params![self.key.workspace, self.key.target],
            )?;
            self.tx.execute(
                "UPDATE content_scopes SET head=NULL,erased=1 WHERE workspace=?1 AND target=?2",
                params![self.key.workspace, self.key.target],
            )?;
            self.collect_physical()
        })
    }

    fn collect_physical(&self) -> Result<()> {
        self.tx.execute("DELETE FROM content_objects AS o WHERE NOT EXISTS(SELECT 1 FROM content_scope_objects s WHERE s.kind=o.kind AND s.hash=o.hash)", [])?;
        Ok(())
    }

    fn atomic<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.tx
            .execute_batch("SAVEPOINT content_history_operation")?;
        match f() {
            Ok(value) => {
                self.tx.execute_batch("RELEASE content_history_operation")?;
                Ok(value)
            }
            Err(error) => {
                self.tx.execute_batch(
                    "ROLLBACK TO content_history_operation; RELEASE content_history_operation",
                )?;
                Err(error)
            }
        }
    }
}

fn parts(id: ObjectId) -> (u8, Vec<u8>) {
    match id {
        ObjectId::Blob(h) => (1, h.as_bytes().to_vec()),
        ObjectId::Tree(h) => (2, h.as_bytes().to_vec()),
        ObjectId::History(h) => (3, h.as_bytes().to_vec()),
    }
}
