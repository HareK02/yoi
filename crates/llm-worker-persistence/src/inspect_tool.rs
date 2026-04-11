//! Built-in `inspect` tool for retrieving stored blob content.
//!
//! When large tool outputs are stored in a [`BlobStore`], only a summary
//! with a `[blob:<id>]` reference is placed in conversation history.
//! This tool lets the LLM retrieve details on demand, with optional
//! selectors for partial access.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta};
use llm_worker::state::Mutable;
use llm_worker::Worker;
use llm_worker::llm_client::LlmClient;

use crate::blob_store::{BlobId, BlobStore};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Maximum lines shown in the default text preview.
const DEFAULT_PREVIEW_LINES: usize = 50;
/// Maximum array elements shown in the default preview.
const DEFAULT_PREVIEW_ELEMENTS: usize = 5;
/// Maximum object keys whose values are shown in the default preview.
const DEFAULT_PREVIEW_KEYS: usize = 3;

// ─── Selector ────────────────────────────────────────────────────────────────

/// Parsed selector for partial blob content retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selector {
    /// Extract a range of lines (1-based, inclusive).
    Lines { start: usize, end: usize },
    /// Extract a range of array elements (0-based, exclusive end).
    Slice { start: usize, end: usize },
    /// Extract a specific key from a JSON object.
    Key(String),
}

fn parse_selector(s: &str) -> Result<Selector, ToolError> {
    if let Some(rest) = s.strip_prefix("lines:") {
        let (a, b) = rest
            .split_once('-')
            .ok_or_else(|| ToolError::InvalidArgument(format!(
                "invalid lines selector '{s}': expected format lines:N-M"
            )))?;
        let start: usize = a.parse().map_err(|_| {
            ToolError::InvalidArgument(format!("invalid start line number: '{a}'"))
        })?;
        let end: usize = b.parse().map_err(|_| {
            ToolError::InvalidArgument(format!("invalid end line number: '{b}'"))
        })?;
        if start == 0 {
            return Err(ToolError::InvalidArgument(
                "line numbers are 1-based, got 0".into(),
            ));
        }
        if start > end {
            return Err(ToolError::InvalidArgument(format!(
                "start line ({start}) must be <= end line ({end})"
            )));
        }
        Ok(Selector::Lines { start, end })
    } else if let Some(rest) = s.strip_prefix("slice:") {
        let (a, b) = rest
            .split_once("..")
            .ok_or_else(|| ToolError::InvalidArgument(format!(
                "invalid slice selector '{s}': expected format slice:N..M"
            )))?;
        let start: usize = a.parse().map_err(|_| {
            ToolError::InvalidArgument(format!("invalid start index: '{a}'"))
        })?;
        let end: usize = b.parse().map_err(|_| {
            ToolError::InvalidArgument(format!("invalid end index: '{b}'"))
        })?;
        if start > end {
            return Err(ToolError::InvalidArgument(format!(
                "start index ({start}) must be <= end index ({end})"
            )));
        }
        Ok(Selector::Slice { start, end })
    } else if let Some(rest) = s.strip_prefix("key:") {
        if rest.is_empty() {
            return Err(ToolError::InvalidArgument("key name must not be empty".into()));
        }
        Ok(Selector::Key(rest.to_string()))
    } else {
        Err(ToolError::InvalidArgument(format!(
            "unrecognized selector format: '{s}'. Expected: lines:N-M, slice:N..M, or key:NAME"
        )))
    }
}

// ─── InspectTool ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct InspectArgs {
    blob_id: String,
    selector: Option<String>,
}

/// Built-in tool that retrieves stored blob content.
pub struct InspectTool<B: BlobStore> {
    blob_store: Arc<B>,
}

impl<B: BlobStore> InspectTool<B> {
    pub fn new(blob_store: Arc<B>) -> Self {
        Self { blob_store }
    }
}

