use std::time::Duration;

use mcp::stdio::{McpErrorKind, McpPhase, McpStdioClient, McpStdioLimits, McpStdioServerSpec};

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
async fn shutdown_terminates_or_kills_uncooperative_server() {
    let mut client = McpStdioClient::connect(mock_server("shutdown-hang"), tight_limits())
        .await
        .expect("initialize succeeds");
    let shutdown = client.shutdown().await.expect("shutdown succeeds");
    assert!(shutdown.terminated || shutdown.killed);
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
