//! Editable source checkouts are caches, not policy/history authorities.
//!
//! A checkout binds one Backend origin and explicit Workspace to the Server's head,
//! content root and complete source snapshot. Source text is never rewritten by push
//! or reconciliation. Pull installs into a new directory: even a concurrent editor's
//! changes in an older checkout cannot be lost. To refresh or resolve conflicts, pull
//! a second checkout and copy the intended edits after comparing both trees.
//!
//! Before commit, the exact request is atomically saved and synced. Once a commit
//! may have escaped, only reconciliation or explicit exact retry can resolve it;
//! NotObserved does not release that request. This is a retry envelope, not a local
//! copy of the Server's adoption state machine or policy resolver.

use serde::{Deserialize, Serialize};
use server_api::{
    ConfigCommitRequest, ConfigContentType, ConfigTreeChange, ConfigurationAdoptionApiClient,
    ConfigurationAdoptionReceipt, ConfigurationAdoptionRequest, ConfigurationApiError,
    ConfigurationReconciliation, ConfigurationSourceResponse,
    client_support::{ClientError, RequestAuthorizer},
};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[cfg(test)]
mod tests;

pub const HELP: &str = r#"Usage: yoi --backend <URL> --workspace-id <ID> config <COMMAND> --dir <PATH>

Commands:
  pull       Download editable Decodal source into a NEW directory
  diff       Show base-relative additions, edits, explicit deletions and entrypoints
  validate   Validate against the actual Server snapshot/schema/policy (no adoption)
  preview    Validate and show Server-authoritative configuration changes
  push       Preview, retain the exact request, then adopt using expected history head
  reconcile  Look up the retained exact request; NotObserved remains unknown
  retry      Explicitly resend ONLY the retained original request, never later edits

Use `yoi login` for this Backend first. Workspace selection is always explicit.
Edit files under <PATH>/source in your normal editor. edit.json holds entrypoints
and content types (new *.dcdl files default to Decodal; other new files to text).
Removing a tracked file explicitly deletes it. Unchanged files are omitted from
the delta, NOT deleted. Omitted configuration fields retain the Server's semantics.
`diff` includes source text; do not share it with untrusted audiences.

Pull never overwrites an existing directory. After push, pull into a new directory
and compare/copy any further local edits. For a conflict, likewise pull a fresh
checkout and manually apply intended edits, then validate/preview/push there.
Do not copy .yoi-config metadata between checkouts or edit its request envelope.
If a push is unknown: keep the directory, reconcile, and if needed explicitly
retry. Neither NotObserved nor a later denial proves that an earlier send failed.
Later edits remain on disk, but retry always sends the retained original payload.
A receipt confirms source/policy storage, not completion of running work; policy
applies at the next current-policy check. No Git executable/protocol is used.

Shared permission configuration example (edit the actual pulled source path):
  { sets = [{ id = "workspace-ticket-query";
      namespaces = [{ namespace = "ticket"; keys = ["ticket.query"]; }];
    }];
    worker_default = { sets = ["workspace-ticket-query"]; };
  }
The imported file is a plain object; main.dcdl keeps its existing schema/import.
Permission configuration defines shared Workspace Sets and the Worker default.
New ordinary Workers select this default when neither an assignment nor a launch
policy is specified, regardless of caller. Omitted configuration selects the
implementer Set; an explicit empty assignment stays empty. Delegated Workers
remain bounded by their source. Subject bodies and Jobs use dedicated authority;
retries and restores retain their issued grant.
Individual Subject policies are Server-managed domain data, not config source.
Keys carry no payloads; namespace denotes Feature ownership, not prefix inclusion.
Only catalog-declared children expand. Use the catalog in the read-only pull
metadata to discover Sets/keys/children; shared Sets cannot replace Feature Sets.

Example workflow (substitute the same explicit Backend and Workspace each time):
  yoi --backend <URL> login
  yoi --backend <URL> --workspace-id <ID> config pull --dir config-before
  $EDITOR config-before/source/<actual-pulled-path>.dcdl
  yoi --backend <URL> --workspace-id <ID> config diff --dir config-before
  yoi --backend <URL> --workspace-id <ID> config validate --dir config-before
  yoi --backend <URL> --workspace-id <ID> config preview --dir config-before
  yoi --backend <URL> --workspace-id <ID> config push --dir config-before
  yoi --backend <URL> --workspace-id <ID> config pull --dir config-after
