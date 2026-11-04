//! Physical ownership is independent of domain authorization. The trusted Host
//! supplies adoption evidence and validates domain attribution inside the identity
//! transaction. Merely finding a file never grants adoption authority.
//!
//! Lock order is lifecycle file, manager state, connection, SQLite transaction.
//! The stable scope lock is shared through consumer commit/rollback and exclusive
//! during provisioning, adoption, deletion and restore, across managers/processes.
//! It is advisory: arbitrary native code bypassing this owner can rename files.
//! Such replacement is rejected on the next access using the file identity and
//! persisted physical identity. Hosts must protect the root and its lock/owner DB
//! against uncooperative writers; this is not a filesystem sandbox.
use super::*;
use rusqlite::OptionalExtension;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::MetadataExt;

const IDENTITY_TABLE: &str = "__yoi_storage_identity";

/// Expected storage target, retained with authorization and rechecked on resume.
/// This is a physical identity, not an authorization credential or a Job lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseIdentity {
    pub scope: String,
    pub feature: String,
    pub physical: String,
}

#[derive(Clone, Debug)]
pub(super) struct Binding {
    pub identity: DatabaseIdentity,
    pub device: u64,
    pub inode: u64,
    pub request: String,
}

pub(super) struct Lifecycle {
    pub root: PathBuf,
    pub scope: String,
}