impl<B: BlobStore + 'static> InspectTool<B> {
    /// Create a [`ToolDefinition`] factory for this tool.
    pub fn tool_definition(blob_store: Arc<B>) -> ToolDefinition {
        Arc::new(move || {
            let meta = ToolMeta::new("inspect")
                .description(
                    "Retrieve content from a stored blob referenced by [blob:<id>] in conversation history. \
                     Supports selectors for partial access: \
                     'lines:N-M' (text line range, 1-based inclusive), \
                     'slice:N..M' (array element range, 0-based exclusive end), \
                     'key:NAME' (object key lookup). \
                     Without a selector, returns metadata and a preview.",
                )
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "blob_id": {
                            "type": "string",
                            "description": "The blob UUID from a [blob:<id>] reference"
                        },
                        "selector": {
                            "type": "string",
                            "description": "Optional: 'lines:N-M', 'slice:N..M', or 'key:NAME'"
                        }
                    },
                    "required": ["blob_id"]
                }));
            let tool = Arc::new(InspectTool::new(Arc::clone(&blob_store))) as Arc<dyn Tool>;
            (meta, tool)
        })
    }
}

#[async_trait]
impl<B: BlobStore + 'static> Tool for InspectTool<B> {
    async fn execute(&self, input_json: &str) -> Result<String, ToolError> {
        let args: InspectArgs = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid arguments: {e}")))?;

        let blob_id: BlobId = args
            .blob_id
            .parse()
            .map_err(|_| ToolError::InvalidArgument(format!(
                "invalid blob_id: '{}' is not a valid UUID", args.blob_id
            )))?;

        let content = self
            .blob_store
            .load(blob_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("{e}")))?;

        match args.selector {
            None => Ok(default_view(&content)),
            Some(sel) => {
                let selector = parse_selector(&sel)?;
                apply_selector(&content, &selector)
            }
        }
    }
}

// ─── Default view ────────────────────────────────────────────────────────────

use llm_worker::tool::Content;

fn default_view(content: &Content) -> String {
    match content {
        Content::Text(text) => default_view_text(text),
        Content::Structured(value) => default_view_structured(value),
    }
}

fn default_view_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let size = text.len();
    let preview_end = total.min(DEFAULT_PREVIEW_LINES);

    let mut out = format!("type: text\nlines: {total}\nsize: {size} bytes\n\n");
    out.push_str(&format!("── preview (lines 1-{preview_end}) ──\n"));
    for line in &lines[..preview_end] {
        out.push_str(line);
        out.push('\n');
    }
    if total > DEFAULT_PREVIEW_LINES {
        out.push_str(&format!("... ({} more lines)\n", total - DEFAULT_PREVIEW_LINES));
    }
    out
}

fn default_view_structured(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Array(arr) => {
            let total = arr.len();
            let preview_end = total.min(DEFAULT_PREVIEW_ELEMENTS);
            let mut out = format!("type: json_array\nentries: {total}\n\n");
            out.push_str(&format!("── preview (0..{preview_end}) ──\n"));
            for item in &arr[..preview_end] {
                if let Ok(json) = serde_json::to_string_pretty(item) {
                    out.push_str(&json);
                    out.push('\n');
                }
            }
            if total > DEFAULT_PREVIEW_ELEMENTS {
                out.push_str(&format!("... ({} more entries)\n", total - DEFAULT_PREVIEW_ELEMENTS));
            }
            out
        }
        Value::Object(map) => {
            let total = map.len();
            let mut out = format!("type: json_object\nkeys: {total}\n\n── keys ──\n");
            for (key, val) in map.iter() {
                out.push_str(&format!("{key}: {}\n", value_type_label(val)));
            }
            // Preview first N key-value pairs
            let preview_keys: Vec<_> = map.iter().take(DEFAULT_PREVIEW_KEYS).collect();
            if !preview_keys.is_empty() {
                out.push_str("\n── preview ──\n");
                for (key, val) in preview_keys {
                    if let Ok(json) = serde_json::to_string_pretty(val) {
                        out.push_str(&format!("{key}: {json}\n"));
                    }
                }
            }
            out
        }
        other => {
            // Scalar — just show it
            serde_json::to_string_pretty(other).unwrap_or_default()
        }
    }
}

