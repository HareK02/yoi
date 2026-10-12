//! Temporary storage receiving `write_extracted` results for the legacy extraction engine.
//!
//! Validates [`ExtractedPayload`] and holds it in [`ExtractWorkerContext`]'s Mutex.
//! Callers retrieve the result with [`ExtractWorkerContext::take_payload`].
//! This module neither persists candidates nor decides whether to adopt them.

use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;

use crate::extract::payload::ExtractedPayload;

const WRITE_EXTRACTED_DESCRIPTION: &str = "Submit extracted memory-candidate JSON for this slice. \
Pass an object with a `candidates` array. Each candidate must have `kind`, `claim`, and `why_useful`; \
`staleness` and `evidence_ids` are optional. Call this exactly once and end the turn. Do not include \
record ids, source anchors, session metadata, or free-form prose — the wrapper attaches staging metadata mechanically.";

/// Output receiver for the extract sub-Engine. Holds one `ExtractedPayload`.
#[derive(Debug, Default)]
pub struct ExtractWorkerContext {
    payload: Mutex<Option<ExtractedPayload>>,
    /// Count of `write_extracted` calls (for debugging).
    /// Later calls overwrite earlier payloads; Workers can use this to emit a warning.
    call_count: Mutex<usize>,
}

impl ExtractWorkerContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by the Worker after the sub-Engine exits to retrieve the payload.
    /// Returns `None` if `write_extracted` was never called.
    pub fn take_payload(&self) -> Option<ExtractedPayload> {
        self.payload
            .lock()
            .expect("extract worker payload poisoned")
            .take()
    }

    pub fn call_count(&self) -> usize {
        *self
            .call_count
            .lock()
            .expect("extract worker call_count poisoned")
    }
}

struct WriteExtractedTool {
    ctx: Arc<ExtractWorkerContext>,
}

#[async_trait]
impl Tool for WriteExtractedTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let payload: ExtractedPayload = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid write_extracted input: {e}"))
        })?;
        let summary = format!(
            "Recorded memory candidates: candidates={}",
            payload.candidates.len(),
        );
        {
            let mut guard = self
                .ctx
                .payload
                .lock()
                .expect("extract worker payload poisoned");
            *guard = Some(payload);
        }
        {
            let mut count = self
                .ctx
                .call_count
                .lock()
                .expect("extract worker call_count poisoned");
            *count += 1;
        }
        Ok(ToolOutput {
            summary,
            content: None,
            attachments: Vec::new(),
        })
    }
}

/// Returns the `write_extracted` tool definition to register with the sub-Engine.
pub fn write_extracted_tool(ctx: Arc<ExtractWorkerContext>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ExtractedPayload);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("write_extracted")
            .description(WRITE_EXTRACTED_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use agen::tool::Tool;

    #[tokio::test]
    async fn write_extracted_records_payload() {
        let ctx = Arc::new(ExtractWorkerContext::new());
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });
        let input = serde_json::json!({
            "candidates": [{
                "kind": "decision",
                "claim": "Use flat staging",
                "why_useful": "Consolidation can resolve candidates independently"
            }]
        })
        .to_string();
        let out = tool.execute(&input, Default::default()).await.unwrap();
        assert!(out.summary.contains("candidates=1"));
        let payload = ctx.take_payload().unwrap();
        assert_eq!(payload.candidates.len(), 1);
        assert_eq!(ctx.call_count(), 1);
    }

    #[tokio::test]
    async fn last_call_wins_on_multiple_invocations() {
        let ctx = Arc::new(ExtractWorkerContext::new());
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });

        let first = serde_json::json!({"candidates": []}).to_string();
        tool.execute(&first, Default::default()).await.unwrap();

        let second = serde_json::json!({
            "candidates": [{
                "kind": "lesson",
                "claim": "Validation should use Nix build",
                "why_useful": "Packaging can fail independently"
            }]
        })
        .to_string();
        tool.execute(&second, Default::default()).await.unwrap();

        let payload = ctx.take_payload().unwrap();
        assert_eq!(payload.candidates.len(), 1);
        assert_eq!(ctx.call_count(), 2);
    }

    #[tokio::test]
    async fn invalid_json_returns_invalid_argument() {
        let ctx = Arc::new(ExtractWorkerContext::new());
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });
        let res = tool.execute("not json", Default::default()).await;
        assert!(matches!(res, Err(ToolError::InvalidArgument(_))));
        assert!(ctx.take_payload().is_none());
    }
}
