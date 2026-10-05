use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use protocol::{
    AlertLevel, AlertSource, CompletionEntry, CompletionKind, ErrorCode, Event, InFlightBlock,
    InFlightSnapshot, InFlightToolCallState, InternalWorkerRef, InternalWorkerSnapshot, Method,
    RewindTarget, RunResult, Segment, WorkerCommandEnvelope, WorkerStateSnapshot, WorkerStatus,
};

use crate::block::{
    Block, CompactEvent, ThinkingBlock, ThinkingState, ToolCallBlock, ToolCallState,
};
use crate::cache::FileCache;
use crate::command::{
    CommandCandidate, CommandEnvironment, CommandExecution, CommandInputMode, CommandRegistry,
};
use crate::composer_history::{
    COMPOSER_INPUT_HISTORY_LIMIT, ComposerHistoryStore, segments_are_blank,
};
use crate::input::InputBuffer;
use crate::scroll::Scroll;
use crate::task::TaskStore;
use crate::text_selection::TextSelectionState;
use crate::view_mode::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCompletionApply {
    Applied,
    Ambiguous,
    NoCandidates,
}

/// In-flight completion popup state. Lives on `App` while the user is
/// typing inside a `@` / `#` / `/` token. Cleared whenever the trigger
/// is invalidated (cursor moved out, whitespace landed inside the
/// token, the sigil was deleted, or the candidate was confirmed).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionSnapshot {
    input_revision: u64,
    generation: u64,
    target: String,
    worker_view: Option<String>,
}

struct PendingFeatureEdit {
    invocation: protocol::FeatureInvocation,
    request_id: String,
    snapshot: CompletionSnapshot,
}

pub struct CompletionState {
    pub request_id: String,
    snapshot: CompletionSnapshot,
    pub kind: CompletionKind,
    /// Atom index of the leading sigil (`@` / `#` / `/`).
    pub prefix_start: usize,
    /// Text typed after the sigil (sigil itself excluded).
    pub prefix: String,
    pub context: Option<protocol::CompletionContext>,
    /// Latest candidate set returned by the Worker for `(kind, prefix)`.
    /// Initially empty until `Event::Completions` lands.
    pub entries: Vec<CompletionEntry>,
    pub selected: usize,
}

impl CompletionState {
    pub fn is_active(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Maximum rows the popup ever renders. Caller can clip to fewer
    /// rows if vertical space is tight.
    pub const MAX_VISIBLE: usize = 6;
}

#[derive(Debug, Clone, Default)]
pub struct RewindPickerScroll {
    pub top_offset: usize,
    pub total_lines: usize,
    pub area_height: u16,
    pub tail_top_offset: usize,
}

#[derive(Debug, Clone)]
pub struct RewindPickerState {
    pub head_entries: usize,
    pub targets: Vec<RewindTarget>,
    pub selected: usize,
    pub scroll: RewindPickerScroll,
    /// True after Enter submitted an authoritative `RewindTo` and before the
    /// Worker replies with either `RewindApplied` or `Error`. While set, the
    /// picker remains visible but further submits/navigation are ignored so a
    /// destructive rewind cannot be queued multiple times by key repeat.
    pub applying: bool,
}

impl RewindPickerState {
    pub fn new(head_entries: usize, targets: Vec<RewindTarget>) -> Self {
        let selected = targets.iter().position(|t| t.eligible).unwrap_or(0);
        Self {
            head_entries,
            targets,
            selected,
            scroll: RewindPickerScroll::default(),
            applying: false,
        }
    }

