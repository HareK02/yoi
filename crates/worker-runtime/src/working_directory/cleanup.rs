//! Ordinary removal authority, not a recovery lifecycle. The content witness is
//! outside the tree being removed so a crash/partial unlink cannot erase it.
//! Every retry checks current identities, mounts and remaining content. Missing
//! entries are expected after partial deletion. The same intact clean checkout
//! may renew its witness after owner recovery, never discarding new ignored content.
use super::*;
use std::io;

fn cause(kind: &str) -> WorkingDirectoryDiagnostic {
    let message = match kind {
        "mount_present" => {
            "A mounted resource remains. Its owner must safely release it before retrying removal; Runtime will not unmount external or unknown resources."
        }
        "mount_check_unavailable" => {
            "Mount-safe removal is unavailable. Restore Runtime mount-check support and retry."
        }
        "permission_denied" => {
            "Removal was denied by filesystem permissions. Ask the resource owner to resolve permissions and retry."
        }
        "resource_busy" => {
            "A filesystem resource is busy or changed during removal. Release the resource and retry."
        }
        "storage_unavailable" => {
            "Cleanup storage is unavailable. Restore storage access and retry."
        }
        "ownership_unknown" => {
            "The remaining directory cannot be matched to Runtime removal authority. Verify its identity and ownership before removal."
        }
        "changes_present" => {
            "New or changed content is present. Preserve or resolve those changes before retrying removal."
        }
        _ => {
            "The checkout is incomplete and remaining content has no usable removal witness. Preserve residual content outside this Workdir and restore the same usable checkout, or ask its owner to remove verified residual content before retrying. Do not edit removal metadata."
        }
    };
    WorkingDirectoryDiagnostic::new(format!("working_directory_cleanup_{kind}"), message)
}

pub(super) fn os_failure(id: &str, action: &str, error: io::Error) -> WorkingDirectoryDiagnostic {
    let kind = match error.raw_os_error() {
        Some(libc::EXDEV) => "mount_present",
        Some(libc::ENOSYS | libc::EOPNOTSUPP | libc::EINVAL) => "mount_check_unavailable",
        Some(libc::EACCES | libc::EPERM) => "permission_denied",
        Some(libc::EBUSY | libc::ENOTEMPTY | libc::EAGAIN) => "resource_busy",
        _ => "storage_unavailable",
    };
    tracing::warn!(target: "yoi::workdir_cleanup", working_directory_id = id,
        action, cause = kind, raw_os_error = ?error.raw_os_error(), error = %error,
        "Runtime-private working directory cleanup failure");
    cause(kind)
}

pub(super) fn remove(
    materializer: &RuntimeGitMaterializer,
    id: &str,
) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
    let result = remove_checked(materializer, id);
    if let Err(error) = &result {
        tracing::warn!(target: "yoi::workdir_cleanup", working_directory_id = id,
            action = "cleanup_working_directory", code = %error.code,
            "working directory removal blocked");
    }
    result
}

#[cfg(target_os = "linux")]
pub(super) fn remaining_status(
    materializer: &RuntimeGitMaterializer,
    id: &str,
) -> Option<WorkingDirectoryStatus> {
    let parent = fs::File::open(&materializer.runtime_root).ok()?;
    let saved = load_authority(&parent, id).ok()??;
    let tree = open_directory(&parent, std::ffi::OsStr::new(id)).ok()?;
    if saved.working_directory.id != id || identity(&tree).ok()? != saved.root_identity {
        return None;
    }
    let mut summary = saved.working_directory.status_summary();
    summary.status = WorkingDirectoryStatusKind::CleanupPending;
    summary.cleanliness = Some(
        match check_mounts(
            &materializer.working_directory_root(id),
            &fs::read("/proc/self/mountinfo").ok()?,
        )
        .and_then(|()| retry_inventory(materializer, id, &tree, &saved))
        {
            Ok(_) => "clean",
            Err(error) if error.code == "working_directory_cleanup_changes_present" => "dirty",
            Err(_) => "unknown",
        }
        .to_string(),
    );
    Some(WorkingDirectoryStatus { summary })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn remaining_status(
    _materializer: &RuntimeGitMaterializer,
    _id: &str,
) -> Option<WorkingDirectoryStatus> {
    None
}

#[cfg(not(target_os = "linux"))]
fn remove_checked(
    _materializer: &RuntimeGitMaterializer,
    _id: &str,
) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
    // No fallback to remove_dir_all: mount safety is a precondition.
    Err(cause("mount_check_unavailable"))
}

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
fn remove_checked(
    materializer: &RuntimeGitMaterializer,
    id: &str,
) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
    remove_with_eraser(materializer, id, erase)
}

#[cfg(target_os = "linux")]
fn remove_with_eraser(
    materializer: &RuntimeGitMaterializer,
    id: &str,
    erase_tree: impl FnOnce(&fs::File, &Path, &BTreeMap<PathBuf, Entry>) -> Result<(), EraseError>,
) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
    remove_with_checks(
        materializer,
        id,
        || fs::read("/proc/self/mountinfo"),
        erase_tree,
    )
}

