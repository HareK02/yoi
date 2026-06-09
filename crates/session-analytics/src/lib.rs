//! Read-only analytics for Yoi session JSONL logs.
//!
//! This crate intentionally parses the persisted JSON shape tolerantly with
//! `serde_json::Value` rather than depending on Pod runtime or TUI crates. The
//! report contains counts, paths, sizes, line/turn indexes, and bounded
//! diagnostics; raw user messages, tool arguments, and tool output snippets are
//! not emitted.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const MAX_DIAGNOSTICS: usize = 200;
const LARGE_EDIT_ARGUMENT_BYTES: usize = 8 * 1024;
const LARGE_RESULT_BYTES: usize = 16 * 1024;
const LARGE_RESULT_LINES: usize = 200;
const LARGE_READ_LINES: usize = 1_000;
const LARGE_GREP_HEAD_LIMIT: u64 = 250;

#[derive(Debug, Error)]
pub enum AnalyzeError {
    #[error("failed to open session log `{path}`: {source}")]
    Open { path: PathBuf, source: io::Error },
    #[error("failed to read session log `{path}`: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to inspect session log `{path}`: {source}")]
    Metadata { path: PathBuf, source: io::Error },
}

/// Analyze one explicit session JSONL path.
pub fn analyze_session(path: impl AsRef<Path>) -> Result<SessionReport, AnalyzeError> {
    Analyzer::analyze(path.as_ref())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionReport {
    pub input: InputSummary,
    pub entries: EntrySummary,
    pub tool_usage: ToolUsageSummary,
    pub file_reads: FileReadSummary,
    pub edits: EditWriteSummary,
    pub tool_results: ToolResultSizeSummary,
    pub context_lifecycle: ContextLifecycleSummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputSummary {
    pub path: PathBuf,
    pub byte_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntrySummary {
    pub total_lines: u64,
    pub parsed_entries: u64,
    pub malformed_entries: u64,
    pub unknown_entries: u64,
    pub turn_count_observed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolUsageSummary {
    pub total_tool_calls: u64,
    pub failed_tool_results: u64,
    pub counts_by_tool: BTreeMap<String, u64>,
    pub counts_by_kind: BTreeMap<String, u64>,
    pub calls_per_turn: Vec<TurnToolCallCount>,
    pub observations: Vec<ToolUsageObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnToolCallCount {
    pub turn_index: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolUsageObservation {
    pub kind: String,
    pub line: u64,
    pub turn_index: u64,
    pub tool_name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileReadSummary {
    pub total_read_calls: u64,
    pub repeated_by_path: Vec<RepeatedReadByPath>,
    pub repeated_by_range: Vec<RepeatedReadByRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedReadByPath {
    pub path: String,
    pub total_reads: u64,
    pub repeated_reads: u64,
    pub repeated_after_mutation: u64,
    pub repeated_after_context_event: u64,
    pub observations: Vec<RepeatedReadObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedReadByRange {
    pub path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub total_reads: u64,
    pub repeated_reads: u64,
    pub repeated_after_mutation: u64,
    pub repeated_after_context_event: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedReadObservation {
    pub previous_line: u64,
    pub line: u64,
    pub previous_turn_index: u64,
    pub turn_index: u64,
    pub same_offset_limit: bool,
    pub intervening_write_or_edit: bool,
    pub after_context_lifecycle_event: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditWriteSummary {
    pub total_edit_calls: u64,
    pub total_write_calls: u64,
    pub by_path: Vec<PathEditWriteStats>,
    pub large_argument_observations: Vec<LargeEditArgumentObservation>,
    pub replace_all_observations: Vec<ReplaceAllObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathEditWriteStats {
    pub path: String,
    pub edit_count: u64,
    pub write_count: u64,
    pub repeated_edits: bool,
    pub old_string_bytes_total: u64,
    pub new_string_bytes_total: u64,
    pub write_content_bytes_total: u64,
    pub replace_all_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LargeEditArgumentObservation {
    pub line: u64,
    pub turn_index: u64,
    pub tool_name: String,
    pub path: Option<String>,
    pub field: String,
    pub byte_size: u64,
    pub threshold_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplaceAllObservation {
    pub line: u64,
    pub turn_index: u64,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultSizeSummary {
    pub total_results: u64,
    pub failed_results: u64,
    pub large_results: Vec<ToolResultSizeObservation>,
    pub truncated_or_saved_bash_outputs: Vec<ToolResultSizeObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultSizeObservation {
    pub line: u64,
    pub turn_index: u64,
    pub tool_name: Option<String>,
    pub content_bytes: u64,
    pub content_lines: u64,
    pub summary_bytes: u64,
    pub summary_lines: u64,
    pub is_error: bool,
    pub observation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextLifecycleSummary {
    pub events: Vec<ContextLifecycleEvent>,
    pub repeated_reads_after_events: Vec<ContextCorrelationObservation>,
    pub repeated_tool_calls_after_events: Vec<ContextCorrelationObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextLifecycleEvent {
    pub line: u64,
    pub turn_index: u64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextCorrelationObservation {
    pub event_line: u64,
    pub event_kind: String,
    pub line: u64,
    pub turn_index: u64,
    pub observation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: Option<u64>,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone)]
struct ToolCallRecord {
    name: String,
}

#[derive(Debug, Clone)]
struct ReadRecord {
    offset: Option<u64>,
    limit: Option<u64>,
    line: u64,
    turn_index: u64,
    mutation_seq: u64,
    context_seq: u64,
    context_event_line: Option<u64>,
    context_event_kind: Option<String>,
}

#[derive(Debug, Default)]
struct ReadStatsBuilder {
    records: Vec<ReadRecord>,
}

#[derive(Debug, Default)]
struct PathMutationState {
    mutation_seq: u64,
}

#[derive(Debug, Default)]
struct PathEditWriteStatsBuilder {
    edit_count: u64,
    write_count: u64,
    old_string_bytes_total: u64,
    new_string_bytes_total: u64,
    write_content_bytes_total: u64,
    replace_all_count: u64,
}

struct Analyzer {
    path: PathBuf,
    byte_size: u64,
    total_lines: u64,
    parsed_entries: u64,
    malformed_entries: u64,
    unknown_entries: u64,
    current_turn: u64,
    max_turn_observed: u64,
    diagnostics: Vec<Diagnostic>,
    total_tool_calls: u64,
    failed_tool_results: u64,
    counts_by_tool: BTreeMap<String, u64>,
    counts_by_kind: BTreeMap<String, u64>,
    calls_per_turn: BTreeMap<u64, u64>,
    tool_usage_observations: Vec<ToolUsageObservation>,
    calls_by_id: HashMap<String, ToolCallRecord>,
    seen_call_signatures: HashMap<String, u64>,
    read_stats_by_path: BTreeMap<String, ReadStatsBuilder>,
    read_stats_by_range: BTreeMap<(String, Option<u64>, Option<u64>), ReadStatsBuilder>,
    path_mutations: BTreeMap<String, PathMutationState>,
    edit_stats_by_path: BTreeMap<String, PathEditWriteStatsBuilder>,
    total_read_calls: u64,
    total_edit_calls: u64,
    total_write_calls: u64,
    large_edit_arguments: Vec<LargeEditArgumentObservation>,
    replace_all_observations: Vec<ReplaceAllObservation>,
    total_tool_results: u64,
    failed_tool_result_items: u64,
    large_result_observations: Vec<ToolResultSizeObservation>,
    truncated_bash_observations: Vec<ToolResultSizeObservation>,
    context_events: Vec<ContextLifecycleEvent>,
    context_seq: u64,
    last_context_event: Option<ContextLifecycleEvent>,
    repeated_tool_after_context: Vec<ContextCorrelationObservation>,
}

impl Analyzer {
    fn analyze(path: &Path) -> Result<SessionReport, AnalyzeError> {
        let metadata = path.metadata().map_err(|source| AnalyzeError::Metadata {
            path: path.to_path_buf(),
            source,
        })?;
        let file = File::open(path).map_err(|source| AnalyzeError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut analyzer = Self::new(path.to_path_buf(), metadata.len());
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|source| AnalyzeError::Read {
                path: path.to_path_buf(),
                source,
            })?;
            analyzer.consume_line(&line);
        }
        Ok(analyzer.finish())
    }

    fn new(path: PathBuf, byte_size: u64) -> Self {
        Self {
            path,
            byte_size,
            total_lines: 0,
            parsed_entries: 0,
            malformed_entries: 0,
            unknown_entries: 0,
            current_turn: 0,
            max_turn_observed: 0,
            diagnostics: Vec::new(),
            total_tool_calls: 0,
            failed_tool_results: 0,
            counts_by_tool: BTreeMap::new(),
            counts_by_kind: BTreeMap::new(),
            calls_per_turn: BTreeMap::new(),
            tool_usage_observations: Vec::new(),
            calls_by_id: HashMap::new(),
            seen_call_signatures: HashMap::new(),
            read_stats_by_path: BTreeMap::new(),
            read_stats_by_range: BTreeMap::new(),
            path_mutations: BTreeMap::new(),
            edit_stats_by_path: BTreeMap::new(),
            total_read_calls: 0,
            total_edit_calls: 0,
            total_write_calls: 0,
            large_edit_arguments: Vec::new(),
            replace_all_observations: Vec::new(),
            total_tool_results: 0,
            failed_tool_result_items: 0,
            large_result_observations: Vec::new(),
            truncated_bash_observations: Vec::new(),
            context_events: Vec::new(),
            context_seq: 0,
            last_context_event: None,
            repeated_tool_after_context: Vec::new(),
        }
    }

    fn consume_line(&mut self, line: &str) {
        self.total_lines += 1;
        let line_no = self.total_lines;
        if line.trim().is_empty() {
            self.add_diagnostic(Some(line_no), "malformed_json", "empty JSONL entry");
            self.malformed_entries += 1;
            return;
        }
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(e) => {
                self.malformed_entries += 1;
                self.add_diagnostic(
                    Some(line_no),
                    "malformed_json",
                    format!("entry is not valid JSON: {e}"),
                );
                return;
            }
        };
        self.parsed_entries += 1;
        self.consume_entry(line_no, &value);
    }

    fn consume_entry(&mut self, line: u64, value: &Value) {
        let Some(kind) = value.get("kind").and_then(Value::as_str) else {
            self.unknown_entries += 1;
            self.add_diagnostic(
                Some(line),
                "unknown_entry",
                "entry has no string `kind` field",
            );
            return;
        };

        match kind {
            "segment_start" => {
                if value.get("compacted_from").is_some_and(|v| !v.is_null()) {
                    self.record_context_event(line, "segment_compacted_from");
                }
                if let Some(history) = value.get("history").and_then(Value::as_array) {
                    for item in history {
                        self.consume_history_item(line, item, true);
                    }
                }
            }
            "assistant_item" => {
                if let Some(item) = value.get("item") {
                    self.consume_history_item(line, item, false);
                } else {
                    self.add_diagnostic(Some(line), "unknown_entry", "assistant_item lacks `item`");
                }
            }
            "tool_result" => {
                if let Some(item) = value.get("item") {
                    self.consume_tool_result(line, item);
                } else {
                    self.add_diagnostic(Some(line), "unknown_entry", "tool_result lacks `item`");
                }
            }
            "turn_end" => {
                if let Some(turn_count) = value.get("turn_count").and_then(Value::as_u64) {
                    self.current_turn = turn_count;
                    self.max_turn_observed = self.max_turn_observed.max(turn_count);
                }
            }
            "invoke" | "user_input" | "system_item" | "run_completed" | "run_errored"
            | "config_changed" | "llm_usage" => {}
            "extension" => {
                if extension_looks_context_related(value) {
                    self.record_context_event(line, "context_extension");
                }
            }
            other => {
                self.unknown_entries += 1;
                self.add_diagnostic(
                    Some(line),
                    "unknown_entry",
                    format!("unknown session entry kind `{other}`"),
                );
            }
        }
    }

    fn consume_history_item(&mut self, line: u64, item: &Value, seeded_history: bool) {
        let item_kind = item.get("kind").and_then(Value::as_str);
        match item_kind {
            Some("tool_call") => self.consume_tool_call(line, item, seeded_history),
            Some("tool_result") => self.consume_tool_result(line, item),
            Some("message" | "reasoning") => {}
            Some(other) => self.add_diagnostic(
                Some(line),
                "unknown_item",
                format!("unknown logged item kind `{other}`"),
            ),
            None => self.add_diagnostic(
                Some(line),
                "unknown_item",
                "logged item has no string `kind` field",
            ),
        }
    }

    fn consume_tool_call(&mut self, line: u64, item: &Value, seeded_history: bool) {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            self.add_diagnostic(
                Some(line),
                "unknown_tool_call",
                "tool_call lacks string `name`",
            );
            return;
        };
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
        let args_value = serde_json::from_str::<Value>(arguments).ok();

        self.total_tool_calls += 1;
        *self.counts_by_tool.entry(name.to_string()).or_insert(0) += 1;
        *self
            .counts_by_kind
            .entry(tool_kind(name).to_string())
            .or_insert(0) += 1;
        *self.calls_per_turn.entry(self.current_turn).or_insert(0) += 1;

        if let Some(id) = call_id {
            self.calls_by_id.insert(
                id,
                ToolCallRecord {
                    name: name.to_string(),
                },
            );
        }

        let signature = stable_tool_signature(name, &args_value, arguments.len());
        if let Some(context_seq) = self.seen_call_signatures.get(&signature)
            && *context_seq < self.context_seq
            && let Some(event) = self.last_context_event.clone()
        {
            self.repeated_tool_after_context
                .push(ContextCorrelationObservation {
                    event_line: event.line,
                    event_kind: event.kind,
                    line,
                    turn_index: self.current_turn,
                    observation: format!(
                        "repeated `{name}` call after context lifecycle event; correlation only"
                    ),
                });
        }
        self.seen_call_signatures
            .insert(signature, self.context_seq);

        if seeded_history {
            self.tool_usage_observations.push(ToolUsageObservation {
                kind: "seeded_history_tool_call".to_string(),
                line,
                turn_index: self.current_turn,
                tool_name: name.to_string(),
                detail: "tool call came from segment_start.history, so exact original turn boundary may be approximate".to_string(),
            });
        }

        self.consume_tool_specific_call(line, name, args_value.as_ref());
    }

    fn consume_tool_specific_call(&mut self, line: u64, name: &str, args: Option<&Value>) {
        match name {
            "Read" => {
                self.total_read_calls += 1;
                let Some(args) = args else {
                    self.add_diagnostic(
                        Some(line),
                        "tool_arguments",
                        "Read arguments were not JSON",
                    );
                    return;
                };
                let Some(path) = path_arg(args) else {
                    self.add_diagnostic(
                        Some(line),
                        "tool_arguments",
                        "Read arguments lack a path field",
                    );
                    return;
                };
                let offset = args.get("offset").and_then(Value::as_u64);
                let limit = args.get("limit").and_then(Value::as_u64);
                if limit.is_some_and(|limit| limit >= LARGE_READ_LINES as u64) {
                    self.tool_usage_observations.push(ToolUsageObservation {
                        kind: "large_read_request".to_string(),
                        line,
                        turn_index: self.current_turn,
                        tool_name: name.to_string(),
                        detail: format!("Read limit is at least {LARGE_READ_LINES} lines"),
                    });
                }
                let mutation_seq = self
                    .path_mutations
                    .get(path)
                    .map(|state| state.mutation_seq)
                    .unwrap_or(0);
                let record = ReadRecord {
                    offset,
                    limit,
                    line,
                    turn_index: self.current_turn,
                    mutation_seq,
                    context_seq: self.context_seq,
                    context_event_line: self.last_context_event.as_ref().map(|event| event.line),
                    context_event_kind: self
                        .last_context_event
                        .as_ref()
                        .map(|event| event.kind.clone()),
                };
                self.read_stats_by_path
                    .entry(path.to_string())
                    .or_default()
                    .records
                    .push(record.clone());
                self.read_stats_by_range
                    .entry((path.to_string(), offset, limit))
                    .or_default()
                    .records
                    .push(record);
            }
            "Edit" | "Write" => self.consume_mutating_file_call(line, name, args),
            "Bash" => {
                if let Some(args) = args
                    && let Some(command) = args.get("command").and_then(Value::as_str)
                    && bash_command_looks_like_file_inspection(command)
                {
                    self.tool_usage_observations.push(ToolUsageObservation {
                        kind: "bash_file_inspection".to_string(),
                        line,
                        turn_index: self.current_turn,
                        tool_name: name.to_string(),
                        detail: "Bash command appears to inspect files; this is an observation, not blame".to_string(),
                    });
                }
            }
            "Grep" => {
                if let Some(args) = args
                    && args
                        .get("head_limit")
                        .and_then(Value::as_u64)
                        .is_some_and(|limit| limit >= LARGE_GREP_HEAD_LIMIT)
                {
                    self.tool_usage_observations.push(ToolUsageObservation {
                        kind: "large_grep_request".to_string(),
                        line,
                        turn_index: self.current_turn,
                        tool_name: name.to_string(),
                        detail: format!("Grep head_limit is at least {LARGE_GREP_HEAD_LIMIT}"),
                    });
                }
            }
            _ => {}
        }
    }

    fn consume_mutating_file_call(&mut self, line: u64, name: &str, args: Option<&Value>) {
        let Some(args) = args else {
            self.add_diagnostic(
                Some(line),
                "tool_arguments",
                format!("{name} arguments were not JSON"),
            );
            return;
        };
        let path = path_arg(args).map(str::to_owned);
        if let Some(path) = path.as_deref() {
            let state = self.path_mutations.entry(path.to_string()).or_default();
            state.mutation_seq += 1;
        }

        match name {
            "Edit" => {
                self.total_edit_calls += 1;
                let old_size = args.get("old_string").and_then(Value::as_str).map(byte_len);
                let new_size = args.get("new_string").and_then(Value::as_str).map(byte_len);
                let replace_all = args.get("replace_all").and_then(Value::as_bool) == Some(true);
                if let Some(path_value) = path.as_deref() {
                    let stats = self
                        .edit_stats_by_path
                        .entry(path_value.to_string())
                        .or_default();
                    stats.edit_count += 1;
                    if let Some(size) = old_size {
                        stats.old_string_bytes_total += size as u64;
                    }
                    if let Some(size) = new_size {
                        stats.new_string_bytes_total += size as u64;
                    }
                    if replace_all {
                        stats.replace_all_count += 1;
                    }
                }
                if let Some(size) = old_size {
                    self.maybe_large_edit_arg(line, name, path.as_deref(), "old_string", size);
                }
                if let Some(size) = new_size {
                    self.maybe_large_edit_arg(line, name, path.as_deref(), "new_string", size);
                }
                if replace_all {
                    self.replace_all_observations.push(ReplaceAllObservation {
                        line,
                        turn_index: self.current_turn,
                        path,
                    });
                }
            }
            "Write" => {
                self.total_write_calls += 1;
                let content_size = args.get("content").and_then(Value::as_str).map(byte_len);
                if let Some(path_value) = path.as_deref() {
                    let stats = self
                        .edit_stats_by_path
                        .entry(path_value.to_string())
                        .or_default();
                    stats.write_count += 1;
                    if let Some(size) = content_size {
                        stats.write_content_bytes_total += size as u64;
                    }
                }
                if let Some(size) = content_size {
                    self.maybe_large_edit_arg(line, name, path.as_deref(), "content", size);
                }
            }
            _ => {}
        }
    }

    fn maybe_large_edit_arg(
        &mut self,
        line: u64,
        tool_name: &str,
        path: Option<&str>,
        field: &str,
        size: usize,
    ) {
        if size >= LARGE_EDIT_ARGUMENT_BYTES {
            self.large_edit_arguments
                .push(LargeEditArgumentObservation {
                    line,
                    turn_index: self.current_turn,
                    tool_name: tool_name.to_string(),
                    path: path.map(str::to_owned),
                    field: field.to_string(),
                    byte_size: size as u64,
                    threshold_bytes: LARGE_EDIT_ARGUMENT_BYTES as u64,
                });
        }
    }

    fn consume_tool_result(&mut self, line: u64, item: &Value) {
        if item.get("kind").and_then(Value::as_str) != Some("tool_result") {
            self.add_diagnostic(
                Some(line),
                "unknown_tool_result",
                "tool_result item has unexpected kind",
            );
            return;
        }
        self.total_tool_results += 1;
        let call_id = item.get("call_id").and_then(Value::as_str);
        let tool_call = call_id.and_then(|id| self.calls_by_id.get(id));
        let tool_name = tool_call.map(|call| call.name.clone());
        let is_error = item
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_error {
            self.failed_tool_results += 1;
            self.failed_tool_result_items += 1;
        }

        let content = item.get("content").and_then(Value::as_str).unwrap_or("");
        let summary = item.get("summary").and_then(Value::as_str).unwrap_or("");
        let content_bytes = byte_len(content) as u64;
        let content_lines = line_count(content) as u64;
        let summary_bytes = byte_len(summary) as u64;
        let summary_lines = line_count(summary) as u64;
        let mut reasons = Vec::new();
        if content_bytes >= LARGE_RESULT_BYTES as u64 || summary_bytes >= LARGE_RESULT_BYTES as u64
        {
            reasons.push(format!("result byte size is at least {LARGE_RESULT_BYTES}"));
        }
        if content_lines >= LARGE_RESULT_LINES as u64 || summary_lines >= LARGE_RESULT_LINES as u64
        {
            reasons.push(format!(
                "result line count is at least {LARGE_RESULT_LINES}"
            ));
        }
        if let Some(name) = tool_name.as_deref()
            && matches!(name, "Read" | "Grep" | "Bash" | "WebFetch" | "WebSearch")
            && (!reasons.is_empty() || content_bytes > 0 || summary_bytes > 0)
            && (content_bytes >= LARGE_RESULT_BYTES as u64
                || content_lines >= LARGE_RESULT_LINES as u64
                || summary_bytes >= LARGE_RESULT_BYTES as u64
                || summary_lines >= LARGE_RESULT_LINES as u64)
        {
            reasons.push(format!("large observable {name} result"));
        }
        if !reasons.is_empty() {
            self.large_result_observations
                .push(ToolResultSizeObservation {
                    line,
                    turn_index: self.current_turn,
                    tool_name: tool_name.clone(),
                    content_bytes,
                    content_lines,
                    summary_bytes,
                    summary_lines,
                    is_error,
                    observation: reasons.join("; "),
                });
        }
        if tool_name.as_deref() == Some("Bash")
            && bash_result_mentions_saved_or_truncated(summary, content)
        {
            self.truncated_bash_observations
                .push(ToolResultSizeObservation {
                    line,
                    turn_index: self.current_turn,
                    tool_name,
                    content_bytes,
                    content_lines,
                    summary_bytes,
                    summary_lines,
                    is_error,
                    observation: "Bash result appears truncated and/or saved to a file".to_string(),
                });
        }
    }

    fn record_context_event(&mut self, line: u64, kind: &str) {
        self.context_seq += 1;
        let event = ContextLifecycleEvent {
            line,
            turn_index: self.current_turn,
            kind: kind.to_string(),
        };
        self.last_context_event = Some(event.clone());
        self.context_events.push(event);
    }

    fn add_diagnostic(&mut self, line: Option<u64>, kind: &str, message: impl Into<String>) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(Diagnostic {
                line,
                kind: kind.to_string(),
                message: message.into(),
            });
        }
    }

    fn finish(self) -> SessionReport {
        let (repeated_by_path, repeated_read_context) =
            build_repeated_by_path(&self.read_stats_by_path);
        let repeated_by_range = build_repeated_by_range(&self.read_stats_by_range);
        let by_path = self
            .edit_stats_by_path
            .into_iter()
            .map(|(path, stats)| PathEditWriteStats {
                path,
                edit_count: stats.edit_count,
                write_count: stats.write_count,
                repeated_edits: stats.edit_count > 1,
                old_string_bytes_total: stats.old_string_bytes_total,
                new_string_bytes_total: stats.new_string_bytes_total,
                write_content_bytes_total: stats.write_content_bytes_total,
                replace_all_count: stats.replace_all_count,
            })
            .collect();
        let calls_per_turn = self
            .calls_per_turn
            .into_iter()
            .map(|(turn_index, count)| TurnToolCallCount { turn_index, count })
            .collect();
        SessionReport {
            input: InputSummary {
                path: self.path,
                byte_size: self.byte_size,
            },
            entries: EntrySummary {
                total_lines: self.total_lines,
                parsed_entries: self.parsed_entries,
                malformed_entries: self.malformed_entries,
                unknown_entries: self.unknown_entries,
                turn_count_observed: self.max_turn_observed.max(self.current_turn),
            },
            tool_usage: ToolUsageSummary {
                total_tool_calls: self.total_tool_calls,
                failed_tool_results: self.failed_tool_results,
                counts_by_tool: self.counts_by_tool,
                counts_by_kind: self.counts_by_kind,
                calls_per_turn,
                observations: self.tool_usage_observations,
            },
            file_reads: FileReadSummary {
                total_read_calls: self.total_read_calls,
                repeated_by_path,
                repeated_by_range,
            },
            edits: EditWriteSummary {
                total_edit_calls: self.total_edit_calls,
                total_write_calls: self.total_write_calls,
                by_path,
                large_argument_observations: self.large_edit_arguments,
                replace_all_observations: self.replace_all_observations,
            },
            tool_results: ToolResultSizeSummary {
                total_results: self.total_tool_results,
                failed_results: self.failed_tool_result_items,
                large_results: self.large_result_observations,
                truncated_or_saved_bash_outputs: self.truncated_bash_observations,
            },
            context_lifecycle: ContextLifecycleSummary {
                events: self.context_events,
                repeated_reads_after_events: repeated_read_context,
                repeated_tool_calls_after_events: self.repeated_tool_after_context,
            },
            diagnostics: self.diagnostics,
        }
    }
}

fn build_repeated_by_path(
    stats: &BTreeMap<String, ReadStatsBuilder>,
) -> (Vec<RepeatedReadByPath>, Vec<ContextCorrelationObservation>) {
    let mut repeated = Vec::new();
    let mut context_correlations = Vec::new();
    for (path, builder) in stats {
        if builder.records.len() < 2 {
            continue;
        }
        let mut observations = Vec::new();
        let mut repeated_after_mutation = 0;
        let mut repeated_after_context = 0;
        for pair in builder.records.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            let intervening_write_or_edit = current.mutation_seq > previous.mutation_seq;
            let after_context_lifecycle_event = current.context_seq > previous.context_seq;
            if intervening_write_or_edit {
                repeated_after_mutation += 1;
            }
            if after_context_lifecycle_event {
                repeated_after_context += 1;
                context_correlations.push(ContextCorrelationObservation {
                    event_line: current.context_event_line.unwrap_or(0),
                    event_kind: current
                        .context_event_kind
                        .clone()
                        .unwrap_or_else(|| "context_lifecycle_event".to_string()),
                    line: current.line,
                    turn_index: current.turn_index,
                    observation: format!(
                        "repeated Read for `{path}` after a context lifecycle event; correlation only"
                    ),
                });
            }
            observations.push(RepeatedReadObservation {
                previous_line: previous.line,
                line: current.line,
                previous_turn_index: previous.turn_index,
                turn_index: current.turn_index,
                same_offset_limit: previous.offset == current.offset
                    && previous.limit == current.limit,
                intervening_write_or_edit,
                after_context_lifecycle_event,
            });
        }
        repeated.push(RepeatedReadByPath {
            path: path.clone(),
            total_reads: builder.records.len() as u64,
            repeated_reads: builder.records.len().saturating_sub(1) as u64,
            repeated_after_mutation,
            repeated_after_context_event: repeated_after_context,
            observations,
        });
    }
    (repeated, context_correlations)
}

fn build_repeated_by_range(
    stats: &BTreeMap<(String, Option<u64>, Option<u64>), ReadStatsBuilder>,
) -> Vec<RepeatedReadByRange> {
    let mut repeated = Vec::new();
    for ((path, offset, limit), builder) in stats {
        if builder.records.len() < 2 {
            continue;
        }
        let mut repeated_after_mutation = 0;
        let mut repeated_after_context = 0;
        for pair in builder.records.windows(2) {
            if pair[1].mutation_seq > pair[0].mutation_seq {
                repeated_after_mutation += 1;
            }
            if pair[1].context_seq > pair[0].context_seq {
                repeated_after_context += 1;
            }
        }
        repeated.push(RepeatedReadByRange {
            path: path.clone(),
            offset: *offset,
            limit: *limit,
            total_reads: builder.records.len() as u64,
            repeated_reads: builder.records.len().saturating_sub(1) as u64,
            repeated_after_mutation,
            repeated_after_context_event: repeated_after_context,
        });
    }
    repeated
}

fn path_arg(args: &Value) -> Option<&str> {
    args.get("file_path")
        .or_else(|| args.get("path"))
        .and_then(Value::as_str)
}

fn byte_len(value: &str) -> usize {
    value.len()
}

fn line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value.lines().count()
    }
}

fn tool_kind(name: &str) -> &'static str {
    match name {
        "Read" | "Write" | "Edit" | "Glob" | "Grep" => "filesystem",
        "Bash" => "shell",
        "WebFetch" | "WebSearch" => "web",
        "SpawnPod" | "SendToPod" | "ReadPodOutput" | "ListPods" | "StopPod" | "RestorePod"
        | "SendToPeerPod" => "pod",
        name if name.starts_with("Memory") || name.starts_with("Knowledge") => "memory",
        name if name.starts_with("Ticket") => "ticket",
        name if name.starts_with("Task") => "task",
        _ => "other",
    }
}

fn stable_tool_signature(name: &str, args: &Option<Value>, arg_len: usize) -> String {
    match args {
        Some(value) => format!(
            "{name}:{}",
            serde_json::to_string(value).unwrap_or_default()
        ),
        None => format!("{name}:invalid-json:{arg_len}"),
    }
}

fn bash_command_looks_like_file_inspection(command: &str) -> bool {
    let trimmed = command.trim_start();
    [
        "cat ", "head ", "tail ", "sed ", "awk ", "grep ", "rg ", "find ", "ls ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix) || trimmed.contains(&format!("&& {prefix}")))
}

fn bash_result_mentions_saved_or_truncated(summary: &str, content: &str) -> bool {
    let text = format!("{summary}\n{content}").to_ascii_lowercase();
    (text.contains("saved to") || text.contains("bash-output"))
        && (text.contains("last 80 lines")
            || text.contains("truncated")
            || text.contains("full output"))
}

fn extension_looks_context_related(value: &Value) -> bool {
    let domain = value.get("domain").and_then(Value::as_str).unwrap_or("");
    if domain.contains("compact") || domain.contains("prun") || domain.contains("context") {
        return true;
    }
    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    let payload_text = serde_json::to_string(&payload)
        .unwrap_or_default()
        .to_ascii_lowercase();
    payload_text.contains("compact")
        || payload_text.contains("prun")
        || payload_text.contains("context")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn write_fixture(lines: &[String]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        use std::io::Write;
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    fn tool_call(id: &str, name: &str, args: Value) -> String {
        serde_json::json!({
            "kind": "assistant_item",
            "ts": 1,
            "item": {
                "kind": "tool_call",
                "call_id": id,
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
        .to_string()
    }

    fn tool_result(id: &str, summary: &str, content: Option<&str>, is_error: bool) -> String {
        serde_json::json!({
            "kind": "tool_result",
            "ts": 1,
            "item": {
                "kind": "tool_result",
                "call_id": id,
                "summary": summary,
                "content": content,
                "is_error": is_error
            }
        })
        .to_string()
    }

    fn turn_end(turn: u64) -> String {
        serde_json::json!({"kind": "turn_end", "ts": 1, "turn_count": turn}).to_string()
    }

    #[test]
    fn repeated_reads_track_intervening_mutation() {
        let fixture = write_fixture(&[
            tool_call(
                "r1",
                "Read",
                serde_json::json!({"file_path":"/tmp/a","offset":0,"limit":20}),
            ),
            tool_call(
                "r2",
                "Read",
                serde_json::json!({"file_path":"/tmp/a","offset":0,"limit":20}),
            ),
            tool_call(
                "e1",
                "Edit",
                serde_json::json!({"file_path":"/tmp/a","old_string":"x","new_string":"y"}),
            ),
            tool_call(
                "r3",
                "Read",
                serde_json::json!({"file_path":"/tmp/a","offset":0,"limit":20}),
            ),
        ]);
        let report = analyze_session(fixture.path()).unwrap();
        let repeated = &report.file_reads.repeated_by_path[0];
        assert_eq!(repeated.path, "/tmp/a");
        assert_eq!(repeated.total_reads, 3);
        assert_eq!(repeated.repeated_reads, 2);
        assert_eq!(repeated.repeated_after_mutation, 1);
        assert!(!repeated.observations[0].intervening_write_or_edit);
        assert!(repeated.observations[1].intervening_write_or_edit);
        assert_eq!(
            report.file_reads.repeated_by_range[0].repeated_after_mutation,
            1
        );
    }

    #[test]
    fn large_edit_argument_and_replace_all_are_observed() {
        let large = "x".repeat(LARGE_EDIT_ARGUMENT_BYTES);
        let fixture = write_fixture(&[tool_call(
            "e1",
            "Edit",
            serde_json::json!({
                "file_path":"/tmp/a",
                "old_string": large,
                "new_string":"short",
                "replace_all": true
            }),
        )]);
        let report = analyze_session(fixture.path()).unwrap();
        let stats = &report.edits.by_path[0];
        assert_eq!(stats.edit_count, 1);
        assert_eq!(stats.replace_all_count, 1);
        assert_eq!(report.edits.large_argument_observations.len(), 1);
        assert_eq!(
            report.edits.large_argument_observations[0].field,
            "old_string"
        );
        assert_eq!(report.edits.replace_all_observations.len(), 1);
    }

    #[test]
    fn tool_failure_counting_and_calls_per_turn() {
        let fixture = write_fixture(&[
            tool_call("r1", "Read", serde_json::json!({"file_path":"/tmp/a"})),
            tool_result("r1", "permission denied", None, true),
            tool_call(
                "g1",
                "Grep",
                serde_json::json!({"path":"/tmp","pattern":"x"}),
            ),
            turn_end(1),
            tool_call("b1", "Bash", serde_json::json!({"command":"echo ok"})),
        ]);
        let report = analyze_session(fixture.path()).unwrap();
        assert_eq!(report.tool_usage.total_tool_calls, 3);
        assert_eq!(report.tool_usage.failed_tool_results, 1);
        assert_eq!(report.tool_results.failed_results, 1);
        assert_eq!(report.tool_usage.calls_per_turn[0].turn_index, 0);
        assert_eq!(report.tool_usage.calls_per_turn[0].count, 2);
        assert_eq!(report.tool_usage.calls_per_turn[1].turn_index, 1);
        assert_eq!(report.tool_usage.calls_per_turn[1].count, 1);
    }

    #[test]
    fn large_and_truncated_results_are_observed_without_content() {
        let large_content = (0..=LARGE_RESULT_LINES)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let fixture = write_fixture(&[
            tool_call(
                "b1",
                "Bash",
                serde_json::json!({"command":"for i in $(seq 1 500); do echo $i; done"}),
            ),
            tool_result(
                "b1",
                "full output is saved to /run/user/1000/yoi/bash-output/x and only the LAST 80 lines are returned",
                Some(&large_content),
                false,
            ),
        ]);
        let report = analyze_session(fixture.path()).unwrap();
        assert_eq!(report.tool_results.large_results.len(), 1);
        assert_eq!(
            report.tool_results.large_results[0].tool_name.as_deref(),
            Some("Bash")
        );
        assert!(report.tool_results.large_results[0].content_lines >= LARGE_RESULT_LINES as u64);
        assert_eq!(report.tool_results.truncated_or_saved_bash_outputs.len(), 1);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("line 42"));
    }

    #[test]
    fn compaction_correlation_is_reported_as_correlation_only() {
        let fixture = write_fixture(&[
            tool_call("r1", "Read", serde_json::json!({"file_path":"/tmp/a"})),
            serde_json::json!({
                "kind": "segment_start",
                "ts": 2,
                "session_id": "00000000-0000-7000-8000-000000000000",
                "config": {},
                "system_prompt": null,
                "history": [],
                "compacted_from": {"segment_id":"00000000-0000-7000-8000-000000000001", "at_turn_index": 1}
            }).to_string(),
            tool_call("r2", "Read", serde_json::json!({"file_path":"/tmp/a"})),
        ]);
        let report = analyze_session(fixture.path()).unwrap();
        assert_eq!(report.context_lifecycle.events.len(), 1);
        assert_eq!(
            report.file_reads.repeated_by_path[0].repeated_after_context_event,
            1
        );
        assert!(
            report.context_lifecycle.repeated_reads_after_events[0]
                .observation
                .contains("correlation only")
        );
    }

    #[test]
    fn malformed_and_unknown_jsonl_entries_are_bounded_diagnostics() {
        let fixture = write_fixture(&[
            "{not-json".to_string(),
            serde_json::json!({"kind":"future_entry","ts":1}).to_string(),
            serde_json::json!({"ts":1}).to_string(),
        ]);
        let report = analyze_session(fixture.path()).unwrap();
        assert_eq!(report.entries.total_lines, 3);
        assert_eq!(report.entries.malformed_entries, 1);
        assert_eq!(report.entries.unknown_entries, 2);
        assert_eq!(report.diagnostics.len(), 3);
    }

    #[test]
    fn bash_file_inspection_is_detected_as_observation() {
        let fixture = write_fixture(&[tool_call(
            "b1",
            "Bash",
            serde_json::json!({"command":"cat crates/yoi/src/main.rs"}),
        )]);
        let report = analyze_session(fixture.path()).unwrap();
        assert_eq!(
            report.tool_usage.observations[0].kind,
            "bash_file_inspection"
        );
        assert!(
            report.tool_usage.observations[0]
                .detail
                .contains("observation")
        );
    }
}