    pub fn selected_target(&self) -> Option<&RewindTarget> {
        self.targets.get(self.selected)
    }
}

struct RollbackSubmitState {
    text: String,
    segments: Vec<Segment>,
    block_start: usize,
    turn_before: usize,
}

struct ComposerInputHistory {
    entries: VecDeque<Vec<Segment>>,
    browse: Option<ComposerInputHistoryBrowse>,
}

struct ComposerInputHistoryBrowse {
    index: usize,
    draft: Vec<Segment>,
}

impl ComposerInputHistory {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            browse: None,
        }
    }

    fn with_entries(entries: VecDeque<Vec<Segment>>) -> Self {
        Self {
            entries,
            browse: None,
        }
    }

    fn record(&mut self, segments: Vec<Segment>) -> bool {
        if segments_are_blank(&segments) {
            return false;
        }
        self.browse = None;
        if self.entries.back() == Some(&segments) {
            return false;
        }
        if self.entries.len() == COMPOSER_INPUT_HISTORY_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back(segments);
        true
    }

    fn is_browsing(&self) -> bool {
        self.browse.is_some()
    }

    fn browse_older(&mut self, draft: Vec<Segment>) -> Option<Vec<Segment>> {
        if self.entries.is_empty() {
            return None;
        }

        let index = match self.browse.as_mut() {
            Some(browse) => {
                if browse.index > 0 {
                    browse.index -= 1;
                }
                browse.index
            }
            None => {
                let index = self.entries.len() - 1;
                self.browse = Some(ComposerInputHistoryBrowse { index, draft });
                index
            }
        };

        self.entries.get(index).cloned()
    }

    fn browse_newer(&mut self) -> Option<Vec<Segment>> {
        let browse = self.browse.as_mut()?;
        if browse.index + 1 < self.entries.len() {
            browse.index += 1;
            return self.entries.get(browse.index).cloned();
        }

        self.browse.take().map(|browse| browse.draft)
    }

    fn cancel_browse(&mut self) {
        self.browse = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ActionbarNoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionbarNoticeSource {
    Tui,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionbarNotice {
    pub text: String,
    pub level: ActionbarNoticeLevel,
    pub source: ActionbarNoticeSource,
    pub expires_at: Instant,
}

impl ActionbarNotice {
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

pub struct InternalWorkerView {
    pub worker: InternalWorkerRef,
    pub revision: u64,
    pub app: Box<App>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerViewTab {
    pub label: String,
    pub selected: bool,
}

pub struct App {
    pub worker_name: String,
    pub connected: bool,
    /// Latest authoritative revisioned live execution state.
    pub worker_state: WorkerStateSnapshot,
    next_command_id: u64,
    /// Derived Runtime-catalog compatibility projection used by existing UI.
    pub worker_status: WorkerStatus,
    /// True while the Worker is in `WorkerStatus::Running`.
    pub running: bool,
    /// True while the Worker is in `WorkerStatus::Paused`.
    pub paused: bool,
    /// Local observation time for the current run. Used only for live UI
    /// elapsed time and spinner animation; it is not persisted in history.
    pub run_started_at: Option<Instant>,
    pub run_requests: usize,
    /// Sum of `input_tokens - cache_read_input_tokens` across the
    /// current turn's LLM requests — i.e. the net tokens this turn
    /// actually had to upload at full price (cache writes included,
    /// cache reads excluded). Reset on `RunEnd`.
    pub run_upload_tokens: u64,
    pub run_output_tokens: u64,
    /// Latest session context tokens reported by the Worker. This is the raw
    /// `input_tokens` value and is independent from per-run upload totals.
    pub session_context_tokens: u64,
    /// Availability and provenance for `session_context_tokens`. `None` means
    /// the current value is unknown and clients must not render a fabricated 0%.
    pub session_context_source: Option<protocol::ContextTokenSource>,
    pub context_window: u64,
    pub turn_index: usize,
    pub current_tool: Option<String>,
    /// Latest LLM wait/retry lifecycle event for actionbar observability.
    pub latest_llm_wait_event: Option<String>,
    /// Latest memory extract/consolidation lifecycle event for actionbar observability.
    pub latest_memory_worker_event: Option<String>,
    /// Current transient actionbar notice. Notices are local UI state only:
    /// they are never appended to transcript/session history or LLM context.
    actionbar_notice: Option<ActionbarNotice>,
    /// Normal composer input that is submitted as `Method::Submit`.
    pub input: InputBuffer,
    /// Separate command-line input. It is never submitted as a user message.
    pub command_input: InputBuffer,
    pub input_mode: CommandInputMode,
    pub command_registry: CommandRegistry,
    command_completion_selected: Option<usize>,
    pub quit: bool,
    /// 2-tap guard for `Ctrl-C` when the Worker is not running. First press
    /// records the instant; a second press within the timeout exits the
    /// TUI (the Worker itself stays alive).
    pub quit_confirm: Option<std::time::Instant>,
    /// Independent 2-tap guard for `Ctrl-X` when the Worker is idle or
    /// stopped. A second press within the timeout shuts down the Worker.
    pub shutdown_confirm: Option<std::time::Instant>,
    /// Full display history in render order.
    pub blocks: Vec<Block>,
    /// Turn/protocol errors retained when a real `SegmentStart` replaces the
    /// replayable conversation rows during segment rotation.
    run_error_messages: Vec<String>,
    /// Current compaction identity/revision used to fence snapshot/live updates.
    active_compaction: Option<(String, u64)>,
    pub compaction_progress: Option<protocol::InFlightCompaction>,
    /// Presentation-only Internal Worker projections keyed by session identity.
    /// They are rendered in separate selectable views and never mixed into `blocks`.
    pub internal_workers: Vec<InternalWorkerView>,
    /// Selected Internal Worker transcript/task view. `None` is the parent (`main`)
    /// view; the stable session identity survives projection reordering.
    selected_internal_worker_session_id: Option<String>,
    /// Terminal child-session fences, reset only by an authoritative snapshot.
    removed_internal_workers: HashMap<String, u64>,
    pub scroll: Scroll,
    pub mode: Mode,
    pub cache: FileCache,
    /// True when the latest AssistantText block is still being streamed
    /// and future text deltas should append to it instead of starting a
    /// fresh block.
    assistant_streaming: bool,
    /// Completion popup state, when an `@` / `#` / `/` token is in
    /// flight. `None` whenever the trigger conditions don't hold.
    pub completion: Option<CompletionState>,
    /// Invocation declarations explicitly selected from this composer's `/`
    /// completion lane. Only these declarations may turn editable text into a
    /// typed FeatureInvoke chip.
    selected_feature_invocations: HashMap<String, protocol::FeatureInvocationDescriptor>,
    pending_feature_edit: Option<PendingFeatureEdit>,
    completion_generation: u64,
    completion_target: String,
    /// Dedicated main-view rewind picker state.
    pub rewind_picker: Option<RewindPickerState>,
    rewind_request_pending: bool,
    /// After a successful rewind restore, ignore any queued live-update events
    /// until the authoritative Worker status/snapshot catches up. This prevents
    /// old stream tail events that were already in transit from re-polluting the
    /// just-restored display.
    rewind_refresh_fence: bool,
    greeting: Option<protocol::Greeting>,
    /// In-TUI mirror of the Worker's session task store, reconstructed
    /// directly from observed `TaskCreate` / `TaskUpdate` tool calls and
    /// `[Session TaskStore snapshot]` system messages — no protocol
    /// surface added on the Worker side.
    pub task_store: TaskStore,
    /// Transient single-Worker transcript text selection. This is viewport-local
    /// UI state only; it is never sent to the Worker, persisted, or appended to
    /// session history/model context.
    pub text_selection: TextSelectionState,
    /// Whether the right-side task pane is currently open.
    pub task_pane_open: bool,
    /// Top entry index of the task pane's visible window. Clamped on
    /// render so it never points past the end of the list.
    pub task_pane_scroll: usize,
    /// Authoritative WorkerSession FIFO summary received from snapshot/live events.
    pending_submissions: protocol::PendingSubmissionsSnapshot,
    /// TUI-local readline-style composer input history. This is intentionally
    /// client-side only: recalled entries are plain drafts until submitted again.
    input_history: ComposerInputHistory,
    /// User-data backed persistence for composer recall entries. The saved
    /// contents are private input drafts and must not be logged or sent to Worker.
    input_history_store: Option<ComposerHistoryStore>,
    /// Local submit state kept until the accepted run either completes
    /// normally or reports that the empty assistant turn was rolled back.
    pending_submit_rollback: Option<RollbackSubmitState>,
    retry_submission: Option<Method>,
    /// Last rolled-back submit that could not be restored because the
    /// composer already contained unsent user input.
    last_rolled_back_input: Option<Vec<Segment>>,
}

impl App {
    pub fn new(worker_name: String) -> Self {
        Self {
            worker_name,
            connected: false,
            worker_state: WorkerStateSnapshot::initial(),
            next_command_id: 1,
            worker_status: WorkerStatus::Idle,
            running: false,
            paused: false,
            run_started_at: None,
            run_requests: 0,
            run_upload_tokens: 0,
            run_output_tokens: 0,
            session_context_tokens: 0,
            session_context_source: None,
            context_window: 0,
            turn_index: 0,
            current_tool: None,
            latest_llm_wait_event: None,
            latest_memory_worker_event: None,
            actionbar_notice: None,
            input: InputBuffer::new(),
            command_input: InputBuffer::new(),
            input_mode: CommandInputMode::Composer,
            command_registry: CommandRegistry::default(),
            command_completion_selected: None,
            quit: false,
            quit_confirm: None,
            shutdown_confirm: None,
            blocks: Vec::new(),
            active_compaction: None,
            compaction_progress: None,
            run_error_messages: Vec::new(),
            internal_workers: Vec::new(),
            selected_internal_worker_session_id: None,
            removed_internal_workers: HashMap::new(),
            scroll: Scroll::default(),
            mode: Mode::Normal,
            cache: FileCache::new(),
            assistant_streaming: false,
            completion: None,
            selected_feature_invocations: HashMap::new(),
            pending_feature_edit: None,
            completion_generation: 0,
            completion_target: protocol::new_submission_request_id(),
            rewind_picker: None,
            rewind_request_pending: false,
            rewind_refresh_fence: false,
            greeting: None,
            task_store: TaskStore::new(),
            text_selection: TextSelectionState::default(),
            task_pane_open: false,
            task_pane_scroll: 0,
            pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
            input_history: ComposerInputHistory::new(),
            input_history_store: None,
            pending_submit_rollback: None,
            retry_submission: None,
            last_rolled_back_input: None,
        }
    }

    pub fn new_with_persistent_input_history(worker_name: String, workspace_root: &Path) -> Self {
        let mut app = Self::new(worker_name);
        match ComposerHistoryStore::default_for_workspace(workspace_root) {
            Ok(Some(store)) => {
                match store.load() {
                    Ok(entries) => {
                        app.input_history = ComposerInputHistory::with_entries(entries);
                    }
                    Err(_) => {
                        app.flash_actionbar_notice(
                            "Could not load saved composer input history; starting with empty local history.",
                            ActionbarNoticeLevel::Warn,
                            ActionbarNoticeSource::Tui,
                            Duration::from_secs(8),
                        );
                    }
                }
                app.input_history_store = Some(store);
            }
            Ok(None) => {
                app.flash_actionbar_notice(
                    "Composer input history persistence is disabled because the yoi data directory could not be resolved.",
                    ActionbarNoticeLevel::Warn,
                    ActionbarNoticeSource::Tui,
                    Duration::from_secs(8),
                );
            }
            Err(_) => {
                app.flash_actionbar_notice(
                    "Composer input history persistence is disabled because the history store could not be initialized.",
                    ActionbarNoticeLevel::Warn,
                    ActionbarNoticeSource::Tui,
                    Duration::from_secs(8),
                );
            }
        }
        app
    }

    #[cfg(test)]
    fn new_with_input_history_store(worker_name: String, store: ComposerHistoryStore) -> Self {
        let mut app = Self::new(worker_name);
        match store.load() {
            Ok(entries) => {
                app.input_history = ComposerInputHistory::with_entries(entries);
            }
            Err(_) => {
                app.flash_actionbar_notice_at(
                    "Could not load saved composer input history; starting with empty local history.",
                    ActionbarNoticeLevel::Warn,
                    ActionbarNoticeSource::Tui,
                    Instant::now(),
                    Duration::from_secs(8),
                );
            }
        }
        app.input_history_store = Some(store);
        app
    }

    pub fn toggle_task_pane(&mut self) {
        self.task_pane_open = !self.task_pane_open;
        if !self.task_pane_open {
            self.selected_worker_view_mut().task_pane_scroll = 0;
        }
    }

    pub fn worker_view_tabs(&self) -> Vec<WorkerViewTab> {
        let selected = self.selected_internal_worker_session_id.as_deref();
        let mut tabs = Vec::with_capacity(self.internal_workers.len().saturating_add(1));
        tabs.push(WorkerViewTab {
            label: "main".to_owned(),
            selected: selected.is_none(),
        });
        tabs.extend(self.internal_workers.iter().map(|view| {
            WorkerViewTab {
                label: view
                    .worker
                    .name
                    .lines()
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or("subworker")
                    .to_owned(),
                selected: selected == Some(view.worker.session_id.as_str()),
            }
        }));
        tabs
    }

    pub fn selected_internal_worker_index(&self) -> Option<usize> {
        let selected = self.selected_internal_worker_session_id.as_deref()?;
        self.internal_workers
            .iter()
            .position(|view| view.worker.session_id == selected)
    }

    pub fn selected_worker_view(&self) -> &App {
        self.selected_internal_worker_index()
            .map(|index| self.internal_workers[index].app.as_ref())
            .unwrap_or(self)
    }

    pub fn selected_worker_view_mut(&mut self) -> &mut App {
        if let Some(index) = self.selected_internal_worker_index() {
            self.internal_workers[index].app.as_mut()
        } else {
            self
        }
    }

    /// Cycle the presentation-only transcript/task view. Input and control
    /// methods continue to target the parent Worker regardless of selection.
    pub fn cycle_worker_view(&mut self) -> bool {
        self.invalidate_completion_generation();
        if self.internal_workers.is_empty() {
            self.selected_internal_worker_session_id = None;
            return false;
        }

        self.selected_internal_worker_session_id = self
            .selected_internal_worker_index()
            .and_then(|index| self.internal_workers.get(index.saturating_add(1)))
            .map(|view| view.worker.session_id.clone())
            .or_else(|| {
                if self.selected_internal_worker_session_id.is_none() {
                    self.internal_workers
                        .first()
                        .map(|view| view.worker.session_id.clone())
                } else {
                    None
                }
            });
        true
    }

    pub fn cycle_mode(&mut self) {
        let mode = self.mode.cycle();
        self.set_mode_recursively(mode);
    }

    fn set_mode_recursively(&mut self, mode: Mode) {
        self.mode = mode;
        for view in &mut self.internal_workers {
            view.app.set_mode_recursively(mode);
        }
    }

    pub fn scroll_task_pane_up(&mut self, n: usize) {
        let view = self.selected_worker_view_mut();
        view.task_pane_scroll = view.task_pane_scroll.saturating_sub(n);
    }

    pub fn scroll_task_pane_down(&mut self, n: usize) {
        let view = self.selected_worker_view_mut();
        view.task_pane_scroll = view.task_pane_scroll.saturating_add(n);
    }

    pub fn set_worker_status(&mut self, status: WorkerStatus) {
        let was_running = self.running;
        self.worker_status = status;
        self.running = status == WorkerStatus::Running;
        self.paused = status == WorkerStatus::Paused;
        if self.running {
            if !was_running {
                self.run_started_at = Some(Instant::now());
            }
            self.quit_confirm = None;
            self.shutdown_confirm = None;
        } else {
            self.run_started_at = None;
        }
    }

    fn completion_snapshot(&self) -> CompletionSnapshot {
        CompletionSnapshot {
            input_revision: self.input.revision(),
            generation: self.completion_generation,
            target: self.completion_target.clone(),
            worker_view: self.selected_internal_worker_session_id.clone(),
        }
    }

    pub(crate) fn set_completion_target(&mut self, target: String) {
        if self.completion_target != target {
            self.completion_target = target;
            self.invalidate_completion_generation();
            self.selected_feature_invocations.clear();
        }
    }

    /// Authority/snapshot/target changes revoke in-flight queries and visible candidates.
    fn invalidate_completion_generation(&mut self) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.completion = None;
        self.pending_feature_edit = None;
    }

    #[cfg(test)]
    pub(crate) fn completion_request_id(&self) -> Option<String> {
        self.completion
            .as_ref()
            .map(|state| state.request_id.clone())
            .or_else(|| {
                self.pending_feature_edit
                    .as_ref()
                    .map(|edit| edit.request_id.clone())
            })
    }

    /// Re-evaluate the completion popup against the current input.
    /// Returns a fresh correlated `Method::ListCompletions` when the prefix,
    /// context, token location, or composer/authority/target snapshot changes.
    /// An unchanged active query returns `None`.
    /// Callers should invoke this after every input mutation that could
    /// move the cursor or change atoms.
    pub fn refresh_completion(&mut self) -> Option<Method> {
        self.pending_feature_edit = None;
        if self.is_command_mode() {
            self.completion = None;
            return None;
        }
        let descriptors = self
            .selected_feature_invocations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let pending = self
            .input
            .pending_feature_argument_completion(&descriptors)
            .map(|(kind, start, prefix, context)| (kind, start, prefix, Some(context)))
            .or_else(|| {
                self.input
                    .pending_completion_prefix()
                    .map(|(kind, start, prefix)| (kind, start, prefix, None))
            });
        match pending {
            Some((kind, start, prefix, context)) => {
                let snapshot = self.completion_snapshot();
                if self.completion.as_ref().is_some_and(|state| {
                    state.kind == kind
                        && state.prefix_start == start
                        && state.prefix == prefix
                        && state.context == context
                        && state.snapshot == snapshot
                }) {
                    return None;
                }
                let request_id = protocol::new_submission_request_id();
                self.completion = Some(CompletionState {
                    request_id: request_id.clone(),
                    snapshot,
                    kind,
                    prefix_start: start,
                    prefix: prefix.clone(),
                    context: context.clone(),
                    entries: Vec::new(),
                    selected: 0,
                });
                Some(Method::ListCompletions {
                    kind,
                    prefix,
                    context,
                    request_id: Some(request_id),
                })
            }
            None => {
                self.completion = None;
                None
            }
        }
    }

    pub fn move_completion_up(&mut self) {
        if let Some(c) = self.completion.as_mut()
            && !c.entries.is_empty()
        {
            c.selected = if c.selected == 0 {
                c.entries.len() - 1
            } else {
                c.selected - 1
            };
        }
    }

    pub fn move_completion_down(&mut self) {
        if let Some(c) = self.completion.as_mut()
            && !c.entries.is_empty()
        {
            c.selected = (c.selected + 1) % c.entries.len();
        }
    }

    pub fn cancel_completion(&mut self) {
        self.invalidate_completion_generation();
    }

    /// Tab path: insert the popup-selected entry's value (with a
    /// trailing `/` when it's a directory) as raw text replacing the
    /// in-flight `@<typed>` portion. The popup state is preserved so
    /// the re-evaluated trigger can fetch fresh candidates for the new
    /// prefix (drill-in for directories, narrow-to-one for files).
    /// Returns the follow-up `Method::ListCompletions` to send when
    /// the new prefix differs from the old one.
    pub fn apply_completion_text(&mut self) -> Option<Method> {
        if self
            .completion
            .as_ref()
            .is_some_and(|state| state.snapshot != self.completion_snapshot())
        {
            self.cancel_completion();
            return None;
        }
        let state = self.completion.as_ref()?;
        if state.entries.is_empty() {
            return None;
        }
        let entry = state.entries[state.selected].clone();
        let mut typed_start = state.prefix_start + 1;
        let text = match state.kind {
            CompletionKind::File => {
                if entry.is_dir {
                    format!("{}/", entry.value)
                } else {
                    entry.value.clone()
                }
            }
            CompletionKind::Feature => {
                let descriptor = entry.invocation.clone()?;
                if descriptor.validate().is_err()
                    || !(entry.value == descriptor.name
                        || descriptor.aliases.contains(&entry.value))
                {
                    return None;
                }
                self.selected_feature_invocations
                    .insert(descriptor.identity.to_string(), descriptor.clone());
                self.input
                    .replace_feature_name_completion(typed_start, &format!("{}(", entry.value));
                self.input
                    .select_feature_invocation(state.prefix_start, descriptor);
                self.input_history.cancel_browse();
                return self.refresh_completion();
            }
            CompletionKind::FeatureArgument => {
                if self.input.can_complete_argument_name(state.prefix_start)
                    && let Some(name) = entry.value.strip_suffix('=')
                    && state
                        .context
                        .as_ref()
                        .and_then(|context| {
                            self.selected_feature_invocations
                                .get(&context.invocation.to_string())
                        })
                        .is_some_and(|descriptor| {
                            descriptor
                                .arguments
                                .iter()
                                .any(|argument| argument.name == name)
                        })
                {
                    self.input
                        .replace_argument_completion(typed_start, &entry.value);
                    self.input_history.cancel_browse();
                    return self.refresh_completion();
                }
                if state
                    .context
                    .as_ref()
                    .and_then(|context| context.argument.as_ref())
                    .is_some()
                {
                    if typed_start > 0 && self.input.char_at(typed_start - 1) == Some('"') {
                        typed_start -= 1;
                    }
                    let context = state.context.as_ref()?;
                    let descriptor = self
                        .selected_feature_invocations
                        .get(&context.invocation.to_string())?;
                    let argument = descriptor
                        .arguments
                        .iter()
                        .find(|argument| Some(&argument.name) == context.argument.as_ref())?;
                    match &argument.value_type {
                        protocol::InvocationArgumentType::Integer => {
                            entry.value.parse::<i32>().ok()?.to_string()
                        }
                        protocol::InvocationArgumentType::Boolean
                            if matches!(entry.value.as_str(), "true" | "false") =>
                        {
                            entry.value.clone()
                        }
                        protocol::InvocationArgumentType::Boolean => return None,
                        _ => protocol::quote_invocation_string(&if entry.is_dir {
                            format!("{}/", entry.value.trim_end_matches('/'))
                        } else {
                            entry.value.clone()
                        }),
                    }
                } else {
                    let context = state.context.as_ref()?;
                    let descriptor = self
                        .selected_feature_invocations
                        .get(&context.invocation.to_string())?;
                    if !entry.value.ends_with('=')
                        || !descriptor
                            .arguments
                            .iter()
                            .any(|argument| argument.name == entry.value.trim_end_matches('='))
                    {
                        return None;
                    }
                    entry.value.clone()
                }
            }
        };
        // `prefix_start` indexes the sigil atom for Feature/file names and one
        // atom before the active replacement range for Feature arguments.
        let text =
            if state.kind == CompletionKind::FeatureArgument && entry.is_dir && text.ends_with('"')
            {
                text[..text.len() - 1].to_string()
            } else {
                text
            };
        self.input_history.cancel_browse();
        if state.kind == CompletionKind::FeatureArgument {
            self.input.replace_argument_completion(typed_start, &text);
        } else {
            self.input.replace_with_text_at(typed_start, &text);
        }
        self.refresh_completion()
    }

    /// Space path: replace the `@<typed>` range with a chip atom and
    /// clear the popup if `prefix` (= the text the user has typed
    /// after the sigil) resolves to a confirmable target. Three
    /// matching modes:
    ///
    /// 1. **Direct value match**: some entry's `value` equals `prefix`
    ///    (covers files and slash-less directory form).
    /// 2. **Slashed directory match**: some directory entry's
    ///    `value + "/"` equals `prefix` (the form Tab inserts).
    /// 3. **Drilled-into-directory match**: `prefix` ends with `/`
    ///    and at least one entry lives under it.
    ///
    /// Directory chips always carry a trailing `/` so the rendered
    /// label reads `@crates/`.
    ///
    /// `selected` is intentionally ignored — terminating with a
    /// space is a typed-based "I'm done with this token" signal,
    /// so a race-y top entry shouldn't block confirmation when the
    /// typed text matches another entry.
    pub fn chipify_completion_if_exact_match(&mut self) -> bool {
        if self
            .completion
            .as_ref()
            .is_some_and(|state| state.snapshot != self.completion_snapshot())
        {
            self.cancel_completion();
            return false;
        }
        let Some(state) = self.completion.as_ref() else {
            return false;
        };
        let direct = state
            .entries
            .iter()
            .find(|e| {
                state.prefix == e.value || (e.is_dir && state.prefix == format!("{}/", e.value))
            })
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.value)
                } else {
                    e.value.clone()
                }
            });
        let drilled = (direct.is_none() && state.prefix.ends_with('/'))
            .then(|| {
                state
                    .entries
                    .iter()
                    .any(|e| e.value.starts_with(&state.prefix))
                    .then(|| state.prefix.clone())
            })
            .flatten();
        let Some(value) = direct.or(drilled) else {
            return false;
        };
        let kind = state.kind;
        let start = state.prefix_start;
        self.input_history.cancel_browse();
        match kind {
            CompletionKind::File => self.input.replace_with_file_ref(start, value),
            CompletionKind::Feature | CompletionKind::FeatureArgument => return false,
        }
        self.completion = None;
        true
    }

    /// Enter path: commit the currently *selected* popup entry,
    /// regardless of how much of its value the user has typed. This
    /// is the popup-UI sense of "Enter accepts the highlighted
    /// suggestion" — partial typing like `@README.` followed by
    /// Enter should chip when the popup is on `README.md`.
    ///
    /// so the caller can fall through to `apply_completion_text`
    /// for drill-in — chip-ifying a directory on Enter would strand
    /// the user with no way to inspect children.
    pub fn chipify_selected_completion_if_committable(&mut self) -> bool {
        if self
            .completion
            .as_ref()
            .is_some_and(|state| state.snapshot != self.completion_snapshot())
        {
            self.cancel_completion();
            return false;
        }
        let Some(state) = self.completion.as_ref() else {
            return false;
        };
        if state.entries.is_empty() {
            return false;
        }
        let entry = &state.entries[state.selected];
        if state.kind == CompletionKind::File && entry.is_dir {
            return false;
        }
        let kind = state.kind;
        let start = state.prefix_start;
        let value = entry.value.clone();
        self.input_history.cancel_browse();
        match kind {
            CompletionKind::File => self.input.replace_with_file_ref(start, value),
            CompletionKind::Feature | CompletionKind::FeatureArgument => return false,
        }
        self.completion = None;
        true
    }

    pub fn is_client_file_completion(&self, context: &protocol::CompletionContext) -> bool {
        self.selected_feature_invocations
            .get(&context.invocation.to_string())
            .is_some_and(|descriptor| {
                descriptor.arguments.iter().any(|argument| {
                    Some(&argument.name) == context.argument.as_ref()
                        && (matches!(
                            argument.completion,
                            protocol::InvocationCompletion::ClientFile
                        ) || matches!(
                            argument.value_type,
                            protocol::InvocationArgumentType::ClientFile
                        ))
                })
            })
    }

    pub fn attachment_invocations(&self) -> Vec<(String, std::path::PathBuf)> {
        self.input
            .submit_segments()
            .into_iter()
            .filter_map(|segment| {
                let Segment::FeatureInvoke { invocation } = segment else {
                    return None;
                };
                let descriptor = self
                    .selected_feature_invocations
                    .get(&invocation.identity.to_string())?;
                if descriptor.client_adapter != Some(protocol::InvocationClientAdapter::Attachment)
                {
                    return None;
                }
                invocation.arguments.iter().find_map(|argument| {
                    let schema = descriptor
                        .arguments
                        .iter()
                        .find(|schema| schema.name == argument.name)?;
                    match (&schema.value_type, &argument.value) {
                        (
                            protocol::InvocationArgumentType::ClientFile,
                            protocol::InvocationValue::String(path),
                        ) if !path.is_empty() => Some((
                            invocation.invocation_id.clone(),
                            std::path::PathBuf::from(path),
                        )),
                        _ => None,
                    }
                })
            })
            .collect()
    }

    /// Alt+Enter reopens an adjacent chip using freshly discovered enabled metadata.
    pub fn edit_adjacent_feature_invocation(&mut self) -> Option<Method> {
        let invocation = self.input.adjacent_feature_invocation()?;
        let prefix = invocation.name.clone();
        let request_id = protocol::new_submission_request_id();
        self.pending_feature_edit = Some(PendingFeatureEdit {
            invocation,
            request_id: request_id.clone(),
            snapshot: self.completion_snapshot(),
        });
        self.completion = None;
        Some(Method::ListCompletions {
            request_id: Some(request_id),
            kind: CompletionKind::Feature,
            prefix,
            context: None,
        })
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        if self.input.has_attachment_stages() {
            self.push_error("Attachment is still staging or failed; wait, retry with Alt+Enter, or delete its chip.");
            return None;
        }
        let confirming = self.input.has_selected_feature_invocations();
        if let Err(error) = self.input.finalize_feature_invocations() {
            self.push_error(format!("Invalid Feature invocation: {error}"));
            return None;
        }
        if confirming || !self.attachment_invocations().is_empty() {
            // Console stages adapters in-place; never record local paths in history or send them.
            self.completion = None;
            return None;
        }
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            // Empty Enter only does something meaningful when the Worker
            // is paused: resume the interrupted turn. Otherwise no-op.
            if self.paused {
                self.input_history.cancel_browse();
                self.input.clear();
                let command = self.next_command_envelope();
                return Some(Method::Resume { command });
            }
            return None;
        }
        self.record_input_history(segments.clone());
        self.input.clear();
        Some(self.method_for_run(segments))
    }

    pub fn submit_notify_input(&mut self) -> Option<Method> {
        if self.input.has_attachment_stages() || self.input.has_selected_feature_invocations() {
            self.push_error("Notify accepts text only; confirm or remove Feature invocations and staging chips first.");
            return None;
        }
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            return None;
        }
        if segments.iter().any(|segment| {
            matches!(
                segment,
                Segment::UploadedFile { .. } | Segment::FeatureInvoke { .. }
            )
        }) {
            self.push_error(
                "Notify accepts text only; remove attachments or Feature invocations or queue a Submit.",
            );
            return None;
        }
        let message = Segment::flatten_to_text(&segments);
        self.record_input_history(segments);
        self.input.clear();
        Some(Method::Notify {
            notification_request_id: protocol::new_submission_request_id(),
            message,
        })
    }

    pub fn restore_unsent_run(&mut self, method: &Method) {
        let Method::Submit { input, .. } = method else {
            return;
        };
        self.pending_submit_rollback = None;
        if self.input.is_empty() || self.input.submit_segments() == *input {
            self.retry_submission = Some(method.clone());
            self.input.replace_with_segments(input);
            self.completion = None;
        } else {
            self.push_error(
                "Submit transport failed; current Composer was preserved and the unsent input was not queued.",
            );
        }
    }

    fn method_for_run(&mut self, segments: Vec<Segment>) -> Method {
        // TurnHeader / UserMessage blocks are pushed only after the Worker
        // emits `Event::UserMessage` from a committed `LogEntry::AnnotatedUserInput`.
        // Locally we only clear the input buffer and forward the method,
        // while remembering enough local state to undo the visible submit if
        // the accepted run produced no assistant output and was rolled back.
        self.pending_submit_rollback = Some(RollbackSubmitState {
            text: Segment::flatten_to_text(&segments),
            segments: segments.clone(),
            block_start: self.blocks.len(),
            turn_before: self.turn_index,
        });
        if let Some(method @ Method::Submit { .. }) = self.retry_submission.take() {
            if matches!(&method, Method::Submit { input, .. } if *input == segments) {
                return method;
            }
        }
        Method::Submit {
            submission_request_id: protocol::new_submission_request_id(),
            input: segments,
        }
    }

    fn record_input_history(&mut self, segments: Vec<Segment>) {
        if !self.input_history.record(segments) {
            return;
        }
        let Some(store) = &self.input_history_store else {
            return;
        };
        if store.save(&self.input_history.entries).is_err() {
            self.flash_actionbar_notice(
                "Could not save composer input history; continuing with in-memory local history.",
                ActionbarNoticeLevel::Warn,
                ActionbarNoticeSource::Tui,
                Duration::from_secs(8),
            );
        }
    }

    pub fn queued_input_count(&self) -> usize {
        self.pending_submissions.submissions.len()
    }

    #[cfg(test)]
    pub fn input_history_len(&self) -> usize {
        self.input_history.entries.len()
    }

    #[cfg(test)]
    pub fn input_history_is_browsing(&self) -> bool {
        self.input_history.is_browsing()
    }

    pub fn can_browse_input_history_older(&self) -> bool {
        self.input_history.is_browsing() || self.input.cursor_at_start()
    }

    pub fn can_browse_input_history_newer(&self) -> bool {
        self.input_history.is_browsing() && self.input.cursor_at_end()
    }

    pub fn browse_input_history_older(&mut self) -> bool {
        if self.input.has_attachment_stages()
            || self.input.has_selected_feature_invocations()
            || self.input_history.entries.is_empty()
        {
            return false;
        }
        let draft = self.input.submit_segments();
        let Some(segments) = self.input_history.browse_older(draft) else {
            return false;
        };
        self.input.replace_with_segments(&segments);
        self.completion = None;
        true
    }

    pub fn browse_input_history_newer(&mut self) -> bool {
        let Some(segments) = self.input_history.browse_newer() else {
            return false;
        };
        self.input.replace_with_segments(&segments);
        self.completion = None;
        true
    }

    pub fn flash_actionbar_notice(
        &mut self,
        text: impl Into<String>,
        level: ActionbarNoticeLevel,
        source: ActionbarNoticeSource,
        duration: Duration,
    ) {
        self.flash_actionbar_notice_at(text, level, source, Instant::now(), duration);
    }

    pub fn flash_actionbar_notice_at(
        &mut self,
        text: impl Into<String>,
        level: ActionbarNoticeLevel,
        source: ActionbarNoticeSource,
        now: Instant,
        duration: Duration,
    ) {
        self.actionbar_notice = Some(ActionbarNotice {
            text: text.into(),
            level,
            source,
            expires_at: now + duration,
        });
    }

    pub fn current_actionbar_notice(&self, now: Instant) -> Option<&ActionbarNotice> {
        self.actionbar_notice
            .as_ref()
            .filter(|notice| !notice.is_expired(now))
    }

    pub fn clear_expired_actionbar_notice(&mut self, now: Instant) {
        if self
            .actionbar_notice
            .as_ref()
            .is_some_and(|notice| notice.is_expired(now))
        {
            self.actionbar_notice = None;
        }
    }

    pub fn continue_pending_method(&self) -> Option<Method> {
        Some(Method::ContinuePending {
            expected_revision: self.pending_submissions.revision,
            expected_head_id: self.pending_submissions.head_id.clone()?,
        })
    }

    pub fn clear_pending_method(&self) -> Method {
        Method::ClearPendingSubmissions {
            expected_revision: self.pending_submissions.revision,
        }
    }

    pub fn cancel_pending_method(&self, submission_id: String) -> Method {
        Method::CancelPendingSubmission {
            submission_id,
            expected_revision: self.pending_submissions.revision,
        }
    }

    pub fn next_queued_input_preview(&self) -> Option<&str> {
        self.pending_submissions
            .submissions
            .first()
            .map(|submission| submission.submission_id.as_str())
    }

    pub fn clear_actionbar_notice(&mut self) {
        self.actionbar_notice = None;
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Worker,
            message: message.into(),
        });
    }

    fn push_run_error(&mut self, message: String) {
        if self
            .run_error_messages
            .iter()
            .any(|existing| run_failure_messages_match(existing, &message))
        {
            return;
        }
        self.run_error_messages.push(message.clone());
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Worker,
            message,
        });
    }

    fn push_durable_run_error(&mut self, message: String) {
        self.run_error_messages
            .retain(|existing| !run_failure_messages_match(existing, &message));
        self.blocks.retain(|block| {
            !matches!(
                block,
                Block::Alert {
                    level: AlertLevel::Error,
                    message: existing,
                    ..
                } if run_failure_messages_match(existing, &message)
            )
        });
        self.run_error_messages.push(message.clone());
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Worker,
            message,
        });
    }

    fn handle_error(&mut self, code: ErrorCode, message: String) {
        if self
            .run_error_messages
            .iter()
            .any(|existing| run_failure_messages_match(existing, &message))
        {
            return;
        }
        let text = format!("[{code:?}] {message}");
        let was_applying = if let Some(picker) = self.rewind_picker.as_mut() {
            let applying = picker.applying;
            picker.applying = false;
            applying
        } else {
            false
        };
        if was_applying {
            self.flash_actionbar_notice(
                format!("Rewind failed: {text}"),
                ActionbarNoticeLevel::Error,
                ActionbarNoticeSource::Tui,
                Duration::from_secs(6),
            );
        }
        self.push_run_error(text);
    }

    fn rewind_submit_pending(&self) -> bool {
        self.rewind_picker
            .as_ref()
            .map(|picker| picker.applying)
            .unwrap_or(false)
    }

    fn push_history_item(&mut self, item: &serde_json::Value) {
        let item_type = item["type"].as_str().unwrap_or("");
        match item_type {
            "message" => {
                let role = item["role"].as_str().unwrap_or("");
                let text = message_text(item);
                match role {
                    "user" => {
                        self.turn_index += 1;
                        self.blocks.push(Block::TurnHeader {
                            turn: self.turn_index,
                        });
                        // Worker attaches the original `Vec<Segment>` to user
                        // messages from live submissions, so we can rebuild
                        // typed atoms (paste chips, refs) here. Seed history
                        // loaded post-compaction has no `segments` field —
                        // fall back to a single text segment.
                        let segments = item
                            .get("segments")
                            .and_then(|v| serde_json::from_value::<Vec<Segment>>(v.clone()).ok())
                            .unwrap_or_else(|| {
                                if text.is_empty() {
                                    Vec::new()
                                } else {
                                    vec![Segment::text(text.clone())]
                                }
                            });
                        if !segments.is_empty() {
                            self.blocks.push(Block::UserMessage { segments });
                        }
                    }
                    "assistant" if !text.is_empty() => {
                        self.blocks.push(Block::AssistantText { text });
                    }
                    "system" if !text.is_empty() => {
                        self.task_store.apply_system_message_text(&text);
                        self.blocks.push(Block::SystemMessage { text });
                    }
                    _ => {}
                }
            }
            "tool_call" => {
                // `Item::ToolCall` serializes the linking key as
                // `call_id`; `id` is a separate optional item-level
                // identifier. Use `call_id` so this matches how
                // Event::ToolCallStart populates the block.
                let id = item["call_id"].as_str().unwrap_or("").to_owned();
                let name = item["name"].as_str().unwrap_or("?").to_owned();
                let arguments = item["arguments"].as_str().map(|s| s.to_owned());
                if let Some(args) = arguments.as_deref() {
                    self.task_store.apply_tool_call(&name, args);
                }
                self.blocks.push(Block::ToolCall(ToolCallBlock {
                    id,
                    name,
                    args_stream: arguments.clone().unwrap_or_default(),
                    arguments,
                    state: ToolCallState::Executing,
                    edit_snapshot: None,
                }));
            }
            "reasoning" => {
                let text = item["text"].as_str().unwrap_or("").to_owned();
                let body = if text.is_empty() {
                    item["summary"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                } else {
                    text
                };
                self.blocks.push(Block::Thinking(ThinkingBlock {
                    text: body,
                    state: ThinkingState::Finished { elapsed_secs: None },
                }));
            }
            "tool_result" => {
                let id = item["call_id"].as_str().unwrap_or("").to_owned();
                let summary = item["summary"].as_str().unwrap_or("").to_owned();
                let output = item["content"].as_str().map(|s| s.to_owned());
                let is_error = item["is_error"].as_bool().unwrap_or(false);
                let (name, args) = self
                    .find_tool_call_mut(&id)
                    .map(|b| (b.name.clone(), b.arguments.clone()))
                    .unwrap_or_default();
                let edit_snapshot = if !is_error && name == "Edit" {
                    args.as_deref()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
                        .and_then(|path| self.cache.get(&path).map(|s| s.to_owned()))
                } else {
                    None
                };
                if let Some(tc) = self.find_tool_call_mut(&id) {
                    if edit_snapshot.is_some() {
                        tc.edit_snapshot = edit_snapshot;
                    }
                    tc.state = if is_error {
                        ToolCallState::Error {
                            summary,
                            output: output.clone(),
                        }
                    } else {
                        ToolCallState::Done {
                            summary,
                            output: output.clone(),
                        }
                    };
                    if !is_error {
                        apply_cache_update(
                            &mut self.cache,
                            &name,
                            args.as_deref(),
                            output.as_deref(),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    pub fn next_command_envelope(&mut self) -> WorkerCommandEnvelope {
        let command_id = self
            .next_command_id
            .max(self.worker_state.last_command_id.saturating_add(1));
        let command = WorkerCommandEnvelope::new(command_id);
        self.next_command_id = command_id.saturating_add(1);
        command
    }

    fn apply_worker_state_snapshot(&mut self, snapshot: &WorkerStateSnapshot) {
        self.worker_state = snapshot.clone();
        self.set_worker_status(self.worker_state.catalog_status());
    }

    pub fn handle_worker_event(&mut self, event: Event) -> Option<Method> {
        if self.rewind_refresh_fence && event_is_stale_after_rewind(&event) {
            return None;
        }

        match event {
            Event::SubmissionAccepted {
                submission_request_id,
                ..
            } => {
                if matches!(&self.retry_submission, Some(Method::Submit { submission_request_id: id, .. }) if *id == submission_request_id)
                {
                    if let Some(Method::Submit { input, .. }) = self.retry_submission.take() {
                        if self.input.submit_segments() == input {
                            self.input.clear();
                            self.completion = None;
                        }
                    }
                }
            }
            Event::NotificationAccepted { .. } => {}
            Event::SubmissionRejected { message, .. }
            | Event::NotificationRejected { message, .. } => self.push_error(message),
            Event::PendingSubmissionsChanged { pending } => {
                self.pending_submissions = pending;
            }
            Event::UserMessage { segments, .. } => {
                self.turn_index += 1;
                self.blocks.push(Block::TurnHeader {
                    turn: self.turn_index,
                });
                self.blocks.push(Block::UserMessage { segments });
                self.assistant_streaming = false;
            }
            Event::SegmentRotated { session } => {
                let retained_run_errors = self.run_error_messages.clone();
                self.restore_session(&session, self.greeting.clone());
                for message in retained_run_errors {
                    self.blocks.push(Block::Alert {
                        level: AlertLevel::Error,
                        source: AlertSource::Worker,
                        message,
                    });
                }
                self.assistant_streaming = false;
            }
            Event::SessionEntryCommitted { entry } => {
                if let protocol::SessionSnapshotEntryData::RunError { message, .. } = entry.data {
                    self.push_durable_run_error(message);
                }
            }
            Event::SystemItem { item, .. } => {
                self.apply_system_item(&item);
                self.assistant_streaming = false;
            }
            Event::TurnStart { .. } => {
                self.run_requests += 1;
                self.current_tool = None;
                self.latest_llm_wait_event = None;
                self.assistant_streaming = false;
            }
            Event::InvokeStart { .. } => {}
            // UI consumers of per-attempt LlmCall semantics remain out of scope;
            // authoritative run state comes only from WorkerStateSnapshot.
            Event::LlmCallStart { .. } | Event::LlmCallEnd { .. } => {
                self.latest_llm_wait_event = None;
            }
            Event::LlmRetry {
                failed_attempt,
                max_attempts,
                wait_ms,
                status,
                error,
                ..
            } => {
                let next_attempt = failed_attempt.saturating_add(1).min(max_attempts);
                let reason = status
                    .map(|code| format!("HTTP {code}"))
                    .unwrap_or_else(|| error);
                self.latest_llm_wait_event = Some(format!(
                    "retrying LLM request after {reason} (attempt {next_attempt}/{max_attempts} in {})",
                    fmt_millis(wait_ms)
                ));
            }
            Event::LlmContinuation {
                attempt,
                max_attempts,
                reason,
                ..
            } => {
                self.latest_llm_wait_event = Some(format!(
                    "LLM stream interrupted; continuing generation ({attempt}/{max_attempts}): {reason}"
                ));
            }
            Event::TextDelta { text } => {
                self.latest_llm_wait_event = None;
                self.append_assistant_text(&text);
            }
            Event::TextDone { .. } => {
                self.assistant_streaming = false;
            }
            Event::ThinkingStart => {
                self.latest_llm_wait_event = None;
                self.assistant_streaming = false;
                self.blocks.push(Block::Thinking(ThinkingBlock {
                    text: String::new(),
                    state: ThinkingState::Streaming {
                        started_at: Instant::now(),
                    },
                }));
            }
            Event::ThinkingDelta { text } => {
                if let Some(b) = self.last_streaming_thinking_mut() {
                    b.text.push_str(&text);
                }
            }
            Event::ThinkingDone { text } => {
                if let Some(b) = self.last_streaming_thinking_mut() {
                    // Delta-accumulated text wins. `text` here is the
                    // Done payload (full body), used only as a fallback
                    // for providers that don't stream deltas.
                    if b.text.is_empty() {
                        b.text = text;
                    }
                    let elapsed = match &b.state {
                        ThinkingState::Streaming { started_at } => {
                            Some(started_at.elapsed().as_secs())
                        }
                        _ => None,
                    };
                    b.state = ThinkingState::Finished {
                        elapsed_secs: elapsed,
                    };
                }
            }
            Event::TurnEnd { .. } => {
                self.assistant_streaming = false;
                self.mark_orphan_tool_calls_incomplete();
                self.mark_orphan_thinking_incomplete();
                self.current_tool = None;
            }
            Event::ToolCallStart { id, name } => {
                self.latest_llm_wait_event = None;
                self.current_tool = Some(name.clone());
                self.assistant_streaming = false;
                self.blocks.push(Block::ToolCall(ToolCallBlock {
                    id,
                    name,
                    args_stream: String::new(),
                    arguments: None,
                    state: ToolCallState::Pending,
                    edit_snapshot: None,
                }));
            }
            Event::ToolCallArgsDelta { id, json } => {
                if let Some(b) = self.find_tool_call_mut(&id) {
                    b.args_stream.push_str(&json);
                    if matches!(b.state, ToolCallState::Pending) {
                        b.state = ToolCallState::Streaming;
                    }
                }
            }
            Event::ToolCallDone { id, arguments, .. } => {
                self.current_tool = None;
                let name = self.find_tool_call_mut(&id).map(|b| b.name.clone());
                if let Some(name) = name.as_deref() {
                    self.task_store.apply_tool_call(name, &arguments);
                }
                if let Some(b) = self.find_tool_call_mut(&id) {
                    b.arguments = Some(arguments);
                    // Only advance the state when it's still in-flight.
                    // If a ToolResult arrived out of order and already
                    // transitioned us to Done/Error, keep that.
                    if matches!(b.state, ToolCallState::Pending | ToolCallState::Streaming) {
                        b.state = ToolCallState::Executing;
                    }
                }
            }
            Event::ToolResult {
                id,
                summary,
                output,
                disposition: _,
                is_error,
            } => {
                self.latest_llm_wait_event = None;
                // Pull the name / args out first so we can look at the
                // (immutable) cache before taking the mutable block
                // borrow below.
                let (name, args) = self
                    .find_tool_call_mut(&id)
                    .map(|b| (b.name.clone(), b.arguments.clone()))
                    .unwrap_or_default();
                let edit_snapshot = if !is_error && name == "Edit" {
                    args.as_deref()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
                        .and_then(|path| self.cache.get(&path).map(|s| s.to_owned()))
                } else {
                    None
                };

                if let Some(b) = self.find_tool_call_mut(&id) {
                    if edit_snapshot.is_some() {
                        b.edit_snapshot = edit_snapshot;
                    }
                    b.state = if is_error {
                        ToolCallState::Error {
                            summary,
                            output: output.clone(),
                        }
                    } else {
                        ToolCallState::Done {
                            summary,
                            output: output.clone(),
                        }
                    };
                    if !is_error {
                        apply_cache_update(
                            &mut self.cache,
                            &name,
                            args.as_deref(),
                            output.as_deref(),
                        );
                    }
                } else {
                    // Result for an unknown tool call. Surface it as an
                    // alert so it isn't silently dropped.
                    let level = if is_error {
                        AlertLevel::Error
                    } else {
                        AlertLevel::Warn
                    };
                    self.blocks.push(Block::Alert {
                        level,
                        source: AlertSource::Worker,
                        message: format!("orphan tool result ({id}): {summary}"),
                    });
                }
            }
            Event::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
            } => {
                if let Some(input_tokens) = input_tokens {
                    self.session_context_tokens = input_tokens;
                    self.session_context_source = Some(protocol::ContextTokenSource::Measured);
                } else {
                    self.session_context_source = None;
                }
                // Subtract the cache-hit portion so a tool loop that
                // re-sends the same prefix on every request doesn't
                // re-count it. cache_creation stays in (it is full
                // price on this request).
                let net_input = input_tokens
                    .unwrap_or(0)
                    .saturating_sub(cache_read_input_tokens.unwrap_or(0));
                self.run_upload_tokens += net_input;
                self.run_output_tokens += output_tokens.unwrap_or(0);
            }
            Event::ContextUsage { usage } => {
                if let Some(usage) = usage {
                    self.session_context_tokens = usage.tokens;
                    self.session_context_source = Some(usage.source);
                } else {
                    self.session_context_source = None;
                }
            }
            Event::Error { code, message } => {
                self.handle_error(code, message);
            }
            Event::RunEnd { result } => {
                self.latest_llm_wait_event = None;
                if matches!(result, RunResult::RolledBack) {
                    self.handle_rolled_back_run();
                } else {
                    self.blocks.push(Block::TurnStats {
                        requests: self.run_requests,
                        upload_tokens: self.run_upload_tokens,
                        output_tokens: self.run_output_tokens,
                    });
                    self.pending_submit_rollback = None;
                    self.reset_run_state();
                }
            }
            Event::CompactionProgress { compaction } => {
                self.compaction_progress = compaction.filter(|progress| {
                    matches!(
                        (&self.worker_state.state, progress.trigger),
                        (
                            protocol::WorkerState::Busy(protocol::WorkerBusyState::Maintenance(
                                protocol::WorkerMaintenanceState::Compacting
                            )),
                            protocol::CompactionTrigger::Manual
                        ) | (
                            protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(_)),
                            protocol::CompactionTrigger::PreRun
                                | protocol::CompactionTrigger::RequestThreshold
                        )
                    )
                });
            }
            Event::CompactStart { lifecycle } => {
                let should_apply = match &self.active_compaction {
                    None => true,
                    Some((id, revision)) => {
                        id == &lifecycle.compaction_id && lifecycle.revision > *revision
                    }
                };
                if should_apply {
                    self.active_compaction = Some((lifecycle.compaction_id, lifecycle.revision));
                    if self.last_streaming_compact_mut().is_none() {
                        self.blocks.push(Block::Compact(CompactEvent::Streaming {
                            started_at: Instant::now(),
                        }));
                    }
                }
            }
            Event::CompactDone { lifecycle } => {
                let should_apply = match &self.active_compaction {
                    None => true,
                    Some((id, revision)) => {
                        id == &lifecycle.compaction_id && lifecycle.revision > *revision
                    }
                };
                if !should_apply {
                    return None;
                }
                self.active_compaction = None;
                let new_segment_id = lifecycle
                    .new_segment_id
                    .as_deref()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                    .unwrap_or_default();
                if let Some(evt) = self.last_streaming_compact_mut() {
                    let elapsed_secs = match evt {
                        CompactEvent::Streaming { started_at } => {
                            Some(started_at.elapsed().as_secs())
                        }
                        _ => None,
                    };
                    *evt = CompactEvent::Done {
                        new_segment_id,
                        elapsed_secs,
                    };
                } else {
                    self.blocks.push(Block::Compact(CompactEvent::Done {
                        new_segment_id,
                        elapsed_secs: None,
                    }));
                }
            }
            Event::CompactFailed { lifecycle } => {
                let should_apply = match &self.active_compaction {
                    None => true,
                    Some((id, revision)) => {
                        id == &lifecycle.compaction_id && lifecycle.revision > *revision
                    }
                };
                if !should_apply {
                    return None;
                }
                self.active_compaction = None;
                let error = lifecycle
                    .error
                    .unwrap_or_else(|| "compaction failed".to_string());
                if let Some(evt) = self.last_streaming_compact_mut() {
                    let elapsed_secs = match evt {
                        CompactEvent::Streaming { started_at } => {
                            Some(started_at.elapsed().as_secs())
                        }
                        _ => None,
                    };
                    *evt = CompactEvent::Failed {
                        error,
                        elapsed_secs,
                    };
                } else {
                    self.blocks.push(Block::Compact(CompactEvent::Failed {
                        error,
                        elapsed_secs: None,
                    }));
                }
            }
            Event::Alert(alert) => {
                if alert.level == AlertLevel::Error
                    && self
                        .run_error_messages
                        .iter()
                        .any(|message| run_failure_messages_match(message, &alert.message))
                {
                    return None;
                }
                self.blocks.push(Block::Alert {
                    level: alert.level,
                    source: alert.source,
                    message: alert.message,
                });
            }
            Event::MemoryWorker(event) => {
                self.latest_memory_worker_event = Some(event.message);
            }
            Event::Snapshot {
                session,
                greeting,
                state,
                in_flight,
                internal_workers,
            } => {
                self.rewind_refresh_fence = false;
                self.pending_submissions = session.pending_submissions.clone();
                self.apply_worker_state_snapshot(&state);
                self.restore_snapshot(&session, greeting, in_flight);
                self.replace_internal_worker_snapshots(internal_workers);
                return self.refresh_completion();
            }
            Event::InternalWorker {
                worker,
                revision,
                event,
            } => self.apply_internal_worker_event(worker, revision, *event),
            Event::InternalWorkerRemoved { worker, revision } => {
                self.remove_internal_worker(worker, revision)
            }
            Event::WorkerState { snapshot } => {
                self.rewind_refresh_fence = false;
                self.apply_worker_state_snapshot(&snapshot);
                if let Some(progress) = self.compaction_progress.take() {
                    let _ = self.handle_worker_event(Event::CompactionProgress {
                        compaction: Some(progress),
                    });
                }
            }
            Event::CommandAcknowledged { acknowledgement } => {
                self.apply_worker_state_snapshot(&acknowledgement.state);
                if let Some(progress) = self.compaction_progress.take() {
                    let _ = self.handle_worker_event(Event::CompactionProgress {
                        compaction: Some(progress),
                    });
                }
            }
            // Command telemetry is an operational Web Console surface. The
            // TUI continues to render the final Bash ToolResult from history.
            Event::Command { .. } => {}
            Event::Completions {
                kind,
                prefix,
                request_id,
                context,
                entries,
            } => {
                let snapshot = self.completion_snapshot();
                if kind == CompletionKind::Feature
                    && context.is_none()
                    && self.pending_feature_edit.as_ref().is_some_and(|edit| {
                        edit.invocation.name == prefix
                            && request_id.as_ref() == Some(&edit.request_id)
                            && edit.snapshot == snapshot
                    })
                {
                    let invocation = self.pending_feature_edit.take().unwrap().invocation;
                    if let Some(descriptor) = entries
                        .iter()
                        .filter_map(|entry| entry.invocation.as_ref())
                        .find(|descriptor| {
                            descriptor.identity == invocation.identity
                                && descriptor.validate().is_ok()
                        })
                    {
                        self.selected_feature_invocations
                            .insert(descriptor.identity.to_string(), descriptor.clone());
                        self.input
                            .edit_feature_invocation(&invocation.invocation_id, descriptor.clone());
                    } else {
                        self.push_error("Feature invocation is unknown or disabled; its typed chip was preserved.");
                    }
                    return self.refresh_completion();
                }
                // Apply only if the popup is still on the same request context;
                // stale argument/provider replies cannot cross invocation scope.
                if let Some(state) = self.completion.as_mut()
                    && request_id.as_ref() == Some(&state.request_id)
                    && state.snapshot == snapshot
                    && state.kind == kind
                    && state.prefix == prefix
                    && state.context == context
                {
                    let mut entries = entries;
                    if kind == CompletionKind::Feature {
                        let mut expanded = Vec::new();
                        for entry in entries {
                            if let Some(descriptor) = &entry.invocation
                                && entry.value == descriptor.name
                                && descriptor.validate().is_ok()
                            {
                                for name in std::iter::once(&descriptor.name)
                                    .chain(descriptor.aliases.iter())
                                    .filter(|name| name.starts_with(&prefix))
                                {
                                    expanded.push(CompletionEntry {
                                        value: name.clone(),
                                        ..entry.clone()
                                    });
                                }
                            } else {
                                expanded.push(entry);
                            }
                        }
                        entries = expanded;
                    } else if kind == CompletionKind::FeatureArgument
                        && prefix.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                        })
                        && self.input.can_complete_argument_name(state.prefix_start)
                        && let Some(descriptor) = context.as_ref().and_then(|context| {
                            self.selected_feature_invocations
                                .get(&context.invocation.to_string())
                        })
                    {
                        for argument in &descriptor.arguments {
                            let value = format!("{}=", argument.name);
                            if argument.name.starts_with(&prefix)
                                && !entries.iter().any(|entry| entry.value == value)
                            {
                                entries.push(CompletionEntry {
                                    value,
                                    description: argument.description.clone(),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                    state.entries = entries;
                    state.selected = 0;
                }
            }
            Event::RewindTargets {
                head_entries,
                targets,
            } => {
                if self.rewind_request_pending {
                    self.rewind_request_pending = false;
                    self.rewind_picker = Some(RewindPickerState::new(head_entries, targets));
                }
            }
            Event::RewindApplied {
                session,
                input,
                summary,
            } => {
                self.restore_rewind_snapshot(&session);
                self.rewind_refresh_fence = true;
                let restored_composer = if self.input.is_empty() {
                    self.input.replace_with_segments(&input);
                    true
                } else {
                    false
                };
                self.completion = None;
                self.close_rewind_picker();
                self.reset_run_state();
                let mut message = if restored_composer {
                    format!(
                        "Rewound session: discarded {} log entries; restored selected input to composer.",
                        summary.discarded_entries
                    )
                } else {
                    format!(
                        "Rewound session: discarded {} log entries. Rewind applied; composer not overwritten because it was not empty.",
                        summary.discarded_entries
                    )
                };
                if summary.tool_side_effect_warning {
                    message.push_str(
                        " History suffix was discarded; tool side effects were not undone.",
                    );
                }
                self.blocks.push(Block::Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message,
                });
            }
            Event::WorkersListed { .. } | Event::WorkerRestored { .. } => {}
            Event::PeerRegistered { result } => {
                let source = result
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("this Worker");
                let peer = result
                    .get("peer")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("peer Worker");
                self.flash_actionbar_notice(
                    format!("Peer metadata registered: `{source}` ↔ `{peer}`"),
                    ActionbarNoticeLevel::Info,
                    ActionbarNoticeSource::Tui,
                    Duration::from_secs(4),
                );
            }
            Event::Shutdown => {
                self.invalidate_completion_generation();
                self.mark_orphan_compacts_incomplete();
                self.quit = true;
            }
        }
        None
    }

    fn reset_run_state(&mut self) {
        self.run_requests = 0;
        self.run_upload_tokens = 0;
        self.run_output_tokens = 0;
        self.current_tool = None;
        self.latest_llm_wait_event = None;
        self.assistant_streaming = false;
    }

    fn handle_rolled_back_run(&mut self) {
        let hint = if let Some(state) = self.pending_submit_rollback.take() {
            self.blocks
                .truncate(state.block_start.min(self.blocks.len()));
            self.turn_index = state.turn_before;
            if self.input.is_empty() {
                self.input.replace_with_segments(&state.segments);
                self.completion = None;
                self.last_rolled_back_input = None;
                "Rolled back empty assistant turn; restored your input.".to_owned()
            } else {
                let preview = rollback_input_preview(&state.text);
                self.last_rolled_back_input = Some(state.segments);
                format!(
                    "Rolled back empty assistant turn; composer was not empty, kept submitted input in backup: {preview}"
                )
            }
        } else {
            "Rolled back empty assistant turn; no local submitted input was available to restore."
                .to_owned()
        };
        self.reset_run_state();
        self.blocks.push(Block::Alert {
            level: AlertLevel::Warn,
            source: AlertSource::Worker,
            message: hint,
        });
    }

    fn apply_in_flight_snapshot(&mut self, snapshot: InFlightSnapshot) {
        let compaction = snapshot.compaction;
        for block in snapshot.blocks {
            match block {
                InFlightBlock::Text { text, finished } => {
                    self.blocks.push(Block::AssistantText { text });
                    self.assistant_streaming = !finished;
                }
                InFlightBlock::Thinking { text, finished } => {
                    let state = if finished {
                        ThinkingState::Finished { elapsed_secs: None }
                    } else {
                        ThinkingState::Streaming {
                            started_at: Instant::now(),
                        }
                    };
                    self.blocks
                        .push(Block::Thinking(ThinkingBlock { text, state }));
                }
                InFlightBlock::ToolCall {
                    id,
                    name,
                    args,
                    state,
                } => {
                    let (tool_state, arguments) = match state {
                        InFlightToolCallState::Pending => (ToolCallState::Pending, None),
                        InFlightToolCallState::StreamingArgs => (ToolCallState::Streaming, None),
                        InFlightToolCallState::Done => {
                            (ToolCallState::Executing, Some(args.clone()))
                        }
                    };
                    self.blocks.push(Block::ToolCall(ToolCallBlock {
                        id,
                        name,
                        args_stream: args,
                        arguments,
                        state: tool_state,
                        edit_snapshot: None,
                    }));
                }
            }
        }
        self.active_compaction = None;
        let _ = self.handle_worker_event(Event::CompactionProgress { compaction });
    }

    fn append_assistant_text(&mut self, text: &str) {
        if self.assistant_streaming {
            if let Some(Block::AssistantText { text: existing }) = self.blocks.last_mut() {
                existing.push_str(text);
                return;
            }
        }
        self.blocks.push(Block::AssistantText {
            text: text.to_owned(),
        });
        self.assistant_streaming = true;
    }

    /// Walk the most recently pushed blocks looking for a thinking
    /// block that's still in `Streaming`. Stops at the current
    /// `TurnHeader` to avoid latching onto a thinking block from a
    /// previous turn after it was somehow left dangling.
    fn last_streaming_thinking_mut(&mut self) -> Option<&mut ThinkingBlock> {
        for b in self.blocks.iter_mut().rev() {
            match b {
                Block::Thinking(t) if matches!(t.state, ThinkingState::Streaming { .. }) => {
                    return Some(t);
                }
                Block::TurnHeader { .. } => return None,
                _ => continue,
            }
        }
        None
    }

    fn mark_orphan_thinking_incomplete(&mut self) {
        // A turn can carry several thinking blocks; we walk all the way
        // to `TurnHeader` and convert every still-Streaming one rather
        // than breaking on the first Finished hit (which is what the
        // tool-call equivalent does, since tool calls finalize in
        // submission order).
        for b in self.blocks.iter_mut().rev() {
            match b {
                Block::Thinking(t) => {
                    if let ThinkingState::Streaming { started_at } = t.state {
                        t.state = ThinkingState::Incomplete {
                            elapsed_secs: Some(started_at.elapsed().as_secs()),
                        };
                    }
                }
                Block::TurnHeader { .. } => break,
                _ => {}
            }
        }
    }

    fn last_streaming_compact_mut(&mut self) -> Option<&mut CompactEvent> {
        for b in self.blocks.iter_mut().rev() {
            match b {
                Block::Compact(evt) if matches!(evt, CompactEvent::Streaming { .. }) => {
                    return Some(evt);
                }
                Block::Compact(_) => return None,
                _ => continue,
            }
        }
        None
    }

    pub(crate) fn mark_orphan_compacts_incomplete(&mut self) {
        for b in self.blocks.iter_mut().rev() {
            if let Block::Compact(evt) = b {
                if let CompactEvent::Streaming { started_at } = evt {
                    *evt = CompactEvent::Incomplete {
                        elapsed_secs: Some(started_at.elapsed().as_secs()),
                    };
                } else {
                    break;
                }
            }
        }
    }

    fn find_tool_call_mut(&mut self, id: &str) -> Option<&mut ToolCallBlock> {
        for b in self.blocks.iter_mut().rev() {
            if let Block::ToolCall(tc) = b
                && tc.id == id
            {
                return Some(tc);
            }
        }
        None
    }

    /// Called on `TurnEnd`: mark any tool call still in an in-progress
    /// state as `Incomplete` so the user sees something was left hanging
    /// instead of a silently-truncated block.
    fn mark_orphan_tool_calls_incomplete(&mut self) {
        for b in self.blocks.iter_mut().rev() {
            if let Block::ToolCall(tc) = b {
                if matches!(
                    tc.state,
                    ToolCallState::Pending | ToolCallState::Streaming | ToolCallState::Executing
                ) {
                    tc.state = ToolCallState::Incomplete;
                } else {
                    // Earlier tool calls in the same list are already
                    // finalized; stop walking.
                    break;
                }
            } else if matches!(b, Block::TurnHeader { .. }) {
                break;
            }
        }
    }

    pub fn is_command_mode(&self) -> bool {
        self.input_mode == CommandInputMode::Command
    }

    pub fn enter_command_mode(&mut self) {
        self.invalidate_completion_generation();
        self.input_mode = CommandInputMode::Command;
        self.completion = None;
        self.command_completion_selected = None;
        self.quit_confirm = None;
    }

    pub fn exit_command_mode(&mut self) {
        self.invalidate_completion_generation();
        self.input_mode = CommandInputMode::Composer;
        self.command_input.clear();
        self.command_completion_selected = None;
    }

    pub fn clear_command_input(&mut self) {
        self.command_input.clear();
        self.command_completion_selected = None;
    }

    pub fn command_text(&self) -> String {
        self.command_input.plain_text()
    }

    pub fn command_suggestions(&self) -> Vec<CommandCandidate> {
        self.command_registry.suggest(&self.command_text())
    }

    pub fn command_completion_selected(&self) -> Option<usize> {
        let selected = self.command_completion_selected?;
        (selected < self.command_suggestions().len()).then_some(selected)
    }

    pub fn command_completion_active(&self) -> bool {
        !self.command_suggestions().is_empty()
    }

    pub fn move_command_completion_up(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.command_completion_selected = None;
            return;
        }
        self.command_completion_selected = Some(match self.command_completion_selected() {
            Some(0) | None => len - 1,
            Some(selected) => selected - 1,
        });
    }

    pub fn move_command_completion_down(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.command_completion_selected = None;
            return;
        }
        self.command_completion_selected = Some(match self.command_completion_selected() {
            Some(selected) => (selected + 1) % len,
            None => 0,
        });
    }

    pub fn apply_command_completion(&mut self) -> CommandCompletionApply {
        let suggestions = self.command_suggestions();
        let candidate = match self.command_completion_selected() {
            Some(selected) => suggestions.get(selected),
            None if suggestions.len() == 1 => suggestions.first(),
            None if suggestions.is_empty() => return CommandCompletionApply::NoCandidates,
            None => return self.ambiguous_command_completion(),
        };

        let Some(candidate) = candidate else {
            self.command_completion_selected = None;
            return CommandCompletionApply::NoCandidates;
        };
        self.replace_command_name(candidate.name);
        self.command_completion_selected = None;
        CommandCompletionApply::Applied
    }

    pub fn submit_command_with_completion(&mut self) -> Option<Method> {
        let selected = self.command_completion_selected().is_some();
        let command_text = self.command_text();
        if command_text.trim().is_empty() && !selected {
            return self.submit_command();
        }
        if !selected && self.command_name_is_complete(&command_text) {
            return self.submit_command();
        }

        match self.apply_command_completion() {
            CommandCompletionApply::Applied | CommandCompletionApply::NoCandidates => {
                self.submit_command()
            }
            CommandCompletionApply::Ambiguous => None,
        }
    }

    fn ambiguous_command_completion(&mut self) -> CommandCompletionApply {
        self.push_command_diagnostic(
            "Ambiguous command completion; select a candidate with Up/Down or keep typing.",
        );
        CommandCompletionApply::Ambiguous
    }

    fn command_name_is_complete(&self, command_line: &str) -> bool {
        let trimmed = command_line.trim_start();
        let name = trimmed
            .find(char::is_whitespace)
            .map(|idx| &trimmed[..idx])
            .unwrap_or(trimmed);
        !name.is_empty() && self.command_registry.find(name).is_some()
    }

    fn replace_command_name(&mut self, canonical_name: &str) {
        let command_line = self.command_text();
        let leading_len = command_line.len() - command_line.trim_start().len();
        let after_leading = &command_line[leading_len..];
        let name_end = after_leading
            .find(char::is_whitespace)
            .map(|idx| leading_len + idx)
            .unwrap_or(command_line.len());
        let rest = &command_line[name_end..];

        let mut completed = String::with_capacity(command_line.len().max(canonical_name.len() + 1));
        completed.push_str(&command_line[..leading_len]);
        completed.push_str(canonical_name);
        if rest.is_empty() {
            completed.push(' ');
        } else {
            completed.push_str(rest);
        }

        self.command_input.clear();
        self.command_input.insert_str(&completed);
    }

    pub fn request_rewind_picker(&mut self) -> Option<Method> {
        self.invalidate_completion_generation();
        // Rewind is a parent Worker control surface. Bring the parent transcript
        // back into view before presenting diagnostics or the picker.
        self.selected_internal_worker_session_id = None;
        if self.rewind_submit_pending() {
            self.push_command_diagnostic(
                "rewind is already applying; wait for the Worker response",
            );
            return None;
        }
        if !self.connected {
            self.push_command_diagnostic("cannot rewind before the Worker is connected");
            return None;
        }
        if self.running {
            self.push_command_diagnostic("cannot rewind while the Worker is running");
            return None;
        }
        self.completion = None;
        self.rewind_picker = None;
        self.rewind_request_pending = true;
        Some(Method::ListRewindTargets)
    }

    pub fn close_rewind_picker(&mut self) {
        self.rewind_picker = None;
        self.rewind_request_pending = false;
    }

    pub fn cancel_rewind_picker(&mut self) {
        if self.rewind_submit_pending() {
            self.flash_actionbar_notice(
                "Rewind is applying; wait for the Worker response.",
                ActionbarNoticeLevel::Warn,
                ActionbarNoticeSource::Tui,
                Duration::from_secs(3),
            );
            return;
        }
        self.close_rewind_picker();
    }

    pub fn rewind_picker_up(&mut self) {
        if let Some(picker) = self.rewind_picker.as_mut() {
            if picker.applying || picker.targets.is_empty() {
                return;
            }
            picker.selected = if picker.selected == 0 {
                picker.targets.len() - 1
            } else {
                picker.selected - 1
            };
        }
    }

    pub fn rewind_picker_down(&mut self) {
        if let Some(picker) = self.rewind_picker.as_mut() {
            if !picker.applying && !picker.targets.is_empty() {
                picker.selected = (picker.selected + 1) % picker.targets.len();
            }
        }
    }

    pub fn submit_rewind_picker(&mut self) -> Option<Method> {
        if self.rewind_submit_pending() {
            self.push_command_diagnostic(
                "rewind is already applying; wait for the Worker response",
            );
            return None;
        }
        if self.paused {
            self.push_command_diagnostic(
                "cannot apply rewind while the Worker is paused; resume or wait for idle first",
            );
            return None;
        }
        if !self.input.is_empty() {
            self.push_command_diagnostic(
                "cannot apply rewind while composer is not empty; clear it before restoring rewind input",
            );
            return None;
        }
        let Some(picker) = self.rewind_picker.as_ref() else {
            return None;
        };
        if picker.applying {
            self.push_command_diagnostic(
                "rewind is already applying; wait for the Worker response",
            );
            return None;
        }
        let (target_id, expected_head_entries) = match picker.selected_target() {
            Some(target) if target.eligible => (target.id.clone(), target.expected_head_entries),
            Some(target) => {
                self.push_command_diagnostic(
                    target
                        .disabled_reason
                        .clone()
                        .unwrap_or_else(|| "rewind target is disabled".into()),
                );
                return None;
            }
            None => {
                self.push_command_diagnostic("no rewind target is available");
                return None;
            }
        };
        if let Some(picker) = self.rewind_picker.as_mut() {
            picker.applying = true;
        }
        Some(Method::RewindTo {
            target: target_id,
            expected_head_entries,
        })
    }

    fn command_environment(&self) -> CommandEnvironment {
        CommandEnvironment {
            connected: self.connected,
            running: self.running,
            paused: self.paused,
        }
    }

    pub fn submit_command(&mut self) -> Option<Method> {
        let command_line = self.command_text();
        let environment = self.command_environment();
        let result = self.command_registry.dispatch(&command_line, &environment);
        self.apply_command_execution(result)
    }

    fn apply_command_execution(&mut self, result: CommandExecution) -> Option<Method> {
        for diagnostic in result.diagnostics {
            self.push_command_diagnostic(diagnostic.message);
        }
        if result.clear_input {
            self.command_input.clear();
            self.command_completion_selected = None;
        }
        if result.exit_command_mode {
            self.input_mode = CommandInputMode::Composer;
            self.command_completion_selected = None;
        }
        let mut method = result.method;
        if let Some(Method::Compact { .. }) = method {
            method = Some(Method::Compact {
                command: self.next_command_envelope(),
            });
        }
        if let Some(Method::ListRewindTargets) = method.as_ref() {
            self.completion = None;
            self.rewind_picker = None;
            self.rewind_request_pending = true;
        }
        method
    }

    fn push_command_diagnostic(&mut self, message: impl Into<String>) {
        self.blocks.push(Block::Alert {
            level: AlertLevel::Warn,
            source: AlertSource::Worker,
            message: format!("TUI command: {}", message.into()),
        });
    }

    fn active_input_mut(&mut self) -> &mut InputBuffer {
        if self.is_command_mode() {
            &mut self.command_input
        } else {
            &mut self.input
        }
    }

    // Input manipulation — thin forwarders so call sites in main.rs
    // stay readable. In command mode these operate on the command line,
    // keeping the normal composer buffer intact.
    pub fn insert_char(&mut self, c: char) {
        let command_mode = self.is_command_mode();
        if !command_mode {
            self.input_history.cancel_browse();
        }
        self.active_input_mut().insert_char(c);
        if command_mode {
            self.command_completion_selected = None;
        }
    }
    pub fn insert_newline(&mut self) {
        let command_mode = self.is_command_mode();
        if !command_mode {
            self.input_history.cancel_browse();
        }
        self.active_input_mut().insert_newline();
        if command_mode {
            self.command_completion_selected = None;
        }
    }
    pub fn insert_paste(&mut self, content: String) {
        if self.is_command_mode() {
            self.command_input.insert_str(&content);
            self.command_completion_selected = None;
        } else {
            self.input_history.cancel_browse();
            self.input.insert_paste(content);
        }
    }
    pub fn delete_char_before(&mut self) {
        let command_mode = self.is_command_mode();
        if !command_mode {
            self.input_history.cancel_browse();
        }
        self.active_input_mut().delete_before();
        if command_mode {
            self.command_completion_selected = None;
        }
    }
    pub fn delete_char_after(&mut self) {
        let command_mode = self.is_command_mode();
        if !command_mode {
            self.input_history.cancel_browse();
        }
        self.active_input_mut().delete_after();
        if command_mode {
            self.command_completion_selected = None;
        }
    }
    pub fn move_cursor_left(&mut self) {
        self.active_input_mut().move_left();
    }
    pub fn move_cursor_right(&mut self) {
        self.active_input_mut().move_right();
    }
    pub fn move_cursor_word_left(&mut self) {
        self.active_input_mut().move_word_left();
    }
    pub fn move_cursor_word_right(&mut self) {
        self.active_input_mut().move_word_right();
    }
    pub fn delete_word_before_cursor(&mut self) {
        let command_mode = self.is_command_mode();
        if !command_mode {
            self.input_history.cancel_browse();
        }
        self.active_input_mut().delete_word_before();
        if command_mode {
            self.command_completion_selected = None;
        }
    }
    pub fn move_cursor_start(&mut self) {
        self.active_input_mut().move_start();
    }
    pub fn move_cursor_home(&mut self) {
        self.active_input_mut().move_home();
    }
    pub fn move_cursor_end(&mut self) {
        self.active_input_mut().move_end();
    }
    pub fn move_cursor_up(&mut self) {
        self.active_input_mut().move_up();
    }
    pub fn move_cursor_down(&mut self) {
        self.active_input_mut().move_down();
    }

    /// Reset the block list and replay a connect-time `Event::Snapshot`.
    ///
    /// Walks the session-log entries in commit order, expanding each
    /// LogEntry variant into the same blocks live events would have
    /// produced. Followed by `Event::Entry` updates for anything
    /// committed after the snapshot.
    fn replace_internal_worker_snapshots(&mut self, snapshots: Vec<InternalWorkerSnapshot>) {
        let mode = self.mode;
        let mut previous = std::mem::take(&mut self.internal_workers);
        self.internal_workers = snapshots
            .into_iter()
            .map(|snapshot| {
                if let Some(index) = previous
                    .iter()
                    .position(|view| view.worker.session_id == snapshot.worker.session_id)
                {
                    let view = previous.remove(index);
                    Self::update_internal_worker_view_from_snapshot(view, snapshot, mode)
                } else {
                    Self::internal_worker_view_from_snapshot(snapshot, mode)
                }
            })
            .collect();
        if self.selected_internal_worker_index().is_none() {
            self.selected_internal_worker_session_id = None;
        }
        self.removed_internal_workers.clear();
    }

    fn update_internal_worker_view_from_snapshot(
        mut previous: InternalWorkerView,
        snapshot: InternalWorkerSnapshot,
        mode: Mode,
    ) -> InternalWorkerView {
        let mut refreshed = Self::internal_worker_view_from_snapshot(snapshot, mode);
        Self::transfer_worker_view_state(&mut previous.app, &mut refreshed.app);
        refreshed
    }

    fn transfer_worker_view_state(previous: &mut App, refreshed: &mut App) {
        refreshed.scroll = std::mem::take(&mut previous.scroll);
        refreshed.text_selection = std::mem::take(&mut previous.text_selection);
        refreshed.task_pane_scroll = previous.task_pane_scroll;
        refreshed.selected_internal_worker_session_id =
            previous.selected_internal_worker_session_id.take();

        let mut previous_children = std::mem::take(&mut previous.internal_workers);
        for child in &mut refreshed.internal_workers {
            if let Some(index) = previous_children
                .iter()
                .position(|old| old.worker.session_id == child.worker.session_id)
            {
                let mut old = previous_children.remove(index);
                Self::transfer_worker_view_state(&mut old.app, &mut child.app);
            }
        }
        if refreshed.selected_internal_worker_index().is_none() {
            refreshed.selected_internal_worker_session_id = None;
        }
    }

    fn internal_worker_view_from_snapshot(
        snapshot: InternalWorkerSnapshot,
        mode: Mode,
    ) -> InternalWorkerView {
        let mut app = App::new(snapshot.worker.name.clone());
        app.mode = mode;
        if let Some(greeting) = snapshot.greeting {
            app.restore_snapshot(&snapshot.session, greeting, snapshot.in_flight);
        } else {
            app.restore_session(&snapshot.session, None);
            app.apply_in_flight_snapshot(snapshot.in_flight);
        }
        app.set_worker_status(snapshot.status);
        if let Some(error) = snapshot.error {
            let _ = app.handle_worker_event(Event::Error {
                code: protocol::ErrorCode::Internal,
                message: error,
            });
        }
        app.replace_internal_worker_snapshots(snapshot.internal_workers);
        InternalWorkerView {
            worker: snapshot.worker,
            revision: snapshot.revision,
            app: Box::new(app),
        }
    }

    fn apply_internal_worker_event(
        &mut self,
        worker: InternalWorkerRef,
        revision: u64,
        event: Event,
    ) {
        if self
            .removed_internal_workers
            .contains_key(&worker.session_id)
        {
            return;
        }
        let index = self
            .internal_workers
            .iter()
            .position(|candidate| candidate.worker.session_id == worker.session_id);
        let target = if let Some(index) = index {
            &mut self.internal_workers[index]
        } else {
            let mut app = App::new(worker.name.clone());
            app.mode = self.mode;
            self.internal_workers.push(InternalWorkerView {
                worker: worker.clone(),
                revision: 0,
                app: Box::new(app),
            });
            self.internal_workers.last_mut().unwrap()
        };
        if revision <= target.revision {
            return;
        }
        target.worker = worker;
        target.revision = revision;
        let _ = target.app.handle_worker_event(event);
    }

    fn remove_internal_worker(&mut self, worker: InternalWorkerRef, revision: u64) {
        self.invalidate_completion_generation();
        let session_id = worker.session_id;
        let Some(index) = self
            .internal_workers
            .iter()
            .position(|candidate| candidate.worker.session_id == session_id)
        else {
            if self.selected_internal_worker_session_id.as_deref() == Some(session_id.as_str()) {
                self.selected_internal_worker_session_id = None;
            }
            self.removed_internal_workers
                .entry(session_id)
                .and_modify(|current| *current = (*current).max(revision))
                .or_insert(revision);
            return;
        };
        if revision <= self.internal_workers[index].revision {
            return;
        }
        self.internal_workers.remove(index);
        if self.selected_internal_worker_session_id.as_deref() == Some(session_id.as_str()) {
            self.selected_internal_worker_session_id = None;
        }
        self.removed_internal_workers.insert(session_id, revision);
    }

    fn restore_snapshot(
        &mut self,
        session: &protocol::SessionSnapshot,
        greeting: protocol::Greeting,
        in_flight: InFlightSnapshot,
    ) {
        self.greeting = Some(greeting.clone());
        self.context_window = greeting.context_window;
        let context_usage = greeting.context_usage.or_else(|| {
            (greeting.context_window > 0).then_some(protocol::ContextUsage {
                tokens: greeting.context_tokens,
                source: protocol::ContextTokenSource::Estimated,
            })
        });
        self.session_context_tokens = context_usage.map(|usage| usage.tokens).unwrap_or_default();
        self.session_context_source = context_usage.map(|usage| usage.source);
        self.restore_session(session, Some(greeting));
        self.apply_in_flight_snapshot(in_flight);
    }

    /// Restore after a successful destructive rewind. The Worker's
    /// `RewindApplied` event already contains the authoritative post-rewind
    /// session tail; always clear/replay from it even if this TUI instance has
    /// somehow lost connect-time greeting metadata. Skipping the restore in
    /// that case would leave old post-target output visible after success.
    fn restore_rewind_snapshot(&mut self, session: &protocol::SessionSnapshot) {
        let greeting = self.greeting.clone().or_else(|| {
            self.blocks.iter().find_map(|b| match b {
                Block::Greeting(g) => Some(g.clone()),
                _ => None,
            })
        });
        let greeting = greeting.map(|mut greeting| {
            greeting.context_tokens = 0;
            greeting.context_usage = None;
            greeting
        });
        if let Some(greeting) = greeting.clone() {
            self.greeting = Some(greeting.clone());
            self.context_window = greeting.context_window;
            self.session_context_tokens = 0;
            self.session_context_source = None;
        }
        let missing_greeting = greeting.is_none();
        self.restore_session(session, greeting);
        if missing_greeting {
            self.blocks.push(Block::Alert {
                level: AlertLevel::Warn,
                source: AlertSource::Worker,
                message: "Rewind applied, but greeting metadata was unavailable; restored the session tail without the header.".to_owned(),
            });
        }
    }

    fn restore_session(
        &mut self,
        session: &protocol::SessionSnapshot,
        greeting: Option<protocol::Greeting>,
    ) {
        self.invalidate_completion_generation();
        self.run_error_messages.clear();
        self.turn_index = 0;
        self.blocks.clear();
        self.cache = FileCache::new();
        self.task_store = TaskStore::new();
        self.task_pane_scroll = 0;
        if let Some(greeting) = greeting {
            self.blocks.push(Block::Greeting(greeting));
        }
        self.assistant_streaming = false;

        for entry in &session.entries {
            use protocol::{SessionContentPart, SessionMessageRole, SessionSnapshotEntryData};
            match &entry.data {
                SessionSnapshotEntryData::UserInput { segments } => {
                    self.turn_index += 1;
                    self.blocks.push(Block::TurnHeader {
                        turn: self.turn_index,
                    });
                    if !segments.is_empty() {
                        self.blocks.push(Block::UserMessage {
                            segments: segments.clone(),
                        });
                    }
                }
                SessionSnapshotEntryData::Message { role, content } => {
                    let role = match role {
                        SessionMessageRole::User => agen::Role::User,
                        SessionMessageRole::Assistant => agen::Role::Assistant,
                    };
                    let item = agen::Item::Message {
                        id: None,
                        role,
                        content: content
                            .iter()
                            .map(|part| match part {
                                SessionContentPart::Text { text } => {
                                    agen::ContentPart::Text { text: text.clone() }
                                }
                                SessionContentPart::Refusal { refusal } => {
                                    agen::ContentPart::Refusal {
                                        refusal: refusal.clone(),
                                    }
                                }
                            })
                            .collect(),
                        status: None,
                    };
                    let value = serde_json::to_value(item).expect("Item is Serialize");
                    self.push_history_item(&value);
                }
                SessionSnapshotEntryData::ToolCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    let item =
                        agen::Item::tool_call(call_id.clone(), name.clone(), arguments.clone());
                    let value = serde_json::to_value(item).expect("Item is Serialize");
                    self.push_history_item(&value);
                }
                SessionSnapshotEntryData::ToolResult {
                    call_id,
                    summary,
                    content,
                    is_error,
                    ..
                } => {
                    let item = agen::Item::tool_result_item(
                        call_id.clone(),
                        summary.clone(),
                        content.clone(),
                        *is_error,
                    );
                    let value = serde_json::to_value(item).expect("Item is Serialize");
                    self.push_history_item(&value);
                }
                SessionSnapshotEntryData::SystemItem { data, .. } => {
                    if let Some(data) = data {
                        self.apply_system_item(data);
                    }
                }
                SessionSnapshotEntryData::RunYielded { .. }
                | SessionSnapshotEntryData::RunResumed { .. }
                | SessionSnapshotEntryData::RunCancelled => {
                    // Durable logical-Run transitions are intentionally not
                    // presentation rows and never restore runtime progress.
                }
                SessionSnapshotEntryData::RunError { message, .. } => {
                    self.push_run_error(message.clone());
                }
            }
        }
        self.mark_orphan_tool_calls_incomplete_pass();
    }

    /// Dispatch one `SystemItem` JSON value into the appropriate block.
    ///
    /// Kind-based routing replaces the old free-text `[Notification]` /
    /// `[File: …]` parsing path: each kind maps directly to a typed
    /// block (`Block::Notify`, `Block::WorkerEvent`, …).
    fn apply_system_item(&mut self, value: &serde_json::Value) {
        let Ok(item) = serde_json::from_value::<session_store::SystemItem>(value.clone()) else {
            // Unknown / forward-compat shape: fall back to rendering the
            // raw text payload (if any) as a generic system message.
            if let Some(text) = value.get("body").and_then(|b| b.as_str()) {
                self.task_store.apply_system_message_text(text);
                self.blocks.push(Block::SystemMessage {
                    text: text.to_owned(),
                });
            }
            return;
        };
        match item {
            session_store::SystemItem::Notification { message, .. } => {
                self.blocks.push(Block::Notify { message });
            }
            session_store::SystemItem::WorkerEvent { event, .. } => {
                self.blocks.push(Block::WorkerEvent { event });
            }
            session_store::SystemItem::FeatureInvocationResult { body, .. }
            | session_store::SystemItem::FileAttachment { body, .. }
            | session_store::SystemItem::SkillActivation { body, .. }
            | session_store::SystemItem::ResidentSummaryRefresh { body, .. }
            | session_store::SystemItem::SubjectBehaviorRefresh { body, .. }
            | session_store::SystemItem::Interrupt { body, .. } => {
                self.task_store.apply_system_message_text(&body);
                self.blocks.push(Block::SystemMessage { text: body });
            }
            session_store::SystemItem::TaskReminder { body, .. } => {
                self.task_store.apply_system_message_text(&body);
                self.blocks.push(Block::TaskReminder { text: body });
            }
            session_store::SystemItem::LegacyIgnored { .. } => {}
            session_store::SystemItem::LegacyKnowledgeIgnored { .. } => {}
        }
    }

    /// Sweep all current tool-call blocks: any that never resolved into
    /// a Done / Error state get marked Incomplete. Called after a
    /// snapshot replay so dangling in-flight tool calls in the seed
    /// log match live semantics.
    fn mark_orphan_tool_calls_incomplete_pass(&mut self) {
        for b in self.blocks.iter_mut() {
            if let Block::ToolCall(tc) = b
                && matches!(
                    tc.state,
                    ToolCallState::Executing | ToolCallState::Pending | ToolCallState::Streaming
                )
            {
                tc.state = ToolCallState::Incomplete;
            }
        }
    }
}

fn event_is_stale_after_rewind(event: &Event) -> bool {
    matches!(
        event,
        Event::Alert(_)
            | Event::MemoryWorker(_)
            | Event::CompactStart { .. }
            | Event::CompactDone { .. }
            | Event::CompactFailed { .. }
            | Event::SegmentRotated { .. }
            | Event::UserMessage { .. }
            | Event::SystemItem { .. }
            | Event::TurnStart { .. }
            | Event::InvokeStart { .. }
            | Event::LlmCallStart { .. }
            | Event::LlmCallEnd { .. }
            | Event::LlmRetry { .. }
            | Event::LlmContinuation { .. }
            | Event::TextDelta { .. }
            | Event::TextDone { .. }
            | Event::ThinkingStart
            | Event::ThinkingDelta { .. }
            | Event::ThinkingDone { .. }
            | Event::ToolCallStart { .. }
            | Event::ToolCallArgsDelta { .. }
            | Event::ToolCallDone { .. }
            | Event::ToolResult { .. }
            | Event::Usage { .. }
            | Event::TurnEnd { .. }
            | Event::RunEnd { .. }
    )
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn run_failure_messages_match(left: &str, right: &str) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && (left == right || left.ends_with(right) || right.ends_with(left))
}

fn fmt_millis(ms: u64) -> String {
    if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
fn public_session(values: Vec<serde_json::Value>) -> protocol::SessionSnapshot {
    let entries = values
        .into_iter()
        .map(|value| serde_json::from_value(value).expect("LogEntry deserializes"))
        .collect::<Vec<session_store::LogEntry>>();
    session_store::public_snapshot::project_current_session_snapshot(&entries)
}

fn message_text(item: &serde_json::Value) -> String {
    item["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Strip the `cat -n` line-number gutter that the Read tool prepends to
/// its output (one `"{n:>6}\t{content}"` per line) and return the raw
/// file body. Lines that don't match the pattern are kept verbatim, so
/// unrelated payloads pass through unharmed.
fn strip_cat_n_prefix(formatted: &str) -> String {
    let mut out = String::with_capacity(formatted.len());
    let mut first = true;
    for line in formatted.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        match line.split_once('\t') {
            Some((prefix, rest)) if prefix.trim().chars().all(|c| c.is_ascii_digit()) => {
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

fn rollback_input_preview(text: &str) -> String {
    const MAX_CHARS: usize = 80;
    let mut one_line = text.replace('\n', "⏎");
    if one_line.chars().count() > MAX_CHARS {
        one_line = one_line.chars().take(MAX_CHARS).collect::<String>();
        one_line.push('…');
    }
    one_line
}

pub fn alert_source_label(source: AlertSource) -> &'static str {
    match source {
        AlertSource::Worker => "worker",
        AlertSource::Engine => "engine",
        AlertSource::Compactor => "compactor",
        AlertSource::AgentsMd => "AGENTS.md",
    }
}

#[cfg(test)]
mod llm_wait_event_tests {
    use super::*;

    #[test]
    fn llm_retry_updates_and_progress_clears_transient_status() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::LlmRetry {
            llm_call: 2,
            failed_attempt: 1,
            max_attempts: 4,
            wait_ms: 1_200,
            elapsed_ms: 50,
            status: Some(504),
            error: "gateway timeout".into(),
        });
        assert_eq!(
            app.latest_llm_wait_event.as_deref(),
            Some("retrying LLM request after HTTP 504 (attempt 2/4 in 1.2s)")
        );

        app.handle_worker_event(Event::TextDelta { text: "ok".into() });
        assert!(app.latest_llm_wait_event.is_none());
    }

    #[test]
    fn llm_continuation_updates_transient_status() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::LlmContinuation {
            llm_call: 3,
            attempt: 1,
            max_attempts: 3,
            reason: "SSE parse error: closed".into(),
        });
        assert_eq!(
            app.latest_llm_wait_event.as_deref(),
            Some("LLM stream interrupted; continuing generation (1/3): SSE parse error: closed")
        );
    }
}

#[cfg(test)]
mod actionbar_notice_tests {
    use super::*;

    #[test]
    fn actionbar_notice_expires_from_injected_time_source() {
        let mut app = App::new("test".into());
        let now = Instant::now();
        let duration = Duration::from_secs(2);

        app.flash_actionbar_notice_at(
            "Worker keeps running",
            ActionbarNoticeLevel::Warn,
            ActionbarNoticeSource::Tui,
            now,
            duration,
        );

        let notice = app.current_actionbar_notice(now).expect("notice is active");
        assert_eq!(notice.text, "Worker keeps running");
        assert_eq!(notice.level, ActionbarNoticeLevel::Warn);
        assert_eq!(notice.source, ActionbarNoticeSource::Tui);
        assert_eq!(notice.expires_at, now + duration);
        assert!(
            app.current_actionbar_notice(now + duration - Duration::from_millis(1))
                .is_some()
        );
        assert!(app.current_actionbar_notice(now + duration).is_none());

        app.clear_expired_actionbar_notice(now + duration);
        assert!(app.current_actionbar_notice(now).is_none());
    }
}

#[cfg(test)]
mod rewind_refresh_tests {
    use super::*;

    #[test]
    fn rewind_applied_replaces_old_live_tail_and_restores_input() {
        let mut app = app_with_rewind_picker();
        app.greeting = Some(greeting());
        app.blocks.push(Block::Greeting(greeting()));
        app.blocks.push(Block::AssistantText {
            text: "old post-target output".into(),
        });

        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: vec![],
            },
            input: vec![Segment::text("selected rewind input")],
            summary: summary(3),
        });

        assert!(app.rewind_picker.is_none());
        assert!(!blocks_contain(&app, "old post-target output"));
        assert_eq!(composer_text(&app), "selected rewind input");
        assert!(blocks_contain(&app, "Rewound session"));
        assert_eq!(app.session_context_tokens, 0);
        assert_eq!(app.session_context_source, None);
        assert_eq!(app.greeting.as_ref().and_then(|g| g.context_usage), None);
    }

    #[test]
    fn rewind_applied_clears_old_tail_even_when_greeting_is_missing() {
        let mut app = app_with_rewind_picker();
        app.blocks.push(Block::AssistantText {
            text: "old live tail without greeting".into(),
        });

        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: vec![],
            },
            input: vec![Segment::text("rewound input")],
            summary: summary(1),
        });

        assert!(!blocks_contain(&app, "old live tail without greeting"));
        assert!(blocks_contain(&app, "greeting metadata was unavailable"));
        assert_eq!(composer_text(&app), "rewound input");
    }

    #[test]
    fn pending_rewind_submit_suppresses_duplicate_enter_and_failure_preserves_display() {
        let mut app = app_with_rewind_picker();
        app.blocks.push(Block::AssistantText {
            text: "still-visible old display on failure".into(),
        });

        let first = app.submit_rewind_picker();
        assert!(matches!(first, Some(Method::RewindTo { .. })));
        assert!(app.rewind_picker.as_ref().unwrap().applying);
        assert!(app.submit_rewind_picker().is_none());

        app.handle_worker_event(Event::Error {
            code: ErrorCode::InvalidRequest,
            message: "stale rewind target".into(),
        });

        assert!(!app.rewind_picker.as_ref().unwrap().applying);
        assert!(blocks_contain(&app, "still-visible old display on failure"));
        let notice = app.current_actionbar_notice(Instant::now()).unwrap();
        assert!(notice.text.contains("Rewind failed"));
    }

    #[test]
    fn stale_live_update_after_success_does_not_repollute_restored_display() {
        let mut app = app_with_rewind_picker();
        app.greeting = Some(greeting());
        app.blocks.push(Block::Greeting(greeting()));
        app.blocks.push(Block::AssistantText {
            text: "old tail before rewind".into(),
        });

        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: vec![],
            },
            input: vec![Segment::text("rewound input")],
            summary: summary(2),
        });
        app.handle_worker_event(Event::TextDelta {
            text: "stale tail after rewind".into(),
        });
        assert!(!blocks_contain(&app, "stale tail after rewind"));

        app.handle_worker_event(Event::WorkerState {
            snapshot: WorkerStatus::Idle.into(),
        });
        app.handle_worker_event(Event::TextDelta {
            text: "new live tail after status".into(),
        });
        assert!(blocks_contain(&app, "new live tail after status"));
    }

    fn app_with_rewind_picker() -> App {
        let mut app = App::new("test".into());
        app.connected = true;
        app.rewind_picker = Some(RewindPickerState::new(
            5,
            vec![RewindTarget {
                id: protocol::RewindTargetId {
                    segment_id: uuid::Uuid::nil(),
                    user_input_entry_index: 1,
                },
                expected_head_entries: 5,
                truncate_entries: 2,
                turn_index: 1,
                timestamp_ms: None,
                preview: "rewind target".into(),
                eligible: true,
                disabled_reason: None,
                warning: None,
            }],
        ));
        app
    }

    fn greeting() -> protocol::Greeting {
        protocol::Greeting {
            worker_name: "test".into(),
            cwd: "/tmp".into(),
            provider: "mock".into(),
            model: "mock".into(),
            scope_summary: "scope".into(),
            tools: vec![],
            context_window: 100,
            context_tokens: 10,
            reasoning: None,
            context_usage: None,
        }
    }

    fn summary(discarded_entries: usize) -> protocol::RewindSummary {
        protocol::RewindSummary {
            truncated_to_entries: 1,
            discarded_entries,
            tool_side_effect_warning: false,
        }
    }

    fn blocks_contain(app: &App, needle: &str) -> bool {
        app.blocks.iter().any(|block| match block {
            Block::AssistantText { text }
            | Block::SystemMessage { text }
            | Block::TaskReminder { text }
            | Block::Alert { message: text, .. } => text.contains(needle),
            Block::UserMessage { segments } => Segment::flatten_to_text(segments).contains(needle),
            _ => false,
        })
    }

    fn composer_text(app: &App) -> String {
        Segment::flatten_to_text(&app.input.submit_segments())
    }
}

