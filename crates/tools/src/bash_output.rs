//! Bash-owned bounded output, independent of Workdir transport and Console subscribers.
//!
//! Output is ephemeral evidence, not a filesystem grant. Every lookup checks the live view;
//! children can see their own subtree and ancestors can retain evidence after child revocation.
//! Root teardown clears all bytes. A new root (including restore) has a new incarnation and
//! never reconstructs grants from history. Expiry is measured from admission, enforced on
//! lookup and by a weak reaper once commands run in Tokio. Parent and all descendants share
//! one pool: admission reserves a whole command budget, evicts the oldest terminal evidence
//! first, and refuses rather than evicting a live collector. Revoking a child denies access
//! but leaves ancestor evidence until pool eviction/expiry; root teardown clears the pool.

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::Serialize;
use workdir::{CommandStatus, CommandStream};

pub(crate) const COMMAND_BYTES: usize = 4 * 1024 * 1024;
const STORE_BYTES: usize = 32 * 1024 * 1024;
const STORE_COUNT: usize = 64;
const ORDER_COUNT: usize = 4096;
const RETENTION: Duration = Duration::from_secs(3600);

#[derive(Clone)]
pub struct BashOutputView {
    store: Arc<Mutex<Store>>,
    lineage: Vec<String>,
    active: Arc<AtomicBool>,
    authority: Arc<dyn Fn() -> bool + Send + Sync>,
}

struct Store {
    records: VecDeque<Arc<Mutex<Record>>>,
    reserved: usize,
    retention: Duration,
    reaper_started: bool,
    _shutdown: tokio::sync::watch::Sender<()>,
}

