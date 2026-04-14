use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::llm_client::ToolDefinition as LlmToolDefinition;
use crate::tool::{Tool, ToolDefinition as WorkerToolDefinition, ToolMeta, ToolOutput};

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
    pub(crate) fn register_tools(&self, factories: impl IntoIterator<Item = WorkerToolDefinition>) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend(factories);
    }

    /// Execute all pending factories and register the resulting tools.
    ///
    /// Called implicitly by `Worker::lock()` before the first turn.
    /// Exposed as `pub` so higher layers (e.g. Pod) can force-materialise
    /// tools earlier — for example when building a system-prompt template
    /// context that needs the list of registered tool names. Redundant
    /// calls are no-ops.
    ///
    /// # Panics
    ///
    /// Panics if any factory produces a tool whose name collides with
    /// an already-registered tool. Duplicate names are a programming
    /// error and should be caught during development.
    pub fn flush_pending(&self) {
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
    pub async fn call_tool(
        &self,
        name: &str,
        input_json: &str,
    ) -> Result<ToolOutput, ToolServerError> {
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

    /// Remove a registered tool by name.
    ///
    /// In-flight calls that already obtained an `Arc<dyn Tool>` clone are
    /// unaffected and will run to completion.
    pub fn unregister(&self, name: &str) -> Result<(), ToolServerError> {
        let mut guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| ToolServerError::ToolNotFound(name.to_string()))
    }

    /// Replace an existing tool with a new implementation.
    ///
    /// The factory is called immediately and the resulting tool overwrites
    /// the entry with the same name. Returns `ToolNotFound` if the name
    /// produced by the factory does not match any registered tool.
    pub fn replace(&self, factory: WorkerToolDefinition) -> Result<(), ToolServerError> {
        let (meta, instance) = factory();
        let mut guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.contains_key(&meta.name) {
            return Err(ToolServerError::ToolNotFound(meta.name));
        }
        guard.insert(meta.name.clone(), (meta, instance));
        Ok(())
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
        async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
            Ok(input_json.to_string().into())
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
        assert_eq!(out.summary, r#"{"x":1}"#);
        assert!(out.content.is_none());

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

    #[test]
    fn unregister_removes_tool() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha"));
        handle.flush_pending();

        handle.unregister("alpha").expect("unregister");
        assert!(handle.get_tool("alpha").is_none());
    }

    #[test]
    fn unregister_not_found() {
        let handle = ToolServer::new().handle();
        let err = handle.unregister("ghost").expect_err("should fail");
        assert_eq!(err, ToolServerError::ToolNotFound("ghost".to_string()));
    }

    #[test]
    fn replace_swaps_implementation() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha"));
        handle.flush_pending();

        // Replace with a tool that returns a fixed string.
        struct FixedTool;

        #[async_trait]
        impl Tool for FixedTool {
            async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
                Ok("replaced".to_string().into())
            }
        }

        let replacement: ToolDefinition = Arc::new(|| {
            (
                ToolMeta::new("alpha")
                    .description("replaced-desc")
                    .input_schema(json!({"type":"object"})),
                Arc::new(FixedTool) as Arc<dyn Tool>,
            )
        });
        handle.replace(replacement).expect("replace");

        let (meta, _) = handle.get_tool("alpha").expect("exists");
        assert_eq!(meta.description, "replaced-desc");
    }

    #[tokio::test]
    async fn replace_updates_call_result() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("echo"));
        handle.flush_pending();

        struct ConstTool;

        #[async_trait]
        impl Tool for ConstTool {
            async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
                Ok("const".to_string().into())
            }
        }

        let replacement: ToolDefinition = Arc::new(|| {
            (
                ToolMeta::new("echo")
                    .description("const")
                    .input_schema(json!({"type":"object"})),
                Arc::new(ConstTool) as Arc<dyn Tool>,
            )
        });
        handle.replace(replacement).expect("replace");

        let out = handle.call_tool("echo", "{}").await.expect("call");
        assert_eq!(out.summary, "const");
    }

    #[tokio::test]
    async fn unregister_during_execution_does_not_affect_inflight() {
        use tokio::sync::Notify;

        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());

        struct GatedTool {
            started: Arc<Notify>,
            finish: Arc<Notify>,
        }

        #[async_trait]
        impl Tool for GatedTool {
            async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
                self.started.notify_one();
                self.finish.notified().await;
                Ok("done".to_string().into())
            }
        }

        let handle = ToolServer::new().handle();
        let s = Arc::clone(&started);
        let f = Arc::clone(&finish);
        handle.register_tool(Arc::new(move || {
            (
                ToolMeta::new("slow")
                    .description("slow")
                    .input_schema(json!({"type":"object"})),
                Arc::new(GatedTool {
                    started: Arc::clone(&s),
                    finish: Arc::clone(&f),
                }) as Arc<dyn Tool>,
            )
        }));
        handle.flush_pending();

        let h = handle.clone();
        let call = tokio::spawn(async move { h.call_tool("slow", "{}").await });

        // Wait until the tool is actually executing.
        started.notified().await;

        // Unregister while the tool is mid-execution.
        handle.unregister("slow").expect("unregister");
        assert!(handle.get_tool("slow").is_none());

        // Let the in-flight call finish.
        finish.notify_one();
        let result = call.await.expect("join");
        assert_eq!(result.expect("call").summary, "done");
    }

    #[tokio::test]
    async fn replace_during_execution_inflight_uses_old_impl() {
        use tokio::sync::Notify;

        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());

        struct OldTool {
            started: Arc<Notify>,
            finish: Arc<Notify>,
        }

        #[async_trait]
        impl Tool for OldTool {
            async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
                self.started.notify_one();
                self.finish.notified().await;
                Ok("old".to_string().into())
            }
        }

        let handle = ToolServer::new().handle();
        let s = Arc::clone(&started);
        let f = Arc::clone(&finish);
        handle.register_tool(Arc::new(move || {
            (
                ToolMeta::new("t")
                    .description("d")
                    .input_schema(json!({"type":"object"})),
                Arc::new(OldTool {
                    started: Arc::clone(&s),
                    finish: Arc::clone(&f),
                }) as Arc<dyn Tool>,
            )
        }));
        handle.flush_pending();

        let h = handle.clone();
        let call = tokio::spawn(async move { h.call_tool("t", "{}").await });

        // Wait until the old tool is mid-execution.
        started.notified().await;

        // Replace while the old tool is executing.
        struct NewTool;

        #[async_trait]
        impl Tool for NewTool {
            async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
                Ok("new".to_string().into())
            }
        }

        handle
            .replace(Arc::new(|| {
                (
                    ToolMeta::new("t")
                        .description("d")
                        .input_schema(json!({"type":"object"})),
                    Arc::new(NewTool) as Arc<dyn Tool>,
                )
            }))
            .expect("replace");

        // Let the old in-flight call finish — it should return "old".
        finish.notify_one();
        let result = call.await.expect("join");
        assert_eq!(result.expect("call").summary, "old");

        // New calls use the replacement.
        let out = handle.call_tool("t", "{}").await.expect("call");
        assert_eq!(out.summary, "new");
    }

    #[test]
    fn unregister_reflects_in_tool_definitions() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha"));
        handle.register_tool(def("beta"));
        handle.flush_pending();

        handle.unregister("alpha").expect("unregister");
        let names: Vec<_> = handle
            .tool_definitions_sorted()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn replace_not_found() {
        let handle = ToolServer::new().handle();
        let factory: ToolDefinition = Arc::new(|| {
            (
                ToolMeta::new("ghost")
                    .description("x")
                    .input_schema(json!({"type":"object"})),
                Arc::new(EchoTool) as Arc<dyn Tool>,
            )
        });
        let err = handle.replace(factory).expect_err("should fail");
        assert_eq!(err, ToolServerError::ToolNotFound("ghost".to_string()));
    }
}
