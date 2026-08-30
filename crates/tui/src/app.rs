use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use protocol::{
    AlertLevel, AlertSource, CompletionEntry, CompletionKind, ErrorCode, Event, InFlightBlock,
    InFlightSnapshot, InFlightToolCallState, InternalWorkerRef, InternalWorkerSnapshot, Method,
    RewindTarget, RunResult, Segment, WorkerStatus,
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
pub struct CompletionState {
    pub kind: CompletionKind,
    /// Atom index of the leading sigil (`@` / `#` / `/`).
    pub prefix_start: usize,
    /// Text typed after the sigil (sigil itself excluded).
    pub prefix: String,
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

#[derive(Clone)]
pub struct QueuedInput {
    segments: Vec<Segment>,
    preview: String,
}

impl QueuedInput {
    fn new(segments: Vec<Segment>) -> Self {
        let preview = Segment::flatten_to_text(&segments);
        Self { segments, preview }
    }

    pub fn preview(&self) -> &str {
        &self.preview
    }
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
    /// Last controller status reported by the Worker. Drives the status line
    /// and Ctrl-key routing; do not infer this solely from replayed history.
    pub worker_status: WorkerStatus,
    /// True while the Worker is in `WorkerStatus::Running`.
    pub running: bool,
    /// True while the Worker is in `WorkerStatus::Paused`.
    pub paused: bool,
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
    /// Normal composer input that is submitted as `Method::Run`.
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
    /// Full display history in render order.
    pub blocks: Vec<Block>,
    /// Turn/protocol errors retained when a real `SegmentStart` replaces the
    /// replayable conversation rows during segment rotation.
    run_error_messages: Vec<String>,
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
    /// TUI-local FIFO of user inputs submitted while the Worker is already running.
    /// Entries have not been sent to the Worker yet, so they remain editable/cancellable locally.
    queued_inputs: VecDeque<QueuedInput>,
    /// TUI-local readline-style composer input history. This is intentionally
    /// client-side only: recalled entries are plain drafts until submitted again.
    input_history: ComposerInputHistory,
    /// User-data backed persistence for composer recall entries. The saved
    /// contents are private input drafts and must not be logged or sent to Worker.
    input_history_store: Option<ComposerHistoryStore>,
    /// Local submit state kept until the accepted run either completes
    /// normally or reports that the empty assistant turn was rolled back.
    pending_submit_rollback: Option<RollbackSubmitState>,
    /// Last rolled-back submit that could not be restored because the
    /// composer already contained unsent user input.
    last_rolled_back_input: Option<Vec<Segment>>,
}

impl App {
    pub fn new(worker_name: String) -> Self {
        Self {
            worker_name,
            connected: false,
            worker_status: WorkerStatus::Idle,
            running: false,
            paused: false,
            run_requests: 0,
            run_upload_tokens: 0,
            run_output_tokens: 0,
            session_context_tokens: 0,
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
            blocks: Vec::new(),
            run_error_messages: Vec::new(),
            internal_workers: Vec::new(),
            selected_internal_worker_session_id: None,
            removed_internal_workers: HashMap::new(),
            scroll: Scroll::default(),
            mode: Mode::Normal,
            cache: FileCache::new(),
            assistant_streaming: false,
            completion: None,
            rewind_picker: None,
            rewind_request_pending: false,
            rewind_refresh_fence: false,
            greeting: None,
            task_store: TaskStore::new(),
            text_selection: TextSelectionState::default(),
            task_pane_open: false,
            task_pane_scroll: 0,
            queued_inputs: VecDeque::new(),
            input_history: ComposerInputHistory::new(),
            input_history_store: None,
            pending_submit_rollback: None,
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
        self.worker_status = status;
        self.running = status == WorkerStatus::Running;
        self.paused = status == WorkerStatus::Paused;
        if self.running {
            self.quit_confirm = None;
        }
    }

    /// Re-evaluate the completion popup against the current input.
    /// Returns a `Method::ListCompletions` to send when the
    /// `(kind, prefix_start, prefix)` triple changed; otherwise `None`.
    /// Callers should invoke this after every input mutation that could
    /// move the cursor or change atoms.
    pub fn refresh_completion(&mut self) -> Option<Method> {
        if self.is_command_mode() {
            self.completion = None;
            return None;
        }
        match self.input.pending_completion_prefix() {
            Some((kind, start, prefix)) => {
                let need_query = match &self.completion {
                    Some(c) => c.kind != kind || c.prefix_start != start || c.prefix != prefix,
                    None => true,
                };
                let entries = match self.completion.take() {
                    Some(c) if c.kind == kind && c.prefix_start == start => c.entries,
                    _ => Vec::new(),
                };
                self.completion = Some(CompletionState {
                    kind,
                    prefix_start: start,
                    prefix: prefix.clone(),
                    entries,
                    selected: 0,
                });
                if need_query {
                    Some(Method::ListCompletions { kind, prefix })
                } else {
                    None
                }
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
        self.completion = None;
    }

    /// Tab path: insert the popup-selected entry's value (with a
    /// trailing `/` when it's a directory) as raw text replacing the
    /// in-flight `@<typed>` portion. The popup state is preserved so
    /// the re-evaluated trigger can fetch fresh candidates for the new
    /// prefix (drill-in for directories, narrow-to-one for files).
    /// Returns the follow-up `Method::ListCompletions` to send when
    /// the new prefix differs from the old one.
    pub fn apply_completion_text(&mut self) -> Option<Method> {
        let state = self.completion.as_ref()?;
        if state.entries.is_empty() {
            return None;
        }
        let entry = &state.entries[state.selected];
        let text = if entry.is_dir {
            format!("{}/", entry.value)
        } else {
            entry.value.clone()
        };
        // `prefix_start` indexes the sigil atom; the text we want to
        // replace lives just after it (sigil itself stays).
        let typed_start = state.prefix_start + 1;
        self.input_history.cancel_browse();
        self.input.replace_with_text_at(typed_start, &text);
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
        }
        self.completion = None;
        true
    }

    pub fn submit_input(&mut self) -> Option<Method> {
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            // Empty Enter only does something meaningful when the Worker
            // is paused: resume the interrupted turn. Otherwise no-op.
            if self.paused {
                self.input_history.cancel_browse();
                self.input.clear();
                return Some(Method::Resume);
            }
            return None;
        }
        self.record_input_history(segments.clone());
        if self.running {
            self.queued_inputs.push_back(QueuedInput::new(segments));
            self.input.clear();
            self.completion = None;
            return None;
        }
        self.input.clear();
        Some(self.method_for_run(segments))
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
        Method::Run { input: segments }
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
        self.queued_inputs.len()
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
        if self.input_history.entries.is_empty() {
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

    pub fn next_queued_input_preview(&self) -> Option<&str> {
        self.queued_inputs.front().map(QueuedInput::preview)
    }

    pub fn clear_queued_inputs(&mut self) -> usize {
        let cleared = self.queued_inputs.len();
        self.queued_inputs.clear();
        cleared
    }

    pub fn restore_next_queued_input_to_composer(&mut self) -> bool {
        if self.queued_inputs.is_empty() {
            return false;
        }
        if !self.input.is_empty() {
            self.push_error("Composer is not empty; clear it before editing queued input.");
            return false;
        }
        let Some(queued) = self.queued_inputs.pop_front() else {
            return false;
        };
        self.input_history.cancel_browse();
        self.input.replace_with_segments(&queued.segments);
        self.completion = None;
        true
    }

    fn pop_next_queued_run(&mut self) -> Option<Method> {
        let queued = self.queued_inputs.pop_front()?;
        Some(self.method_for_run(queued.segments))
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Worker,
            message: message.into(),
        });
    }

    fn push_run_error(&mut self, message: String) {
        self.run_error_messages.push(message.clone());
        self.blocks.push(Block::Alert {
            level: AlertLevel::Error,
            source: AlertSource::Worker,
            message,
        });
    }

    fn handle_error(&mut self, code: ErrorCode, message: String) {
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

    pub fn handle_worker_event(&mut self, event: Event) -> Option<Method> {
        if self.rewind_refresh_fence && event_is_stale_after_rewind(&event) {
            return None;
        }

        match event {
            Event::UserMessage { segments } => {
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
            Event::SystemItem { item } => {
                self.apply_system_item(&item);
                self.assistant_streaming = false;
            }
            Event::TurnStart { .. } => {
                self.set_worker_status(WorkerStatus::Running);
                self.run_requests += 1;
                self.current_tool = None;
                self.latest_llm_wait_event = None;
                self.assistant_streaming = false;
            }
            // UI consumers of Invoke / LlmCall semantics are out of scope
            // for `tickets/invoke-turn-llmcall-semantics.md`; events flow
            // through to subscribers but the TUI currently derives its
            // turn header from `UserMessage` / `SystemItem` arrivals.
            Event::InvokeStart { .. } | Event::LlmCallStart { .. } | Event::LlmCallEnd { .. } => {
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
                self.session_context_tokens = input_tokens.unwrap_or(0);
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
                    self.reset_run_state(match result {
                        RunResult::Paused => WorkerStatus::Paused,
                        RunResult::Finished | RunResult::LimitReached | RunResult::RolledBack => {
                            WorkerStatus::Idle
                        }
                    });
                    if matches!(result, RunResult::Finished | RunResult::LimitReached) {
                        return self.pop_next_queued_run();
                    }
                }
            }
            Event::CompactStart { .. } => {
                if self.last_streaming_compact_mut().is_none() {
                    self.blocks.push(Block::Compact(CompactEvent::Streaming {
                        started_at: Instant::now(),
                    }));
                }
            }
            Event::CompactDone { lifecycle } => {
                self.session_context_tokens = 0;
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
                status,
                in_flight,
                internal_workers,
            } => {
                self.rewind_refresh_fence = false;
                self.restore_snapshot(&session, greeting, in_flight);
                self.replace_internal_worker_snapshots(internal_workers);
                self.set_worker_status(status);
            }
            Event::InternalWorker {
                worker,
                revision,
                event,
            } => self.apply_internal_worker_event(worker, revision, *event),
            Event::InternalWorkerRemoved { worker, revision } => {
                self.remove_internal_worker(worker, revision)
            }
            Event::Status { status } => {
                self.rewind_refresh_fence = false;
                self.set_worker_status(status);
            }
            // Command telemetry is an operational Web Console surface. The
            // TUI continues to render the final Bash ToolResult from history.
            Event::Command { .. } => {}
            Event::Completions { kind, entries } => {
                // Apply only if the popup is still on the same
                // (kind, prefix) the request was issued for; an
                // out-of-date reply (the user typed past it) is dropped.
                if let Some(state) = self.completion.as_mut()
                    && state.kind == kind
                {
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
                self.reset_run_state(self.worker_status);
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
                self.mark_orphan_compacts_incomplete();
                self.quit = true;
            }
        }
        None
    }

    fn reset_run_state(&mut self, status: WorkerStatus) {
        self.set_worker_status(status);
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
        self.reset_run_state(WorkerStatus::Idle);
        self.blocks.push(Block::Alert {
            level: AlertLevel::Warn,
            source: AlertSource::Worker,
            message: hint,
        });
    }

    fn apply_in_flight_snapshot(&mut self, snapshot: InFlightSnapshot) {
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
        self.input_mode = CommandInputMode::Command;
        self.completion = None;
        self.command_completion_selected = None;
        self.quit_confirm = None;
    }

    pub fn exit_command_mode(&mut self) {
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
        if let Some(Method::ListRewindTargets) = result.method.as_ref() {
            self.completion = None;
            self.rewind_picker = None;
            self.rewind_request_pending = true;
        }
        result.method
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
        app.restore_session(&snapshot.session, None);
        app.apply_in_flight_snapshot(snapshot.in_flight);
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
        self.session_context_tokens = greeting.context_tokens;
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
        if let Some(greeting) = greeting.clone() {
            self.greeting = Some(greeting.clone());
            self.context_window = greeting.context_window;
            self.session_context_tokens = greeting.context_tokens;
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
                SessionSnapshotEntryData::RunError { message } => {
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
            session_store::SystemItem::FileAttachment { body, .. }
            | session_store::SystemItem::SkillActivation { body, .. }
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
            session: protocol::SessionSnapshot { entries: vec![] },
            input: vec![Segment::text("selected rewind input")],
            summary: summary(3),
        });

        assert!(app.rewind_picker.is_none());
        assert!(!blocks_contain(&app, "old post-target output"));
        assert_eq!(composer_text(&app), "selected rewind input");
        assert!(blocks_contain(&app, "Rewound session"));
    }

    #[test]
    fn rewind_applied_clears_old_tail_even_when_greeting_is_missing() {
        let mut app = app_with_rewind_picker();
        app.blocks.push(Block::AssistantText {
            text: "old live tail without greeting".into(),
        });

        app.handle_worker_event(Event::RewindApplied {
            session: protocol::SessionSnapshot { entries: vec![] },
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
            session: protocol::SessionSnapshot { entries: vec![] },
            input: vec![Segment::text("rewound input")],
            summary: summary(2),
        });
        app.handle_worker_event(Event::TextDelta {
            text: "stale tail after rewind".into(),
        });
        assert!(!blocks_contain(&app, "stale tail after rewind"));

        app.handle_worker_event(Event::Status {
            status: WorkerStatus::Idle,
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
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));

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
            Some(Method::Run { input }) => input,
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
            Some(Method::ListCompletions { kind, prefix }) => {
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
            Some(Method::ListCompletions { kind, prefix }) => {
                assert_eq!(kind, CompletionKind::File);
                assert_eq!(prefix, "s");
            }
            other => panic!("expected ListCompletions, got {other:?}"),
        }
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
            },
            CompletionEntry {
                value: "crates/agen".into(),
                is_dir: true,
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
            },
            CompletionEntry {
                value: "src/main.rs".into(),
                is_dir: false,
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
        for result in [RunResult::Paused, RunResult::Finished] {
            let mut app = App::new("test".into());
            let submitted = submit_text(&mut app, "normal run");
            app.handle_worker_event(Event::UserMessage {
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
    fn running_submit_is_queued_locally_and_clears_composer() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Running);
        insert_text(&mut app, "queued turn");

        assert!(app.submit_input().is_none());

        assert_eq!(app.queued_input_count(), 1);
        assert_eq!(app.next_queued_input_preview(), Some("queued turn"));
        assert_eq!(input_text(&app), "");
    }

    #[test]
    fn finished_run_auto_sends_next_queued_input() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Running);
        insert_text(&mut app, "next turn");
        assert!(app.submit_input().is_none());

        let method = app.handle_worker_event(Event::RunEnd {
            result: RunResult::Finished,
        });

        match method {
            Some(Method::Run { input }) => {
                assert_eq!(Segment::flatten_to_text(&input), "next turn");
            }
            other => panic!("expected queued Run, got {other:?}"),
        }
        assert_eq!(app.queued_input_count(), 0);
    }

    #[test]
    fn limit_reached_run_auto_sends_next_queued_input() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Running);
        insert_text(&mut app, "next after limit");
        assert!(app.submit_input().is_none());

        let method = app.handle_worker_event(Event::RunEnd {
            result: RunResult::LimitReached,
        });

        match method {
            Some(Method::Run { input }) => {
                assert_eq!(Segment::flatten_to_text(&input), "next after limit");
            }
            other => panic!("expected queued Run, got {other:?}"),
        }
        assert_eq!(app.queued_input_count(), 0);
    }

    #[test]
    fn paused_and_rolled_back_run_do_not_auto_send_queue() {
        for result in [RunResult::Paused, RunResult::RolledBack] {
            let mut app = App::new("test".into());
            app.set_worker_status(WorkerStatus::Running);
            insert_text(&mut app, "held turn");
            assert!(app.submit_input().is_none());

            let method = app.handle_worker_event(Event::RunEnd { result });

            assert!(method.is_none());
            assert_eq!(app.queued_input_count(), 1);
            assert_eq!(app.next_queued_input_preview(), Some("held turn"));
        }
    }

    #[test]
    fn paused_empty_submit_still_resumes_immediately() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Paused);

        assert!(matches!(app.submit_input(), Some(Method::Resume)));
        assert_eq!(app.queued_input_count(), 0);
    }

    #[test]
    fn queued_input_can_be_restored_to_composer_or_cleared() {
        let mut app = App::new("test".into());
        app.set_worker_status(WorkerStatus::Running);
        insert_text(&mut app, "edit me");
        assert!(app.submit_input().is_none());

        assert!(app.restore_next_queued_input_to_composer());
        assert_eq!(app.queued_input_count(), 0);
        assert_eq!(input_text(&app), "edit me");

        app.input.clear();
        insert_text(&mut app, "clear me");
        assert!(app.submit_input().is_none());
        assert_eq!(app.clear_queued_inputs(), 1);
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
            Some(Method::Run { input }) => input,
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
            status: WorkerStatus::Running,
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        assert!(matches!(app.worker_status, WorkerStatus::Running));
        assert!(app.running);
        assert_eq!(app.blocks.len(), 1);
        assert!(matches!(app.blocks.first(), Some(Block::Greeting(_))));
    }

    #[test]
    fn snapshot_replaces_live_error_with_one_durable_run_error_block() {
        let mut app = App::new("test".into());
        app.handle_worker_event(Event::Error {
            code: ErrorCode::ProviderError,
            message: "provider unavailable".into(),
        });
        app.handle_worker_event(Event::Status {
            status: WorkerStatus::Idle,
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
            interrupted: false,
            message: "provider unavailable".into(),
        };
        app.handle_worker_event(Event::Snapshot {
            greeting: test_greeting(),
            session: public_session(vec![serde_json::to_value(run_errored).unwrap()]),
            status: WorkerStatus::Idle,
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
                entries: Vec::new(),
            },
            status: WorkerStatus::Running,
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
        app.handle_worker_event(Event::SystemItem { item });

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
            session: protocol::SessionSnapshot {
                entries: Vec::new(),
            },
            in_flight: protocol::InFlightSnapshot::default(),
            error: None,
            internal_workers: Vec::new(),
        }
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
                entries: Vec::new(),
            },
            status: WorkerStatus::Idle,
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
                entries: Vec::new(),
            },
            status: WorkerStatus::Idle,
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
                    entries: Vec::new(),
                },
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
        app.handle_worker_event(Event::SystemItem { item });
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
        app.handle_worker_event(Event::SystemItem { item });
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
        }
    }

    #[test]
    fn snapshot_initializes_context_usage() {
        let mut app = App::new("test".into());
        let mut greeting = test_greeting();
        greeting.context_window = 123_000;
        greeting.context_tokens = 45_000;

        app.handle_worker_event(Event::Snapshot {
            session: protocol::SessionSnapshot {
                entries: Vec::new(),
            },
            greeting,
            status: WorkerStatus::Idle,
            in_flight: Default::default(),
            internal_workers: Vec::new(),
        });

        assert_eq!(app.context_window, 123_000);
        assert_eq!(app.session_context_tokens, 45_000);
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
        assert_eq!(app.run_upload_tokens, 2_000);
        assert_eq!(app.run_output_tokens, 9);
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
    fn compact_done_resets_session_context_tokens() {
        let mut app = App::new("test".into());
        app.session_context_tokens = 42_000;

        let mut lifecycle = test_compaction_lifecycle(protocol::CompactionLifecycleState::Done);
        lifecycle.new_segment_id = Some(uuid::Uuid::nil().to_string());
        app.handle_worker_event(Event::CompactDone { lifecycle });

        assert_eq!(app.session_context_tokens, 0);
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
            status: WorkerStatus::Running,
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
    fn input_history_records_queued_inputs_and_suppresses_consecutive_duplicates() {
        let mut app = App::new("test".into());
        app.running = true;

        for c in "repeat".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_none());
        assert_eq!(app.input_history_len(), 1);
        assert_eq!(app.queued_input_count(), 1);

        for c in "repeat".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_none());
        assert_eq!(app.input_history_len(), 1);
        assert_eq!(app.queued_input_count(), 2);

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
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));

        assert!(app.browse_input_history_older());
        assert_eq!(app.input.submit_segments(), original);
    }

    #[test]
    fn input_history_restores_non_empty_draft_after_newest() {
        let mut app = App::new("test".into());
        for c in "sent".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));

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
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));

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
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));
        for c in "second".chars() {
            app.insert_char(c);
        }
        assert!(matches!(app.submit_input(), Some(Method::Run { .. })));

        assert!(app.browse_input_history_older());
        assert!(app.browse_input_history_older());
        let method = app.submit_input();
        match method {
            Some(Method::Run { input }) => assert_eq!(Segment::flatten_to_text(&input), "first"),
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
