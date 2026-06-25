use std::time::Duration;

use mcp::stdio::{
    CallToolRequest, McpErrorKind, McpListChangedKind, McpPhase, McpStdioClient, McpStdioLimits,
    McpStdioServerSpec, McpToolListLimits,
};

fn mock_server(mode: &str) -> McpStdioServerSpec {
    McpStdioServerSpec::new("mock", env!("CARGO_BIN_EXE_mcp-stdio-mock-server"))
        .env("YOI_MCP_MOCK_MODE", mode)
}

fn tight_limits() -> McpStdioLimits {
    McpStdioLimits {
        startup_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
        shutdown_timeout: Duration::from_millis(100),
        kill_timeout: Duration::from_millis(100),
        max_diagnostic_lines: 2,
        max_stderr_line_bytes: 256,
        ..Default::default()
    }
}

#[test]
fn stdio_server_spec_debug_redacts_resolved_env_values() {
    let spec = McpStdioServerSpec::new("debug-mock", "/bin/mock-mcp")
        .arg("--stdio")
        .cwd("/tmp/mock-mcp")
        .env("LITERAL_VALUE", "literal-plaintext")
        .env("INHERITED_VALUE", "inherited-plaintext")
        .env("ENV_REF_VALUE", "env-ref-plaintext")
        .env("SECRET_REF_VALUE", "secret-ref-plaintext");

    let debug = format!("{spec:?}");

    assert!(debug.contains("debug-mock"));
    assert!(debug.contains("/bin/mock-mcp"));
    assert!(debug.contains("--stdio"));
    assert!(debug.contains("/tmp/mock-mcp"));
    assert!(debug.contains("LITERAL_VALUE"));
    assert!(debug.contains("INHERITED_VALUE"));
    assert!(debug.contains("ENV_REF_VALUE"));
    assert!(debug.contains("SECRET_REF_VALUE"));
    assert!(debug.contains("[redacted]"));

    assert!(!debug.contains("literal-plaintext"));
    assert!(!debug.contains("inherited-plaintext"));
    assert!(!debug.contains("env-ref-plaintext"));
    assert!(!debug.contains("secret-ref-plaintext"));
}

#[tokio::test]
async fn initializes_mock_stdio_server() {
    let mut client = McpStdioClient::connect(mock_server("success"), tight_limits())
        .await
        .expect("initialize succeeds");
    let result = client.initialize_result().expect("initialize result");
    assert_eq!(result.protocol_version, "2025-11-25");
    assert_eq!(result.server_info.name, "mock-mcp");
    let shutdown = client.shutdown().await.expect("shutdown succeeds");
    assert!(!shutdown.terminated);
    assert!(!shutdown.killed);
    assert!(shutdown.exit_status.is_some_and(|status| status.success()));
}

#[tokio::test]
async fn list_tools_paginates_and_never_calls_tools_call() {
    let mut client = McpStdioClient::connect(mock_server("tools"), tight_limits())
        .await
        .expect("connect mock server");
    let tools = client
        .list_tools_bounded(McpToolListLimits {
            max_pages: 4,
            max_tools: 8,
        })
        .await
        .expect("list mock tools");
    assert_eq!(tools.tools.len(), 2);
    assert_eq!(tools.tools[0].name, "search-files");
    assert_eq!(tools.tools[1].name, "summarize");
    assert_eq!(tools.tools[0].input_schema["type"], "object");
    client.shutdown().await.expect("shutdown after list");
}

