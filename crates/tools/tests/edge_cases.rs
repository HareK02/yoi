//! Edge-case regression tests that should stay green.

use std::sync::Arc;

use llm_worker::tool::{Tool, ToolDefinition};
use manifest::Scope;
use serde_json::json;
use tempfile::TempDir;
use tools::{ReadTracker, ScopedFs, builtin_tools};

struct Registry {
    entries: Vec<(llm_worker::tool::ToolMeta, Arc<dyn Tool>)>,
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

fn setup() -> (TempDir, Registry) {
    let dir = TempDir::new().unwrap();
    let fs = ScopedFs::new(Scope::new(dir.path()).unwrap());
    let tracker = ReadTracker::new();
    (dir, Registry::new(builtin_tools(fs, tracker)))
}

#[tokio::test]
async fn unicode_path_and_content() {
    let (dir, reg) = setup();
    let file = dir.path().join("日本語ファイル.txt");
    let content = "こんにちは 🦀 世界\nabc\n";

    let write = reg.get("Write");
    write
        .execute(
            &json!({
                "file_path": file.to_str().unwrap(),
                "content": content,
            })
            .to_string(),
        )
        .await
        .unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(
            &json!({ "file_path": file.to_str().unwrap() })
                .to_string(),
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

    let (dir, reg) = setup();
    let outside = TempDir::new().unwrap();
    let outside_target = outside.path().join("secret.txt");
    std::fs::write(&outside_target, "secret").unwrap();

    // Create a symlink inside the scope pointing to the outside file.
    let link = dir.path().join("linked.txt");
    symlink(&outside_target, &link).unwrap();

    // Read tool must work against the symlink (read is unrestricted).
    let read = reg.get("Read");
    read.execute(
        &json!({ "file_path": link.to_str().unwrap() }).to_string(),
    )
    .await
    .unwrap();

    // Write through the symlink must be rejected because canonicalization
    // resolves it to outside the scope.
    let write = reg.get("Write");
    let err = write
        .execute(
            &json!({
                "file_path": link.to_str().unwrap(),
                "content": "overwritten",
            })
            .to_string(),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("outside allowed scope"),
        "symlink escape not rejected: {msg}"
    );
    // Outside file must not have been touched.
    assert_eq!(std::fs::read_to_string(&outside_target).unwrap(), "secret");
}

#[tokio::test]
async fn empty_file_read_and_edit() {
    let (dir, reg) = setup();
    let file = dir.path().join("empty.txt");
    std::fs::write(&file, "").unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(&json!({ "file_path": file.to_str().unwrap() }).to_string())
        .await
        .unwrap();
    assert!(out.summary.contains("0 line"));

    // Edit on empty file must produce StringNotFound
    let edit = reg.get("Edit");
    let err = edit
        .execute(
            &json!({
                "file_path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar",
            })
            .to_string(),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not found"));
}

#[tokio::test]
async fn very_long_single_line() {
    let (dir, reg) = setup();
    let file = dir.path().join("long.txt");
    let big: String = "x".repeat(1024 * 1024); // 1 MiB, no newlines
    std::fs::write(&file, &big).unwrap();

    let read = reg.get("Read");
    let out = read
        .execute(&json!({ "file_path": file.to_str().unwrap() }).to_string())
        .await
        .unwrap();
    // Should return exactly 1 line
    assert!(out.summary.contains("1 line"));
}

#[tokio::test]
async fn relative_path_is_rejected() {
    let (_dir, reg) = setup();
    let read = reg.get("Read");
    let err = read
        .execute(&json!({ "file_path": "relative.txt" }).to_string())
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("absolute"));
}

#[tokio::test]
async fn directory_target_is_rejected_for_read() {
    let (dir, reg) = setup();
    let read = reg.get("Read");
    let err = read
        .execute(&json!({ "file_path": dir.path().to_str().unwrap() }).to_string())
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("directory"));
}

#[tokio::test]
async fn deeply_nested_new_file_is_created() {
    let (dir, reg) = setup();
    let deep = dir.path().join("a/b/c/d/e/deep.txt");
    let write = reg.get("Write");
    write
        .execute(
            &json!({
                "file_path": deep.to_str().unwrap(),
                "content": "deep\n",
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&deep).unwrap(), "deep\n");
}

#[tokio::test]
async fn replace_preserves_unicode() {
    let (dir, reg) = setup();
    let file = dir.path().join("u.txt");
    std::fs::write(&file, "🦀 rust 🦀\n").unwrap();

    let read = reg.get("Read");
    read.execute(&json!({ "file_path": file.to_str().unwrap() }).to_string())
        .await
        .unwrap();

    let edit = reg.get("Edit");
    edit.execute(
        &json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "rust",
            "new_string": "ラスト",
        })
        .to_string(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "🦀 ラスト 🦀\n");
}

#[tokio::test]
async fn grep_handles_unicode_pattern() {
    let (dir, reg) = setup();
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
        )
        .await
        .unwrap();
    let body = out.content.unwrap();
    assert!(body.contains("日本語"));
}