#[cfg(target_os = "linux")]
fn remove_with_checks(
    materializer: &RuntimeGitMaterializer,
    id: &str,
    mount_info: impl FnOnce() -> io::Result<Vec<u8>>,
    erase_tree: impl FnOnce(&fs::File, &Path, &BTreeMap<PathBuf, Entry>) -> Result<(), EraseError>,
) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
    let parent = match fs::File::open(&materializer.runtime_root) {
        Ok(parent) => parent,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_not_found",
                "Runtime working directory was not found",
            ));
        }
        Err(error) => return Err(os_failure(id, "open_runtime_root", error)),
    };
    let tree = match open_directory(&parent, std::ffi::OsStr::new(id)) {
        Ok(tree) => tree,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_not_found",
                "Runtime working directory was not found",
            ));
        }
        Err(error) if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
            return Err(cause("ownership_unknown"));
        }
        Err(error) => return Err(os_failure(id, "open_cleanup_root", error)),
    };
    let root_identity = identity(&tree).map_err(|e| os_failure(id, "identify_root", e))?;
    let saved = load_authority(&parent, id).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidData {
            cause("ownership_unknown")
        } else {
            os_failure(id, "read_removal_authority", error)
        }
    })?;
    // Mount table preflight catches mounted files and same-device bind mounts
    // before Git/status/hash reads; descriptor NO_XDEV remains the race guard.
    check_mounts(
        &materializer.working_directory_root(id),
        &mount_info().map_err(|e| {
            let _ = os_failure(id, "read_mountinfo", e);
            cause("mount_check_unavailable")
        })?,
    )?;
    let authority = if let Some(saved) = saved {
        if saved.working_directory.id != id || saved.root_identity != root_identity {
            return Err(cause("ownership_unknown"));
        }
        let entries = retry_inventory(materializer, id, &tree, &saved)?;
        if verify_remaining(&saved.entries, &entries).is_ok() {
            saved
        } else {
            let authority = Authority { entries, ..saved };
            // Renew obsolete content evidence only from a current clean, intact
            // checkout. Partial deletion without Git retains the old witness.
            persist_authority(&parent, id, &authority)
                .map_err(|e| os_failure(id, "refresh_removal_authority", e))?;
            authority
        }
    } else {
        let mut binding = materializer
            .read_binding(id)
            .map_err(|_| cause("ownership_unknown"))?;
        validate_binding(materializer, id, &binding)?;
        // Preflight the entire tree without following links or crossing mounts,
        // before asking Git to inspect the checkout.
        let before = inventory(&tree).map_err(|e| os_failure(id, "inspect_tree", e))?;
        // Validated metadata-only residue contains no user content to discard.
        if !metadata_only(&before) {
            check_clean_checkout(id, &binding, &before, None)?;
        }
        if inventory(&tree).map_err(|e| os_failure(id, "recheck_inspected_tree", e))? != before {
            return Err(cause("changes_present"));
        }
        binding.working_directory.status = WorkingDirectoryStatusKind::CleanupPending;
        let record = WorkingDirectoryMaterializationRecord {
            working_directory: binding.working_directory.clone(),
            root: binding.root.clone(),
        };
        let raw = serde_json::to_vec_pretty(&record).map_err(|_| cause("storage_unavailable"))?;
        rewrite_record(&tree, &raw, before.get(Path::new(MATERIALIZATION_RECORD)))
            .map_err(|e| os_failure(id, "write_cleanup_pending", e))?;
        let entries = inventory(&tree).map_err(|e| os_failure(id, "capture_removal_content", e))?;
        // Only our status-record write may differ between the clean observation
        // and durable witness. Never authorize a newly arrived user change.
        let mut without_record = entries.clone();
        without_record.remove(Path::new(MATERIALIZATION_RECORD));
        let mut before_without_record = before;
        before_without_record.remove(Path::new(MATERIALIZATION_RECORD));
        if without_record != before_without_record {
            return Err(cause("changes_present"));
        }
        let authority = Authority {
            working_directory: binding.working_directory.clone(),
            root_identity,
            entries,
        };
        persist_authority(&parent, id, &authority)
            .map_err(|e| os_failure(id, "persist_removal_authority", e))?;
        authority
    };
    let current = inventory(&tree).map_err(|e| os_failure(id, "recheck_remaining_content", e))?;
    verify_remaining(&authority.entries, &current)?;
    // Repeat the content/identity comparison at each unlink. No unverified
    // recursion into a replaced entry, even if a mount arrives after preflight.
    erase_tree(&tree, Path::new(""), &authority.entries).map_err(|e| match e {
        EraseError::Io(e) => os_failure(id, "unlink_remaining_tree", e),
        EraseError::Changed => cause("changes_present"),
    })?;
    let current_root = open_directory(&parent, std::ffi::OsStr::new(id))
        .map_err(|e| os_failure(id, "recheck_root_identity", e))?;
    if identity(&current_root).map_err(|e| os_failure(id, "recheck_root_identity", e))?
        != authority.root_identity
    {
        return Err(cause("ownership_unknown"));
    }
    unlink(&parent, std::ffi::OsStr::new(id), true)
        .map_err(|e| os_failure(id, "unlink_cleanup_root", e))?;
    parent
        .sync_all()
        .map_err(|e| os_failure(id, "sync_physical_removal", e))?;
    // The authority is obsolete only after physical removal. Its removal cannot
    // convert an already completed physical cleanup to an unknown outcome.
    if let Err(error) = discard_authority(&parent, id) {
        tracing::warn!(target: "yoi::workdir_cleanup", working_directory_id = id,
            action = "remove_obsolete_authority", raw_os_error = ?error.raw_os_error(), error = %error);
    }
    let _ = materializer.repository_access.remove_pending(id);
    let mut summary = authority.working_directory.status_summary();
    summary.status = WorkingDirectoryStatusKind::NotFound;
    summary.cleanliness = Some("unknown".to_string());
    Ok(WorkingDirectoryStatus { summary })
}

#[cfg(target_os = "linux")]
fn metadata_only(entries: &BTreeMap<PathBuf, Entry>) -> bool {
    entries.iter().all(|(path, entry)| {
        path == Path::new(MATERIALIZATION_RECORD)
            || (path == Path::new(CHECKOUT_DIR) && entry.mode & libc::S_IFMT == libc::S_IFDIR)
    })
}

#[cfg(target_os = "linux")]
fn same_materialization(saved: &WorkingDirectory, current: &WorkingDirectory) -> bool {
    let immutable = |directory: &WorkingDirectory| {
        let mut directory = directory.clone();
        // These fields are refreshed by authorize_repository_access, not a new
        // materialization. All repository, creation and cleanup identity stays.
        directory.evidence.operation_id = None;
        directory.evidence.credential_revision = None;
        directory.evidence.host_trust_revision = None;
        directory
    };
    immutable(saved) == immutable(current)
}

#[cfg(target_os = "linux")]
fn retry_inventory(
    materializer: &RuntimeGitMaterializer,
    id: &str,
    tree: &fs::File,
    saved: &Authority,
) -> Result<BTreeMap<PathBuf, Entry>, WorkingDirectoryDiagnostic> {
    let before = inventory(tree).map_err(|e| os_failure(id, "inspect_retry_tree", e))?;
    if verify_remaining(&saved.entries, &before).is_ok() {
        // With no Git left, only the old witness authorizes the surviving subset.
        return Ok(before);
    }
    let binding = materializer
        .read_binding(id)
        .map_err(|_| cause("changes_present"))?;
    validate_binding(materializer, id, &binding)?;
    if !same_materialization(&saved.working_directory, &binding.working_directory)
        || before
            .get(Path::new(MATERIALIZATION_RECORD))
            .is_none_or(|entry| entry.mode & libc::S_IFMT != libc::S_IFREG)
    {
        return Err(cause("ownership_unknown"));
    }
    check_clean_checkout(id, &binding, &before, Some(saved))?;
    let parent = fs::File::open(&materializer.runtime_root)
        .map_err(|e| os_failure(id, "reopen_runtime_root", e))?;
    let live = open_directory(&parent, std::ffi::OsStr::new(id))
        .map_err(|e| os_failure(id, "recheck_retry_identity", e))?;
    if identity(&live).map_err(|e| os_failure(id, "recheck_retry_identity", e))?
        != saved.root_identity
    {
        return Err(cause("ownership_unknown"));
    }
    if inventory(tree).map_err(|e| os_failure(id, "recheck_retry_content", e))? != before {
        return Err(cause("changes_present"));
    }
    Ok(before)
}