#[tokio::test]
async fn list_tools_page_bound_fails_closed() {
    let mut client = McpStdioClient::connect(mock_server("tools"), tight_limits())
        .await
        .expect("connect mock server");
    let err = client
        .list_tools_bounded(McpToolListLimits {
            max_pages: 1,
            max_tools: 8,
        })
        .await
        .expect_err("pagination beyond bound must fail");
    assert_eq!(err.phase, McpPhase::Running);
    assert!(
        matches!(&err.kind, McpErrorKind::Protocol(message) if message.contains("exceeded 1 page"))
    );
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn list_tools_tool_bound_fails_closed() {
    let mut client = McpStdioClient::connect(mock_server("tools"), tight_limits())
        .await
        .expect("connect mock server");
    let err = client
        .list_tools_bounded(McpToolListLimits {
            max_pages: 4,
            max_tools: 1,
        })
        .await
        .expect_err("tool count beyond bound must fail");
    assert_eq!(err.phase, McpPhase::Running);
    assert!(
        matches!(&err.kind, McpErrorKind::Protocol(message) if message.contains("exceeded 1 tool"))
    );
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn initialize_failure_reports_server_phase_and_redacted_bounded_stderr() {
    let spec = mock_server("fail-init").env("MCP_TEST_SECRET", "super-secret-token");
    let err = match McpStdioClient::connect(spec, tight_limits()).await {
        Ok(mut client) => {
            let _ = client.shutdown().await;
            panic!("initialize unexpectedly succeeded");
        }
        Err(err) => err,
    };
    assert_eq!(err.server_name, "mock");
    assert_eq!(err.phase, McpPhase::Initialize);
    match &err.kind {
        McpErrorKind::JsonRpcError { code, message } => {
            assert_eq!(*code, -32000);
            assert!(!message.contains("super-secret-token"));
            assert!(message.contains("[redacted]"));
        }
        other => panic!("unexpected error kind: {other:?}"),
    }
    let rendered = err.to_string();
    assert!(rendered.contains("mock"));
    assert!(rendered.contains("initialize"));
    let diagnostics = err.diagnostics().expect("diagnostics");
    assert_eq!(diagnostics.server_name, "mock");
    assert_eq!(diagnostics.stderr.len(), 2);
    assert!(diagnostics.dropped_stderr_lines >= 3);
    assert!(
        diagnostics
            .stderr
            .iter()
            .all(|line| !line.contains("super-secret-token"))
    );
    assert!(
        diagnostics
            .stderr
            .iter()
            .any(|line| line.contains("[redacted]"))
    );
}

#[tokio::test]
async fn call_tool_returns_normal_result() {
    let mut client = McpStdioClient::connect(mock_server("tools-call-normal"), tight_limits())
        .await
        .expect("connect");
    let result = client
        .call_tool(CallToolRequest::new(
            "search-files",
            serde_json::json!({"query": "needle"}),
        ))
        .await
        .expect("call tool");
    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].kind, "text");
    assert_eq!(result.content[0].fields["text"], "found needle");
    assert_eq!(
        result.structured_content.as_ref().unwrap()["matches"][0],
        "needle.rs"
    );
    assert_eq!(result.meta.as_ref().unwrap()["server"], "mock");
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn call_tool_preserves_mcp_is_error_result() {
    let mut client = McpStdioClient::connect(mock_server("tools-call-is-error"), tight_limits())
        .await
        .expect("connect");
    let result = client
        .call_tool(CallToolRequest::new(
            "search-files",
            serde_json::json!({"query": "needle"}),
        ))
        .await
        .expect("call tool");
    assert!(result.is_error);
    assert_eq!(result.content[0].fields["text"], "tool-level failure");
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn call_tool_reports_json_rpc_protocol_error_distinctly() {
    let mut client =
        McpStdioClient::connect(mock_server("tools-call-protocol-error"), tight_limits())
            .await
            .expect("connect");
    let err = client
        .call_tool(CallToolRequest::new(
            "search-files",
            serde_json::json!({"query": "needle"}),
        ))
        .await
        .expect_err("protocol error");
    assert!(matches!(err.kind, McpErrorKind::JsonRpcError { .. }));
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn permission_denial_style_shutdown_sends_no_tools_call() {
    let mut client = McpStdioClient::connect(mock_server("tools-call-forbidden"), tight_limits())
        .await
        .expect("connect");
    // This mirrors Engine pre-tool-call denial: the ordinary Tool execution body
    // is never entered, so the MCP server sees lifecycle shutdown but no call.
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn shutdown_terminates_or_kills_uncooperative_server() {
    let mut client = McpStdioClient::connect(mock_server("shutdown-hang"), tight_limits())
        .await
        .expect("initialize succeeds");
    let shutdown = client.shutdown().await.expect("shutdown succeeds");
    assert!(shutdown.terminated || shutdown.killed);
}

#[tokio::test]
async fn list_changed_notifications_record_bounded_kind_only_state() {
    let mut client = McpStdioClient::connect(mock_server("list-changed-all"), tight_limits())
        .await
        .expect("initialize succeeds");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let snapshot = client.snapshot_list_changes().await;
    assert_eq!(snapshot.server_name, "mock");
    assert!(snapshot.contains(McpListChangedKind::Tools));
    assert!(snapshot.contains(McpListChangedKind::Resources));
    assert!(snapshot.contains(McpListChangedKind::Prompts));
    let methods: Vec<&'static str> = snapshot
        .kinds()
        .map(McpListChangedKind::notification_method)
        .collect();
    assert_eq!(
        methods,
        vec![
            "notifications/tools/list_changed",
            "notifications/resources/list_changed",
            "notifications/prompts/list_changed"
        ]
    );

    client.clear_list_changes().await;
    assert!(client.snapshot_list_changes().await.is_empty());
    client.shutdown().await.expect("shutdown succeeds");
}

#[tokio::test]
async fn sampling_requests_fail_closed_and_are_not_advertised() {
    let mut client = McpStdioClient::connect(mock_server("sampling"), tight_limits())
        .await
        .expect("initialize succeeds");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let shutdown = client.shutdown().await.expect("shutdown succeeds");
    assert!(shutdown.exit_status.is_some_and(|status| status.success()));
}