#[cfg(test)]
mod composer_history_persistence_tests {
    use super::*;
    use crate::composer_history::{COMPOSER_INPUT_HISTORY_LIMIT, ComposerHistoryStore};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn recall_history_survives_reload_for_same_workspace() {
        let data_dir = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace.path());
        let mut app = App::new_with_input_history_store("test".into(), store.clone());
        submit_text(&mut app, "first synthetic entry");
        submit_text(&mut app, "second synthetic entry");

        let mut reloaded = App::new_with_input_history_store("test".into(), store);
        assert!(reloaded.browse_input_history_older());
        assert_eq!(input_text(&reloaded), "second synthetic entry");
        assert!(reloaded.browse_input_history_older());
        assert_eq!(input_text(&reloaded), "first synthetic entry");
    }

    #[test]
    fn recall_histories_are_separated_by_workspace() {
        let data_dir = TempDir::new().unwrap();
        let workspace_a = TempDir::new().unwrap();
        let workspace_b = TempDir::new().unwrap();
        let store_a = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace_a.path());
        let store_b = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace_b.path());
        let mut app_a = App::new_with_input_history_store("test".into(), store_a.clone());
        let mut app_b = App::new_with_input_history_store("test".into(), store_b.clone());

        submit_text(&mut app_a, "workspace a synthetic entry");
        submit_text(&mut app_b, "workspace b synthetic entry");

        let mut reloaded_a = App::new_with_input_history_store("test".into(), store_a);
        let mut reloaded_b = App::new_with_input_history_store("test".into(), store_b);
        assert!(reloaded_a.browse_input_history_older());
        assert!(reloaded_b.browse_input_history_older());
        assert_eq!(input_text(&reloaded_a), "workspace a synthetic entry");
        assert_eq!(input_text(&reloaded_b), "workspace b synthetic entry");
    }

    #[test]
    fn persistence_keeps_typed_segments_instead_of_flattening() {
        let data_dir = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace.path());
        let mut app = App::new_with_input_history_store("test".into(), store.clone());
        app.input.replace_with_segments(&[
            Segment::text("inspect "),
            Segment::FileRef {
                path: "src/lib.rs".into(),
            },
        ]);
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));

        let mut reloaded = App::new_with_input_history_store("test".into(), store);
        assert!(reloaded.browse_input_history_older());
        let segments = reloaded.input.submit_segments();
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], Segment::Text { content } if content == "inspect "));
        assert!(matches!(&segments[1], Segment::FileRef { path } if path == "src/lib.rs"));
    }

    #[test]
    fn persistence_bounds_history_and_suppresses_consecutive_duplicates() {
        let data_dir = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace.path());
        let mut app = App::new_with_input_history_store("test".into(), store.clone());
        submit_text(&mut app, "duplicate synthetic entry");
        submit_text(&mut app, "duplicate synthetic entry");
        for i in 0..COMPOSER_INPUT_HISTORY_LIMIT + 5 {
            submit_text(&mut app, &format!("bounded synthetic entry {i}"));
        }

        let reloaded = App::new_with_input_history_store("test".into(), store);
        assert_eq!(
            reloaded.input_history.entries.len(),
            COMPOSER_INPUT_HISTORY_LIMIT
        );
        assert_eq!(
            reloaded.input_history.entries.front(),
            Some(&vec![Segment::text("bounded synthetic entry 5")])
        );
        assert_eq!(
            reloaded.input_history.entries.back(),
            Some(&vec![Segment::text("bounded synthetic entry 34")])
        );
    }

    #[test]
    fn corrupt_history_file_falls_back_to_empty_with_bounded_warning() {
        let data_dir = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace.path());
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(store.path(), b"not json").unwrap();

        let app = App::new_with_input_history_store("test".into(), store);
        assert_eq!(app.input_history.entries.len(), 0);
        let notice = app.current_actionbar_notice(Instant::now()).unwrap();
        assert_eq!(notice.level, ActionbarNoticeLevel::Warn);
        assert!(
            notice
                .text
                .contains("Could not load saved composer input history")
        );
        assert!(!notice.text.contains("not json"));
    }

    #[test]
    fn persistence_does_not_write_workspace_yoi_directory() {
        let data_dir = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace.path());
        let mut app = App::new_with_input_history_store("test".into(), store);
        submit_text(&mut app, "synthetic entry outside workspace yoi");

        assert!(
            data_dir
                .path()
                .join("client")
                .join("composer-history")
                .exists()
        );
        assert!(!data_dir.path().join("composer-history").exists());
        assert!(!workspace.path().join(".yoi").exists());
    }

    fn submit_text(app: &mut App, text: &str) -> Vec<Segment> {
        for c in text.chars() {
            app.insert_char(c);
        }
        match app.submit_input() {
            Some(Method::Submit { input, .. }) => input,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    fn input_text(app: &App) -> String {
        Segment::flatten_to_text(&app.input.submit_segments())
    }
}

#[cfg(test)]
mod completion_flow_tests {
    use super::*;

    fn annotated(item: agen::Item) -> session_store::LoggedHistoryEntry {
        session_store::LoggedHistoryEntry {
            item: session_store::LoggedItem::from(item),
            metadata: session_store::LoggedSessionHistoryMetadata {
                entry_id: session_store::LoggedSessionHistoryEntryId::new(),
                origin: session_store::LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        }
    }

    #[test]
    fn typing_at_creates_completion_state_and_emits_query() {
        let mut app = App::new("test".into());
        app.insert_char('@');
        let method = app.refresh_completion();
        match method {
            Some(Method::ListCompletions { kind, prefix, .. }) => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
        assert!(app.completion.is_some());
    }

    #[test]
    fn appending_to_token_emits_updated_query() {
        let mut app = App::new("test".into());
        app.insert_char('@');
        let _ = app.refresh_completion();
        app.insert_char('s');
        let method = app.refresh_completion();
        match method {
            Some(Method::ListCompletions { kind, prefix, .. }) => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "s");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
    }

    #[test]
    fn stale_completion_reply_for_prior_prefix_is_ignored() {
        let mut app = App::new("test".into());
        for c in "@a".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.insert_char('b');
        let _ = app.refresh_completion();
        app.handle_worker_event(Event::Completions {
            request_id: app.completion_request_id(),
            kind: CompletionKind::File,
            prefix: "a".into(),
            context: None,
            entries: vec![CompletionEntry {
                value: "alpha".into(),
                ..CompletionEntry::default()
            }],
        });
        assert_eq!(app.completion.as_ref().unwrap().prefix, "ab");
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
    }

    #[test]
    fn space_after_token_clears_completion_state() {
        let mut app = App::new("test".into());
        for c in "@x".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        assert!(app.completion.is_some());
        app.insert_char(' ');
        let method = app.refresh_completion();
        assert!(method.is_none());
        assert!(app.completion.is_none());
    }

    #[test]
    fn tab_inserts_entry_value_as_text_for_file() {
        let mut app = App::new("test".into());
        for c in "@s".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
            ..CompletionEntry::default()
        }];
        // Tab path: text inserted, popup re-triggered with new prefix
        // (still File kind since the typed range stays after `@`).
        let _ = app.apply_completion_text();
        // The input now reads `@src/main.rs` as plain Char atoms; no
        // chip yet.
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@src/main.rs"));
        assert!(app.completion.is_some());
    }

    #[test]
    fn tab_appends_trailing_slash_for_directory() {
        let mut app = App::new("test".into());
        for c in "@cr".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
            ..CompletionEntry::default()
        }];
        let _ = app.apply_completion_text();
        // Typed prefix advances to `crates/` so the next query can
        // descend into the directory.
        assert_eq!(app.completion.as_ref().unwrap().prefix, "crates/");
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@crates/"));
    }

    #[test]
    fn space_chipifies_on_exact_match() {
        let mut app = App::new("test".into());
        for c in "@src/main.rs".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
            ..CompletionEntry::default()
        }];
        assert!(app.chipify_completion_if_exact_match());
        assert!(app.completion.is_none());
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "src/main.rs"));
    }

    #[test]
    fn space_does_not_chipify_on_partial_match() {
        let mut app = App::new("test".into());
        for c in "@s".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "src/main.rs".into(),
            is_dir: false,
            ..CompletionEntry::default()
        }];
        // typed = "s", expected = "src/main.rs" → no match, no chip.
        assert!(!app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@s"));
    }

    #[test]
    fn space_chipifies_directory_with_or_without_trailing_slash() {
        // Slash-less typed form chipifies the directory; the chip's
        // path keeps a trailing slash so the rendered label is `@crates/`.
        let mut app = App::new("test".into());
        for c in "@crates".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
            ..CompletionEntry::default()
        }];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));

        // Slashed typed form (the shape Tab inserts) — same chip.
        let mut app = App::new("test".into());
        for c in "@crates/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
            ..CompletionEntry::default()
        }];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));
    }

    #[test]
    fn space_chipifies_directory_when_popup_shows_its_children() {
        // `@crates/` is the form Tab leaves you in after picking a
        // directory; the popup is showing the children of `crates/`.
        // Hitting space at this point should chipify `crates`, not
        // require the user to back up and remove the trailing slash.
        let mut app = App::new("test".into());
        for c in "@crates/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![
            CompletionEntry {
                value: "crates/client".into(),
                is_dir: true,
                ..CompletionEntry::default()
            },
            CompletionEntry {
                value: "crates/agen".into(),
                is_dir: true,
                ..CompletionEntry::default()
            },
        ];
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "crates/"));
    }

    #[test]
    fn enter_does_not_chipify_directory_so_drill_in_works() {
        // Enter on a selected directory entry must NOT chipify —
        // otherwise the user can never drill into the dir to see
        // its children.
        let mut app = App::new("test".into());
        for c in "@crates".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates".into(),
            is_dir: true,
            ..CompletionEntry::default()
        }];
        assert!(!app.chipify_selected_completion_if_committable());
        // Popup is still active so the caller can fall through to
        // apply_completion_text.
        assert!(app.completion.is_some());
    }

    #[test]
    fn enter_path_appends_trailing_space_after_file_chip() {
        // Mirrors the main.rs Enter handler sequence: chipify the
        // selected entry, then insert a space so the cursor is ready
        // for the next token without a manual separator.
        let mut app = App::new("test".into());
        for c in "@README.".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "README.md".into(),
            is_dir: false,
            ..CompletionEntry::default()
        }];
        assert!(app.chipify_selected_completion_if_committable());
        app.insert_char(' ');
        let segs = app.input.submit_segments();
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "README.md"));
        assert!(matches!(&segs[1], Segment::Text { content } if content == " "));
    }

    #[test]
    fn enter_chipifies_selected_file_even_when_typed_is_partial() {
        // Enter respects the selected entry: typed text may be a
        // prefix of the entry's value, but the popup-highlighted
        // file should still chipify on Enter.
        let mut app = App::new("test".into());
        for c in "@README.".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "README.md".into(),
            is_dir: false,
            ..CompletionEntry::default()
        }];
        assert!(app.chipify_selected_completion_if_committable());
        assert!(app.completion.is_none());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "README.md"));
    }

    #[test]
    fn space_does_not_chipify_drilled_state_with_unrelated_entries() {
        // Stale entries that don't live under the typed prefix should
        // not satisfy the drilled-into-directory rule.
        let mut app = App::new("test".into());
        for c in "@xyz/".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![CompletionEntry {
            value: "crates/client".into(),
            is_dir: true,
            ..CompletionEntry::default()
        }];
        assert!(!app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::Text { content } if content == "@xyz/"));
    }

    #[test]
    fn chipify_finds_match_outside_selected_index() {
        // Regression guard for the race where a stale reply leaves a
        // non-matching entry at index 0 but an entry deeper in the
        // list does match the current typed text.
        let mut app = App::new("test".into());
        for c in "@src/main.rs".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        app.completion.as_mut().unwrap().entries = vec![
            CompletionEntry {
                value: "src/main.rs.bak".into(),
                is_dir: false,
                ..CompletionEntry::default()
            },
            CompletionEntry {
                value: "src/main.rs".into(),
                is_dir: false,
                ..CompletionEntry::default()
            },
        ];
        // selected stays at 0 (the non-matching one) but find() should
        // still locate the match.
        assert!(app.chipify_completion_if_exact_match());
        let segs = app.input.submit_segments();
        assert!(matches!(&segs[0], Segment::FileRef { path } if path == "src/main.rs"));
    }

    #[test]
    fn apply_completion_text_with_no_entries_is_a_noop() {
        let mut app = App::new("test".into());
        for c in "@x".chars() {
            app.insert_char(c);
        }
        let _ = app.refresh_completion();
        // No `Event::Completions` arrived yet — entries is still empty.
        assert!(app.apply_completion_text().is_none());
        assert!(app.completion.is_some());
    }

    #[test]
    fn committed_user_message_survives_fresh_segment_rotation() {
        let mut app = App::new("test".into());
        let start = session_store::LogEntry::AnnotatedSegmentStart {
            ts: session_store::segment_log::now_millis(),
            session_id: uuid::Uuid::nil(),
            system_prompt: None,
            config: agen::llm_client::RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        };

        app.handle_worker_event(Event::SegmentRotated {
            session: public_session(vec![
                serde_json::to_value(start).expect("LogEntry is Serialize"),
            ]),
        });
        app.handle_worker_event(Event::UserMessage {
            entry_id: None,
            segments: vec![Segment::text("first persisted message")],
        });

        assert_eq!(app.turn_index, 1);
        assert!(app.blocks.iter().any(|b| matches!(
            b,
            Block::UserMessage { segments }
                if Segment::flatten_to_text(segments) == "first persisted message"
        )));
    }

    #[test]
    fn rolled_back_run_restores_input_and_removes_submit_blocks() {
        let mut app = App::new("test".into());
        let submitted = submit_text(&mut app, "please wait");
        assert_eq!(input_text(&app), "");

        app.handle_worker_event(Event::UserMessage {
            entry_id: None,
            segments: submitted,
        });
        // Simulate run-derived attachment display after the submitted user line.
        app.blocks.push(Block::SystemMessage {
            text: "[File: README.md]".into(),
        });
        app.handle_worker_event(Event::TurnStart { turn: 1 });
        app.handle_worker_event(Event::Usage {
            input_tokens: Some(100),
            output_tokens: Some(0),
            cache_read_input_tokens: Some(40),
        });
        app.handle_worker_event(Event::RunEnd {
            result: RunResult::RolledBack,
        });

        assert_eq!(input_text(&app), "please wait");
        assert_eq!(app.turn_index, 0);
        assert!(app.blocks.iter().all(|b| !matches!(
            b,
            Block::TurnHeader { .. }
                | Block::UserMessage { .. }
                | Block::SystemMessage { .. }
                | Block::TurnStats { .. }
        )));
        assert!(warning_contains(&app, "restored your input"));
        assert!(matches!(app.worker_status, WorkerStatus::Idle));
        assert!(!app.running);
        assert!(!app.paused);
        assert_eq!(app.run_requests, 0);
        assert_eq!(app.run_upload_tokens, 0);
        assert_eq!(app.run_output_tokens, 0);
        assert!(app.current_tool.is_none());
    }

    #[test]
    fn rolled_back_run_does_not_overwrite_existing_unsent_input() {
        let mut app = App::new("test".into());
        let submitted = submit_text(&mut app, "original submit");
        app.handle_worker_event(Event::UserMessage {
            entry_id: None,
            segments: submitted,
        });
        for c in "draft while running".chars() {
            app.insert_char(c);
        }

        app.handle_worker_event(Event::RunEnd {
            result: RunResult::RolledBack,
        });

        assert_eq!(input_text(&app), "draft while running");
        assert_eq!(
            Segment::flatten_to_text(app.last_rolled_back_input.as_ref().unwrap()),
            "original submit"
        );
        assert!(warning_contains(&app, "composer was not empty"));
        assert!(app.blocks.iter().all(|b| !matches!(
            b,
            Block::TurnHeader { .. } | Block::UserMessage { .. } | Block::TurnStats { .. }
        )));
    }

    #[test]
    fn non_rolled_back_run_end_keeps_submitted_blocks_and_does_not_restore_input() {
        for result in [RunResult::Paused, RunResult::Finished, RunResult::Cancelled] {
            let mut app = App::new("test".into());
            let submitted = submit_text(&mut app, "normal run");
            app.handle_worker_event(Event::UserMessage {
                entry_id: None,
                segments: submitted,
            });
            app.handle_worker_event(Event::RunEnd { result });

            assert_eq!(input_text(&app), "");
            assert!(
                app.blocks
                    .iter()
                    .any(|b| matches!(b, Block::TurnHeader { .. }))
            );
            assert!(
                app.blocks
                    .iter()
                    .any(|b| matches!(b, Block::UserMessage { .. }))
            );
            assert!(
                app.blocks
                    .iter()
                    .any(|b| matches!(b, Block::TurnStats { .. }))
            );
            assert!(!warning_contains(&app, "Rolled back empty assistant turn"));
            assert!(app.last_rolled_back_input.is_none());
        }
    }

    #[test]
    fn running_status_starts_and_stops_live_run_clock() {
        let mut app = App::new("test".into());

        app.set_worker_status(WorkerStatus::Running);
        assert!(app.run_started_at.is_some());

        app.set_worker_status(WorkerStatus::Idle);
        assert!(app.run_started_at.is_none());
    }

    #[test]
    fn running_submit_is_sent_to_the_worker_and_not_queued_locally() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Running);
        insert_text(&mut app, "queued turn");

        let method = app.submit_input();

        assert!(matches!(method, Some(Method::Submit { .. })));
        assert_eq!(app.queued_input_count(), 0);
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn pending_submission_projection_is_worker_authoritative() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::PendingSubmissionsChanged {
            pending: protocol::PendingSubmissionsSnapshot {
                revision: 3,
                notification_count: 0,
                notification_previews: vec![],
                head_id: Some("submission-1".into()),
                submissions: vec![protocol::PendingSubmissionSummary {
                    submission_id: "submission-1".into(),
                    preview: None,
                    accepted_at_ms: 7,
                    segment_count: 2,
                    byte_len: 42,
                }],
            },
        });

        assert_eq!(app.queued_input_count(), 1);
        assert_eq!(app.next_queued_input_preview(), Some("submission-1"));
        assert!(
            app.handle_worker_event(Event::RunEnd {
                result: RunResult::Finished,
            })
            .is_none()
        );
        assert_eq!(app.queued_input_count(), 1);
    }

    #[test]
    fn paused_non_empty_submit_is_sent_without_resuming() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Paused);
        insert_text(&mut app, "next turn");

        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));
        assert_eq!(app.worker_status, WorkerStatus::Paused);
        assert_eq!(app.queued_input_count(), 0);
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn paused_empty_submit_still_resumes_immediately() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Paused);

        assert!(matches!(app.submit_input(), Some(Method::Resume { .. })));
        assert_eq!(app.queued_input_count(), 0);
    }

    fn insert_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.insert_char(c);
        }
    }

    fn submit_text(app: &mut App, text: &str) -> Vec<Segment> {
        for c in text.chars() {
            app.insert_char(c);
        }
        match app.submit_input() {
            Some(Method::Submit { input, .. }) => input,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    fn input_text(app: &App) -> String {
        Segment::flatten_to_text(&app.input.submit_segments())
    }

    fn warning_contains(app: &App, needle: &str) -> bool {
        app.blocks.iter().any(|block| {
            matches!(
                block,
                Block::Alert {
                    level: AlertLevel::Warn,
                    message,
                    ..
                } if message.contains(needle)
            )
        })
    }

    #[test]
    fn snapshot_excludes_system_prompt_history_from_public_blocks() {
        let mut app = App::new("test".into());
        let session_start = session_store::LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id: uuid::Uuid::nil(),
            system_prompt: None,
            config: Default::default(),
            history: vec![annotated(agen::Item::system_message(
                "[File: src/main.rs]\nfn main() {}",
            ))],
            forked_from: None,
            compacted_from: None,
        };
        let session_start_value = serde_json::to_value(&session_start).unwrap();
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: public_session(vec![session_start_value]),
            state: test_worker_state(WorkerStatus::Running),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        assert!(matches!(app.worker_status, WorkerStatus::Running));
        assert!(app.running);
        assert_eq!(app.blocks.len(), 1);
        assert!(matches!(app.blocks.first(), Some(Block::Greeting(_))));
    }

    #[test]
    fn occurrence_events_do_not_infer_foreground_worker_state() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::TurnStart { turn: 1 });
        app.handle_worker_event(Event::InvokeStart {
            kind: protocol::InvokeKind::UserSend,
        });
        app.handle_worker_event(Event::RunEnd {
            result: RunResult::Paused,
        });
        assert_eq!(app.worker_state.state, protocol::WorkerState::Idle);
        assert_eq!(app.worker_status, WorkerStatus::Idle);

        let running = WorkerStateSnapshot {
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
            last_command_id: 0,
            last_finished_submission_request_id: None,
        };
        app.handle_worker_event(Event::WorkerState {
            snapshot: running.clone(),
        });
        app.handle_worker_event(Event::RunEnd {
            result: RunResult::Finished,
        });
        assert_eq!(app.worker_state, running);
        assert_eq!(app.worker_status, WorkerStatus::Running);
    }

    #[test]
    fn worker_state_events_and_acknowledgements_replace_full_state() {
        let mut app = App::new("test".into());
        let running = WorkerStateSnapshot {
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
            last_command_id: 2,
            last_finished_submission_request_id: Some("request-old".to_string()),
        };
        app.handle_worker_event(Event::WorkerState {
            snapshot: running.clone(),
        });
        assert_eq!(app.worker_state, running);

        let fresh_idle = WorkerStateSnapshot {
            state: protocol::WorkerState::Idle,
            last_command_id: 0,
            last_finished_submission_request_id: None,
        };
        app.handle_worker_event(Event::WorkerState {
            snapshot: fresh_idle.clone(),
        });
        assert_eq!(app.worker_state, fresh_idle);

        let paused = WorkerStateSnapshot {
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Paused,
            )),
            last_command_id: 3,
            last_finished_submission_request_id: None,
        };
        app.handle_worker_event(Event::CommandAcknowledged {
            acknowledgement: protocol::WorkerCommandAcknowledgement {
                command_id: 3,
                command: protocol::WorkerCommandKind::Pause,
                disposition: protocol::WorkerCommandDisposition::Accepted,
                state: paused.clone(),
            },
        });
        assert_eq!(app.worker_state, paused);
    }

    #[test]
    fn snapshot_replaces_live_error_with_one_durable_run_error_block() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::Error {
            code: ErrorCode::ProviderError,
            message: "provider unavailable".into(),
        });
        app.handle_worker_event(Event::WorkerState {
            snapshot: WorkerStatus::Idle.into(),
        });

        let live_errors = app
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Alert {
                    level: AlertLevel::Error,
                    source: AlertSource::Worker,
                    message,
                } => Some(message.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(live_errors, ["[ProviderError] provider unavailable"]);

        let run_errored = session_store::LogEntry::RunErrored {
            ts: 3,
            entry_id: None,
            interrupted: false,
            message: "provider unavailable".into(),
            failure: None,
        };
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: public_session(vec![serde_json::to_value(run_errored).unwrap()]),
            state: test_worker_state(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        let errors = app
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Alert {
                    level: AlertLevel::Error,
                    source: AlertSource::Worker,
                    message,
                } => Some(message.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(errors, ["provider unavailable"]);
    }

    #[test]
    fn live_durable_compaction_failure_reconciles_transient_alert_and_error() {
        let mut app = App::new("test".into());
        let message = "mid-run compaction failed: summary unavailable";
        app.handle_worker_event(Event::Alert(protocol::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Compactor,
            message: message.into(),
            timestamp_ms: 1,
        }));
        app.handle_worker_event(Event::SessionEntryCommitted {
            entry: protocol::SessionSnapshotEntry {
                entry_id: "run-failure".into(),
                timestamp: 2,
                provenance: protocol::SessionEntryProvenance::LegacyUnknown,
                derived_from: Vec::new(),
                data: protocol::SessionSnapshotEntryData::RunError {
                    message: message.into(),
                    failure: Some(protocol::RunFailureKind::Compaction),
                },
            },
        });
        app.handle_worker_event(Event::Error {
            code: ErrorCode::Internal,
            message: "summary unavailable".into(),
        });

        let errors = app
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Alert {
                    level: AlertLevel::Error,
                    message,
                    ..
                } => Some(message.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(errors, [message]);
    }

    #[test]
    fn segment_rotation_retains_live_error_across_real_segment_start() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::Error {
            code: ErrorCode::ProviderError,
            message: "provider unavailable".into(),
        });
        let segment_start = session_store::LogEntry::AnnotatedSegmentStart {
            ts: 5,
            session_id: uuid::Uuid::nil(),
            system_prompt: None,
            config: Default::default(),
            history: Vec::new(),
            forked_from: None,
            compacted_from: None,
        };
        app.handle_worker_event(Event::SegmentRotated {
            session: public_session(vec![serde_json::to_value(segment_start).unwrap()]),
        });

        let errors = app
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Alert {
                    level: AlertLevel::Error,
                    source: AlertSource::Worker,
                    message,
                } => Some(message.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(errors, ["[ProviderError] provider unavailable"]);
    }

    #[test]
    fn snapshot_in_flight_blocks_continue_with_live_deltas() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            state: test_worker_state(WorkerStatus::Running),
            in_flight: InFlightSnapshot {
                blocks: vec![
                    InFlightBlock::Thinking {
                        text: "why".into(),
                        finished: false,
                    },
                    InFlightBlock::ToolCall {
                        id: "call_1".into(),
                        name: "Read".into(),
                        args: r#"{\"file"#.into(),
                        state: InFlightToolCallState::StreamingArgs,
                    },
                    InFlightBlock::Text {
                        text: "hel".into(),
                        finished: false,
                    },
                ],
                commands: Vec::new(),
                compaction: None,
            },
            internal_workers: Vec::new(),
        });

        app.handle_worker_event(Event::TextDelta { text: "lo".into() });
        app.handle_worker_event(Event::ThinkingDelta { text: "?".into() });
        app.handle_worker_event(Event::ToolCallArgsDelta {
            id: "call_1".into(),
            json: r#"\":\"src/lib.rs\"}"#.into(),
        });

        assert!(matches!(
            app.blocks.iter().find(|block| matches!(block, Block::AssistantText { .. })),
            Some(Block::AssistantText { text }) if text == "hello"
        ));
        assert!(matches!(
            app.blocks.iter().find(|block| matches!(block, Block::Thinking(_))),
            Some(Block::Thinking(thinking)) if thinking.text == "why?"
        ));
        assert!(matches!(
            app.blocks.iter().find(|block| matches!(block, Block::ToolCall(_))),
            Some(Block::ToolCall(call)) if call.args_stream == r#"{\"file\":\"src/lib.rs\"}"#
        ));
    }

    #[test]
    fn live_legacy_workflow_system_item_is_ignored() {
        let mut app = App::new("test".into());
        let item = serde_json::json!({
            "kind": "workflow",
            "slug": "build",
            "body": "[Workflow /build]\nRun the build",
        });
        app.handle_worker_event(Event::SystemItem {
            entry_id: None,
            item,
        });

        assert!(app.blocks.is_empty());
    }

    #[test]
    fn internal_worker_events_project_by_session_without_mixing_parent_blocks() {
        let mut app = App::new("parent".into());
        let worker = InternalWorkerRef {
            session_id: "child-session".into(),
            name: "research".into(),
            parent_session_id: Some("parent-session".into()),
            kind: protocol::InternalWorkerKind::SubWorker,
        };
        app.handle_worker_event(Event::InternalWorker {
            worker: worker.clone(),
            revision: 2,
            event: Box::new(Event::TextDelta {
                text: "child output".into(),
            }),
        });
        app.handle_worker_event(Event::InternalWorker {
            worker,
            revision: 1,
            event: Box::new(Event::TextDelta {
                text: "stale".into(),
            }),
        });

        assert!(app.blocks.is_empty());
        assert_eq!(app.internal_workers.len(), 1);
        assert_eq!(app.internal_workers[0].revision, 2);
        assert!(
            app.internal_workers[0].app.blocks.iter().any(
                |block| matches!(block, Block::AssistantText { text } if text == "child output")
            )
        );
    }

    fn test_internal_worker_snapshot(
        session_id: &str,
        name: &str,
        revision: u64,
    ) -> InternalWorkerSnapshot {
        InternalWorkerSnapshot {
            worker: InternalWorkerRef {
                session_id: session_id.into(),
                name: name.into(),
                parent_session_id: Some("parent".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision,
            status: WorkerStatus::Idle,
            greeting: None,
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            in_flight: protocol::InFlightSnapshot::default(),
            error: None,
            internal_workers: Vec::new(),
        }
    }

    #[test]
    fn internal_worker_snapshot_uses_child_greeting_metadata() {
        let mut app = App::new("parent".into());
        let mut child = test_internal_worker_snapshot("child", "research", 1);
        let mut greeting = test_greeting();
        greeting.worker_name = "research".into();
        greeting.model = "child-model".into();
        greeting.context_window = 64_000;
        greeting.context_usage = Some(protocol::ContextUsage {
            tokens: 12_000,
            source: protocol::ContextTokenSource::Measured,
        });
        child.greeting = Some(greeting);

        app.replace_internal_worker_snapshots(vec![child]);

        let child_app = &app.internal_workers[0].app;
        assert_eq!(child_app.greeting.as_ref().unwrap().model, "child-model");
        assert_eq!(child_app.context_window, 64_000);
        assert_eq!(child_app.session_context_tokens, 12_000);
    }

    #[test]
    fn live_internal_worker_snapshot_installs_child_metadata_before_context_updates() {
        let mut app = App::new("parent".into());
        let worker = test_internal_worker_snapshot("child", "research", 1).worker;
        let mut greeting = test_greeting();
        greeting.worker_name = "research".into();
        greeting.model = "child-model".into();
        greeting.context_window = 64_000;
        greeting.context_usage = Some(protocol::ContextUsage {
            tokens: 12_000,
            source: protocol::ContextTokenSource::Measured,
        });

        app.handle_worker_event(Event::InternalWorker {
            worker: worker.clone(),
            revision: 1,
            event: Box::new(Event::Snapshot {
                session: protocol::SessionSnapshot {
                    pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                    entries: Vec::new(),
                },
                greeting,
                state: WorkerStatus::Running.into(),
                in_flight: Default::default(),
                internal_workers: Vec::new(),
            }),
        });
        app.handle_worker_event(Event::InternalWorker {
            worker,
            revision: 2,
            event: Box::new(Event::ContextUsage {
                usage: Some(protocol::ContextUsage {
                    tokens: 14_000,
                    source: protocol::ContextTokenSource::Estimated,
                }),
            }),
        });

        let child = &app.internal_workers[0].app;
        assert_eq!(child.greeting.as_ref().unwrap().model, "child-model");
        assert_eq!(child.context_window, 64_000);
        assert_eq!(child.session_context_tokens, 14_000);
        assert_eq!(
            child.session_context_source,
            Some(protocol::ContextTokenSource::Estimated)
        );
    }

    #[test]
    fn worker_view_cycle_uses_stable_session_identity_and_wraps_to_main() {
        let mut app = App::new("parent".into());
        for (session_id, name) in [("child-a", "alpha"), ("child-b", "beta")] {
            app.internal_workers.push(InternalWorkerView {
                worker: InternalWorkerRef {
                    session_id: session_id.into(),
                    name: name.into(),
                    parent_session_id: Some("parent".into()),
                    kind: protocol::InternalWorkerKind::SubWorker,
                },
                revision: 1,
                app: Box::new(App::new(name.into())),
            });
        }

        assert_eq!(app.selected_worker_view().worker_name, "parent");
        assert!(app.cycle_worker_view());
        assert_eq!(app.selected_worker_view().worker_name, "alpha");

        app.internal_workers.swap(0, 1);
        assert_eq!(app.selected_worker_view().worker_name, "alpha");
        assert!(app.cycle_worker_view());
        assert_eq!(app.selected_worker_view().worker_name, "parent");
        assert!(app.cycle_worker_view());
        assert_eq!(app.selected_worker_view().worker_name, "beta");
    }

    #[test]
    fn worker_view_cycle_preserves_each_views_text_selection() {
        use crate::text_selection::{HistoryViewport, SelectionRow};

        fn select_first_row(app: &mut App, text: &str) {
            app.text_selection.set_history_snapshot(
                HistoryViewport {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 1,
                    top_offset: 0,
                    total_lines: 1,
                },
                vec![SelectionRow::new(text.into(), true)],
            );
            assert!(app.text_selection.begin_drag(0, 0));
        }

        let mut app = App::new("parent".into());
        app.replace_internal_worker_snapshots(vec![test_internal_worker_snapshot(
            "child", "child", 1,
        )]);
        select_first_row(&mut app, "parent selection");
        select_first_row(app.internal_workers[0].app.as_mut(), "child selection");

        app.cycle_worker_view();
        assert!(app.selected_worker_view().text_selection.has_selection());
        app.cycle_worker_view();

        assert!(app.text_selection.has_selection());
        assert!(app.internal_workers[0].app.text_selection.has_selection());
    }

    #[test]
    fn snapshot_removal_falls_selected_worker_view_back_to_main() {
        let mut app = App::new("parent".into());
        app.internal_workers.push(InternalWorkerView {
            worker: InternalWorkerRef {
                session_id: "old".into(),
                name: "old".into(),
                parent_session_id: Some("parent".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            app: Box::new(App::new("old".into())),
        });
        app.cycle_worker_view();
        assert_eq!(app.selected_worker_view().worker_name, "old");

        app.replace_internal_worker_snapshots(Vec::new());

        assert_eq!(app.selected_worker_view().worker_name, "parent");
        assert_eq!(
            app.worker_view_tabs(),
            vec![WorkerViewTab {
                label: "main".into(),
                selected: true,
            }]
        );
    }

    #[test]
    fn same_session_snapshot_preserves_subworker_view_local_state() {
        use crate::text_selection::{HistoryViewport, SelectionRow};

        let mut app = App::new("parent".into());
        app.replace_internal_worker_snapshots(vec![test_internal_worker_snapshot(
            "child", "child", 1,
        )]);
        let child = app.internal_workers[0].app.as_mut();
        child.scroll.follow_tail = false;
        child.scroll.top_offset = 7;
        child.task_pane_scroll = 4;
        child.text_selection.set_history_snapshot(
            HistoryViewport {
                x: 0,
                y: 0,
                width: 20,
                height: 1,
                top_offset: 0,
                total_lines: 1,
            },
            vec![SelectionRow::new("selected".into(), true)],
        );
        assert!(child.text_selection.begin_drag(0, 0));

        app.replace_internal_worker_snapshots(vec![test_internal_worker_snapshot(
            "child",
            "renamed-child",
            2,
        )]);

        let view = &app.internal_workers[0];
        assert_eq!(view.revision, 2);
        assert_eq!(view.app.worker_name, "renamed-child");
        assert!(!view.app.scroll.follow_tail);
        assert_eq!(view.app.scroll.top_offset, 7);
        assert_eq!(view.app.task_pane_scroll, 4);
        assert!(view.app.text_selection.has_selection());
    }

    #[test]
    fn task_pane_scroll_is_local_to_selected_worker_view() {
        let mut app = App::new("parent".into());
        app.replace_internal_worker_snapshots(vec![
            test_internal_worker_snapshot("child-a", "alpha", 1),
            test_internal_worker_snapshot("child-b", "beta", 1),
        ]);
        app.task_pane_scroll = 3;

        app.cycle_worker_view();
        app.scroll_task_pane_down(5);
        assert_eq!(app.selected_worker_view().task_pane_scroll, 5);

        app.cycle_worker_view();
        app.scroll_task_pane_down(7);
        assert_eq!(app.selected_worker_view().task_pane_scroll, 7);

        app.cycle_worker_view();
        assert_eq!(app.selected_worker_view().worker_name, "parent");
        assert_eq!(app.task_pane_scroll, 3);
        assert_eq!(app.internal_workers[0].app.task_pane_scroll, 5);
        assert_eq!(app.internal_workers[1].app.task_pane_scroll, 7);

        app.cycle_worker_view();
        app.toggle_task_pane();
        app.toggle_task_pane();
        assert_eq!(app.internal_workers[0].app.task_pane_scroll, 0);
        assert_eq!(app.internal_workers[1].app.task_pane_scroll, 7);
        assert_eq!(app.task_pane_scroll, 3);
    }

    #[test]
    fn terminal_internal_worker_removal_drops_descendants_and_fences_late_events() {
        let mut app = App::new("parent".into());
        let worker = InternalWorkerRef {
            session_id: "child-session".into(),
            name: "child".into(),
            parent_session_id: Some("parent-session".into()),
            kind: protocol::InternalWorkerKind::SubWorker,
        };
        let nested = InternalWorkerRef {
            session_id: "grandchild-session".into(),
            name: "grandchild".into(),
            parent_session_id: Some("child-session".into()),
            kind: protocol::InternalWorkerKind::SubWorker,
        };
        app.handle_worker_event(Event::InternalWorker {
            worker: worker.clone(),
            revision: 2,
            event: Box::new(Event::InternalWorker {
                worker: nested,
                revision: 1,
                event: Box::new(Event::TextDone {
                    text: "nested".into(),
                }),
            }),
        });
        assert_eq!(app.internal_workers.len(), 1);
        assert_eq!(app.internal_workers[0].app.internal_workers.len(), 1);
        app.cycle_worker_view();
        assert_eq!(app.selected_worker_view().worker_name, "child");

        app.handle_worker_event(Event::InternalWorkerRemoved {
            worker: worker.clone(),
            revision: 3,
        });
        app.handle_worker_event(Event::InternalWorker {
            worker,
            revision: 4,
            event: Box::new(Event::TextDone {
                text: "late".into(),
            }),
        });

        assert!(app.internal_workers.is_empty());
        assert_eq!(app.selected_worker_view().worker_name, "parent");
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            state: test_worker_state(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });
        assert!(app.internal_workers.is_empty());
        assert!(app.removed_internal_workers.is_empty());
    }

    #[test]
    fn stale_internal_worker_removal_keeps_newer_projection() {
        let mut app = App::new("parent".into());
        let worker = InternalWorkerRef {
            session_id: "child-session".into(),
            name: "child".into(),
            parent_session_id: Some("parent-session".into()),
            kind: protocol::InternalWorkerKind::SubWorker,
        };
        app.handle_worker_event(Event::InternalWorker {
            worker: worker.clone(),
            revision: 4,
            event: Box::new(Event::TextDone {
                text: "current".into(),
            }),
        });
        app.handle_worker_event(Event::InternalWorkerRemoved {
            worker,
            revision: 3,
        });

        assert_eq!(app.internal_workers.len(), 1);
        assert_eq!(app.internal_workers[0].revision, 4);
    }

    #[test]
    fn snapshot_authoritatively_replaces_internal_worker_views() {
        let mut app = App::new("parent".into());
        app.internal_workers.push(InternalWorkerView {
            worker: InternalWorkerRef {
                session_id: "old".into(),
                name: "old".into(),
                parent_session_id: None,
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            app: Box::new(App::new("old".into())),
        });
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            state: test_worker_state(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: vec![InternalWorkerSnapshot {
                worker: InternalWorkerRef {
                    session_id: "replacement".into(),
                    name: "replacement".into(),
                    parent_session_id: Some("parent-session".into()),
                    kind: protocol::InternalWorkerKind::SubWorker,
                },
                revision: 4,
                session: protocol::SessionSnapshot {
                    pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                    entries: Vec::new(),
                },
                greeting: None,
                status: WorkerStatus::Running,
                error: None,
                in_flight: Default::default(),
                internal_workers: Vec::new(),
            }],
        });

        assert_eq!(app.internal_workers.len(), 1);
        assert_eq!(app.internal_workers[0].worker.session_id, "replacement");
        assert_eq!(app.internal_workers[0].revision, 4);
        assert_eq!(
            app.internal_workers[0].app.worker_status,
            WorkerStatus::Running
        );
    }

    #[test]
    fn live_system_item_notification_appends_notify_block() {
        let mut app = App::new("test".into());
        let item = serde_json::json!({
            "kind": "notification",
            "message": "hi",
            "body": "[Notification] hi",
        });
        app.handle_worker_event(Event::SystemItem {
            entry_id: None,
            item,
        });
        assert!(matches!(
            app.blocks.as_slice(),
            [Block::Notify { message }] if message == "hi"
        ));
    }

    #[test]
    fn live_system_item_worker_event_appends_worker_event_block() {
        let mut app = App::new("test".into());
        let item = serde_json::json!({
            "kind": "worker_event",
            "event": { "kind": "turn_ended", "worker_name": "child" },
            "body": "[Notification] worker `child` finished a turn",
        });
        app.handle_worker_event(Event::SystemItem {
            entry_id: None,
            item,
        });
        assert_eq!(app.blocks.len(), 1);
        match &app.blocks[0] {
            Block::WorkerEvent {
                event: protocol::WorkerEvent::TurnEnded { worker_name },
            } => assert_eq!(worker_name, "child"),
            _ => panic!("expected a WorkerEvent block"),
        }
    }

    fn test_compaction_lifecycle(
        state: protocol::CompactionLifecycleState,
    ) -> protocol::CompactionLifecycle {
        protocol::CompactionLifecycle {
            schema_version: 2,
            compaction_id: "compaction-test".into(),
            revision: 1,
            internal_worker: None,
            state,
            started_at_ms: 1,
            ended_at_ms: None,
            summary: None,
            error: None,
            new_segment_id: None,
        }
    }

    #[test]
    fn compact_done_replaces_live_block() {
        let mut app = App::new("test".into());
        let id = uuid::Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();

        app.handle_worker_event(Event::CompactStart {
            lifecycle: test_compaction_lifecycle(protocol::CompactionLifecycleState::Running),
        });
        let mut lifecycle = test_compaction_lifecycle(protocol::CompactionLifecycleState::Done);
        lifecycle.revision = 2;
        lifecycle.new_segment_id = Some(id.to_string());
        app.handle_worker_event(Event::CompactDone { lifecycle });

        assert_eq!(compact_block_count(&app), 1);
        assert!(matches!(
            app.blocks.as_slice(),
            [Block::Compact(CompactEvent::Done {
                new_segment_id,
                elapsed_secs: Some(_),
            })] if *new_segment_id == id
        ));
    }

    #[test]
    fn compact_failed_replaces_live_block() {
        let mut app = App::new("test".into());

        app.handle_worker_event(Event::CompactStart {
            lifecycle: test_compaction_lifecycle(protocol::CompactionLifecycleState::Running),
        });
        let mut lifecycle = test_compaction_lifecycle(protocol::CompactionLifecycleState::Failed);
        lifecycle.revision = 2;
        lifecycle.error = Some("provider 429".into());
        app.handle_worker_event(Event::CompactFailed { lifecycle });

        assert_eq!(compact_block_count(&app), 1);
        assert!(matches!(
            app.blocks.as_slice(),
            [Block::Compact(CompactEvent::Failed {
                error,
                elapsed_secs: Some(_),
            })] if error == "provider 429"
        ));
    }

    #[test]
    fn compaction_progress_is_hidden_when_worker_state_is_inconsistent() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::CompactionProgress {
            compaction: Some(protocol::InFlightCompaction {
                phase: protocol::CompactionPhase::Preparing,
                started_at_ms: 100,
                trigger: protocol::CompactionTrigger::Manual,
            }),
        });
        assert!(app.compaction_progress.is_none());
    }

    #[test]
    fn snapshot_restores_and_runtime_clear_removes_compaction_progress() {
        let mut app = App::new("test".into());
        assert_eq!(app.worker_state.state, protocol::WorkerState::Idle);
        let mut state = protocol::WorkerStateSnapshot::initial();
        state.state = protocol::WorkerState::Busy(protocol::WorkerBusyState::Maintenance(
            protocol::WorkerMaintenanceState::Compacting,
        ));
        app.handle_worker_event(Event::Snapshot {
            session: public_session(Vec::new()),
            greeting: test_greeting(),
            state,
            in_flight: InFlightSnapshot {
                compaction: Some(protocol::InFlightCompaction {
                    phase: protocol::CompactionPhase::Summarizing,
                    started_at_ms: 100,
                    trigger: protocol::CompactionTrigger::Manual,
                }),
                ..InFlightSnapshot::default()
            },
            internal_workers: Vec::new(),
        });
        assert_eq!(compact_block_count(&app), 0);
        assert_eq!(
            app.compaction_progress.as_ref().map(|item| item.phase),
            Some(protocol::CompactionPhase::Summarizing)
        );

        app.handle_worker_event(Event::CompactionProgress { compaction: None });

        assert!(app.compaction_progress.is_none());
    }

    #[test]
    fn shutdown_marks_live_compact_incomplete() {
        let mut app = App::new("test".into());

        app.handle_worker_event(Event::CompactStart {
            lifecycle: test_compaction_lifecycle(protocol::CompactionLifecycleState::Running),
        });
        app.handle_worker_event(Event::Shutdown);

        assert!(app.quit);
        assert!(matches!(
            app.blocks.as_slice(),
            [Block::Compact(CompactEvent::Incomplete {
                elapsed_secs: Some(_),
            })]
        ));
    }

    fn compact_block_count(app: &App) -> usize {
        app.blocks
            .iter()
            .filter(|block| matches!(block, Block::Compact(_)))
            .count()
    }

    fn test_worker_state(status: WorkerStatus) -> WorkerStateSnapshot {
        WorkerStateSnapshot::from(status)
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            worker_name: "test".into(),
            cwd: "/tmp".into(),
            provider: "test-provider".into(),
            model: "test-model".into(),
            scope_summary: String::new(),
            tools: Vec::new(),
            context_window: 200_000,
            context_tokens: 0,
            reasoning: None,
            context_usage: None,
        }
    }

    #[test]
    fn snapshot_prefers_typed_context_usage_over_legacy_mirror() {
        let mut app = App::new("test".into());
        let mut greeting = test_greeting();
        greeting.context_window = 123_000;
        greeting.context_tokens = 1;
        greeting.context_usage = Some(protocol::ContextUsage {
            tokens: 45_000,
            source: protocol::ContextTokenSource::Measured,
        });

        app.handle_worker_event(Event::Snapshot {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            greeting,
            state: test_worker_state(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        assert_eq!(app.context_window, 123_000);
        assert_eq!(app.session_context_tokens, 45_000);
        assert_eq!(
            app.session_context_source,
            Some(protocol::ContextTokenSource::Measured)
        );
    }

    #[test]
    fn snapshot_falls_back_to_legacy_context_tokens() {
        let mut app = App::new("test".into());
        let mut greeting = test_greeting();
        greeting.context_window = 100_000;
        greeting.context_tokens = 25_000;

        app.handle_worker_event(Event::Snapshot {
            session: protocol::SessionSnapshot {
                pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
                entries: Vec::new(),
            },
            greeting,
            state: test_worker_state(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        assert_eq!(app.context_window, 100_000);
        assert_eq!(app.session_context_tokens, 25_000);
        assert_eq!(
            app.session_context_source,
            Some(protocol::ContextTokenSource::Estimated)
        );
    }

    #[test]
    fn usage_updates_session_context_tokens_without_cache_discount() {
        let mut app = App::new("test".into());

        app.handle_worker_event(Event::Usage {
            input_tokens: Some(42_000),
            output_tokens: Some(9),
            cache_read_input_tokens: Some(40_000),
        });

        assert_eq!(app.session_context_tokens, 42_000);
        assert_eq!(
            app.session_context_source,
            Some(protocol::ContextTokenSource::Measured)
        );
        assert_eq!(app.run_upload_tokens, 2_000);
        assert_eq!(app.run_output_tokens, 9);
    }

    #[test]
    fn context_usage_event_converges_live_state_without_changing_run_traffic() {
        let mut app = App::new("test".into());
        app.run_upload_tokens = 2_000;
        app.run_output_tokens = 9;

        app.handle_worker_event(Event::ContextUsage {
            usage: Some(protocol::ContextUsage {
                tokens: 45_000,
                source: protocol::ContextTokenSource::Estimated,
            }),
        });

        assert_eq!(app.session_context_tokens, 45_000);
        assert_eq!(
            app.session_context_source,
            Some(protocol::ContextTokenSource::Estimated)
        );
        assert_eq!(app.run_upload_tokens, 2_000);
        assert_eq!(app.run_output_tokens, 9);

        app.handle_worker_event(Event::ContextUsage { usage: None });
        assert_eq!(app.session_context_source, None);
    }

    #[test]
    fn memory_worker_event_updates_actionbar_state() {
        let mut app = App::new("test".into());

        app.handle_worker_event(Event::MemoryWorker(protocol::MemoryWorkerEvent {
            worker: "extract".into(),
            status: "done".into(),
            run_id: "00000000-0000-0000-0000-000000000000".into(),
            trigger: "token_threshold".into(),
            reason: "completed_staging_written".into(),
            message: "memory extract done: completed_staging_written".into(),
            timestamp_ms: 0,
        }));

        assert_eq!(
            app.latest_memory_worker_event.as_deref(),
            Some("memory extract done: completed_staging_written")
        );
    }

    #[test]
    fn legacy_compact_done_does_not_fabricate_zero_context_usage() {
        let mut app = App::new("test".into());
        app.session_context_tokens = 42_000;

        let mut lifecycle = test_compaction_lifecycle(protocol::CompactionLifecycleState::Done);
        lifecycle.new_segment_id = Some(uuid::Uuid::nil().to_string());
        app.handle_worker_event(Event::CompactDone { lifecycle });

        assert_eq!(app.session_context_tokens, 42_000);
    }

    #[test]
    fn turn_start_and_run_end_do_not_reset_session_context_tokens() {
        let mut app = App::new("test".into());
        app.session_context_tokens = 42_000;

        app.handle_worker_event(Event::TurnStart { turn: 1 });
        app.handle_worker_event(Event::RunEnd {
            result: RunResult::Finished,
        });

        assert_eq!(app.session_context_tokens, 42_000);
    }

    #[test]
    fn live_task_create_updates_task_store() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::ToolCallStart {
            id: "c1".into(),
            name: "TaskCreate".into(),
        });
        app.handle_worker_event(Event::ToolCallDone {
            id: "c1".into(),
            name: "TaskCreate".into(),
            arguments: r#"{"subject":"impl tasks","description":"do it"}"#.into(),
        });
        let tasks = app.task_store.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "impl tasks");
        assert_eq!(tasks[0].status, crate::task::TaskStatus::Pending);
    }

    #[test]
    fn live_task_update_advances_status() {
        let mut app = App::new("test".into());
        for (id, args) in [
            ("c1", r#"{"subject":"a","description":"A"}"#),
            ("u1", r#"{"taskid":1,"status":"completed"}"#),
        ] {
            let name = if id.starts_with('c') {
                "TaskCreate"
            } else {
                "TaskUpdate"
            };
            app.handle_worker_event(Event::ToolCallStart {
                id: id.into(),
                name: name.into(),
            });
            app.handle_worker_event(Event::ToolCallDone {
                id: id.into(),
                name: name.into(),
                arguments: args.into(),
            });
        }
        let tasks = app.task_store.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, crate::task::TaskStatus::Completed);
    }

    #[test]
    fn live_system_snapshot_replaces_task_store() {
        let mut app = App::new("test".into());
        // Stale entry that the snapshot must wipe out.
        app.handle_worker_event(Event::ToolCallStart {
            id: "c1".into(),
            name: "TaskCreate".into(),
        });
        app.handle_worker_event(Event::ToolCallDone {
            id: "c1".into(),
            name: "TaskCreate".into(),
            arguments: r#"{"subject":"stale","description":""}"#.into(),
        });

        let snapshot = "[Session TaskStore snapshot]\n\n\
            TaskStore: 1 task(s)\n\n\
            ```json\n{\n  \"tasks\": [\n    {\n      \"taskid\": 4,\n      \
            \"status\": \"inprogress\",\n      \"subject\": \"from snapshot\",\n      \
            \"description\": \"d\"\n    }\n  ]\n}\n```\n";
        // Snapshot text injected through an active system item kind; legacy
        // workflow items are intentionally ignored and must not carry active state.
        app.handle_worker_event(Event::SystemItem {
            entry_id: None,
            item: serde_json::json!({
                "kind": "task_reminder",
                "body": snapshot,
            }),
        });

        let tasks = app.task_store.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].taskid, 4);
        assert_eq!(tasks[0].subject, "from snapshot");
        assert!(matches!(
            app.blocks.last(),
            Some(Block::TaskReminder { text }) if text == snapshot
        ));
    }

    #[test]
    fn snapshot_reconstructs_task_store() {
        let mut app = App::new("test".into());
        // Live tool call before the snapshot lands — restore must wipe
        // this so it doesn't double-count after replay.
        app.handle_worker_event(Event::ToolCallStart {
            id: "live".into(),
            name: "TaskCreate".into(),
        });
        app.handle_worker_event(Event::ToolCallDone {
            id: "live".into(),
            name: "TaskCreate".into(),
            arguments: r#"{"subject":"live","description":""}"#.into(),
        });

        let assistant_item_entries = vec![
            serde_json::to_value(session_store::LogEntry::AnnotatedAssistantItem {
                ts: 1,
                entry: annotated(agen::Item::tool_call(
                    "c1",
                    "TaskCreate",
                    r#"{"subject":"a","description":"A"}"#,
                )),
            })
            .unwrap(),
            serde_json::to_value(session_store::LogEntry::AnnotatedAssistantItem {
                ts: 2,
                entry: annotated(agen::Item::tool_call(
                    "c2",
                    "TaskCreate",
                    r#"{"subject":"b","description":"B"}"#,
                )),
            })
            .unwrap(),
            serde_json::to_value(session_store::LogEntry::AnnotatedAssistantItem {
                ts: 3,
                entry: annotated(agen::Item::tool_call(
                    "u1",
                    "TaskUpdate",
                    r#"{"taskid":2,"status":"inprogress"}"#,
                )),
            })
            .unwrap(),
        ];
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: public_session(assistant_item_entries),
            state: test_worker_state(WorkerStatus::Running),
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        let tasks = app.task_store.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].subject, "a");
        assert_eq!(tasks[1].subject, "b");
        assert_eq!(tasks[1].status, crate::task::TaskStatus::Inprogress);
    }

    #[test]
    fn input_history_records_running_submits_and_suppresses_consecutive_duplicates() {
        let mut app = App::new("test".into());
        app.running = true;

        for c in "repeat".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_some());
        assert_eq!(app.input_history_len(), 1);
        assert_eq!(app.queued_input_count(), 0);

        for c in "repeat".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_some());
        assert_eq!(app.input_history_len(), 1);
        assert_eq!(app.queued_input_count(), 0);

        app.insert_char(' ');
        assert!(app.submit_input().is_none());
        assert_eq!(app.input_history_len(), 1);
    }

    #[test]
    fn input_history_preserves_typed_segments() {
        let mut app = App::new("test".into());
        let original = vec![
            Segment::Text {
                content: "see ".into(),
            },
            Segment::FileRef {
                path: "src/main.rs".into(),
            },
            Segment::Text {
                content: " and ".into(),
            },
            Segment::Paste {
                id: 1,
                chars: 13,
                lines: 1,
                content: "literal paste".into(),
            },
        ];
        app.input.replace_with_segments(&original);
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));

        assert!(app.browse_input_history_older());
        assert_eq!(app.input.submit_segments(), original);
    }

    #[test]
    fn input_history_restores_non_empty_draft_after_newest() {
        let mut app = App::new("test".into());
        for c in "sent".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));

        for c in "draft".chars() {
            app.insert_char(c);
        }
        assert!(app.browse_input_history_older());
        assert_eq!(input_text(&app), "sent");
        assert!(app.browse_input_history_newer());
        assert_eq!(input_text(&app), "draft");
        assert!(!app.input_history_is_browsing());
    }

    #[test]
    fn editing_recalled_input_exits_history_browse_mode() {
        let mut app = App::new("test".into());
        for c in "sent".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));

        assert!(app.browse_input_history_older());
        assert!(app.input_history_is_browsing());
        app.insert_char('!');
        assert!(!app.input_history_is_browsing());
        assert_eq!(input_text(&app), "sent!");
        assert!(!app.browse_input_history_newer());
        assert_eq!(input_text(&app), "sent!");
    }

    #[test]
    fn submitting_recalled_history_sends_normally_and_records_if_not_duplicate() {
        let mut app = App::new("test".into());
        for c in "first".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));
        for c in "second".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Submit { .. })));

        assert!(app.browse_input_history_older());
        assert!(app.browse_input_history_older());
        let method = app.submit_input();
        match method {
            Some(Method::Submit { input, .. }) => {
                assert_eq!(Segment::flatten_to_text(&input), "first")
            }
            other => panic!("expected recalled run, got {other:?}"),
        }
        assert_eq!(app.input_history_len(), 3);
        assert!(!app.input_history_is_browsing());
    }

    #[test]
    fn task_pane_toggle_flips_state_and_resets_scroll() {
        let mut app = App::new("test".into());
        app.task_pane_scroll = 7;
        assert!(!app.task_pane_open);
        app.toggle_task_pane();
        assert!(app.task_pane_open);
        // Scroll position is preserved on open so the user keeps their
        // place if they re-open after closing.
        assert_eq!(app.task_pane_scroll, 7);
        app.toggle_task_pane();
        assert!(!app.task_pane_open);
        assert_eq!(app.task_pane_scroll, 0);
    }
}