fn value_type_label(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ─── Selector application ────────────────────────────────────────────────────

fn apply_selector(content: &Content, selector: &Selector) -> Result<String, ToolError> {
    match (content, selector) {
        (Content::Text(text), Selector::Lines { start, end }) => {
            let lines: Vec<&str> = text.lines().collect();
            let total = lines.len();
            // Convert 1-based inclusive to 0-based
            let from = (*start - 1).min(total);
            let to = (*end).min(total);
            if from >= total {
                return Ok(format!("(no lines — content has {total} lines)"));
            }
            Ok(lines[from..to].join("\n"))
        }

        (Content::Structured(serde_json::Value::Array(arr)), Selector::Slice { start, end }) => {
            let total = arr.len();
            let from = (*start).min(total);
            let to = (*end).min(total);
            let slice = &arr[from..to];
            serde_json::to_string_pretty(slice)
                .map_err(|e| ToolError::Internal(format!("JSON serialization error: {e}")))
        }

        (Content::Structured(serde_json::Value::Object(map)), Selector::Key(key)) => {
            match map.get(key.as_str()) {
                Some(val) => serde_json::to_string_pretty(val)
                    .map_err(|e| ToolError::Internal(format!("JSON serialization error: {e}"))),
                None => {
                    let available: Vec<_> = map.keys().collect();
                    Err(ToolError::InvalidArgument(format!(
                        "key '{key}' not found. Available keys: {available:?}"
                    )))
                }
            }
        }

        // Type mismatches
        (Content::Text(_), Selector::Slice { .. }) => Err(ToolError::InvalidArgument(
            "slice selector only applies to JSON arrays, but this blob contains text. Use 'lines:N-M' instead.".into(),
        )),
        (Content::Text(_), Selector::Key(_)) => Err(ToolError::InvalidArgument(
            "key selector only applies to JSON objects, but this blob contains text. Use 'lines:N-M' instead.".into(),
        )),
        (Content::Structured(_), Selector::Lines { .. }) => Err(ToolError::InvalidArgument(
            "lines selector only applies to text content, but this blob contains JSON. Use 'slice:N..M' or 'key:NAME' instead.".into(),
        )),
        (Content::Structured(serde_json::Value::Object(_)), Selector::Slice { .. }) => Err(ToolError::InvalidArgument(
            "slice selector only applies to JSON arrays, but this blob is a JSON object. Use 'key:NAME' instead.".into(),
        )),
        (Content::Structured(serde_json::Value::Array(_)), Selector::Key(_)) => Err(ToolError::InvalidArgument(
            "key selector only applies to JSON objects, but this blob is a JSON array. Use 'slice:N..M' instead.".into(),
        )),
        (Content::Structured(_), Selector::Slice { .. }) => Err(ToolError::InvalidArgument(
            "slice selector only applies to JSON arrays.".into(),
        )),
        (Content::Structured(_), Selector::Key(_)) => Err(ToolError::InvalidArgument(
            "key selector only applies to JSON objects.".into(),
        )),
    }
}

// ─── Registration helper ─────────────────────────────────────────────────────

/// Register the `inspect` tool on a [`Worker`].
///
/// Call this alongside [`BlobOutputProcessor`](crate::BlobOutputProcessor)
/// setup so the LLM can retrieve stored blob content.
pub fn register_inspect_tool<C, B>(
    worker: &mut Worker<C, Mutable>,
    blob_store: Arc<B>,
) where
    C: LlmClient,
    B: BlobStore + 'static,
{
    worker.register_tool(InspectTool::<B>::tool_definition(blob_store));
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_store::{new_blob_id, BlobStoreError};
    use llm_worker::tool::Content;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    // ── In-memory BlobStore for tests ────────────────────────────────────

    struct MemBlobStore {
        blobs: Mutex<HashMap<BlobId, Content>>,
    }

    impl MemBlobStore {
        fn new() -> Self {
            Self {
                blobs: Mutex::new(HashMap::new()),
            }
        }
    }

    impl BlobStore for MemBlobStore {
        async fn store(&self, content: &Content) -> Result<BlobId, BlobStoreError> {
            let id = new_blob_id();
            self.blobs.lock().await.insert(id, content.clone());
            Ok(id)
        }

        async fn load(&self, id: BlobId) -> Result<Content, BlobStoreError> {
            self.blobs
                .lock()
                .await
                .get(&id)
                .cloned()
                .ok_or(BlobStoreError::NotFound(id))
        }

        async fn exists(&self, id: BlobId) -> Result<bool, BlobStoreError> {
            Ok(self.blobs.lock().await.contains_key(&id))
        }
    }

    // ── Selector parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_lines_valid() {
        assert_eq!(
            parse_selector("lines:1-50").unwrap(),
            Selector::Lines { start: 1, end: 50 }
        );
        assert_eq!(
            parse_selector("lines:5-5").unwrap(),
            Selector::Lines { start: 5, end: 5 }
        );
    }

    #[test]
    fn parse_lines_zero_start() {
        let err = parse_selector("lines:0-5").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn parse_lines_inverted() {
        let err = parse_selector("lines:50-20").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn parse_lines_missing_dash() {
        let err = parse_selector("lines:20").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn parse_slice_valid() {
        assert_eq!(
            parse_selector("slice:0..10").unwrap(),
            Selector::Slice { start: 0, end: 10 }
        );
        assert_eq!(
            parse_selector("slice:3..8").unwrap(),
            Selector::Slice { start: 3, end: 8 }
        );
    }

    #[test]
    fn parse_slice_inverted() {
        let err = parse_selector("slice:10..3").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn parse_key_valid() {
        assert_eq!(
            parse_selector("key:results").unwrap(),
            Selector::Key("results".into())
        );
        // Key name with colon
        assert_eq!(
            parse_selector("key:nested:key").unwrap(),
            Selector::Key("nested:key".into())
        );
    }

    #[test]
    fn parse_key_empty() {
        let err = parse_selector("key:").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn parse_unknown_prefix() {
        let err = parse_selector("unknown:foo").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    // ── Default view ─────────────────────────────────────────────────────

    #[test]
    fn default_view_text_short() {
        let text = "line1\nline2\nline3\n";
        let content = Content::Text(text.into());
        let view = default_view(&content);
        assert!(view.contains("type: text"));
        assert!(view.contains("lines: 3"));
        assert!(view.contains("line1"));
        assert!(!view.contains("more lines"));
    }

    #[test]
    fn default_view_text_long() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let content = Content::Text(text);
        let view = default_view(&content);
        assert!(view.contains("type: text"));
        assert!(view.contains("lines: 100"));
        assert!(view.contains("line 1"));
        assert!(view.contains("line 50"));
        assert!(!view.contains("line 51\n"));
        assert!(view.contains("50 more lines"));
    }

    #[test]
    fn default_view_array() {
        let arr: Vec<serde_json::Value> = (0..20).map(|i| json!({"id": i})).collect();
        let content = Content::Structured(json!(arr));
        let view = default_view(&content);
        assert!(view.contains("type: json_array"));
        assert!(view.contains("entries: 20"));
        assert!(view.contains("15 more entries"));
    }

    #[test]
    fn default_view_object() {
        let content = Content::Structured(json!({
            "name": "test",
            "count": 42,
            "items": [1, 2, 3],
            "nested": {"a": 1}
        }));
        let view = default_view(&content);
        assert!(view.contains("type: json_object"));
        assert!(view.contains("keys: 4"));
        assert!(view.contains("── keys ──"));
        assert!(view.contains("── preview ──"));
    }

    // ── Selector application ─────────────────────────────────────────────

    #[test]
    fn apply_lines_on_text() {
        let text = "a\nb\nc\nd\ne\nf\n";
        let content = Content::Text(text.into());
        let result = apply_selector(&content, &Selector::Lines { start: 2, end: 4 }).unwrap();
        assert_eq!(result, "b\nc\nd");
    }

    #[test]
    fn apply_lines_clamp() {
        let text = "a\nb\nc\n";
        let content = Content::Text(text.into());
        let result = apply_selector(&content, &Selector::Lines { start: 2, end: 100 }).unwrap();
        assert_eq!(result, "b\nc");
    }

    #[test]
    fn apply_lines_beyond_content() {
        let text = "a\nb\n";
        let content = Content::Text(text.into());
        let result = apply_selector(&content, &Selector::Lines { start: 10, end: 20 }).unwrap();
        assert!(result.contains("no lines"));
    }

    #[test]
    fn apply_slice_on_array() {
        let content = Content::Structured(json!([10, 20, 30, 40, 50]));
        let result = apply_selector(&content, &Selector::Slice { start: 1, end: 3 }).unwrap();
        let parsed: Vec<i64> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, vec![20, 30]);
    }

    #[test]
    fn apply_slice_clamp() {
        let content = Content::Structured(json!([10, 20, 30]));
        let result = apply_selector(&content, &Selector::Slice { start: 1, end: 100 }).unwrap();
        let parsed: Vec<i64> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, vec![20, 30]);
    }

    #[test]
    fn apply_key_on_object() {
        let content = Content::Structured(json!({"name": "test", "count": 42}));
        let result = apply_selector(&content, &Selector::Key("name".into())).unwrap();
        assert_eq!(result.trim(), "\"test\"");
    }

    #[test]
    fn apply_key_not_found() {
        let content = Content::Structured(json!({"name": "test"}));
        let err = apply_selector(&content, &Selector::Key("missing".into())).unwrap_err();
        match err {
            ToolError::InvalidArgument(msg) => {
                assert!(msg.contains("missing"));
                assert!(msg.contains("name"));
            }
            _ => panic!("expected InvalidArgument"),
        }
    }

    // ── Type mismatch errors ─────────────────────────────────────────────

    #[test]
    fn lines_on_json_error() {
        let content = Content::Structured(json!([1, 2, 3]));
        let err = apply_selector(&content, &Selector::Lines { start: 1, end: 3 }).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn slice_on_text_error() {
        let content = Content::Text("hello".into());
        let err = apply_selector(&content, &Selector::Slice { start: 0, end: 3 }).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn key_on_text_error() {
        let content = Content::Text("hello".into());
        let err = apply_selector(&content, &Selector::Key("foo".into())).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn slice_on_object_error() {
        let content = Content::Structured(json!({"a": 1}));
        let err = apply_selector(&content, &Selector::Slice { start: 0, end: 3 }).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[test]
    fn key_on_array_error() {
        let content = Content::Structured(json!([1, 2, 3]));
        let err = apply_selector(&content, &Selector::Key("foo".into())).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    // ── Integration via execute() ────────────────────────────────────────

    #[tokio::test]
    async fn execute_default_view() {
        let store = Arc::new(MemBlobStore::new());
        let text = (1..=100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let blob_id = store.store(&Content::Text(text)).await.unwrap();

        let tool = InspectTool::new(store);
        let result = tool
            .execute(&json!({"blob_id": blob_id.to_string()}).to_string())
            .await
            .unwrap();
        assert!(result.contains("type: text"));
        assert!(result.contains("lines: 100"));
    }

    #[tokio::test]
    async fn execute_with_selector() {
        let store = Arc::new(MemBlobStore::new());
        let blob_id = store
            .store(&Content::Structured(json!({"name": "test", "value": 42})))
            .await
            .unwrap();

        let tool = InspectTool::new(store);
        let result = tool
            .execute(&json!({"blob_id": blob_id.to_string(), "selector": "key:name"}).to_string())
            .await
            .unwrap();
        assert_eq!(result.trim(), "\"test\"");
    }

    #[tokio::test]
    async fn execute_invalid_blob_id() {
        let store = Arc::new(MemBlobStore::new());
        let tool = InspectTool::new(store);
        let err = tool
            .execute(&json!({"blob_id": "not-a-uuid"}).to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn execute_blob_not_found() {
        let store = Arc::new(MemBlobStore::new());
        let tool = InspectTool::new(store);
        let fake_id = new_blob_id();
        let err = tool
            .execute(&json!({"blob_id": fake_id.to_string()}).to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
    }
}