Use the actual paths returned by pull; the CLI does not generate policy data.
To reconcile:
  yoi --backend <URL> --workspace-id <ID> config reconcile --dir config-before
If still unknown, `config retry --dir config-before` uses the saved identity and
complete payload, NOT source files edited since that send. Keep both checkouts.

Exit codes: 0 confirmed success; 1 CLI argument/target-selection error;
2 local checkout/input/I/O error; 3 Server validation rejected;
4 head/CAS conflict; 5 confirmed authorization refusal; 6 transport/read failure;
7 adoption outcome unknown (retain request and reconcile).
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Pull,
    Diff,
    Validate,
    Preview,
    Push,
    Reconcile,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub command: Command,
    pub directory: PathBuf,
}

pub fn parse(args: &[String]) -> Result<Option<Options>, Error> {
    if args.is_empty()
        || args
            .iter()
            .any(|a| matches!(a.as_str(), "--help" | "-h" | "help"))
    {
        return Ok(None);
    }
    let command = match args[0].as_str() {
        "pull" => Command::Pull,
        "diff" => Command::Diff,
        "validate" => Command::Validate,
        "preview" => Command::Preview,
        "push" => Command::Push,
        "reconcile" => Command::Reconcile,
        "retry" => Command::Retry,
        _ => return Err(local("unknown config command")),
    };
    let directory = match &args[1..] {
        [flag, value] if flag == "--dir" && !value.is_empty() && !value.starts_with('-') => {
            PathBuf::from(value)
        }
        [value] if value.starts_with("--dir=") && value.len() > 6 => PathBuf::from(&value[6..]),
        _ => return Err(local("config requires exactly --dir <PATH>")),
    };
    Ok(Some(Options { command, directory }))
}

