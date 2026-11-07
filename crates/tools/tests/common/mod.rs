use agen::tool::ToolError;
use serde_json::Value;

pub fn workdir_report(error: ToolError, classification: &str) -> Value {
    workdir_failure(error, classification, "invalid_argument")
}

pub fn workdir_failure(error: ToolError, classification: &str, class: &str) -> Value {
    let ToolError::ExecutionFailure(output) = error else {
        panic!("expected Workdir failure report, got {error:?}")
    };
    let report: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(report["failure"]["domain"], "workdir");
    assert_eq!(
        report["failure"]["details"]["classification"],
        classification
    );
    assert_eq!(report["result"]["error_class"], class);
    assert_eq!(report["result"]["mutation"], "not_reported");
    report
}
