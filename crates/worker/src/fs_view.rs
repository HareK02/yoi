//! Worker 視点のファイルシステム操作。
//!
//! `WorkdirSession` の上に「Worker が読み取りたい / 列挙したい」操作を集約する軽い wrapper。
//!
//! - `ReadRequirement` と `render_auto_read` — compact worker が `mark_read_required`
//!   で nominate したファイルを再読し、`[Auto-read file: ...]` system message に
//!   変換する経路。`Worker::compact` から呼ばれる。
//! - `slice_lines` — 行 offset / limit でテキストを切り出す純粋ヘルパ。
//!   compact tool 側の `mark_read_required` でも使用。
//! - `list_file_completions` — TUI 補完用、prefix マッチでファイル候補を列挙する経路。
//!   IPC `Method::ListCompletions` 経由で呼ばれる前提（Phase 2 で接続）。

use std::path::{Path, PathBuf};

use agen::Item;
use tools::ToolsError;
use tracing::warn;
#[cfg(test)]
use workdir::LocalWorkdirSession;
use workdir::{
    EntryKind, ListRequest, ReadRequest, StatRequest, WorkdirPath, WorkdirSessionHandle,
};

/// 補完候補1件の最大数。`list_file_completions` がこの値を超えたら打ち切り。
const COMPLETION_LIMIT: usize = 100;
/// submit-time directory FileRef の shallow listing で返す最大 entry 数。
/// TUI completion と同じ浅い一覧という意味論に揃えるため、同じ上限を使う。
const DIR_FILE_REF_ENTRY_LIMIT: usize = COMPLETION_LIMIT;
/// Provider-side bound for auto-read and submit-time referenced-file reads.
const AUTO_READ_BYTE_LIMIT: usize = 4 * 1024 * 1024;

/// Compact worker が `mark_read_required` で nominate した「次セッション開始時に
/// 自動で再読すべきファイル」のエントリ。
#[derive(Debug, Clone)]
pub struct ReadRequirement {
    pub path: PathBuf,
    /// 0-based line offset. `None` means from the start of the file.
    pub offset: Option<usize>,
    /// Maximum number of lines. `None` means to the end of the file.
    pub limit: Option<usize>,
}

/// Worker から見えるファイルシステム操作の入口。Clone は cheap（`WorkdirSession` 内 `Arc`）。
#[derive(Debug, Clone)]
pub struct WorkerFsView {
    session: WorkdirSessionHandle,
}

/// `list_file_completions` が返す候補1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCandidate {
    /// 入力 prefix と整合する形のパス（prefix が absolute なら absolute、
    /// relative なら cwd 相対）。
    pub path: String,
    pub is_dir: bool,
}