struct Record {
    id: String,
    owner: Vec<String>,
    created: Instant,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    order: Vec<OutputRange>,
    stdout_seen: u64,
    stderr_seen: u64,
    truncated: bool,
    limit: Option<RetentionLimit>,
    transport_counts: Option<(u64, u64)>,
    transport_truncation: Option<workdir::CommandOutputTruncation>,
    terminal: Option<OutputTerminal>,
    cleanup: Option<CleanupDiagnostics>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputRange {
    pub stream: CommandStream,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputTerminal {
    pub status: Option<CommandStatus>,
    pub exit_code: Option<i32>,
    pub process_failure: Option<workdir::CommandFailure>,
    pub output_collection: Option<workdir::CommandOutputCollection>,
    /// True only for confirmed EOF collection (quota is reported separately). A terminal
    /// fence or exit zero alone does not confirm the unobserved process tail.
    pub transport_complete: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionLimit {
    ByteQuota,
    OrderQuota,
}

#[derive(Clone, Debug, Serialize)]
pub struct BashOutputMetadata {
    pub id: String,
    pub stdout_saved: usize,
    pub stderr_saved: usize,
    pub stdout_seen: u64,
    pub stderr_seen: u64,
    pub stdout_delivered: u64,
    pub stderr_delivered: u64,
    pub retention_limit: Option<RetentionLimit>,
    pub transport_truncation: Option<workdir::CommandOutputTruncation>,
    pub storage_truncated: bool,
    pub terminal: OutputTerminal,
    pub cleanup: Option<CleanupDiagnostics>,
    pub text_encoding: &'static str,
    pub ordering: &'static str,
}

/// Drop cleanup observations, not assertions that cancellation stopped a process.
/// Correlate with the enclosing output UUID and this host-assigned execution generation;
/// caller identifiers, provider handles, commands and error text are deliberately excluded.
#[derive(Clone, Debug, Serialize)]
pub struct CleanupDiagnostics {
    pub execution_nonce: u64,
    pub cancel: CleanupCancel,
    pub drain: CleanupDrain,
    pub deadline_expired: bool,
    pub registration: CleanupRegistration,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CleanupCancel {
    Acknowledged,
    Failed { reason: CleanupProviderFailure },
    DeadlineExceeded,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CleanupDrain {
    TerminalConfirmed,
    Failed { reason: CleanupFailure },
    DeadlineExceeded,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CleanupFailure {
    Provider { reason: CleanupProviderFailure },
    SequenceGap,
    CursorGap,
    DeliveryBudgetExceeded,
    RetainedChunkInvalid,
    TerminalMetadataInvalid,
}

#[derive(Clone, Debug, Serialize)]
pub struct CleanupProviderFailure {
    pub failure_reason: Option<workdir::WorkdirFailureReason>,
    pub denial_reason: Option<workdir::WorkdirDenialReason>,
}

impl From<&workdir::WorkdirError> for CleanupProviderFailure {
    fn from(error: &workdir::WorkdirError) -> Self {
        Self {
            failure_reason: error.failure_reason(),
            denial_reason: error.denial_reason(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupRegistration {
    Released,
    Superseded,
}

/// A bounded reservation acquired before command start. Drop terminalizes partial evidence.
pub struct BashOutputWriter {
    record: Arc<Mutex<Record>>,
}

impl Default for BashOutputView {
    fn default() -> Self {
        Self::new()
    }
}

impl BashOutputView {
    pub fn new() -> Self {
        let (shutdown, _) = tokio::sync::watch::channel(());
        let store = Arc::new(Mutex::new(Store {
            records: VecDeque::new(),
            reserved: 0,
            retention: RETENTION,
            reaper_started: false,
            _shutdown: shutdown,
        }));
        let view = Self {
            store,
            lineage: vec![uuid::Uuid::now_v7().to_string()],
            active: Arc::new(AtomicBool::new(true)),
            authority: Arc::new(|| true),
        };
        view.start_reaper();
        view
    }

    fn start_reaper(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut store = self.store.lock().unwrap();
        if store.reaper_started {
            return;
        }
        let receiver = store._shutdown.subscribe();
        store.reaper_started = true;
        let weak = Arc::downgrade(&self.store);
        runtime.spawn(reap(weak, receiver));
    }

    pub fn byte_budget(&self) -> u64 {
        COMMAND_BYTES as u64
    }

    pub fn child(&self, authority: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        let parent = self.clone();
        let mut lineage = self.lineage.clone();
        lineage.push(uuid::Uuid::now_v7().to_string());
        Self {
            store: self.store.clone(),
            lineage,
            active: Arc::new(AtomicBool::new(true)),
            authority: Arc::new(move || parent.is_active() && authority()),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) && (self.authority)()
    }

    pub fn is_root(&self) -> bool {
        self.lineage.len() == 1
    }

    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
        if self.lineage.len() == 1 {
            let mut store = self.store.lock().unwrap();
            store.records.clear();
            store.reserved = 0;
        }
    }

    pub fn begin(&self) -> Result<BashOutputWriter, String> {
        if !self.is_active() {
            return Err("Bash output authority revoked".into());
        }
        // A host may construct the Feature before entering Tokio. Command admission
        // happens inside the runtime, so idle expiry must start here as well.
        self.start_reaper();
        let mut store = self.store.lock().unwrap();
        if !self.is_active() {
            return Err("Bash output authority revoked".into());
        }
        store.expire(Instant::now());
        while store.records.len() >= STORE_COUNT || store.reserved + COMMAND_BYTES > STORE_BYTES {
            let Some(index) = store
                .records
                .iter()
                .position(|r| r.lock().unwrap().terminal.is_some())
            else {
                return Err("Bash output capacity is occupied by live commands".into());
            };
            store.records.remove(index);
            store.reserved -= COMMAND_BYTES;
        }
        let record = Arc::new(Mutex::new(Record {
            id: uuid::Uuid::now_v7().to_string(),
            owner: self.lineage.clone(),
            created: Instant::now(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            order: Vec::new(),
            stdout_seen: 0,
            stderr_seen: 0,
            truncated: false,
            limit: None,
            transport_counts: None,
            transport_truncation: None,
            terminal: None,
            cleanup: None,
        }));
        store.records.push_back(record.clone());
        store.reserved += COMMAND_BYTES;
        Ok(BashOutputWriter { record })
    }

    fn record(&self, id: &str) -> Option<Arc<Mutex<Record>>> {
        if !self.is_active() {
            return None;
        }
        let mut store = self.store.lock().unwrap();
        if !self.is_active() {
            return None;
        }
        store.expire(Instant::now());
        store
            .records
            .iter()
            .find(|r| {
                let r = r.lock().unwrap();
                r.id == id && r.owner.starts_with(&self.lineage) && r.terminal.is_some()
            })
            .cloned()
    }

    pub fn metadata(&self, id: &str) -> Option<BashOutputMetadata> {
        let record = self.record(id)?;
        let r = record.lock().unwrap();
        Some(r.metadata())
    }

    /// Copies at most the caller's bounded range; never publishes output bodies in descriptors.
    pub fn read(
        &self,
        id: &str,
        stream: CommandStream,
        offset: usize,
        limit: usize,
    ) -> Option<Vec<u8>> {
        let record = self.record(id)?;
        let r = record.lock().unwrap();
        let bytes = match stream {
            CommandStream::Stdout => &r.stdout,
            CommandStream::Stderr => &r.stderr,
        };
        let start = offset.min(bytes.len());
        Some(bytes[start..start.saturating_add(limit.min(64 * 1024)).min(bytes.len())].to_vec())
    }

    /// Scan the bounded raw source without cloning it, decoding only the requested line window.
    /// A long single line uses the common byte reader instead of allocating an oversized text page.
    pub fn line_window(
        &self,
        id: &str,
        stream: CommandStream,
        offset: usize,
        limit: usize,
    ) -> Option<Result<(String, bool), String>> {
        let record = self.record(id)?;
        let r = record.lock().unwrap();
        let bytes = match stream {
            CommandStream::Stdout => &r.stdout,
            CommandStream::Stderr => &r.stderr,
        };
        let mut lines = bytes.split_inclusive(|b| *b == b'\n').skip(offset);
        let mut window = Vec::new();
        for line in lines.by_ref().take(limit.min(2000)) {
            if window.len() + line.len() > 64 * 1024 {
                return Some(Err(
                    "text source window exceeds 64 KiB; use the common byte reader".into(),
                ));
            }
            window.extend_from_slice(line);
        }
        let more = lines.next().is_some();
        Some(Ok((String::from_utf8_lossy(&window).into_owned(), more)))
    }

    pub fn order(&self, id: &str) -> Option<Vec<OutputRange>> {
        let record = self.record(id)?;
        let r = record.lock().unwrap();
        Some(r.order.clone())
    }
}

impl Record {
    fn metadata(&self) -> BashOutputMetadata {
        BashOutputMetadata {
            id: self.id.clone(),
            stdout_saved: self.stdout.len(),
            stderr_saved: self.stderr.len(),
            stdout_seen: self.transport_counts.map_or(self.stdout_seen, |c| c.0),
            stderr_seen: self.transport_counts.map_or(self.stderr_seen, |c| c.1),
            stdout_delivered: self.stdout_seen,
            stderr_delivered: self.stderr_seen,
            retention_limit: self.limit,
            transport_truncation: self.transport_truncation.clone(),
            storage_truncated: self.truncated || self.transport_truncation.is_some(),
            terminal: self.terminal.clone().expect("finalized record"),
            cleanup: self.cleanup.clone(),
            text_encoding: "utf8_lossy",
            ordering: "provider_read_observation",
        }
    }
}

impl Store {
    fn expire(&mut self, now: Instant) {
        let before = self.records.len();
        self.records
            .retain(|r| now.duration_since(r.lock().unwrap().created) < self.retention);
        self.reserved -= (before - self.records.len()) * COMMAND_BYTES;
    }
}

async fn reap(weak: Weak<Mutex<Store>>, mut shutdown: tokio::sync::watch::Receiver<()>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {},
            _ = shutdown.changed() => return,
        }
        let Some(store) = weak.upgrade() else {
            return;
        };
        store.lock().unwrap().expire(Instant::now());
    }
}

impl BashOutputWriter {
    pub(crate) fn id(&self) -> String {
        self.record.lock().unwrap().id.clone()
    }

    pub(crate) fn finish_cleanup(&mut self, diagnostics: CleanupDiagnostics) {
        self.record.lock().unwrap().cleanup = Some(diagnostics);
    }

    pub fn append_chunk(&mut self, chunk: &workdir::CommandByteChunk) -> Result<(), String> {
        let expected = {
            let r = self.record.lock().unwrap();
            match chunk.stream {
                CommandStream::Stdout => r.stdout_seen,
                CommandStream::Stderr => r.stderr_seen,
            }
        };
        if chunk.offset != expected || chunk.bytes.is_empty() {
            return Err("command byte transport stream gap or empty chunk".into());
        }
        self.append(chunk.stream, &chunk.bytes);
        Ok(())
    }

    pub fn append(&mut self, stream: CommandStream, bytes: &[u8]) {
        let mut r = self.record.lock().unwrap();
        let seen = match stream {
            CommandStream::Stdout => &mut r.stdout_seen,
            CommandStream::Stderr => &mut r.stderr_seen,
        };
        let start = *seen;
        *seen = seen.saturating_add(bytes.len() as u64);
        let available = COMMAND_BYTES.saturating_sub(r.stdout.len() + r.stderr.len());
        let n = if r.truncated || r.order.len() >= ORDER_COUNT {
            0
        } else {
            available.min(bytes.len())
        };
        if n < bytes.len() {
            r.truncated = true;
            if r.limit.is_none() {
                r.limit = Some(if r.order.len() >= ORDER_COUNT {
                    RetentionLimit::OrderQuota
                } else {
                    RetentionLimit::ByteQuota
                });
            }
        }
        if n > 0 {
            match stream {
                CommandStream::Stdout => r.stdout.extend_from_slice(&bytes[..n]),
                CommandStream::Stderr => r.stderr.extend_from_slice(&bytes[..n]),
            }
            if let Some(last) = r
                .order
                .last_mut()
                .filter(|last| last.stream == stream && last.end == start)
            {
                last.end += n as u64;
            } else {
                r.order.push(OutputRange {
                    stream,
                    start,
                    end: start + n as u64,
                });
            }
        }
    }

    pub fn finish_transport(
        &mut self,
        terminal: &workdir::CommandTerminal,
    ) -> Result<BashOutputMetadata, String> {
        let mut r = self.record.lock().unwrap();
        if !terminal.metadata_valid()
            || terminal.stdout_delivered_bytes != r.stdout_seen
            || terminal.stderr_delivered_bytes != r.stderr_seen
        {
            return Err("command terminal counts do not fence delivered bytes".into());
        }
        r.transport_counts = Some((terminal.stdout_bytes, terminal.stderr_bytes));
        r.transport_truncation = terminal.truncation_reason.clone();
        r.terminal = Some(OutputTerminal {
            status: Some(terminal.status),
            exit_code: terminal.exit_code,
            transport_complete: matches!(
                terminal.output_collection,
                workdir::CommandOutputCollection::Complete
            ),
            process_failure: terminal.process_failure,
            output_collection: Some(terminal.output_collection),
        });
        Ok(r.metadata())
    }

    pub fn finish(&mut self, terminal: OutputTerminal) -> BashOutputMetadata {
        let mut r = self.record.lock().unwrap();
        r.terminal = Some(terminal);
        r.metadata()
    }
}

impl Drop for BashOutputWriter {
    fn drop(&mut self) {
        let mut r = self.record.lock().unwrap();
        if r.terminal.is_none() {
            r.terminal = Some(OutputTerminal {
                status: None,
                exit_code: None,
                transport_complete: false,
                process_failure: None,
                output_collection: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(view: &BashOutputView, text: &[u8]) -> String {
        let mut writer = view.begin().unwrap();
        writer.append(CommandStream::Stdout, text);
        writer
            .finish(OutputTerminal {
                status: Some(CommandStatus::Completed),
                exit_code: Some(0),
                transport_complete: true,
                process_failure: None,
                output_collection: Some(workdir::CommandOutputCollection::Complete),
            })
            .id
    }

    #[test]
    fn revoked_child_is_denied_while_parent_keeps_evidence_and_sibling_is_denied() {
        let root = BashOutputView::new();
        let lease = Arc::new(AtomicBool::new(true));
        let child = root.child({
            let lease = lease.clone();
            Arc::new(move || lease.load(Ordering::Acquire))
        });
        let sibling = root.child(Arc::new(|| true));
        let id = finish(&child, b"child");
        assert_eq!(
            child.read(&id, CommandStream::Stdout, 0, 20).unwrap(),
            b"child"
        );
        assert!(sibling.metadata(&id).is_none());
        assert!(BashOutputView::new().metadata(&id).is_none());
        lease.store(false, Ordering::Release);
        assert!(child.metadata(&id).is_none());
        assert_eq!(
            root.read(&id, CommandStream::Stdout, 0, 20).unwrap(),
            b"child"
        );
        root.revoke();
        let restored = BashOutputView::new();
        assert!(root.metadata(&id).is_none());
        assert!(restored.metadata(&id).is_none());
        let new_id = finish(&restored, b"new incarnation");
        assert_ne!(new_id, id);
        assert!(child.metadata(&new_id).is_none());
        assert!(root.begin().is_err());
    }

    #[test]
    fn parent_and_descendants_share_admission_without_evicting_live_evidence() {
        let root = BashOutputView::new();
        let child = root.child(Arc::new(|| true));
        let sibling = root.child(Arc::new(|| true));
        let mut writers = (0..STORE_BYTES / COMMAND_BYTES)
            .map(|index| {
                if index % 2 == 0 {
                    root.begin()
                } else {
                    child.begin()
                }
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(sibling.begin().is_err());
        let mut first = writers.remove(0);
        first.append(CommandStream::Stdout, b"finished");
        let metadata = first.finish(OutputTerminal {
            status: Some(CommandStatus::Completed),
            exit_code: Some(0),
            transport_complete: true,
            process_failure: None,
            output_collection: Some(workdir::CommandOutputCollection::Complete),
        });
        assert!(root.metadata(&metadata.id).is_some());
        let _replacement = sibling.begin().unwrap();
        assert!(root.metadata(&metadata.id).is_none());
        assert!(root.begin().is_err());
        assert_eq!(writers.len(), STORE_BYTES / COMMAND_BYTES - 1);
    }

    #[test]
    fn quota_and_forced_drop_are_distinct_from_preview_omission() {
        let root = BashOutputView::new();
        let mut writer = root.begin().unwrap();
        writer.append(CommandStream::Stdout, &vec![b'x'; COMMAND_BYTES + 1]);
        writer.append(CommandStream::Stderr, b"tail");
        let id = writer.record.lock().unwrap().id.clone();
        drop(writer);
        let m = root.metadata(&id).unwrap();
        assert_eq!(m.stdout_saved, COMMAND_BYTES);
        assert_eq!(m.stdout_seen, COMMAND_BYTES as u64 + 1);
        assert_eq!(m.stderr_seen, 4);
        assert!(m.storage_truncated);
        assert!(!m.terminal.transport_complete);
        assert_eq!(
            root.read(&id, CommandStream::Stdout, 0, usize::MAX)
                .unwrap()
                .len(),
            64 * 1024
        );
    }

    #[test]
    fn expiry_and_count_eviction_never_reuse_old_references() {
        let root = BashOutputView::new();
        let id = finish(&root, b"old");
        root.store.lock().unwrap().retention = Duration::ZERO;
        assert!(root.metadata(&id).is_none());
        root.store.lock().unwrap().retention = RETENTION;
        let first = finish(&root, b"first");
        for _ in 0..STORE_COUNT {
            finish(&root, b"later");
        }
        assert!(root.metadata(&first).is_none());
        assert!(root.store.lock().unwrap().reserved <= STORE_BYTES);
    }
    #[tokio::test(start_paused = true)]
    async fn expiry_reclaims_bytes_without_another_read_and_reaper_ends_with_owner() {
        // Exercise host construction outside a runtime, followed by real async admission.
        let root = std::thread::spawn(BashOutputView::new).join().unwrap();
        finish(&root, b"expires without lookup");
        let mut closed = root.store.lock().unwrap()._shutdown.subscribe();
        tokio::task::yield_now().await;
        // The store uses monotonic wall time for publication and Tokio time only for scheduling.
        // Age the record deterministically rather than delaying this test in real time.
        root.store.lock().unwrap().records[0]
            .lock()
            .unwrap()
            .created = Instant::now() - RETENTION;
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        assert_eq!(root.store.lock().unwrap().reserved, 0);
        drop(root);
        assert!(closed.changed().await.is_err());
    }
    #[test]
    fn terminal_counts_must_fence_the_received_prefix_before_marking_evidence_complete() {
        let root = BashOutputView::new();
        let mut writer = root.begin().unwrap();
        writer.append(CommandStream::Stdout, b"abc");
        let mut terminal = serde_json::json!({"status":"completed","exit_code":0,"process_failure":null,"output_collection":{"state":"complete"},"stdout_bytes":9,"stderr_bytes":0,"stdout_delivered_bytes":4,"stderr_delivered_bytes":0,"truncation_reason":"byte_budget"});
        assert!(
            writer
                .finish_transport(&serde_json::from_value(terminal.clone()).unwrap())
                .is_err()
        );
        terminal["stdout_delivered_bytes"] = serde_json::json!(3);
        let metadata = writer
            .finish_transport(&serde_json::from_value(terminal).unwrap())
            .unwrap();
        assert_eq!(metadata.stdout_saved, 3);
        assert_eq!(metadata.stdout_delivered, 3);
        assert_eq!(metadata.stdout_seen, 9);
        assert!(metadata.storage_truncated);
        assert!(metadata.retention_limit.is_none());
        assert_eq!(
            metadata.transport_truncation,
            Some(workdir::CommandOutputTruncation::ByteBudget)
        );
    }

    #[test]
    fn incomplete_collection_with_quota_retains_both_causes_without_claiming_complete_output() {
        let root = BashOutputView::new();
        let mut writer = root.begin().unwrap();
        writer.append(CommandStream::Stdout, b"abc");
        let terminal: workdir::CommandTerminal = serde_json::from_value(serde_json::json!({
            "status":"completed", "exit_code":0, "process_failure":null,
            "output_collection":{"state":"incomplete","failure":{"stage":"stdout_drain","reason":"other","os_code":null}},
            "stdout_bytes":9, "stderr_bytes":0, "stdout_delivered_bytes":3, "stderr_delivered_bytes":0,
            "truncation_reason":"byte_budget"
        })).unwrap();
        let metadata = writer.finish_transport(&terminal).unwrap();
        assert!(!metadata.terminal.transport_complete);
        assert_eq!(metadata.terminal.exit_code, Some(0));
        assert_eq!(
            metadata.terminal.output_collection,
            Some(terminal.output_collection)
        );
        assert_eq!(
            metadata.transport_truncation,
            Some(workdir::CommandOutputTruncation::ByteBudget)
        );
        assert_eq!(metadata.stdout_seen, 9);
        assert_eq!(metadata.stdout_saved, 3);
        assert!(metadata.storage_truncated);
        assert!(metadata.retention_limit.is_none());
        assert_eq!(
            root.read(&metadata.id, CommandStream::Stdout, 0, 100)
                .unwrap(),
            b"abc"
        );
        assert_eq!(
            serde_json::to_value(root.metadata(&metadata.id).unwrap()).unwrap(),
            serde_json::to_value(metadata).unwrap()
        );
    }
}