/// Seed / mutate the file-content cache based on a completed tool call.
///
/// Each built-in file tool has its own rule: Read copies the result body
/// into the cache, Write replaces it with `args.content`, Edit applies
/// the `old_string → new_string` swap in-place.
fn apply_cache_update(
    cache: &mut FileCache,
    name: &str,
    arguments: Option<&str>,
    output: Option<&str>,
) {
    let args = arguments.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    match name {
        "Read" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            if let Some(content) = output {
                // The Read tool emits a `cat -n` style display: each
                // line is "{lineno:>6}\tcontent". Strip that framing
                // so the cache mirrors the real file body and the
                // Edit diff renderer has a faithful "before" view.
                cache.put(path, strip_cat_n_prefix(content));
            }
        }
        "Write" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            let Some(content) = args["content"].as_str() else {
                return;
            };
            cache.put(path, content.to_owned());
        }
        "Edit" => {
            let Some(args) = args.as_ref() else { return };
            let Some(path) = args["file_path"].as_str() else {
                return;
            };
            let Some(old) = args["old_string"].as_str() else {
                return;
            };
            let Some(new) = args["new_string"].as_str() else {
                return;
            };
            cache.apply_edit(path, old, new);
        }
        _ => {}
    }
}

#[cfg(test)]
mod completion_correlation_tests {
    use super::*;

