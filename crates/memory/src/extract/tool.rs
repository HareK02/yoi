//! `write_extracted` ツール実装と sub-Engine 用 context。
//!
//! sub-Engine からは extract worker が出した [`ExtractedPayload`] を
//! 受け取って `Mutex` 越しに [`ExtractWorkerContext`] に置くだけ。
//! Pod 側はランループ完了後に `take_payload()` で取り出して
//! [`super::staging::write_staging`] に渡す。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};

use crate::extract::payload::ExtractedPayload;

const WRITE_EXTRACTED_DESCRIPTION: &str = "Submit the final activity-log JSON for this slice. \
Pass an object with `decisions`, `discussions`, `attempts`, and `requests` arrays (any may be empty). \
Call this exactly once and end the turn. Do not include `source`, session metadata, or free-form prose — \
the wrapper attaches provenance mechanically.";

/// extract sub-Engine の出力受け口。`ExtractedPayload` 1 件をホストする。
#[derive(Debug, Default)]
pub struct ExtractWorkerContext {
    payload: Mutex<Option<ExtractedPayload>>,
    /// `write_extracted` が複数回呼ばれた回数（debug 用）。
    /// 後勝ちで上書きするが、Pod 側で warn を出したい場合に参照する。
    call_count: Mutex<usize>,
}

impl ExtractWorkerContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// sub-Engine 終了後に Pod が呼んで payload を取り出す。
    /// 一度も `write_extracted` が呼ばれなければ `None`。
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
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let payload: ExtractedPayload = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid write_extracted input: {e}"))
        })?;
        let summary = format!(
            "Recorded activity log: decisions={} discussions={} attempts={} requests={}",
            payload.decisions.len(),
            payload.discussions.len(),
            payload.attempts.len(),
            payload.requests.len(),
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
        })
    }
}

/// sub-Engine に register する `write_extracted` ツール定義を返す。
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

    use llm_engine::tool::Tool;

    #[tokio::test]
    async fn write_extracted_records_payload() {
        let ctx = Arc::new(ExtractWorkerContext::new());
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });
        let input = serde_json::json!({
            "decisions": [{
                "options": ["a", "b"],
                "chosen": "a",
                "rationale": "test"
            }],
            "discussions": [],
            "attempts": [],
            "requests": []
        })
        .to_string();
        let out = tool.execute(&input, Default::default()).await.unwrap();
        assert!(out.summary.contains("decisions=1"));
        let payload = ctx.take_payload().unwrap();
        assert_eq!(payload.decisions.len(), 1);
        assert_eq!(ctx.call_count(), 1);
    }

    #[tokio::test]
    async fn last_call_wins_on_multiple_invocations() {
        let ctx = Arc::new(ExtractWorkerContext::new());
        let tool: Arc<dyn Tool> = Arc::new(WriteExtractedTool { ctx: ctx.clone() });

        let first =
            serde_json::json!({"decisions": [], "discussions": [], "attempts": [], "requests": []})
                .to_string();
        tool.execute(&first, Default::default()).await.unwrap();

        let second = serde_json::json!({
            "decisions": [],
            "discussions": [],
            "attempts": [{"action": "x", "result": "ok", "succeeded": true}],
            "requests": []
        })
        .to_string();
        tool.execute(&second, Default::default()).await.unwrap();

        let payload = ctx.take_payload().unwrap();
        assert_eq!(payload.attempts.len(), 1);
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