#[derive(Debug)]
pub struct Error {
    pub exit_code: u8,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
fn local(message: impl Into<String>) -> Error {
    Error {
        exit_code: 2,
        message: message.into(),
    }
}
fn io_error(_: std::io::Error) -> Error {
    local("local checkout I/O failed; files and retained request were not discarded")
}
fn json_error(_: serde_json::Error) -> Error {
    local("invalid checkout JSON; preserve metadata and source before repairing")
}

fn remote(error: ClientError<ConfigurationApiError>, adopting: bool) -> Error {
    let explanation = match &error {
        ClientError::Public { error, .. } => error.details().and_then(|details| {
            let facts = error_contract::FailureFacts::new(*details).ok()?;
            error_contract::project_failure::<error_contract::Tool, _>(
                &facts,
                &server_api::configuration_failure::ConfigurationFailureVisibility {
                    can_view_cause: true,
                },
            )
            .ok()
            .map(|projected| projected.explanation().to_owned())
        }),
        _ => None,
    };
    let (exit_code, mut message) = match error {
        ClientError::Public { status, .. } if matches!(status.as_u16(), 401 | 403) => {
            (5, "authorization refused by Server".to_owned())
        }
        ClientError::Public { status, .. } if status.as_u16() == 409 => (
            4,
            "Server head/CAS conflict; pull a fresh checkout and compare intended edits".to_owned(),
        ),
        ClientError::Public { status, .. } if matches!(status.as_u16(), 400 | 422) => (
            3,
            "Server validation rejected source/request; no new adoption confirmed".to_owned(),
        ),
        error if adopting => (
            7,
            format!(
                "adoption outcome unknown ({error}); retain checkout, reconcile or exact retry"
            ),
        ),
        error => (
            6,
            format!("Server read/preview transport failed ({error}); no commit was sent"),
        ),
    };
    if let Some(explanation) = explanation {
        message.push_str("; ");
        message.push_str(&explanation);
    }
    Error { exit_code, message }
}

fn unresolved(error: ClientError<ConfigurationApiError>) -> Error {
    let mut error = remote(error, true);
    error
        .message
        .push_str("; earlier outcome still unresolved, original request retained");
    error
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkout {
    version: u32,
    backend: String,
    workspace: String,
    base: EditingBase,
    pending: Option<ConfigurationAdoptionRequest>,
}

/// Only observations needed for authoring and CAS, never a local effective policy.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditingBase {
    state: server_api::WorkspaceConfigTreeResponse,
    history_head: Option<String>,
    catalog: feature_permissions::PermissionCatalog,
    catalog_digest: String,
    expected_legacy_export: Option<String>,
}
impl From<ConfigurationSourceResponse> for EditingBase {
    fn from(source: ConfigurationSourceResponse) -> Self {
        Self {
            state: source.state,
            history_head: source.history_head,
            catalog: source.catalog,
            catalog_digest: source.catalog_digest,
            expected_legacy_export: source.expected_legacy_export,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Edit {
    entrypoints: Vec<String>,
    content_types: BTreeMap<String, ConfigContentType>,
}

fn edit_for(base: &ConfigurationSourceResponse) -> Edit {
    Edit {
        entrypoints: base.state.contract.entrypoints.clone(),
        content_types: base
            .state
            .snapshot
            .entries
            .iter()
            .map(|(p, e)| (p.clone(), e.content_type))
            .collect(),
    }
}

fn safe_relative(path: &str) -> Result<&Path, Error> {
    let value = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !value
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(local("source path must be a canonical relative path"));
    }
    Ok(value)
}
fn reject_symlink(path: &Path) -> Result<(), Error> {
    if fs::symlink_metadata(path)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(local("checkout symlinks are not followed"));
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    reject_symlink(path)?;
    serde_json::from_slice(&fs::read(path).map_err(io_error)?).map_err(json_error)
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| local("missing metadata parent"))?;
    let temp = parent.join(format!(".write-{}", uuid::Uuid::now_v7()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(io_error)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(json_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    fs::rename(&temp, path).map_err(io_error)?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}

struct LockedCheckout {
    state: Checkout,
    directory: PathBuf,
    _lock: File,
}
impl LockedCheckout {
    fn open(directory: &Path, backend: &str, workspace: &str) -> Result<Self, Error> {
        reject_symlink(directory)?;
        reject_symlink(&directory.join(".yoi-config"))?;
        let lock_path = directory.join(".yoi-config/lock");
        reject_symlink(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(io_error)?;
        if !fs4::fs_std::FileExt::try_lock_exclusive(&lock).map_err(io_error)? {
            return Err(local("another config command holds this checkout"));
        }
        let state: Checkout = read_json(&directory.join(".yoi-config/state.json"))?;
        if state.version != 1 || state.backend != backend || state.workspace != workspace {
            return Err(local(
                "checkout belongs to a different Backend/Workspace or unsupported version",
            ));
        }
        Ok(Self {
            state,
            directory: directory.to_owned(),
            _lock: lock,
        })
    }
    fn save(&self) -> Result<(), Error> {
        atomic_json(&self.directory.join(".yoi-config/state.json"), &self.state)
    }
    fn request(&self) -> Result<ConfigurationAdoptionRequest, Error> {
        let edit: Edit = read_json(&self.directory.join("edit.json"))?;
        for path in &edit.entrypoints {
            safe_relative(path)?;
        }
        for path in edit.content_types.keys() {
            safe_relative(path)?;
        }
        let mut files = BTreeMap::new();
        scan(&self.directory.join("source"), "", &mut files)?;
        let base = &self.state.base;
        let mut changes = Vec::new();
        for (path, content) in &files {
            let content_type = edit.content_types.get(path).copied().unwrap_or_else(|| {
                if path.ends_with(".dcdl") {
                    ConfigContentType::Decodal
                } else {
                    ConfigContentType::Text
                }
            });
            match base.state.snapshot.entries.get(path) {
                None => changes.push(ConfigTreeChange::Create {
                    path: path.clone(),
                    content_type,
                    content: content.clone(),
                }),
                Some(old) if old.content_type != content_type => {
                    return Err(local(
                        "changing an existing content type requires explicit deletion and a later create",
                    ));
                }
                Some(old) if old.content != *content => changes.push(ConfigTreeChange::Update {
                    path: path.clone(),
                    expected_digest: old.content_digest.clone(),
                    content: content.clone(),
                }),
                Some(_) => (),
            }
        }
        for (path, old) in &base.state.snapshot.entries {
            if !files.contains_key(path) {
                changes.push(ConfigTreeChange::Delete {
                    path: path.clone(),
                    expected_digest: old.content_digest.clone(),
                });
            }
        }
        Ok(ConfigurationAdoptionRequest {
            operation_id: uuid::Uuid::now_v7().to_string(),
            expected_head: base.history_head.clone(),
            expected_catalog_digest: base.catalog_digest.clone(),
            expected_legacy_export: base.expected_legacy_export.clone(),
            source: ConfigCommitRequest {
                base_digest: base.state.snapshot.digest.clone(),
                changes,
                entrypoints: edit.entrypoints,
            },
        })
    }
    fn adopted(&mut self, receipt: &ConfigurationAdoptionReceipt) -> Result<(), Error> {
        let pending = self
            .state
            .pending
            .as_ref()
            .ok_or_else(|| local("receipt without retained request"))?;
        if receipt.workspace_id != self.state.workspace
            || receipt.operation_id != pending.operation_id
            || !receipt.source_stored
            || !receipt.policy_committed
        {
            return Err(Error { exit_code: 7, message: "receipt did not confirm this request's source and policy adoption; request retained".into() });
        }
        // Only the Server receipt advances the editing baseline. Never rewrite files:
        // an editor may have changed them after the original commit request escaped.
        self.state.base.state = receipt.state.clone();
        self.state.base.history_head = Some(receipt.history_head.clone());
        self.state.base.expected_legacy_export = None;
        self.state.pending = None;
        self.save().map_err(|_| Error { exit_code: 7, message: "Server receipt confirmed adoption, but local baseline save failed; preserve checkout and reconcile the retained request if present".into() })
    }
}
fn scan(root: &Path, prefix: &str, files: &mut BTreeMap<String, String>) -> Result<(), Error> {
    reject_symlink(root)?;
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| local("source names must be UTF-8"))?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        safe_relative(&path)?;
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            scan(&entry.path(), &path, files)?;
        } else if kind.is_file() {
            files.insert(path, fs::read_to_string(entry.path()).map_err(io_error)?);
        } else {
            return Err(local(
                "source accepts only regular files/directories, not symlinks or devices",
            ));
        }
    }
    Ok(())
}

fn install(
    directory: &Path,
    backend: &str,
    workspace: &str,
    base: ConfigurationSourceResponse,
) -> Result<(), Error> {
    // create_dir, not create_dir_all: a second pull cannot claim an existing tree.
    fs::create_dir(directory).map_err(|_| {
        local("pull needs a NEW directory; existing local edits/requests are never overwritten")
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    fs::create_dir(directory.join(".yoi-config")).map_err(io_error)?;
    fs::create_dir(directory.join("source")).map_err(io_error)?;
    File::create(directory.join(".yoi-config/lock")).map_err(io_error)?;
    for (path, entry) in &base.state.snapshot.entries {
        let path = directory.join("source").join(safe_relative(path)?);
        fs::create_dir_all(path.parent().unwrap()).map_err(io_error)?;
        fs::write(path, &entry.content).map_err(io_error)?;
    }
    atomic_json(&directory.join("edit.json"), &edit_for(&base))?;
    atomic_json(
        &directory.join(".yoi-config/state.json"),
        &Checkout {
            version: 1,
            backend: backend.into(),
            workspace: workspace.into(),
            base: base.into(),
            pending: None,
        },
    )
}

fn receipt_output(receipt: &ConfigurationAdoptionReceipt) -> String {
    format!(
        "adopted operation={} head={} content_root={} source_stored={} policy_committed={} runtime_application={:?}\nLocal source files unchanged; pull into a NEW directory to verify current Server source.\n",
        receipt.operation_id,
        receipt.history_head,
        receipt.state.snapshot.digest,
        receipt.source_stored,
        receipt.policy_committed,
        receipt.runtime_application
    )
}

/// Execute against the generated transport, allowing connection tests to use the
/// same CLI implementation with real Server routes and explicitly scoped credentials.
pub async fn run<A: RequestAuthorizer>(
    api: &ConfigurationAdoptionApiClient<A>,
    backend: &str,
    workspace: &str,
    options: &Options,
) -> Result<String, Error> {
    use Command::*;
    if workspace.is_empty() {
        return Err(local("an explicit Workspace ID is required"));
    }
    if options.command == Pull {
        if fs::symlink_metadata(&options.directory).is_ok() {
            return Err(local(
                "pull needs a NEW directory; existing local edits/requests are never overwritten",
            ));
        }
        let base = api
            .configuration_source(workspace.to_owned())
            .await
            .map_err(|e| remote(e, false))?;
        let output = format!(
            "pulled Workspace={} head={:?} content_root={} source_stored={}\nEdit source/ files and edit.json; validate and preview before push.\n",
            workspace, base.history_head, base.state.snapshot.digest, base.source_stored
        );
        install(&options.directory, backend, workspace, base)?;
        return Ok(output);
    }
    let mut checkout = LockedCheckout::open(&options.directory, backend, workspace)?;
    if options.command == Diff {
        let request = checkout.request()?;
        let mut output = format!(
            "expected_head={:?} content_root={}\nUnchanged paths are omitted (retained). Missing tracked files below are explicit deletions.\n",
            request.expected_head, request.source.base_digest
        );
        for change in &request.source.changes {
            match change {
                ConfigTreeChange::Create { path, content, .. } => {
                    output.push_str(&format!("create {path}\n+++\n{content}\n"))
                }
                ConfigTreeChange::Update { path, content, .. } => output.push_str(&format!(
                    "update {path}\n---\n{}\n+++\n{content}\n",
                    checkout.state.base.state.snapshot.entries[path].content
                )),
                ConfigTreeChange::Delete { path, .. } => output.push_str(&format!(
                    "delete {path}\n---\n{}\n",
                    checkout.state.base.state.snapshot.entries[path].content
                )),
                ConfigTreeChange::Rename { .. } => unreachable!(),
            }
        }
        output.push_str(&format!(
            "entrypoints before={:?} after={:?}\npending_operation={:?}\n",
            checkout.state.base.state.contract.entrypoints,
            request.source.entrypoints,
            checkout.state.pending.as_ref().map(|r| &r.operation_id)
        ));
        return Ok(output);
    }
    if matches!(options.command, Reconcile | Retry) {
        let request = checkout
            .state
            .pending
            .clone()
            .ok_or_else(|| local("no unresolved request retained"))?;
        if options.command == Reconcile {
            match api.configuration_reconcile(workspace.to_owned(), request).await.map_err(unresolved)? {
                ConfigurationReconciliation::Adopted { receipt } => { checkout.adopted(&receipt)?; return Ok(receipt_output(&receipt)); }
                ConfigurationReconciliation::NotObserved => return Err(Error { exit_code: 7, message: "NotObserved: outcome remains unknown, NOT failure; request retained. Reconcile later or explicitly retry the SAME request.".into() }),
                ConfigurationReconciliation::PendingTransfer { .. } => return Err(Error { exit_code: 7, message: "pending transfer: adoption not confirmed; retain request and explicitly retry the SAME request".into() }),
            }
        }
        // Any denial now is about this retry, not proof that the first send failed.
        let receipt = api
            .configuration_adopt(workspace.to_owned(), request)
            .await
            .map_err(unresolved)?;
        checkout.adopted(&receipt)?;
        return Ok(receipt_output(&receipt));
    }
    if checkout.state.pending.is_some() {
        return Err(Error { exit_code: 7, message: "unresolved request retained: use reconcile or exact retry; later local edits remain on disk".into() });
    }
    let request = checkout.request()?;
    let preview = api
        .configuration_preview(workspace.to_owned(), request.clone())
        .await
        .map_err(|e| remote(e, false))?;
    let summary = format!(
        "validated expected_head={:?}\nchanged_paths={:?}\nchanged_subjects={:?}\n",
        preview.expected_head, preview.changed_paths, preview.changed_subjects
    );
    if options.command != Push {
        return Ok(summary);
    }
    checkout.state.pending = Some(request.clone());
    checkout.save()?;
    match api.configuration_adopt(workspace.to_owned(), request).await {
        Ok(receipt) => {
            checkout.adopted(&receipt)?;
            Ok(format!("{summary}{}", receipt_output(&receipt)))
        }
        Err(error) => {
            let error = remote(error, true);
            // Only an authoritative first-send refusal (not a retry refusal) can
            // establish that this newly saved envelope was not adopted.
            if matches!(error.exit_code, 3..=5) {
                checkout.state.pending = None;
                checkout.save()?;
            }
            Err(error)
        }
    }
}