    fn reply(request: &Method, value: &str) -> Event {
        let Method::ListCompletions {
            kind,
            prefix,
            context,
            request_id,
        } = request
        else {
            panic!("completion query expected")
        };
        assert!(request_id.as_ref().is_some_and(|id| !id.is_empty()));
        Event::Completions {
            kind: *kind,
            prefix: prefix.clone(),
            context: context.clone(),
            request_id: request_id.clone(),
            entries: vec![CompletionEntry {
                value: value.into(),
                ..Default::default()
            }],
        }
    }

    fn request_id(request: &Method) -> &str {
        let Method::ListCompletions {
            request_id: Some(id),
            ..
        } = request
        else {
            panic!("correlated completion query expected")
        };
        id
    }

    fn authority_snapshot(scope: &str) -> Event {
        Event::Snapshot {
            session: protocol::SessionSnapshot {
                entries: Vec::new(),
                pending_submissions: Default::default(),
            },
            greeting: protocol::Greeting {
                worker_name: "test".into(),
                cwd: "/tmp".into(),
                provider: "test".into(),
                model: "test".into(),
                reasoning: None,
                scope_summary: scope.into(),
                tools: vec![],
                context_window: 0,
                context_tokens: 0,
                context_usage: None,
            },
            state: protocol::WorkerStateSnapshot::from(WorkerStatus::Idle),
            in_flight: Default::default(),
            internal_workers: vec![],
        }
    }