#[cfg(target_os = "linux")]
fn check_clean_checkout(
    id: &str,
    binding: &WorkingDirectoryBinding,
    before: &BTreeMap<PathBuf, Entry>,
    saved: Option<&Authority>,
) -> Result<(), WorkingDirectoryDiagnostic> {
    for required in [Path::new(CHECKOUT_DIR), Path::new("checkout/.git")] {
        let Some(entry) = before
            .get(required)
            .filter(|entry| entry.mode & libc::S_IFMT == libc::S_IFDIR)
        else {
            return Err(cause("changes_unknown"));
        };
        if let Some(saved) = saved {
            if saved.entries.get(required) != Some(entry) {
                return Err(cause("ownership_unknown"));
            }
        }
    }
    let run = |args: &[&str]| -> Result<Vec<u8>, WorkingDirectoryDiagnostic> {
        let output = isolated_git_command()
            .env("GIT_OPTIONAL_LOCKS", "0")
            // Do not run repository-configured fsmonitor hooks while Runtime
            // occupancy is held, or trust their cached cleanliness observation.
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
            ])
            .arg("-C")
            .arg(binding.root())
            .args(args)
            .output()
            .map_err(|e| os_failure(id, "inspect_changes", e))?;
        if !output.status.success() {
            return Err(cause("changes_unknown"));
        }
        Ok(output.stdout)
    };
    // An empty/unborn index is not a usable committed checkout.
    run(&["rev-parse", "--verify", "HEAD^{tree}"])?;
    if !run(&["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        return Err(cause("changes_present"));
    }
    if let Some(saved) = saved {
        use std::os::unix::ffi::OsStrExt;
        let tracked = run(&["ls-files", "--cached", "-z"])?;
        let mut git_paths = std::collections::BTreeSet::new();
        for raw in tracked.split(|b| *b == 0).filter(|raw| !raw.is_empty()) {
            let path = Path::new(CHECKOUT_DIR).join(std::ffi::OsStr::from_bytes(raw));
            for path in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
                git_paths.insert(path.to_path_buf());
            }
        }
        // Git clean is not ignored-content permission. Current tracked content
        // and Git metadata may change; all other entries stay witness-protected.
        for (path, entry) in before {
            if path != Path::new(MATERIALIZATION_RECORD)
                && !path.starts_with("checkout/.git")
                && !git_paths.contains(path)
                && saved.entries.get(path) != Some(entry)
            {
                return Err(cause("changes_present"));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_recovery_binding(
    materializer: &RuntimeGitMaterializer,
    id: &str,
    binding: &WorkingDirectoryBinding,
) -> Result<(), WorkingDirectoryDiagnostic> {
    validate_binding(materializer, id, binding)?;
    #[cfg(target_os = "linux")]
    {
        let parent = fs::File::open(&materializer.runtime_root)
            .map_err(|e| os_failure(id, "open_bind_root", e))?;
        let tree = open_directory(&parent, std::ffi::OsStr::new(id))
            .map_err(|_| cause("ownership_unknown"))?;
        let checkout = open_directory(&tree, std::ffi::OsStr::new(CHECKOUT_DIR))
            .map_err(|_| cause("changes_unknown"))?;
        let git = open_directory(&checkout, std::ffi::OsStr::new(".git"))
            .map_err(|_| cause("changes_unknown"))?;
        if inspect(&tree, std::ffi::OsStr::new(MATERIALIZATION_RECORD))
            .map_err(|_| cause("ownership_unknown"))?
            .mode
            & libc::S_IFMT
            != libc::S_IFREG
        {
            return Err(cause("ownership_unknown"));
        }
        if let Some(saved) = load_authority(&parent, id).map_err(|_| cause("ownership_unknown"))? {
            if saved.working_directory.id != id
                || !same_materialization(&saved.working_directory, &binding.working_directory)
                || identity(&tree).ok() != Some(saved.root_identity)
                || saved
                    .entries
                    .get(Path::new(CHECKOUT_DIR))
                    .map(|entry| entry.identity)
                    != identity(&checkout).ok()
                || saved
                    .entries
                    .get(Path::new("checkout/.git"))
                    .map(|entry| entry.identity)
                    != identity(&git).ok()
            {
                return Err(cause("ownership_unknown"));
            }
        }
    }
    // Dirty is intentional here: ordinary attachment lets the owner preserve
    // changes/release resources. Cleanup still separately rechecks occupancy.
    if !binding_paths_are_available(binding)
        || git_stdout(binding.root(), ["rev-parse", "--verify", "HEAD^{tree}"]).is_err()
    {
        return Err(cause("changes_unknown"));
    }
    Ok(())
}

pub(super) fn validate_binding(
    materializer: &RuntimeGitMaterializer,
    id: &str,
    binding: &WorkingDirectoryBinding,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let directory = &binding.working_directory;
    if directory.id != id
        || directory.materializer_kind != MaterializerKind::RuntimeGitClone
        || directory.evidence.materializer_kind != MaterializerKind::RuntimeGitClone
        || directory.repository_id != directory.evidence.repository_id
        || directory.cleanup_target.working_directory_id != id
        || directory.cleanup_target.repository_id != directory.repository_id
        || directory.cleanup_target.kind != "runtime_git_clone"
        || binding.root != materializer.working_directory_root(id).join(CHECKOUT_DIR)
    {
        return Err(cause("ownership_unknown"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Serialize, Deserialize)]
struct Authority {
    working_directory: WorkingDirectory,
    root_identity: (u64, u64),
    entries: BTreeMap<PathBuf, Entry>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    identity: (u64, u64),
    mode: u32,
    digest: Option<String>,
}

#[cfg(target_os = "linux")]
fn verify_remaining(
    expected: &BTreeMap<PathBuf, Entry>,
    current: &BTreeMap<PathBuf, Entry>,
) -> Result<(), WorkingDirectoryDiagnostic> {
    if current
        .iter()
        .any(|(name, entry)| expected.get(name) != Some(entry))
    {
        return Err(cause("changes_present"));
    }
    Ok(())
}

// mountinfo escapes spaces, tabs, newlines and backslashes as octal. Parse bytes
// instead of lossy Unicode; malformed input is not permission to recurse.
#[cfg(target_os = "linux")]
fn check_mounts(root: &Path, raw: &[u8]) -> Result<(), WorkingDirectoryDiagnostic> {
    use std::os::unix::ffi::OsStringExt;
    let root = root
        .canonicalize()
        .map_err(|_| cause("ownership_unknown"))?;
    if raw.is_empty() {
        return Err(cause("mount_check_unavailable"));
    }
    for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let fields: Vec<_> = line.split(|b| *b == b' ').collect();
        if fields.len() < 10 || !fields.contains(&b"-".as_slice()) {
            return Err(cause("mount_check_unavailable"));
        }
        let mut path = Vec::new();
        let mut i = 0;
        while i < fields[4].len() {
            if fields[4][i] == b'\\' {
                let digits = fields[4]
                    .get(i + 1..i + 4)
                    .ok_or_else(|| cause("mount_check_unavailable"))?;
                if !digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
                    return Err(cause("mount_check_unavailable"));
                }
                let value = (digits[0] - b'0') as u16 * 64
                    + (digits[1] - b'0') as u16 * 8
                    + (digits[2] - b'0') as u16;
                path.push(u8::try_from(value).map_err(|_| cause("mount_check_unavailable"))?);
                i += 4;
            } else {
                path.push(fields[4][i]);
                i += 1;
            }
        }
        let mount = PathBuf::from(std::ffi::OsString::from_vec(path));
        if !mount.is_absolute() {
            return Err(cause("mount_check_unavailable"));
        }
        if mount.starts_with(&root) {
            return Err(cause("mount_present"));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
const MAX_AUTHORITY_BYTES: usize = 128 * 1024 * 1024;

#[cfg(target_os = "linux")]
fn bounded_authority_bytes(authority: &Authority, limit: usize) -> io::Result<Vec<u8>> {
    let raw =
        serde_json::to_vec(authority).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    if raw.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "removal content authority exceeds its readable size limit",
        ));
    }
    Ok(raw)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    fn open(parent: &fs::File, name: &OsStr, flags: i32) -> io::Result<fs::File> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        // NO_XDEV catches bind mounts too (st_dev comparisons do not).
        // NO_SYMLINKS + O_PATH|O_NOFOLLOW allows reading a final symlink
        // descriptor, but never following it to another directory.
        let how = OpenHow {
            // openat2 rejects O_NONBLOCK with O_PATH (unlike legacy open).
            // Other opens remain nonblocking so raced-in FIFOs cannot hang.
            flags: (flags
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | if flags & libc::O_PATH == 0 {
                    libc::O_NONBLOCK
                } else {
                    0
                }) as u64,
            mode: if flags & libc::O_CREAT != 0 { 0o600 } else { 0 },
            resolve: 0x01 | 0x04 | 0x08,
        }; // NO_XDEV | NO_SYMLINKS | BENEATH
        // SAFETY: stable NUL-terminated name, valid descriptor and OpenHow ABI.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                parent.as_raw_fd(),
                name.as_ptr(),
                &how,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: syscall returned a newly owned descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd as i32) })
    }

    pub(super) fn open_directory(parent: &fs::File, name: &OsStr) -> io::Result<fs::File> {
        open(parent, name, libc::O_RDONLY | libc::O_DIRECTORY)
    }

    fn authority_directory(parent: &fs::File, create: bool) -> io::Result<fs::File> {
        if create {
            // SAFETY: valid parent descriptor and fixed NUL-terminated name.
            let result =
                unsafe { libc::mkdirat(parent.as_raw_fd(), c".cleanup-authority".as_ptr(), 0o700) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let directory = open_directory(parent, OsStr::new(".cleanup-authority"))?;
        let metadata = directory.metadata()?;
        // SAFETY: geteuid has no arguments or memory access preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        Ok(directory)
    }

    pub(super) fn load_authority(parent: &fs::File, id: &str) -> io::Result<Option<Authority>> {
        let read = || -> io::Result<Authority> {
            let directory = authority_directory(parent, false)?;
            let file = open(
                &directory,
                OsStr::new(&format!("{id}.json")),
                libc::O_RDONLY,
            )?;
            if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_AUTHORITY_BYTES as u64 {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            let mut raw = Vec::new();
            file.take(MAX_AUTHORITY_BYTES as u64 + 1)
                .read_to_end(&mut raw)?;
            if raw.len() > MAX_AUTHORITY_BYTES {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            serde_json::from_slice(&raw).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
        };
        match read() {
            Ok(authority) => Ok(Some(authority)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn persist_authority(
        parent: &fs::File,
        id: &str,
        authority: &Authority,
    ) -> io::Result<()> {
        // Never publish a witness that the next normal retry cannot read.
        let raw = bounded_authority_bytes(authority, MAX_AUTHORITY_BYTES)?;
        let directory = authority_directory(parent, true)?;
        let temporary = CString::new(format!(".{id}-{}.tmp", uuid::Uuid::now_v7())).unwrap();
        let target = CString::new(format!("{id}.json")).unwrap();
        let mut file = open(
            &directory,
            OsStr::from_bytes(temporary.as_bytes()),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        file.write_all(&raw)?;
        file.sync_all()?;
        // SAFETY: both names are valid C strings relative to the same owned fd.
        let result = unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temporary.as_ptr(),
                directory.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        directory.sync_all()?;
        // Persist the newly created authority directory before any deletion.
        parent.sync_all()
    }

    pub(super) fn discard_authority(parent: &fs::File, id: &str) -> io::Result<()> {
        let directory = authority_directory(parent, false)?;
        unlink(&directory, OsStr::new(&format!("{id}.json")), false)?;
        directory.sync_all()
    }

    pub(super) fn identity(file: &fs::File) -> io::Result<(u64, u64)> {
        let metadata = file.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    fn names(dir: &fs::File) -> io::Result<Vec<std::ffi::OsString>> {
        // /proc gives a descriptor-anchored directory listing; all access to
        // listed entries still goes through mount-constrained openat2.
        let mut names = fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        names.sort();
        Ok(names)
    }

    pub(super) fn inspect(dir: &fs::File, name: &OsStr) -> io::Result<Entry> {
        let file = open(dir, name, libc::O_PATH)?;
        let metadata = file.metadata()?;
        let digest = if metadata.is_file() {
            let mut reader = open(dir, name, libc::O_RDONLY)?;
            if identity(&reader)? != (metadata.dev(), metadata.ino()) {
                return Err(io::Error::from_raw_os_error(libc::EAGAIN));
            }
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            Some(
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            )
        } else if metadata.file_type().is_symlink() {
            let mut buffer = vec![0u8; 65536];
            // SAFETY: valid O_PATH symlink descriptor; empty path reads that
            // symlink, not its target. The mutable buffer has the stated length.
            let count = unsafe {
                libc::readlinkat(
                    file.as_raw_fd(),
                    c"".as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            buffer.truncate(count as usize);
            Some(
                Sha256::digest(&buffer)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            )
        } else if metadata.is_dir() {
            None
        } else {
            return Err(io::Error::from_raw_os_error(libc::EBUSY));
        };
        Ok(Entry {
            identity: (metadata.dev(), metadata.ino()),
            // Permission repair is not a content change. Preserve the file type
            // and executable bits, which Git treats as user-visible changes.
            mode: metadata.mode() & libc::S_IFMT
                | if metadata.is_file() {
                    metadata.mode() & 0o111
                } else {
                    0
                },
            digest,
        })
    }

    pub(super) fn inventory(dir: &fs::File) -> io::Result<BTreeMap<PathBuf, Entry>> {
        fn walk(
            dir: &fs::File,
            path: &Path,
            entries: &mut BTreeMap<PathBuf, Entry>,
        ) -> io::Result<()> {
            if path.components().count() > 256 || entries.len() > 1_000_000 {
                return Err(io::Error::from_raw_os_error(libc::EFBIG));
            }
            for name in names(dir)? {
                let entry = inspect(dir, &name)?;
                let is_dir = entry.mode & libc::S_IFMT == libc::S_IFDIR;
                let path = path.join(&name);
                entries.insert(path.clone(), entry);
                if is_dir {
                    walk(&open_directory(dir, &name)?, &path, entries)?;
                }
            }
            Ok(())
        }
        let mut entries = BTreeMap::new();
        walk(dir, Path::new(""), &mut entries)?;
        Ok(entries)
    }

    pub(super) fn rewrite_record(
        dir: &fs::File,
        bytes: &[u8],
        expected: Option<&Entry>,
    ) -> io::Result<()> {
        let expected = expected.ok_or_else(|| io::Error::from_raw_os_error(libc::EAGAIN))?;
        if expected.mode & libc::S_IFMT != libc::S_IFREG {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }
        let mut file = open(dir, OsStr::new(MATERIALIZATION_RECORD), libc::O_WRONLY)?;
        if identity(&file)? != expected.identity {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }
        file.set_len(0)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    pub(super) fn unlink(dir: &fs::File, name: &OsStr, directory: bool) -> io::Result<()> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        // SAFETY: valid descriptor and NUL-terminated single entry name.
        let result = unsafe {
            libc::unlinkat(
                dir.as_raw_fd(),
                name.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    // Read-only kernel proof: use existing mounts, never mount/unmount anything.
    // Linux /proc and openat2 are production prerequisites. Also exercise every
    // accessible same-device mount when present (e.g. a read-only store bind).
    #[test]
    fn kernel_no_xdev_rejects_existing_cross_device_and_same_device_mounts() {
        let root = fs::File::open("/").unwrap();
        let proc_error = open_directory(&root, OsStr::new("proc")).unwrap_err();
        assert_eq!(proc_error.raw_os_error(), Some(libc::EXDEV));
        let root_device = root.metadata().unwrap().dev();
        let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap();
        let mut same_device = 0;
        for line in mounts.lines() {
            let mount = line.split(' ').nth(4).unwrap();
            if mount == "/" || mount.contains('\\') {
                continue;
            }
            let Ok(metadata) = fs::metadata(mount) else {
                continue;
            };
            if metadata.is_dir() && metadata.dev() == root_device {
                let error =
                    open_directory(&root, OsStr::new(mount.trim_start_matches('/'))).unwrap_err();
                assert_eq!(error.raw_os_error(), Some(libc::EXDEV), "{mount}: {error}");
                same_device += 1;
            }
        }
        eprintln!(
            "NO_XDEV kernel: /proc rejected; {same_device} existing same-device mounts rejected"
        );
    }

    pub(super) enum EraseError {
        Io(io::Error),
        Changed,
    }
    impl From<io::Error> for EraseError {
        fn from(error: io::Error) -> Self {
            Self::Io(error)
        }
    }

    pub(super) fn erase(
        dir: &fs::File,
        path: &Path,
        expected: &BTreeMap<PathBuf, Entry>,
    ) -> Result<(), EraseError> {
        let mut children = names(dir)?;
        // Keep identity evidence until the rest of the tree has been erased.
        if path.as_os_str().is_empty() {
            children.sort_by_key(|name| name == MATERIALIZATION_RECORD);
        }
        for name in children {
            let child = path.join(&name);
            let entry = inspect(dir, &name)?;
            if expected.get(&child) != Some(&entry) {
                return Err(EraseError::Changed);
            }
            let is_dir = entry.mode & libc::S_IFMT == libc::S_IFDIR;
            if is_dir {
                let directory = open_directory(dir, &name)?;
                if identity(&directory)? != entry.identity {
                    return Err(EraseError::Changed);
                }
                erase(&directory, &child, expected)?;
                if inspect(dir, &name)? != entry {
                    return Err(EraseError::Changed);
                }
            }
            unlink(dir, &name, is_dir)?;
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(super) mod tests {
    use super::super::tests::{create_clean_repo, request};
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        RuntimeGitMaterializer,
        WorkingDirectoryBinding,
    ) {
        let repo = create_clean_repo();
        let runtime = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitMaterializer::new(runtime.path());
        let binding = materializer.create(&request(repo.path())).unwrap();
        (runtime, materializer, binding)
    }

    pub(crate) fn fail_once(materializer: &RuntimeGitMaterializer, id: &str) {
        let error = remove_with_eraser(materializer, id, |_, _, _| {
            Err(EraseError::Io(io::Error::from_raw_os_error(libc::EACCES)))
        })
        .unwrap_err();
        assert_eq!(error.code, "working_directory_cleanup_permission_denied");
    }

    #[test]
    fn persisted_removal_witness_must_fit_the_same_readable_limit() {
        let (runtime, _, binding) = fixture();
        let authority = Authority {
            working_directory: binding.working_directory.clone(),
            root_identity: (0, 0),
            entries: BTreeMap::new(),
        };
        let bytes = bounded_authority_bytes(&authority, MAX_AUTHORITY_BYTES).unwrap();
        assert_eq!(
            bounded_authority_bytes(&authority, bytes.len()).unwrap(),
            bytes
        );
        assert_eq!(
            bounded_authority_bytes(&authority, bytes.len() - 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!runtime.path().join(".cleanup-authority").exists());
    }

    #[test]
    fn cleanup_pending_rechecks_current_cleanliness_and_retries_without_state_edit() {
        let (_runtime, materializer, binding) = fixture();
        let id = &binding.working_directory.id;
        fail_once(&materializer, id);
        let status = materializer.working_directory_status(id).unwrap();
        assert_eq!(
            status.summary.status,
            WorkingDirectoryStatusKind::CleanupPending
        );
        assert_eq!(status.summary.cleanliness.as_deref(), Some("clean"));
        let removed = materializer.cleanup_working_directory(id).unwrap();
        assert_eq!(removed.summary.status, WorkingDirectoryStatusKind::NotFound);
        assert!(!binding.working_directory_root.exists());
    }

    #[test]
    fn partial_deletion_missing_checkout_and_restart_finish_with_durable_content_authority() {
        for removed in [
            "checkout/README.md",
            "checkout/.git",
            CHECKOUT_DIR,
            MATERIALIZATION_RECORD,
        ] {
            let (runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            let error = remove_with_eraser(&materializer, id, |_, _, _| {
                let path = binding.working_directory_root.join(removed);
                if path.is_dir() {
                    fs::remove_dir_all(path).unwrap();
                } else {
                    fs::remove_file(path).unwrap();
                }
                Err(EraseError::Io(io::Error::from_raw_os_error(libc::EBUSY)))
            })
            .unwrap_err();
            assert_eq!(error.code, "working_directory_cleanup_resource_busy");
            drop(materializer);
            let restarted = RuntimeGitMaterializer::new(runtime.path());
            let retained = restarted.working_directory_status(id).unwrap();
            assert_eq!(
                retained.summary.repository_id,
                binding.working_directory.repository_id
            );
            assert_eq!(
                retained.summary.cleanup_target.as_ref(),
                Some(&binding.working_directory.cleanup_target)
            );
            assert_eq!(retained.summary.cleanliness.as_deref(), Some("clean"));
            assert_eq!(
                restarted.list_working_directories().unwrap()[0].summary,
                retained.summary
            );
            let removed_status = restarted.cleanup_working_directory(id).unwrap();
            assert_eq!(
                removed_status.summary.status,
                WorkingDirectoryStatusKind::NotFound,
                "{removed}"
            );
            assert!(!binding.working_directory_root.exists());
        }
    }

    #[test]
    fn missing_checkout_with_only_validated_metadata_can_be_removed_without_witness() {
        let (_runtime, materializer, binding) = fixture();
        fs::remove_dir_all(binding.root()).unwrap();
        let removed = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap();
        assert_eq!(removed.summary.status, WorkingDirectoryStatusKind::NotFound);
        assert!(!binding.working_directory_root.exists());
    }

    #[test]
    fn incomplete_checkout_without_witness_preserves_residue_and_explains_recovery() {
        let (_runtime, materializer, binding) = fixture();
        fs::remove_dir_all(binding.root.join(".git")).unwrap();
        let error = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap_err();
        assert_eq!(error.code, "working_directory_cleanup_changes_unknown");
        assert!(error.message.contains("Preserve residual content"));
        assert!(binding.root.join("README.md").exists());
    }

    #[test]
    fn failure_then_dirty_then_owner_resolution_renews_witness_on_ordinary_retry() {
        use super::super::tests::git;
        for resolution in ["commit", "restore"] {
            let (runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            fail_once(&materializer, id);
            fs::write(binding.root.join("README.md"), "preserved change").unwrap();
            assert_eq!(
                materializer
                    .working_directory_status(id)
                    .unwrap()
                    .summary
                    .cleanliness
                    .as_deref(),
                Some("dirty")
            );
            assert_eq!(
                materializer.cleanup_working_directory(id).unwrap_err().code,
                "working_directory_cleanup_changes_present"
            );
            let recovery = materializer.bind_working_directory(id, None).unwrap();
            assert_eq!(
                recovery.working_directory.status,
                WorkingDirectoryStatusKind::CleanupPending
            );
            if resolution == "commit" {
                git(binding.root(), &["add", "README.md"]);
                git(
                    binding.root(),
                    &[
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.com",
                        "commit",
                        "-m",
                        "Preserve recovery changes",
                    ],
                );
            } else {
                fs::copy(
                    binding.root.join("README.md"),
                    runtime.path().join("preserved.txt"),
                )
                .unwrap();
                // Replaces the tracked file inode and Git index legitimately.
                fs::remove_file(binding.root.join("README.md")).unwrap();
                git(binding.root(), &["restore", "README.md"]);
            }
            let restarted = RuntimeGitMaterializer::new(runtime.path());
            assert_eq!(
                restarted
                    .working_directory_status(id)
                    .unwrap()
                    .summary
                    .cleanliness
                    .as_deref(),
                Some("clean"),
                "{resolution}"
            );
            // Renewal itself can fail at unlink. Its new witness must survive a
            // later partial deletion/restart, even with no remaining Git.
            fail_once(&restarted, id);
            fs::remove_dir_all(binding.root.join(".git")).unwrap();
            let restarted = RuntimeGitMaterializer::new(runtime.path());
            restarted.cleanup_working_directory(id).unwrap();
            assert!(!binding.working_directory_root.exists());
        }
    }

    #[test]
    fn clean_git_retry_preserves_new_or_modified_ignored_and_untracked_content() {
        use super::super::tests::git;
        for change in ["ignored_new", "ignored_modified", "untracked", "sibling"] {
            let (_runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            fs::write(binding.root.join(".git/info/exclude"), "ignored/\n").unwrap();
            fs::create_dir(binding.root.join("ignored")).unwrap();
            fs::write(binding.root.join("ignored/existing"), "initial").unwrap();
            fail_once(&materializer, id);
            let file = match change {
                "ignored_new" => binding.root.join("ignored/new"),
                "ignored_modified" => binding.root.join("ignored/existing"),
                "untracked" => binding.root.join("new"),
                _ => binding.working_directory_root.join("new"),
            };
            fs::write(&file, "preserve me").unwrap();
            git(
                binding.root(),
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "Clean Git metadata changed",
                ],
            );
            assert_eq!(
                materializer.cleanup_working_directory(id).unwrap_err().code,
                "working_directory_cleanup_changes_present",
                "{change}"
            );
            assert_eq!(fs::read_to_string(&file).unwrap(), "preserve me");
            fs::remove_file(&file).unwrap();
            // Removing a pre-existing ignored file is safe; no new content remains.
            materializer.cleanup_working_directory(id).unwrap();
        }
    }

    #[test]
    fn pending_witness_accepts_access_refresh_but_rejects_materialization_substitution() {
        for update in ["access", "repository", "creation"] {
            let (_runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            fail_once(&materializer, id);
            let mut record = materializer.read_binding(id).unwrap();
            match update {
                "access" => {
                    // The same serialized fields changed by authorize_repository_access.
                    record.working_directory.evidence.operation_id =
                        Some("reattach-operation".into());
                    record.working_directory.evidence.credential_revision = Some(2);
                    record.working_directory.evidence.host_trust_revision = Some(2);
                }
                "repository" => {
                    record.working_directory.repository_id = "substitute".into();
                    record.working_directory.evidence.repository_id = "substitute".into();
                    record.working_directory.cleanup_target.repository_id = "substitute".into();
                }
                _ => record.working_directory.evidence.resolved_commit = "substitute".into(),
            }
            materializer.write_record(&record).unwrap();
            if update == "access" {
                materializer
                    .preflight_bind_working_directory(id, None)
                    .unwrap();
                // Read-only preflight does not bypass actual SSH delivery requirements.
                assert_eq!(
                    materializer
                        .bind_working_directory(id, None)
                        .unwrap_err()
                        .code,
                    "working_directory_remote_repository_access_required"
                );
                materializer.cleanup_working_directory(id).unwrap();
                assert!(!binding.working_directory_root.exists());
            } else {
                assert_eq!(
                    materializer
                        .preflight_bind_working_directory(id, None)
                        .unwrap_err()
                        .code,
                    "working_directory_cleanup_ownership_unknown"
                );
                assert_eq!(
                    materializer.cleanup_working_directory(id).unwrap_err().code,
                    "working_directory_cleanup_ownership_unknown"
                );
                assert!(binding.root.join("README.md").exists());
            }
        }
    }

    #[test]
    fn cleanup_pending_binding_rejects_missing_git_and_replaced_checkout() {
        for replacement in [false, true] {
            let (_runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            fail_once(&materializer, id);
            if replacement {
                fs::rename(
                    binding.root(),
                    binding.working_directory_root.join("old-checkout"),
                )
                .unwrap();
                fs::create_dir(binding.root()).unwrap();
                fs::create_dir(binding.root.join(".git")).unwrap();
            } else {
                fs::remove_dir_all(binding.root.join(".git")).unwrap();
            }
            assert!(materializer.bind_working_directory(id, None).is_err());
            assert!(binding.working_directory_root.exists());
        }
    }

    #[test]
    fn retry_protects_new_files_modified_content_and_replaced_directories() {
        for change in ["new", "modified", "replacement"] {
            let (runtime, materializer, binding) = fixture();
            let id = &binding.working_directory.id;
            fail_once(&materializer, id);
            match change {
                "new" => fs::write(binding.root.join("new.txt"), b"preserve me").unwrap(),
                "modified" => fs::write(binding.root.join("README.md"), b"preserve me").unwrap(),
                _ => {
                    fs::rename(
                        &binding.working_directory_root,
                        runtime.path().join("original"),
                    )
                    .unwrap();
                    fs::create_dir(&binding.working_directory_root).unwrap();
                }
            }
            let restarted = RuntimeGitMaterializer::new(runtime.path());
            let error = restarted.cleanup_working_directory(id).unwrap_err();
            assert_eq!(
                error.code,
                if change == "replacement" {
                    "working_directory_cleanup_ownership_unknown"
                } else {
                    "working_directory_cleanup_changes_present"
                },
                "{change}"
            );
            assert!(binding.working_directory_root.exists());
        }
    }

    #[test]
    fn cleanup_does_not_execute_repository_fsmonitor_hooks_under_occupancy_guard() {
        use super::super::tests::git;
        use std::os::unix::fs::PermissionsExt;
        let (runtime, materializer, binding) = fixture();
        let hook = runtime.path().join("fsmonitor");
        let marker = runtime.path().join("hook-called");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        git(
            binding.root(),
            &["config", "core.fsmonitor", hook.to_str().unwrap()],
        );
        fail_once(&materializer, &binding.working_directory.id);
        fs::write(binding.root.join("README.md"), "dirty").unwrap();
        assert_eq!(
            materializer
                .cleanup_working_directory(&binding.working_directory.id)
                .unwrap_err()
                .code,
            "working_directory_cleanup_changes_present"
        );
        assert!(!marker.exists());
    }

    #[test]
    fn dirty_initial_checkout_is_never_authorized_for_removal() {
        let (runtime, materializer, binding) = fixture();
        fs::write(binding.root.join("README.md"), "preserve me").unwrap();
        let error = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap_err();
        assert_eq!(error.code, "working_directory_cleanup_changes_present");
        assert_eq!(
            fs::read_to_string(binding.root.join("README.md")).unwrap(),
            "preserve me"
        );
        assert!(!runtime.path().join(".cleanup-authority").exists());
    }

    #[test]
    fn mounted_or_unknown_resources_are_preserved_then_normal_retry_succeeds() {
        let (_runtime, materializer, mut binding) = fixture();
        // Legacy cleanup_pending is an input, not authority to discard content.
        binding.working_directory.status = WorkingDirectoryStatusKind::CleanupPending;
        materializer.write_record(&binding).unwrap();
        let mount = binding.root.join(".yoi/mount");
        fs::create_dir_all(&mount).unwrap();
        fs::write(mount.join("external.txt"), b"external resource").unwrap();
        // .yoi is ignored, exactly as in the reported mount residue.
        fs::write(binding.root.join(".git/info/exclude"), ".yoi/\n").unwrap();
        let raw = format!(
            "1 0 8:1 / / rw - ext4 /dev/root rw\n2 1 8:1 / {} rw - ext4 /dev/root rw\n",
            mount.display()
        );
        let error = remove_with_checks(
            &materializer,
            &binding.working_directory.id,
            || Ok(raw.into_bytes()),
            erase,
        )
        .unwrap_err();
        assert_eq!(error.code, "working_directory_cleanup_mount_present");
        assert_eq!(
            fs::read(mount.join("external.txt")).unwrap(),
            b"external resource"
        );
        assert!(!error.message.contains(&mount.to_string_lossy().to_string()));
        // The mount has been safely released by its owner. There is no state edit.
        let removed = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap();
        assert_eq!(removed.summary.status, WorkingDirectoryStatusKind::NotFound);
    }

    #[test]
    fn mountinfo_unavailable_or_malformed_never_reaches_deletion() {
        let (_runtime, materializer, binding) = fixture();
        for raw in [
            Vec::new(),
            b"invalid".to_vec(),
            b"1 0 8:1 / /bad\\zzz rw - ext4 /dev/root rw\n".to_vec(),
        ] {
            let error = remove_with_checks(
                &materializer,
                &binding.working_directory.id,
                || Ok(raw),
                |_, _, _| panic!("must not unlink without a mount check"),
            )
            .unwrap_err();
            assert_eq!(
                error.code,
                "working_directory_cleanup_mount_check_unavailable"
            );
        }
        let error = remove_with_checks(
            &materializer,
            &binding.working_directory.id,
            || Err(io::Error::from_raw_os_error(libc::EACCES)),
            erase,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            "working_directory_cleanup_mount_check_unavailable"
        );
        assert!(binding.root.exists());
    }

    #[test]
    fn mountinfo_decodes_escaped_paths_and_uses_component_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work dir");
        fs::create_dir(&root).unwrap();
        let escaped = root.to_string_lossy().replace(' ', "\\040");
        let raw = format!("1 0 8:1 / {escaped}/mount rw - ext4 /dev/root rw\n");
        assert_eq!(
            check_mounts(&root, raw.as_bytes()).unwrap_err().code,
            "working_directory_cleanup_mount_present"
        );
        let sibling = format!("1 0 8:1 / {escaped}-other/mount rw - ext4 /dev/root rw\n");
        check_mounts(&root, sibling.as_bytes()).unwrap();
    }

    #[test]
    fn symlinks_are_unlinked_without_traversing_external_content() {
        let (_runtime, materializer, binding) = fixture();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("keep.txt"), b"keep").unwrap();
        fs::write(binding.root.join(".git/info/exclude"), "link\n").unwrap();
        std::os::unix::fs::symlink(external.path(), binding.root.join("link")).unwrap();
        materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap();
        assert_eq!(fs::read(external.path().join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn record_identity_mismatch_and_symlink_roots_never_grant_removal_authority() {
        let (runtime, materializer, mut binding) = fixture();
        binding
            .working_directory
            .cleanup_target
            .working_directory_id = "other".to_string();
        materializer.write_record(&binding).unwrap();
        let error = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap_err();
        assert_eq!(error.code, "working_directory_cleanup_ownership_unknown");
        fs::rename(
            &binding.working_directory_root,
            runtime.path().join("original"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            runtime.path().join("original"),
            &binding.working_directory_root,
        )
        .unwrap();
        assert!(
            materializer
                .cleanup_working_directory(&binding.working_directory.id)
                .is_err()
        );
        assert!(runtime.path().join("original/checkout/README.md").exists());
    }

    #[test]
    fn permission_repair_without_content_changes_allows_normal_retry() {
        use std::os::unix::fs::PermissionsExt;
        let (_runtime, materializer, binding) = fixture();
        fail_once(&materializer, &binding.working_directory.id);
        fs::set_permissions(
            binding.root.join("README.md"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap();
        assert!(!binding.working_directory_root.exists());
    }

    #[test]
    fn malformed_or_external_removal_authority_is_never_used_or_overwritten() {
        for external in [false, true] {
            let (runtime, materializer, binding) = fixture();
            let private = runtime.path().join(".cleanup-authority");
            let outside = tempfile::tempdir().unwrap();
            let file = outside
                .path()
                .join(format!("{}.json", binding.working_directory.id));
            fs::write(&file, "do not overwrite").unwrap();
            if external {
                std::os::unix::fs::symlink(outside.path(), &private).unwrap();
            } else {
                fs::create_dir(&private).unwrap();
                fs::write(
                    private.join(format!("{}.json", binding.working_directory.id)),
                    "not json",
                )
                .unwrap();
            }
            let error = materializer
                .cleanup_working_directory(&binding.working_directory.id)
                .unwrap_err();
            assert_eq!(
                error.code,
                if external {
                    "working_directory_cleanup_storage_unavailable"
                } else {
                    "working_directory_cleanup_ownership_unknown"
                }
            );
            assert!(binding.root.exists());
            assert_eq!(fs::read_to_string(file).unwrap(), "do not overwrite");
        }
    }

    #[test]
    fn cleanup_os_error_is_logged_privately_with_workdir_and_action_correlation() {
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = Capture(output.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let public = tracing::subscriber::with_default(subscriber, || {
            // Other parallel cleanup tests can register this callsite while no
            // subscriber is installed on their thread. Refresh its interest for
            // this thread-local capture; never install a process-global logger.
            tracing::callsite::rebuild_interest_cache();
            os_failure(
                "workdir-correlated",
                "unlink_checkout",
                io::Error::from_raw_os_error(libc::EACCES),
            )
        });
        let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains("workdir-correlated") && log.contains("unlink_checkout"),
            "{log}"
        );
        assert!(log.contains("raw_os_error=Some(13)"), "{log}");
        assert!(!public.message.contains("os error"));
    }

    #[test]
    fn authoritative_absence_is_distinct_from_os_failure_and_unknown_changes() {
        let runtime = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitMaterializer::new(runtime.path());
        assert_eq!(
            materializer
                .cleanup_working_directory("absent")
                .unwrap_err()
                .code,
            "working_directory_not_found"
        );
        for (errno, code) in [
            (libc::EACCES, "permission_denied"),
            (libc::EBUSY, "resource_busy"),
            (libc::ENOSPC, "storage_unavailable"),
            (libc::EXDEV, "mount_present"),
            (libc::ENOSYS, "mount_check_unavailable"),
        ] {
            let error = os_failure(
                "workdir-test",
                "unlink",
                io::Error::from_raw_os_error(errno),
            );
            assert_eq!(error.code, format!("working_directory_cleanup_{code}"));
            assert!(error.message.len() < 512);
            assert!(!error.message.contains("/"));
        }
    }
}
