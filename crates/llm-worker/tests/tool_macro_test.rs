//! Tool macro tests
//!
//! Verify the behavior of `#[tool_registry]` and `#[tool]` macros.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Imports needed for macro expansion
use schemars;
use serde;

use llm_worker::ToolExecutionContext;
use llm_worker_macros::tool_registry;

// =============================================================================
// Test: Basic Tool Generation
// =============================================================================

/// Simple context struct
#[derive(Clone)]
struct SimpleContext {
    prefix: String,
}

#[tool_registry]
impl SimpleContext {
    /// Add greeting to message
    ///
    /// Returns the message with a prefix added.
    #[tool]
    async fn greet(&self, message: String) -> String {
        format!("{}: {}", self.prefix, message)
    }

    /// Add two numbers
    #[tool]
    async fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }

    /// Tool with no arguments
    #[tool]
    async fn get_prefix(&self) -> String {
        self.prefix.clone()
    }

    /// Tool that observes execution context
    #[tool]
    async fn context_echo(&self, ctx: ToolExecutionContext, message: String) -> String {
        format!(
            "{}:{}:{}:{}",
            ctx.batch_id, ctx.call_index, ctx.call_id, message
        )
    }
}

#[tokio::test]
async fn test_basic_tool_generation() {
    let ctx = SimpleContext {
        prefix: "Hello".to_string(),
    };

    // Get ToolDefinition from factory method
    let greet_definition = ctx.greet_definition();

    // Call factory to get Meta and Tool
    let (meta, tool) = greet_definition();

    // Verify meta information
    assert_eq!(meta.name, "greet");
    assert!(
        meta.description.contains("Add greeting to message"),
        "Description should contain doc comment: {}",
        meta.description
    );
    assert!(
        meta.input_schema.get("properties").is_some(),
        "Schema should have properties"
    );

    println!(
        "Schema: {}",
        serde_json::to_string_pretty(&meta.input_schema).unwrap()
    );

    // Execution test
    let result = tool
        .execute(r#"{"message": "World"}"#, Default::default())
        .await;
    assert!(result.is_ok(), "Should execute successfully");
    let output = result.unwrap();
    assert!(
        output.summary.contains("Hello"),
        "Output should contain prefix"
    );
    assert!(
        output.summary.contains("World"),
        "Output should contain message"
    );
}

#[tokio::test]
async fn test_multiple_arguments() {
    let ctx = SimpleContext {
        prefix: "".to_string(),
    };

    let (meta, tool) = ctx.add_definition()();

    assert_eq!(meta.name, "add");

    let result = tool
        .execute(r#"{"a": 10, "b": 20}"#, Default::default())
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(
        output.summary.contains("30"),
        "Should contain sum: {:?}",
        output
    );
}

#[tokio::test]
async fn test_no_arguments() {
    let ctx = SimpleContext {
        prefix: "TestPrefix".to_string(),
    };

    let (meta, tool) = ctx.get_prefix_definition()();

    assert_eq!(meta.name, "get_prefix");

    // Call with empty JSON object
    let result = tool.execute(r#"{}"#, Default::default()).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(
        output.summary.contains("TestPrefix"),
        "Should contain prefix: {:?}",
        output
    );
}

#[tokio::test]
async fn test_invalid_arguments() {
    let ctx = SimpleContext {
        prefix: "".to_string(),
    };

    let (_, tool) = ctx.greet_definition()();

    // Invalid JSON
    let result = tool
        .execute(r#"{"wrong_field": "value"}"#, Default::default())
        .await;
    assert!(result.is_err(), "Should fail with invalid arguments");
}

// =============================================================================
// Test: Result Return Type
// =============================================================================

#[derive(Clone)]
struct FallibleContext;

#[derive(Debug)]
struct MyError(String);

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[tool_registry]
impl FallibleContext {
    /// Validate the given value
    #[tool]
    async fn validate(&self, value: i32) -> Result<String, MyError> {
        if value > 0 {
            Ok(format!("Valid: {}", value))
        } else {
            Err(MyError("Value must be positive".to_string()))
        }
    }
}

#[tokio::test]
async fn test_result_return_type_success() {
    let ctx = FallibleContext;
    let (_, tool) = ctx.validate_definition()();

    let result = tool.execute(r#"{"value": 42}"#, Default::default()).await;
    assert!(result.is_ok(), "Should succeed for positive value");
    let output = result.unwrap();
    assert!(
        output.summary.contains("Valid"),
        "Should contain Valid: {:?}",
        output
    );
}

#[tokio::test]
async fn test_result_return_type_error() {
    let ctx = FallibleContext;
    let (_, tool) = ctx.validate_definition()();

    let result = tool.execute(r#"{"value": -1}"#, Default::default()).await;
    assert!(result.is_err(), "Should fail for negative value");

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("positive"),
        "Error should mention positive: {}",
        err
    );
}

// =============================================================================
// Test: Synchronous Methods
// =============================================================================

#[derive(Clone)]
struct SyncContext {
    counter: Arc<AtomicUsize>,
}

#[tool_registry]
impl SyncContext {
    /// Increment counter and return (non-async)
    #[tool]
    fn increment(&self) -> usize {
        self.counter.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[tokio::test]
async fn test_sync_method() {
    let ctx = SyncContext {
        counter: Arc::new(AtomicUsize::new(0)),
    };

    let (_, tool) = ctx.increment_definition()();

    // Execute 3 times
    let result1 = tool.execute(r#"{}"#, Default::default()).await;
    let result2 = tool.execute(r#"{}"#, Default::default()).await;
    let result3 = tool.execute(r#"{}"#, Default::default()).await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_ok());

    // Counter should be 3
    assert_eq!(ctx.counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_tool_macro_passes_execution_context() {
    let ctx = SimpleContext {
        prefix: "Test".to_string(),
    };
    let (_, tool) = ctx.context_echo_definition()();

    let output = tool
        .execute(
            r#"{"message":"hello"}"#,
            ToolExecutionContext::new("call-ctx", "batch-ctx", 7),
        )
        .await
        .unwrap();

    assert_eq!(output.summary, "\"batch-ctx:7:call-ctx:hello\"");
}

// =============================================================================
// Test: ToolMeta Immutability
// =============================================================================

#[tokio::test]
async fn test_tool_meta_immutability() {
    let ctx = SimpleContext {
        prefix: "Test".to_string(),
    };

    // Verify same meta info is returned on multiple calls
    let (meta1, _) = ctx.greet_definition()();
    let (meta2, _) = ctx.greet_definition()();

    assert_eq!(meta1.name, meta2.name);
    assert_eq!(meta1.description, meta2.description);
    assert_eq!(meta1.input_schema, meta2.input_schema);
}