    #[test]
    fn identical_file_prefix_aba_rejects_old_nonce_and_accepts_latest_query() {
        let mut app = App::new("test".into());
        app.input.insert_str("@same");
        let first = app.refresh_completion().unwrap();
        assert!(
            app.refresh_completion().is_none(),
            "an unchanged request is not reissued"
        );
        app.insert_char('x');
        let second = app.refresh_completion().unwrap();
        app.delete_char_before();
        let third = app.refresh_completion().unwrap();
        assert_ne!(request_id(&first), request_id(&second));
        assert_ne!(request_id(&first), request_id(&third));
        app.handle_worker_event(reply(&first, "old"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&third, "current"));
        app.handle_worker_event(reply(&first, "late old"));
        assert_eq!(app.completion.as_ref().unwrap().entries[0].value, "current");
    }

    #[test]
    fn identical_argument_context_aba_rejects_reply_before_and_after_refresh() {
        let mut app = App::new("test".into());
        crate::invocation_tests::select(&mut app, crate::invocation_tests::descriptor(), "run");
        app.input.insert_str("\"資料/a");
        let first = app.refresh_completion().unwrap();
        app.input.insert_char('x');
        app.input.delete_before();
        // Prefix, context, token location, and cursor are identical; semantic revision is not.
        app.handle_worker_event(reply(&first, "old"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        let second = app.refresh_completion().unwrap();
        assert_ne!(request_id(&first), request_id(&second));
        app.handle_worker_event(reply(&first, "old"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&second, "資料/current"));
        assert_eq!(
            app.completion.as_ref().unwrap().entries[0].value,
            "資料/current"
        );
    }

    #[test]
    fn identical_cursor_aba_and_same_prefix_at_another_location_are_fenced() {
        let mut app = App::new("test".into());
        app.input.insert_str("@same @same");
        let last_location = app.refresh_completion().unwrap();
        app.input.move_home();
        for _ in 0..5 {
            app.input.move_right();
        }
        let first_location = app.refresh_completion().unwrap();
        app.handle_worker_event(reply(&last_location, "wrong location"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.input.move_end();
        let returned_location = app.refresh_completion().unwrap();
        app.handle_worker_event(reply(&last_location, "old same location"));
        app.handle_worker_event(reply(&first_location, "wrong location"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        assert_ne!(request_id(&last_location), request_id(&returned_location));
        app.input.move_left();
        app.input.move_right();
        app.handle_worker_event(reply(&returned_location, "cursor ABA before refresh"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        let current = app.refresh_completion().unwrap();
        app.handle_worker_event(reply(&current, "current"));
        assert_eq!(app.completion.as_ref().unwrap().entries[0].value, "current");
    }

    #[test]
    fn visible_candidates_cannot_be_inserted_after_unrefreshed_aba_edit() {
        let mut app = App::new("test".into());
        app.input.insert_str("@same");
        let request = app.refresh_completion().unwrap();
        app.handle_worker_event(reply(&request, "candidate"));
        app.input.move_left();
        app.input.move_right();
        assert!(app.apply_completion_text().is_none());
        assert_eq!(app.input.plain_text(), "@same");
        assert!(app.completion.is_none());
    }

    #[test]
    fn cancelled_and_reopened_identical_feature_discovery_gets_new_nonce() {
        let mut app = App::new("test".into());
        app.input.insert_str("/");
        let first = app.refresh_completion().unwrap();
        app.cancel_completion();
        let current = app.refresh_completion().unwrap();
        assert_ne!(request_id(&first), request_id(&current));
        app.handle_worker_event(reply(&first, "old feature"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&current, "current feature"));
        assert_eq!(
            app.completion.as_ref().unwrap().entries[0].value,
            "current feature"
        );
    }

    #[test]
    fn permission_snapshot_generation_aba_revokes_queries_even_when_scope_returns_to_original() {
        let mut app = App::new("test".into());
        app.handle_worker_event(authority_snapshot("Writable: /tmp"));
        crate::invocation_tests::select(&mut app, crate::invocation_tests::descriptor(), "run");
        app.input.insert_str("\"資料/a");
        let first = app.refresh_completion().unwrap();
        let denied = app
            .handle_worker_event(authority_snapshot("Readable: /tmp"))
            .unwrap();
        let current = app
            .handle_worker_event(authority_snapshot("Writable: /tmp"))
            .unwrap();
        assert_ne!(request_id(&first), request_id(&denied));
        assert_ne!(request_id(&first), request_id(&current));
        app.handle_worker_event(reply(&first, "old permission generation"));
        app.handle_worker_event(reply(&denied, "intermediate generation"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&current, "資料/current"));
        assert_eq!(
            app.completion.as_ref().unwrap().entries[0].value,
            "資料/current"
        );
    }

    #[test]
    fn actual_worker_target_switch_aba_revokes_identical_queries() {
        let mut app = App::new("test".into());
        app.set_completion_target("workspace/runtime/worker-a".into());
        app.input.insert_str("@same");
        let first = app.refresh_completion().unwrap();
        app.set_completion_target("workspace/runtime/worker-b".into());
        let other = app.refresh_completion().unwrap();
        app.set_completion_target("workspace/runtime/worker-a".into());
        let current = app.refresh_completion().unwrap();
        assert_ne!(request_id(&first), request_id(&current));
        app.handle_worker_event(reply(&first, "worker-a old"));
        app.handle_worker_event(reply(&other, "worker-b"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&current, "worker-a current"));
        assert_eq!(
            app.completion.as_ref().unwrap().entries[0].value,
            "worker-a current"
        );
    }

    #[test]
    fn presentation_worker_view_switch_aba_revokes_queries_without_redirecting_parent_controls() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::InternalWorker {
            worker: InternalWorkerRef {
                session_id: "child".into(),
                name: "child".into(),
                parent_session_id: Some("parent".into()),
                kind: protocol::InternalWorkerKind::SubWorker,
            },
            revision: 1,
            event: Box::new(Event::WorkerState {
                snapshot: protocol::WorkerStateSnapshot::from(WorkerStatus::Idle),
            }),
        });
        app.input.insert_str("@same");
        let first = app.refresh_completion().unwrap();
        assert!(app.cycle_worker_view());
        let child_view = app.refresh_completion().unwrap();
        assert!(app.cycle_worker_view());
        let current = app.refresh_completion().unwrap();
        assert!(app.selected_internal_worker_session_id.is_none());
        app.handle_worker_event(reply(&first, "old parent"));
        app.handle_worker_event(reply(&child_view, "old child view"));
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&current, "current parent"));
        assert_eq!(
            app.completion.as_ref().unwrap().entries[0].value,
            "current parent"
        );
    }

    #[test]
    fn feature_chip_reedit_is_nonce_and_snapshot_correlated() {
        let descriptor = crate::invocation_tests::descriptor();
        let invocation = protocol::parse_feature_invocation("/run(x)", 0, &descriptor, "stable-id")
            .unwrap()
            .invocation;
        let mut app = App::new("test".into());
        app.input
            .replace_with_segments(&[Segment::FeatureInvoke { invocation }]);
        let first = app.edit_adjacent_feature_invocation().unwrap();
        app.input.move_home();
        app.input.move_end();
        let mut event = reply(&first, "run");
        if let Event::Completions { entries, .. } = &mut event {
            entries[0].invocation = Some(descriptor.clone());
        }
        app.handle_worker_event(event);
        assert!(matches!(
            app.input.submit_segments().as_slice(),
            [Segment::FeatureInvoke { .. }]
        ));
        let current = app.edit_adjacent_feature_invocation().unwrap();
        assert_ne!(request_id(&first), request_id(&current));
        let mut old = reply(&first, "run");
        if let Event::Completions { entries, .. } = &mut old {
            entries[0].invocation = Some(descriptor.clone());
        }
        app.handle_worker_event(old);
        assert!(matches!(
            app.input.submit_segments().as_slice(),
            [Segment::FeatureInvoke { .. }]
        ));
        let mut event = reply(&current, "run");
        if let Event::Completions { entries, .. } = &mut event {
            entries[0].invocation = Some(descriptor);
        }
        app.handle_worker_event(event);
        assert_eq!(app.input.plain_text(), "/run(path=\"x\")");
        app.delete_char_before(); // Reopen the quoted argument prefix.
        let arguments = app.refresh_completion().unwrap();
        assert_ne!(request_id(&current), request_id(&arguments));
    }

    #[test]
    fn uncorrelated_legacy_reply_and_wrong_context_are_never_applied_to_correlated_queries() {
        let mut app = App::new("test".into());
        app.input.insert_str("@same");
        let request = app.refresh_completion().unwrap();
        let mut missing = reply(&request, "missing nonce");
        if let Event::Completions { request_id, .. } = &mut missing {
            *request_id = None;
        }
        app.handle_worker_event(missing);
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        let mut mismatched = reply(&request, "wrong context");
        if let Event::Completions { context, .. } = &mut mismatched {
            *context = Some(protocol::CompletionContext {
                invocation: protocol::FeatureInvocationIdentity("plugin:other/run".into()),
                argument: None,
            });
        }
        app.handle_worker_event(mismatched);
        assert!(app.completion.as_ref().unwrap().entries.is_empty());
        app.handle_worker_event(reply(&request, "current"));
        assert_eq!(app.completion.as_ref().unwrap().entries[0].value, "current");
    }
}