/// `resolve_file_ref` の失敗理由。Worker 側で Alert に振り分けるために
/// WorkdirSession / 内部判定の両方を区別できるよう保持する。
#[derive(Debug)]
pub enum ResolveError {
    /// Path resolution / scope check failed via `WorkdirSession`.
    Fs(ToolsError),
    /// File contents are not valid UTF-8 (binary / non-text).
    Binary { path: PathBuf },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Fs(e) => write!(f, "{e}"),
            ResolveError::Binary { path } => {
                write!(f, "file is not valid UTF-8 text: {}", path.display())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

impl WorkerFsView {
    pub fn new(session: WorkdirSessionHandle) -> Self {
        Self { session }
    }
    pub fn session(&self) -> &WorkdirSessionHandle {
        &self.session
    }

    pub async fn render_auto_read(&self, requirements: &[ReadRequirement]) -> Vec<Item> {
        let mut out = Vec::with_capacity(requirements.len());
        for req in requirements {
            let path = match WorkdirPath::new(req.path.to_string_lossy()) {
                Ok(path) => path,
                Err(error) => {
                    warn!(path = %req.path.display(), %error, "invalid auto-read path");
                    continue;
                }
            };
            match self
                .session
                .read(ReadRequest {
                    path: path.clone(),
                    offset: req.offset.unwrap_or(0),
                    limit: req.limit.unwrap_or(usize::MAX),
                    max_bytes: AUTO_READ_BYTE_LIMIT,
                })
                .await
            {
                Ok(result) => {
                    let body = String::from_utf8_lossy(&result.bytes);
                    let range = format_range(req.offset, req.limit);
                    out.push(Item::system_message(format!(
                        "[Auto-read file: {path}{range}]\n{body}"
                    )));
                }
                Err(error) => {
                    warn!(path = %path, %error, "auto-read target could not be read; skipping")
                }
            }
        }
        out
    }

    pub async fn resolve_file_ref(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> Result<Item, ResolveError> {
        let logical = WorkdirPath::new(path)
            .map_err(ToolsError::from)
            .map_err(ResolveError::Fs)?;
        let stat = self
            .session
            .stat(StatRequest {
                path: logical.clone(),
            })
            .await
            .map_err(ToolsError::from)
            .map_err(ResolveError::Fs)?;
        if stat.kind == EntryKind::Directory {
            let result = self
                .session
                .list(ListRequest {
                    path: logical.clone(),
                    limit: DIR_FILE_REF_ENTRY_LIMIT,
                })
                .await
                .map_err(ToolsError::from)
                .map_err(ResolveError::Fs)?;
            let listing = result
                .entries
                .into_iter()
                .map(|entry| match entry.kind {
                    EntryKind::Directory => format!("{}/", entry.path),
                    EntryKind::Symlink => format!("{}@", entry.path),
                    _ => entry.path.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            let suffix = format!(
                "\n[{} readable entries total, {} bytes total]{}",
                result.total_entries,
                result.total_bytes,
                if result.truncated {
                    "\n[...listing truncated; use Glob for more]"
                } else {
                    ""
                }
            );
            let header = format!("[Dir: {logical}]\n");
            let listing_budget = max_bytes.saturating_sub(header.len() + suffix.len());
            let (bounded_listing, truncated) = truncate_utf8_bytes(&listing, listing_budget);
            let mut text = format!("{header}{bounded_listing}{suffix}");
            if truncated {
                text.push_str("\n[...directory attachment truncated; use Glob or Read for more]");
            }
            return Ok(Item::system_message(text));
        }
        let result = self
            .session
            .read(ReadRequest {
                path: logical.clone(),
                offset: 0,
                limit: usize::MAX,
                max_bytes,
            })
            .await
            .map_err(ToolsError::from)
            .map_err(ResolveError::Fs)?;
        let total = stat.size;
        let end = result.bytes.len().min(max_bytes);
        let body = std::str::from_utf8(&result.bytes[..end]).map_err(|_| ResolveError::Binary {
            path: PathBuf::from(logical.as_str()),
        })?;
        let mut text = format!("[File: {logical}]\n{body}");
        if end < result.bytes.len() || result.truncated {
            text.push_str(&format!(
                "\n[...truncated, {total} bytes total — use Read for the rest]"
            ));
        }
        Ok(Item::system_message(text))
    }

    pub async fn list_file_completions(&self, prefix: &str) -> Vec<FileCandidate> {
        let prefix_path = Path::new(prefix);
        let (parent, needle) = if prefix.ends_with('/') {
            (prefix_path, String::new())
        } else {
            (
                prefix_path.parent().unwrap_or_else(|| Path::new("")),
                prefix_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        };
        let Ok(parent) = WorkdirPath::new(parent.to_string_lossy()) else {
            return Vec::new();
        };
        let Ok(result) = self
            .session
            .list(ListRequest {
                path: parent,
                limit: COMPLETION_LIMIT,
            })
            .await
        else {
            return Vec::new();
        };
        let mut out = result
            .entries
            .into_iter()
            .filter_map(|entry| {
                let name = Path::new(entry.path.as_str())
                    .file_name()?
                    .to_string_lossy();
                name.starts_with(&needle).then_some(FileCandidate {
                    path: entry.path.to_string(),
                    is_dir: entry.kind == EntryKind::Directory,
                })
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });
        out
    }
}

/// `text` の `offset` 行目から `limit` 行（None なら末尾まで）を、元の改行で繋いで返す。
pub fn slice_lines(text: &str, offset: usize, limit: Option<usize>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.min(lines.len());
    let end = limit
        .map(|n| start.saturating_add(n).min(lines.len()))
        .unwrap_or(lines.len());
    lines[start..end].join("\n")
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

fn format_range(offset: Option<usize>, limit: Option<usize>) -> String {
    match (offset, limit) {
        (None, None) => String::new(),
        (Some(off), None) => format!(":{}-", off + 1),
        (None, Some(lim)) => format!(":1-{lim}"),
        (Some(off), Some(lim)) => format!(":{}-{}", off + 1, off.saturating_add(lim)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::ContentPart;
    use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fs_for(dir: &TempDir) -> WorkdirSessionHandle {
        Arc::new(LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ))
    }

    fn touch(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn system_text(item: &Item) -> &str {
        let Item::Message { content, .. } = item else {
            panic!("expected message item");
        };
        let Some(ContentPart::Text { text }) = content.first() else {
            panic!("expected text content");
        };
        text
    }

    #[tokio::test]
    async fn slice_lines_handles_offset_and_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, 0, None), "a\nb\nc\nd");
        assert_eq!(slice_lines(text, 1, Some(2)), "b\nc");
        assert_eq!(slice_lines(text, 10, None), "");
    }

    #[tokio::test]
    async fn render_auto_read_emits_system_messages_with_range_label() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        let view = WorkerFsView::new(fs_for(&dir));
        let items = view
            .render_auto_read(&[ReadRequirement {
                path: PathBuf::from("hello.txt"),
                offset: Some(1),
                limit: Some(1),
            }])
            .await;

        assert_eq!(items.len(), 1);
        let rendered = format!("{:?}", items[0]);
        assert!(rendered.contains("Auto-read file"));
        assert!(rendered.contains(":2-2"));
        assert!(rendered.contains("beta"));
        assert!(!rendered.contains("alpha"));
    }

    #[tokio::test]
    async fn resolve_file_ref_emits_system_message_with_path_header() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("hello.txt", 1024).await.unwrap();
        let text = format!("{item:?}");
        assert!(text.contains("[File: hello.txt]"));
        assert!(text.contains("hello world"));
        assert!(!text.contains("truncated"));
    }

    #[tokio::test]
    async fn resolve_file_ref_truncates_with_hint_when_over_cap() {
        let dir = TempDir::new().unwrap();
        let body = "x".repeat(2048);
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("big.txt", 256).await.unwrap();
        let text = format!("{item:?}");
        assert!(text.contains("[File: big.txt]"));
        assert!(text.contains("truncated"));
        assert!(text.contains("2048 bytes total"));
    }

    #[tokio::test]
    async fn resolve_file_ref_lists_directory_shallow_entries() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("docs/sub")).unwrap();
        touch(&dir.path().join("docs/.hidden"), "hidden");
        touch(&dir.path().join("docs/.gitignore"), "ignored.txt\n");
        touch(
            &dir.path().join("docs/ignored.txt"),
            "not ignored for FileRef",
        );
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).await.unwrap();
        let text = system_text(&item);
        assert!(text.starts_with("[Dir: docs]\n"));
        assert!(text.contains("sub/"));
        assert!(text.contains(".hidden"));
        assert!(text.contains(".gitignore"));
        assert!(text.contains("ignored.txt"));

        let sub_pos = text.find("sub/").unwrap();
        let hidden_pos = text.find(".hidden").unwrap();
        assert!(
            sub_pos < hidden_pos,
            "directories should sort before files:\n{text}"
        );
    }

    #[tokio::test]
    async fn resolve_file_ref_directory_listing_filters_unreadable_entries() {
        let dir = TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        let secret = docs.join("secret");
        std::fs::create_dir_all(&secret).unwrap();
        touch(&docs.join("visible.txt"), "ok");
        touch(&secret.join("hidden.txt"), "nope");

        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: vec![ScopeRule {
                target: secret.clone(),
                permission: Permission::Read,
                recursive: true,
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let fs: WorkdirSessionHandle =
            Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
        let view = WorkerFsView::new(fs);

        let item = view.resolve_file_ref("docs", 4096).await.unwrap();
        let text = system_text(&item);
        assert!(text.contains("visible.txt"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("hidden.txt"));
    }

    #[tokio::test]
    async fn resolve_file_ref_directory_listing_uses_upload_byte_cap() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        touch(&dir.path().join("docs/very-long-file-name.txt"), "");
        touch(&dir.path().join("docs/another-long-file-name.txt"), "");
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 10).await.unwrap();
        let text = system_text(&item);
        assert!(text.starts_with("[Dir: docs]\n"));
        assert!(text.contains("truncated"));
        assert!(text.contains("bytes total"));
        assert!(text.contains("use Glob or Read for more"));
    }

    #[tokio::test]
    async fn resolve_file_ref_directory_listing_uses_completion_entry_limit() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        for i in 0..(DIR_FILE_REF_ENTRY_LIMIT + 5) {
            touch(&dir.path().join(format!("docs/file-{i:03}.txt")), "");
        }
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).await.unwrap();
        let text = system_text(&item);
        assert!(text.contains("105 readable entries total"));
        assert!(text.contains("file-099.txt"));
        assert!(!text.contains("file-100.txt"));
        assert!(text.contains("use Glob for more"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_file_ref_directory_listing_marks_readable_symlink_entries() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        touch(&dir.path().join("docs/target.txt"), "target");
        symlink("target.txt", dir.path().join("docs/link.txt")).unwrap();
        let view = WorkerFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).await.unwrap();
        let text = system_text(&item);
        assert!(text.contains("link.txt@"));
    }

    #[tokio::test]
    async fn resolve_file_ref_rejects_binary_with_binary_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("blob.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let view = WorkerFsView::new(fs_for(&dir));

        let err = view.resolve_file_ref("blob.bin", 1024).await.unwrap_err();
        assert!(matches!(err, ResolveError::Binary { .. }));
    }

    #[tokio::test]
    async fn resolve_file_ref_returns_fs_error_for_out_of_scope() {
        let outer = TempDir::new().unwrap();
        let inner = outer.path().join("scoped");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(outer.path().join("secret.txt"), "nope").unwrap();
        let scope = Scope::writable(&inner).unwrap();
        let fs: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(scope, inner.clone()));
        let view = WorkerFsView::new(fs);

        // Absolute path outside of scope.
        let outside = outer.path().join("secret.txt");
        let err = view
            .resolve_file_ref(outside.to_str().unwrap(), 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, ResolveError::Fs(_)));
    }

    #[tokio::test]
    async fn render_auto_read_skips_unreadable_targets() {
        let dir = TempDir::new().unwrap();
        let view = WorkerFsView::new(fs_for(&dir));
        let items = view
            .render_auto_read(&[ReadRequirement {
                path: dir.path().join("missing.txt"),
                offset: None,
                limit: None,
            }])
            .await;
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn list_file_completions_lists_pwd_when_prefix_empty() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("alpha.rs"), "");
        touch(&dir.path().join("beta.rs"), "");
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let view = WorkerFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("").await;
        // ディレクトリ first
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names, vec!["subdir", "alpha.rs", "beta.rs"]);
        assert!(cands[0].is_dir);
    }

    #[tokio::test]
    async fn list_file_completions_filters_by_name_prefix() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("alpha.rs"), "");
        touch(&dir.path().join("beta.rs"), "");
        let view = WorkerFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("al").await;
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].path, "alpha.rs");
    }

    #[tokio::test]
    async fn list_file_completions_descends_into_subdir_with_trailing_slash() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("sub/x.rs"), "");
        touch(&dir.path().join("sub/y.rs"), "");
        let view = WorkerFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("sub/").await;
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names, vec!["sub/x.rs", "sub/y.rs"]);
    }

    #[tokio::test]
    async fn list_file_completions_filters_out_non_readable_under_scope() {
        let dir = TempDir::new().unwrap();
        let secret = dir.path().join("secret");
        std::fs::create_dir(&secret).unwrap();
        touch(&dir.path().join("visible.rs"), "");
        touch(&secret.join("hidden.rs"), "");

        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: vec![ScopeRule {
                target: secret.clone(),
                permission: Permission::Read,
                recursive: true,
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let fs: WorkdirSessionHandle =
            Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
        let view = WorkerFsView::new(fs);

        let cands = view.list_file_completions("").await;
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert!(names.contains(&"visible.rs"));
        assert!(!names.contains(&"secret"));
    }

    #[tokio::test]
    async fn list_file_completions_rejects_absolute_prefix() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("a.rs"), "");
        let view = WorkerFsView::new(fs_for(&dir));

        let prefix = format!("{}/", dir.path().display());
        let cands = view.list_file_completions(&prefix).await;
        assert!(cands.is_empty());
    }
}