impl Lifecycle {
    pub fn lock(&self, exclusive: bool) -> Result<File> {
        durable_create_directory(&self.root.join(".lifecycle"), &mut sync_directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(
                self.root
                    .join(".lifecycle")
                    .join(format!("{}.lock", self.scope)),
            )?;
        if exclusive {
            lock.lock()?;
        } else {
            lock.lock_shared()?;
        }
        Ok(lock)
    }

    fn catalog(&self, create: bool) -> Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | if create {
                OpenFlags::SQLITE_OPEN_CREATE
            } else {
                OpenFlags::empty()
            };
        let connection = Connection::open_with_flags(self.root.join(".owner.sqlite"), flags)?;
        configure_connection(&connection)?;
        if create {
            connection.execute_batch(
                "
CREATE TABLE IF NOT EXISTS storage_requests (
 request TEXT PRIMARY KEY, scope TEXT NOT NULL, feature TEXT NOT NULL,
 physical TEXT NOT NULL, device INTEGER NOT NULL, inode INTEGER NOT NULL,
 kind TEXT NOT NULL, evidence TEXT NOT NULL, completed INTEGER NOT NULL DEFAULT 0,
 current INTEGER NOT NULL DEFAULT 1);
CREATE UNIQUE INDEX IF NOT EXISTS storage_current_target ON storage_requests(scope,feature) WHERE current=1;
CREATE TABLE IF NOT EXISTS storage_deletions (scope TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS storage_restores (scope TEXT PRIMARY KEY, evidence TEXT NOT NULL, published INTEGER NOT NULL DEFAULT 0);
",
            )?;
        }
        Ok(connection)
    }

    pub fn fenced(&self, scope: &str) -> Result<bool> {
        if legacy_owner::fenced(&self.root, scope)? {
            return Ok(true);
        }
        if !self.root.join(".owner.sqlite").try_exists()? {
            return Ok(false);
        }
        Ok(self.catalog(false)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_deletions WHERE scope=?1)",
            [scope],
            |r| r.get(0),
        )?)
    }

    /// Reserve the entire restore before creating staging. An unfinished copy
    /// cannot be adopted by another source/request, even if its files look valid.
    pub fn reserve_restore(
        &self,
        scope: &str,
        evidence: &str,
        temporary: &Path,
        destination: &Path,
    ) -> Result<()> {
        let mut catalog = self.catalog(true)?;
        let tx = catalog.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String, bool)> = tx
            .query_row(
                "SELECT evidence,published FROM storage_restores WHERE scope=?1",
                [scope],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match previous {
            Some((saved, _)) if saved == evidence => {}
            Some((_, false)) => {
                return Err(FeatureStorageError::Identity(
                    "another restore is unfinished; reconcile its exact source/request".into(),
                ));
            }
            Some((_, true)) if destination.exists() || temporary.exists() => {
                return Err(FeatureStorageError::Identity(
                    "another restore owns the existing destination/staging".into(),
                ));
            }
            None if destination.exists() => {
                return Err(FeatureStorageError::DestinationExists(
                    destination.display().to_string(),
                ));
            }
            None if temporary.exists() => {
                return Err(FeatureStorageError::Identity(
                    "unowned restore staging requires explicit reconciliation".into(),
                ));
            }
            _ => {
                tx.execute("INSERT INTO storage_restores(scope,evidence,published) VALUES(?1,?2,0) ON CONFLICT(scope) DO UPDATE SET evidence=excluded.evidence,published=0", params![scope,evidence])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn publish_restore(&self, scope: &str, evidence: &str) -> Result<()> {
        let changed = self.catalog(false)?.execute(
            "UPDATE storage_restores SET published=1 WHERE scope=?1 AND evidence=?2",
            params![scope, evidence],
        )?;
        if changed != 1 {
            return Err(FeatureStorageError::Identity(
                "restore request changed".into(),
            ));
        }
        Ok(())
    }

    pub fn fenced_for_restore(&self, scope: &str) -> Result<bool> {
        if legacy_owner::fenced_for_restore(&self.root, scope)? {
            return Ok(true);
        }
        if !self.root.join(".owner.sqlite").try_exists()? {
            return Ok(false);
        }
        Ok(self.catalog(false)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_deletions WHERE scope=?1)",
            [scope],
            |r| r.get(0),
        )?)
    }

    pub fn delete(&self, scope: &str) -> Result<()> {
        let directory = self.root.join(".lifecycle");
        durable_create_directory(&directory, &mut sync_directory)?;
        let fence = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(format!("{scope}.deleted")))?;
        fence.sync_all()?;
        sync_directory(&directory)?;
        self.catalog(true)?.execute(
            "INSERT OR IGNORE INTO storage_deletions(scope) VALUES(?1)",
            [scope],
        )?;
        Ok(())
    }

    pub fn lookup(&self, scope: &str, feature: &str) -> Result<Option<Binding>> {
        if self.fenced(scope)? {
            return Err(FeatureStorageError::ScopeMaintenance(scope.into()));
        }
        if !self.root.join(".owner.sqlite").try_exists()? {
            return Ok(None);
        }
        let catalog = self.catalog(false)?;
        Ok(catalog.query_row("SELECT physical,device,inode,request FROM storage_requests WHERE scope=?1 AND feature=?2 AND completed=1 AND current=1", params![scope,feature], |r| Ok(Binding {
            identity: DatabaseIdentity { scope: scope.into(), feature: feature.into(), physical: r.get(0)? },
            device: r.get(1)?, inode: r.get(2)?, request: r.get(3)?,
        })).optional()?)
    }

    /// Called under exclusive lifecycle lock. The reservation precedes identity
    /// persistence and survives response loss; exact retries reconcile the same
    /// inode and UUID rather than manufacture past provisioning permission.
    pub fn reserve(
        &self,
        scope: &str,
        feature: &str,
        path: &Path,
        request: &str,
        kind: &str,
        evidence: &str,
    ) -> Result<Binding> {
        if request.trim().is_empty() || evidence.trim().is_empty() {
            return Err(FeatureStorageError::Identity(
                "request and Host authority evidence are required".into(),
            ));
        }
        let fenced = if kind == "restore" {
            self.fenced_for_restore(scope)?
        } else {
            self.fenced(scope)?
        };
        if fenced {
            return Err(FeatureStorageError::ScopeMaintenance(scope.into()));
        }
        if kind != "provision" && !path.is_file() {
            return Err(FeatureStorageError::Identity(
                "expected existing database is missing".into(),
            ));
        }
        let mut catalog = self.catalog(true)?;
        let tx = catalog.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<(String,String,String,u64,u64,String,String,bool)> = tx.query_row(
            "SELECT scope,feature,physical,device,inode,kind,evidence,current FROM storage_requests WHERE request=?1", [request],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
        if let Some((s, f, physical, device, inode, k, e, current)) = saved {
            if s != scope || f != feature || k != kind || e != evidence || !current {
                return Err(FeatureStorageError::Identity(
                    "request identity was reused for a different target or authority".into(),
                ));
            }
            let binding = Binding {
                identity: DatabaseIdentity {
                    scope: s,
                    feature: f,
                    physical,
                },
                device,
                inode,
                request: request.into(),
            };
            binding.check_file(path)?;
            return Ok(binding);
        }
        let busy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_requests WHERE scope=?1 AND feature=?2 AND current=1)",
            params![scope, feature],
            |r| r.get(0),
        )?;
        if busy && kind != "restore" {
            return Err(FeatureStorageError::Identity(
                "target already has an ownership request; reconcile that request".into(),
            ));
        }
        if kind == "provision" {
            durable_create_directory(
                path.parent()
                    .ok_or_else(|| FeatureStorageError::Identity("missing parent".into()))?,
                &mut sync_directory,
            )?;
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.sync_all()?;
            sync_directory(path.parent().unwrap())?;
        }
        // A verified restore supersedes the active physical lifecycle, but keeps
        // every old request/receipt as provenance. It cannot replay as current authority.
        if kind == "restore" {
            tx.execute(
                "UPDATE storage_requests SET current=0 WHERE scope=?1 AND feature=?2 AND current=1",
                params![scope, feature],
            )?;
        }
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(FeatureStorageError::Identity(
                "database is not a regular file".into(),
            ));
        }
        let binding = Binding {
            identity: DatabaseIdentity {
                scope: scope.into(),
                feature: feature.into(),
                physical: if kind == "adopt" {
                    legacy_owner::identity(path, scope, feature)?
                        .unwrap_or_else(|| Uuid::now_v7().to_string())
                } else {
                    Uuid::now_v7().to_string()
                },
            },
            device: metadata.dev(),
            inode: metadata.ino(),
            request: request.into(),
        };
        tx.execute("INSERT INTO storage_requests(request,scope,feature,physical,device,inode,kind,evidence) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![request,scope,feature,binding.identity.physical,binding.device,binding.inode,kind,evidence])?;
        tx.commit()?;
        Ok(binding)
    }

    /// Snapshot bytes are not authority. Match their saved identity to this
    /// explicitly supplied source owner's completed provenance, including an old
    /// lifecycle superseded by a prior restore. The caller holds its lifecycle lock.
    pub fn verify_snapshot(
        &self,
        scope: &str,
        feature: &str,
        path: &Path,
    ) -> Result<(String, String)> {
        if self.fenced_for_restore(scope)? {
            return Err(FeatureStorageError::ScopeMaintenance(scope.into()));
        }
        if !self.root.join(".owner.sqlite").is_file() {
            return Err(FeatureStorageError::Identity(
                "snapshot source owner is missing".into(),
            ));
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (s, f, physical, request): (String, String, String, String) = connection.query_row(
            "SELECT scope,feature,physical,request FROM __yoi_storage_identity WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        Uuid::parse_str(&physical).map_err(|_| {
            FeatureStorageError::Identity("invalid snapshot physical identity".into())
        })?;
        let known: bool = self.catalog(false)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_requests WHERE scope=?1 AND feature=?2 AND physical=?3 AND request=?4 AND completed=1)",
            params![scope, feature, physical, request], |r| r.get(0))?;
        if s != scope || f != feature || !known {
            return Err(FeatureStorageError::Identity(
                "snapshot does not belong to the selected source owner".into(),
            ));
        }
        Ok((physical, request))
    }

    pub fn request_kind(&self, request: &str) -> Result<Option<String>> {
        if !self.root.join(".owner.sqlite").try_exists()? {
            return Ok(None);
        }
        Ok(self
            .catalog(false)?
            .query_row(
                "SELECT kind FROM storage_requests WHERE request=?1 AND current=1",
                [request],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn complete(&self, binding: &Binding) -> Result<()> {
        let changed = self.catalog(false)?.execute("UPDATE storage_requests SET completed=1 WHERE request=?1 AND scope=?2 AND feature=?3 AND physical=?4 AND device=?5 AND inode=?6 AND current=1",params![binding.request,binding.identity.scope,binding.identity.feature,binding.identity.physical,binding.device,binding.inode])?;
        if changed != 1 {
            return Err(FeatureStorageError::Identity(
                "ownership reservation changed".into(),
            ));
        }
        Ok(())
    }

    pub fn verify(&self, binding: &Binding, path: &Path, connection: &Connection) -> Result<()> {
        let current = self
            .lookup(&binding.identity.scope, &binding.identity.feature)?
            .ok_or_else(|| FeatureStorageError::Identity("no completed ownership record".into()))?;
        if current.identity != binding.identity
            || current.device != binding.device
            || current.inode != binding.inode
            || current.request != binding.request
        {
            return Err(FeatureStorageError::Identity(
                "physical ownership changed".into(),
            ));
        }
        binding.check_file(path)?;
        binding.check_database(connection)
    }
}

impl Binding {
    pub fn unknown(&self, error: FeatureStorageError) -> FeatureStorageError {
        FeatureStorageError::OutcomeUnknown {
            identity: self.identity.clone(),
            detail: error.to_string(),
        }
    }
    pub fn check_file(&self, path: &Path) -> Result<()> {
        let metadata = fs::metadata(path)?;
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(FeatureStorageError::Identity(
                "database path was replaced; explicit authorized adoption is required".into(),
            ));
        }
        Ok(())
    }

    pub fn check_database(&self, connection: &Connection) -> Result<()> {
        Uuid::parse_str(&self.identity.physical)
            .map_err(|_| FeatureStorageError::Identity("invalid physical identity".into()))?;
        let found: Option<(String, String, String, String)> = connection
            .query_row(
                &format!(
                    "SELECT scope,feature,physical,request FROM {IDENTITY_TABLE} WHERE singleton=1"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if found
            != Some((
                self.identity.scope.clone(),
                self.identity.feature.clone(),
                self.identity.physical.clone(),
                self.request.clone(),
            ))
        {
            return Err(FeatureStorageError::Identity(
                "database identity/receipt is missing or mismatched".into(),
            ));
        }
        Ok(())
    }

    /// Restore is a new physical lifecycle. Retain old provenance but never
    /// return its identity as authority over the restored inode.
    pub fn restore_identity(&self, connection: &mut Connection) -> Result<()> {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='__yoi_storage_identity')",
            [],
            |r| r.get(0),
        )?;
        if exists {
            if self.check_database(&tx).is_ok() {
                return Ok(());
            }
            let (scope, feature): (String, String) = tx.query_row(
                "SELECT scope,feature FROM __yoi_storage_identity WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if scope != self.identity.scope || feature != self.identity.feature {
                return Err(FeatureStorageError::Identity(
                    "snapshot belongs to another scope".into(),
                ));
            }
            tx.execute_batch("CREATE TABLE IF NOT EXISTS __yoi_storage_origins (scope TEXT NOT NULL, feature TEXT NOT NULL, physical TEXT NOT NULL, request TEXT NOT NULL, PRIMARY KEY(physical,request)); INSERT OR IGNORE INTO __yoi_storage_origins SELECT scope,feature,physical,request FROM __yoi_storage_identity; DELETE FROM __yoi_storage_identity;")?;
        }
        self.persist(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn persist(&self, tx: &Transaction<'_>) -> Result<()> {
        tx.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {IDENTITY_TABLE} (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1), scope TEXT NOT NULL,
 feature TEXT NOT NULL, physical TEXT NOT NULL, request TEXT NOT NULL);"
        ))?;
        let count: i64 =
            tx.query_row(&format!("SELECT COUNT(*) FROM {IDENTITY_TABLE}"), [], |r| {
                r.get(0)
            })?;
        if count == 0 {
            tx.execute(
                &format!("INSERT INTO {IDENTITY_TABLE} VALUES(1,?1,?2,?3,?4)"),
                params![
                    self.identity.scope,
                    self.identity.feature,
                    self.identity.physical,
                    self.request
                ],
            )?;
        }
        self.check_database(tx)
    }
}
