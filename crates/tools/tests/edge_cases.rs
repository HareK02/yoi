//! Edge-case regression tests that should stay green.

use std::sync::Arc;

use llm_engine::tool::{Tool, ToolDefinition};
use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
use serde_json::json;
use tempfile::TempDir;
use tools::{Tracker, core_builtin_tools};
use workdir::{LocalWorkdirSession, WorkdirSessionHandle};

struct Registry {
    entries: Vec<(llm_engine::tool::ToolMeta, Arc<dyn Tool>)>,
}

impl Registry {
    fn new(defs: Vec<ToolDefinition>) -> Self {
        Self {
            entries: defs.into_iter().map(|f| f()).collect(),
        }
    }
    fn get(&self, name: &str) -> Arc<dyn Tool> {
        self.entries
            .iter()
            .find(|(m, _)| m.name == name)
            .map(|(_, t)| Arc::clone(t))
            .unwrap()
    }
}

fn setup() -> (TempDir, TempDir, Registry) {
    let dir = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    let base = Scope::writable(dir.path()).unwrap();
    let mut config = ScopeConfig {
        allow: base.allow_rules(),
        deny: base.deny_rules(),
    };
    config.allow.push(ScopeRule {
        target: spill.path().to_path_buf(),
        permission: Permission::Read,
        recursive: true,
    });
    let scope = Scope::from_config(&config).unwrap();
    let fs: WorkdirSessionHandle =
        std::sync::Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let tracker = Tracker::new();
    let reg = Registry::new(core_builtin_tools(fs, tracker, spill.path().to_path_buf()));
    (dir, spill, reg)
}

#[tokio::test]
async fn unicode_path_and_content() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("日本語ファイル.txt");
    let content = "こんにちは 🦀 世界\nabc\n";

    let write = reg.get("Write");
    write
        .execute(
            &json!({
                "file_path": file.file_name().unwrap().to_str().unwrap(),
                "content": content,
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(
            &json!({ "file_path": file.file_name().unwrap().to_str().unwrap() }).to_string(),
            Default::default(),
        )
        .await
        .unwrap();
    let body = out.content.unwrap();
    assert!(body.contains("🦀"));
    assert!(body.contains("こんにちは"));
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_to_outside_scope_is_rejected_for_write() {
    use std::os::unix::fs::symlink;

    let (dir, _spill, reg) = setup();
    let outside = TempDir::new().unwrap();
    let outside_target = outside.path().join("secret.txt");
    std::fs::write(&outside_target, "secret").unwrap();

    // Create a symlink inside the scope pointing to the outside file.
    let link = dir.path().join("linked.txt");
    symlink(&outside_target, &link).unwrap();

    // Read through the symlink must be rejected because the resolved
    // target sits outside the scope.
    let read = reg.get("Read");
    let read_err = read
        .execute(
            &json!({ "file_path": link.file_name().unwrap().to_str().unwrap() }).to_string(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{read_err}").contains("outside allowed read scope"),
        "symlink read escape not rejected: {read_err}"
    );
    assert!(
        !format!("{read_err}").contains(&outside_target.display().to_string()),
        "symlink diagnostics must not expose provider-internal paths: {read_err}"
    );

    // Write through the symlink must be rejected for the same reason.
    let write = reg.get("Write");
    let err = write
        .execute(
            &json!({
                "file_path": link.file_name().unwrap().to_str().unwrap(),
                "content": "overwritten",
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("outside allowed read scope")
            || msg.contains("outside allowed write scope")
            || msg.contains("has not been read"),
        "symlink escape not rejected: {msg}"
    );
    if !msg.contains("has not been read") {
        assert!(
            msg.contains("add the symlink target"),
            "symlink escape diagnostic should include remediation: {msg}"
        );
    }
    // Outside file must not have been touched.
    assert_eq!(std::fs::read_to_string(&outside_target).unwrap(), "secret");
}

#[cfg(unix)]
#[tokio::test]
async fn broken_symlink_reports_target_and_repair_hint() {
    use std::os::unix::fs::symlink;

    let (dir, _spill, reg) = setup();
    let link = dir.path().join("external-project");
    let target = dir.path().join("missing-target");
    symlink(&target, &link).unwrap();

    let read = reg.get("Read");
    let err = read
        .execute(
            &json!({ "file_path": link.file_name().unwrap().to_str().unwrap() }).to_string(),
            Default::default(),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("broken symlink"), "{msg}");
    assert!(msg.contains("external-project"), "{msg}");
    assert!(!msg.contains(&target.display().to_string()), "{msg}");
    assert!(msg.contains("correct relative target"), "{msg}");
}

#[tokio::test]
async fn empty_file_read_and_edit() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("empty.txt");
    std::fs::write(&file, "").unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(
            &json!({ "file_path": file.file_name().unwrap().to_str().unwrap() }).to_string(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(out.summary.contains("0 line"));

    // Edit on empty file must produce StringNotFound
    let edit = reg.get("Edit");
    let err = edit
        .execute(
            &json!({
                "file_path": file.file_name().unwrap().to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar",
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not found"));
}

#[tokio::test]
async fn very_long_single_line() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("long.txt");
    let big: String = "x".repeat(1024 * 1024); // 1 MiB, no newlines
    std::fs::write(&file, &big).unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(
            &json!({ "file_path": file.file_name().unwrap().to_str().unwrap() }).to_string(),
            Default::default(),
        )
        .await
        .unwrap();
    // Should return exactly 1 line
    assert!(out.summary.contains("1 line"));
}

#[tokio::test]
async fn absolute_path_is_rejected() {
    let (dir, _spill, reg) = setup();
    let read = reg.get("Read");
    let err = read
        .execute(
            &json!({ "file_path": dir.path().join("outside.txt") }).to_string(),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("invalid Workdir path"));
}

#[tokio::test]
async fn directory_target_is_rejected_for_read() {
    let (dir, _spill, reg) = setup();
    let read = reg.get("Read");
    let err = read
        .execute(&json!({ "file_path": "." }).to_string(), Default::default())
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("directory"));
}

#[tokio::test]
async fn deeply_nested_new_file_is_created() {
    let (dir, _spill, reg) = setup();
    let deep = dir.path().join("a/b/c/d/e/deep.txt");
    let write = reg.get("Write");
    write
        .execute(
            &json!({
                "file_path": "a/b/c/d/e/deep.txt",
                "content": "deep\n",
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&deep).unwrap(), "deep\n");
}

#[tokio::test]
async fn replace_preserves_unicode() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("u.txt");
    std::fs::write(&file, "🦀 rust 🦀\n").unwrap();

    let read = reg.get("Read");
    read.execute(
        &json!({ "file_path": file.file_name().unwrap().to_str().unwrap() }).to_string(),
        Default::default(),
    )
    .await
    .unwrap();

    let edit = reg.get("Edit");
    edit.execute(
        &json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "rust",
            "new_string": "ラスト",
        })
        .to_string(),
        Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "🦀 ラスト 🦀\n");
}

#[tokio::test]
async fn grep_handles_unicode_pattern() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("u.txt");
    std::fs::write(&file, "English\n日本語\nрусский\n").unwrap();

    let grep = reg.get("Grep");
    let out = grep
        .execute(
            &json!({
                "pattern": "日本語",
                "output_mode": "content",
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap();
    let body = out.content.unwrap();
    assert!(body.contains("日本語"));
}
