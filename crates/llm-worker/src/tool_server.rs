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
        }
    }
}

/// Shareable handle to a tool server.
#[derive(Clone, Default)]
pub struct ToolServerHandle {
    tools: Arc<Mutex<ToolMap>>,
}

impl ToolServerHandle {
    /// Register one tool.
    pub(crate) fn register_tool(
        &self,
        factory: WorkerToolDefinition,
    ) -> Result<(), ToolServerError> {
        let (meta, instance) = factory();
        let mut guard = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        if guard.contains_key(&meta.name) {
            return Err(ToolServerError::DuplicateName(meta.name));
        }
        guard.insert(meta.name.clone(), (meta, instance));
        Ok(())
    }

    /// Register many tools.
    pub(crate) fn register_tools(
        &self,
        factories: impl IntoIterator<Item = WorkerToolDefinition>,
    ) -> Result<(), ToolServerError> {
        for factory in factories {
            self.register_tool(factory)?;
        }
        Ok(())
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
    fn register_duplicate_name_fails() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("alpha")).expect("first register");
        let err = handle
            .register_tool(def("alpha"))
            .expect_err("duplicate should fail");
        assert_eq!(err, ToolServerError::DuplicateName("alpha".to_string()));
    }

    #[tokio::test]
    async fn call_tool_success_and_not_found() {
        let handle = ToolServer::new().handle();
        handle.register_tool(def("echo")).expect("register");

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
        handle.register_tool(def("zeta")).expect("register zeta");
        handle.register_tool(def("alpha")).expect("register alpha");
        handle.register_tool(def("beta")).expect("register beta");

        let names: Vec<_> = handle
            .tool_definitions_sorted()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["alpha", "beta", "zeta"]);
    }
}
