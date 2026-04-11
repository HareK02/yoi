use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::llm_client::ToolDefinition as LlmToolDefinition;
use crate::tool::{Tool, ToolDefinition as WorkerToolDefinition, ToolMeta};

type ToolMap = HashMap<String, (ToolMeta, Arc<dyn Tool>)>;

/// Errors produced by ToolServer operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolServerError {
    /// A tool with the same name already exists.
    #[error("Tool with name '{0}' already registered")]
    DuplicateName(String),
    /// Requested tool was not found.
    #[error("Tool '{0}' not found")]
    ToolNotFound(String),
    /// Tool execution failed.
    #[error("Tool execution failed: {0}")]
    ToolExecution(String),
}

/// In-memory tool server.
#[derive(Clone, Default)]
pub struct ToolServer {
    tools: Arc<Mutex<ToolMap>>,
    pending: Arc<Mutex<Vec<WorkerToolDefinition>>>,
}

impl ToolServer {
    /// Create a new empty tool server.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a handle for shared access.
    pub fn handle(&self) -> ToolServerHandle {
        ToolServerHandle {
            tools: Arc::clone(&self.tools),
            pending: Arc::clone(&self.pending),
        }
    }
}

/// Shareable handle to a tool server.
#[derive(Clone, Default)]
pub struct ToolServerHandle {
    tools: Arc<Mutex<ToolMap>>,
    pending: Arc<Mutex<Vec<WorkerToolDefinition>>>,
}

impl ToolServerHandle {
    /// Queue a tool factory for deferred initialization.
    ///
    /// The factory is **not** called here; it is stored and executed
    /// when [`flush_pending`](Self::flush_pending) is called (typically
    /// at the start of `Worker::run()`).
    pub(crate) fn register_tool(&self, factory: WorkerToolDefinition) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(factory);
    }

    /// Queue many tool factories for deferred initialization.
    pub(crate) fn register_tools(
        &self,
        factories: impl IntoIterator<Item = WorkerToolDefinition>,
    ) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend(factories);
    }

    /// Execute all pending factories and register the resulting tools.
    ///
    /// # Panics
    ///
    /// Panics if any factory produces a tool whose name collides with
    /// an already-registered tool. Duplicate names are a programming
    /// error and should be caught during development.
    pub(crate) fn flush_pending(&self) {
        let pending: Vec<_> = {
            let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *guard)
        };
        if pending.is_empty() {
            return;
        }
        // Execute all factories first, then validate and insert atomically.
        let materialized: Vec<_> = pending.into_iter().map(|f| f()).collect();
        let mut tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        for (meta, instance) in materialized {
            assert!(
                !tools.contains_key(&meta.name),
                "duplicate tool name: '{}'",
                meta.name,
            );
            tools.insert(meta.name.clone(), (meta, instance));
        }
    }

    /// Get a tool by name for hook contexts.
    pub fn get_tool(&self, name: &str) -> Option<(ToolMeta, Arc<dyn Tool>)> {
        let guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(name)
            .map(|(meta, tool)| (meta.clone(), Arc::clone(tool)))
    }

    /// Execute a tool by name.
    pub async fn call_tool(&self, name: &str, input_json: &str) -> Result<String, ToolServerError> {
        let tool = {
            let guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
            let (_, tool) = guard
                .get(name)
                .ok_or_else(|| ToolServerError::ToolNotFound(name.to_string()))?;
            Arc::clone(tool)
        };
        tool.execute(input_json)
            .await
            .map_err(|e| ToolServerError::ToolExecution(e.to_string()))
    }

    /// Build deterministic tool definitions sorted by tool name.
    pub fn tool_definitions_sorted(&self) -> Vec<LlmToolDefinition> {
        let guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        let mut defs: Vec<_> = guard
            .values()
            .map(|(meta, _)| {
                LlmToolDefinition::new(&meta.name)
                    .description(&meta.description)
                    .input_schema(meta.input_schema.clone())
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::tool::{Tool, ToolDefinition, ToolError, ToolMeta};

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        async fn execute(&self, input_json: &str) -> Result<String, ToolError> {
            Ok(input_json.to_string())
        }
    }

    fn def(name: &'static str) -> ToolDefinition {
        Arc::new(move || {
            (
                ToolMeta::new(name)
                    .description(format!("desc-{name}"))
                    .input_schema(json!({"type":"object"})),
                Arc::new(EchoTool) as Arc<dyn Tool>,
            )
        })
    }

    #[test]
    fn flush_pending_registers_tools() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha"));
        handle.register_tool(def("beta"));

        // Before flush, no tools are available
        assert!(handle.get_tool("alpha").is_none());

        handle.flush_pending();

        // After flush, tools are available
        assert!(handle.get_tool("alpha").is_some());
        assert!(handle.get_tool("beta").is_some());
    }

    #[test]
    #[should_panic(expected = "duplicate tool name: 'alpha'")]
    fn flush_pending_duplicate_name_panics() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha"));
        handle.flush_pending();

        handle.register_tool(def("alpha"));
        handle.flush_pending(); // panics
    }

    #[tokio::test]
    async fn call_tool_success_and_not_found() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("echo"));
        handle.flush_pending();

        let out = handle.call_tool("echo", r#"{"x":1}"#).await.expect("call");
        assert_eq!(out, r#"{"x":1}"#);

        let err = handle
            .call_tool("missing", "{}")
            .await
            .expect_err("missing tool");
        assert_eq!(err, ToolServerError::ToolNotFound("missing".to_string()));
    }

    #[test]
    fn tool_definitions_are_sorted() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("zeta"));
        handle.register_tool(def("alpha"));
        handle.register_tool(def("beta"));
        handle.flush_pending();

        let names: Vec<_> = handle
            .tool_definitions_sorted()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn flush_pending_is_noop_when_empty() {
        let handle = ToolServer::new().handle();
        handle.flush_pending();
        handle.flush_pending();
    }
}
