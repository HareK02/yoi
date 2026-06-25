//! Pod 視点のファイルシステム操作。
//!
//! `ScopedFs` の上に「Pod が読み取りたい / 列挙したい」操作を集約する軽い wrapper。
//!
//! - `ReadRequirement` と `render_auto_read` — compact worker が `mark_read_required`
//!   で nominate したファイルを再読し、`[Auto-read file: ...]` system message に
//!   変換する経路。`Pod::compact` から呼ばれる。
//! - `slice_lines` — 行 offset / limit でテキストを切り出す純粋ヘルパ。
//!   compact tool 側の `mark_read_required` でも使用。
//! - `list_file_completions` — TUI 補完用、prefix マッチでファイル候補を列挙する経路。
//!   IPC `Method::ListCompletions` 経由で呼ばれる前提（Phase 2 で接続）。

use std::path::{Path, PathBuf};

use llm_engine::Item;
use manifest::Scope;
use tools::scoped_fs::first_symlink;
use tools::{ScopedFs, ToolsError};
use tracing::warn;

/// 補完候補1件の最大数。`list_file_completions` がこの値を超えたら打ち切り。
const COMPLETION_LIMIT: usize = 100;
/// submit-time directory FileRef の shallow listing で返す最大 entry 数。
/// TUI completion と同じ浅い一覧という意味論に揃えるため、同じ上限を使う。
const DIR_FILE_REF_ENTRY_LIMIT: usize = COMPLETION_LIMIT;

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

/// Pod から見えるファイルシステム操作の入口。Clone は cheap（`ScopedFs` 内 `Arc`）。
#[derive(Debug, Clone)]
pub struct PodFsView {
    fs: ScopedFs,
}

/// `list_file_completions` が返す候補1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCandidate {
    /// 入力 prefix と整合する形のパス（prefix が absolute なら absolute、
    /// relative なら cwd 相対）。
    pub path: String,
    pub is_dir: bool,
}

/// `resolve_file_ref` の失敗理由。Pod 側で Alert に振り分けるために
/// ScopedFs / 内部判定の両方を区別できるよう保持する。
#[derive(Debug)]
pub enum ResolveError {
    /// Path resolution / scope check failed via `ScopedFs`.
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

impl PodFsView {
    pub fn new(fs: ScopedFs) -> Self {
        Self { fs }
    }

    pub fn fs(&self) -> &ScopedFs {
        &self.fs
    }

    /// `requirements` の各エントリを `ScopedFs` 経由で再読し、
    /// `[Auto-read file: <path>:<range>]\n<body>` 形式の system message に変換する。
    /// 読み取り失敗（NotFound / OutOfScope 等）は warn で記録してスキップする
    /// — compact 全体を落とさないため。
    pub fn render_auto_read(&self, requirements: &[ReadRequirement]) -> Vec<Item> {
        let mut out = Vec::with_capacity(requirements.len());
        for req in requirements {
            match self.fs.read_bytes(&req.path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let body = slice_lines(&text, req.offset.unwrap_or(0), req.limit);
                    let range = format_range(req.offset, req.limit);
                    out.push(Item::system_message(format!(
                        "[Auto-read file: {}{range}]\n{body}",
                        req.path.display()
                    )));
                }
                Err(e) => {
                    warn!(
                        path = %req.path.display(),
                        error = %e,
                        "auto-read target could not be read; skipping",
                    );
                }
            }
        }
        out
    }

    /// `path` を ScopedFs 経由で解決し、submit 時の `Segment::FileRef`
    /// attachment 用 system message を返す。
    ///
    /// - `path` は relative なら cwd 相対、absolute なら absolute として解釈
    /// - 通常ディレクトリは浅い entry listing として `[Dir: <path>]\n<body>` に展開する
    /// - ディレクトリ listing は hidden / gitignore を特別扱いせず、scope 上 readable な
    ///   直下 entry だけを最大 `DIR_FILE_REF_ENTRY_LIMIT` 件返す
    /// - ファイル本文またはディレクトリ listing 本文が `max_bytes` を超える場合は切り詰める
    /// - 非 UTF-8 (バイナリ) は `ResolveError::Binary` で拒否
    /// - スコープ外 / NotFound / symlink directory 等は `ResolveError::Fs` で返す
    pub fn resolve_file_ref(&self, path: &str, max_bytes: usize) -> Result<Item, ResolveError> {
        let p = Path::new(path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.fs.cwd().join(p)
        };

        // 通常ディレクトリだけを FileRef listing として扱う。symlink を含むパスは
        // `ScopedFs::read_bytes` に委ね、既存の symlink 診断
        // (`SymlinkTargetIsDirectory` / `SymlinkOutOfScope` 等) を保つ。
        if first_symlink(&abs).is_none() {
            let scope = self.fs.scope();
            if !scope.is_readable(&abs) {
                return Err(ResolveError::Fs(ToolsError::OutOfScope(abs)));
            }
            let meta = metadata_for_file_ref(&abs).map_err(ResolveError::Fs)?;
            if meta.is_dir() {
                return render_dir_file_ref(path, &abs, max_bytes, scope.as_ref());
            }
        }

        let bytes = self.fs.read_bytes(&abs).map_err(ResolveError::Fs)?;
        let total = bytes.len();
        let (body_bytes, truncated) = if total > max_bytes {
            (&bytes[..max_bytes], true)
        } else {
            (bytes.as_slice(), false)
        };
        let body = std::str::from_utf8(body_bytes)
            .map_err(|_| ResolveError::Binary { path: abs.clone() })?;
        let mut text = format!("[File: {path}]\n{body}");
        if truncated {
            text.push_str(&format!(
                "\n[...truncated, {total} bytes total — use read_file for the rest]"
            ));
        }
        Ok(Item::system_message(text))
    }

    /// `prefix` にマッチするファイル / ディレクトリを scope 内で浅く列挙する。
    ///
    /// - `prefix` が空 or `cwd` 相対のときは cwd 直下を見る
    /// - `prefix` が末尾 `/` のときはそのディレクトリ直下を全列挙
    /// - 末尾が名前部分のときは、その名前を starts_with でフィルタ
    /// - scope 上 readable なエントリのみ返す
    /// - ディレクトリ → ファイル の順、各グループ内は名前昇順
    /// - 上限 `COMPLETION_LIMIT` 件で打ち切り（深い列挙はしない）
    pub fn list_file_completions(&self, prefix: &str) -> Vec<FileCandidate> {
        let cwd = self.fs.cwd();
        let scope = self.fs.scope();
        let (dir, name_prefix, is_absolute) = split_prefix(prefix, cwd);

        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut out = Vec::new();
        for entry in read_dir.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if !name.starts_with(&name_prefix) {
                continue;
            }
            let path = entry.path();
            if !scope.is_readable(&path) {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let display = if is_absolute {
                path.display().to_string()
            } else {
                path.strip_prefix(cwd)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string())
            };
            out.push(FileCandidate {
                path: display,
                is_dir,
            });
        }

        out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });
        out.truncate(COMPLETION_LIMIT);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirListingEntry {
    display: String,
    kind_rank: u8,
}

fn metadata_for_file_ref(path: &Path) -> Result<std::fs::Metadata, ToolsError> {
    std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolsError::NotFound(path.to_path_buf()),
        _ => ToolsError::io(path, e),
    })
}

fn render_dir_file_ref(
    original_path: &str,
    abs: &Path,
    max_bytes: usize,
    scope: &Scope,
) -> Result<Item, ResolveError> {
    let read_dir = std::fs::read_dir(abs).map_err(|e| ResolveError::Fs(ToolsError::io(abs, e)))?;
    let mut entries = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|e| ResolveError::Fs(ToolsError::io(abs, e)))?;
        let path = entry.path();
        if !scope.is_readable(&path) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => return Err(ResolveError::Fs(ToolsError::io(&path, e))),
        };
        let mut display = entry.file_name().to_string_lossy().into_owned();
        let kind_rank = if file_type.is_dir() {
            display.push('/');
            0
        } else if file_type.is_symlink() {
            display.push('@');
            1
        } else {
            2
        };
        entries.push(DirListingEntry { display, kind_rank });
    }

    entries.sort_by(|a, b| {
        a.kind_rank
            .cmp(&b.kind_rank)
            .then_with(|| a.display.cmp(&b.display))
    });

    let total_entries = entries.len();
    let entry_truncated = total_entries > DIR_FILE_REF_ENTRY_LIMIT;
    let body = if total_entries == 0 {
        "(empty directory)".to_string()
    } else {
        entries
            .iter()
            .take(DIR_FILE_REF_ENTRY_LIMIT)
            .map(|e| e.display.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body_total_bytes = body.len();
    let (body, byte_truncated) = truncate_utf8_bytes(&body, max_bytes);

    let mut text = format!("[Dir: {original_path}]\n{body}");
    if entry_truncated || byte_truncated {
        text.push('\n');
        text.push_str(&dir_listing_truncation_hint(
            entry_truncated,
            byte_truncated,
            total_entries,
            body_total_bytes,
        ));
    }
    Ok(Item::system_message(text))
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

fn dir_listing_truncation_hint(
    entry_truncated: bool,
    byte_truncated: bool,
    total_entries: usize,
    body_total_bytes: usize,
) -> String {
    match (entry_truncated, byte_truncated) {
        (true, true) => format!(
            "[...truncated, {total_entries} readable entries total; first {DIR_FILE_REF_ENTRY_LIMIT} entries were {body_total_bytes} bytes before byte cap — use Glob for more]"
        ),
        (true, false) => {
            format!("[...truncated, {total_entries} readable entries total — use Glob for more]")
        }
        (false, true) => {
            format!("[...truncated, {body_total_bytes} bytes total — use Glob or Read for more]")
        }
        (false, false) => String::new(),
    }
}

fn format_range(offset: Option<usize>, limit: Option<usize>) -> String {
    match (offset, limit) {
        (None, None) => String::new(),
        (Some(off), None) => format!(":{}-", off + 1),
        (None, Some(lim)) => format!(":1-{lim}"),
        (Some(off), Some(lim)) => format!(":{}-{}", off + 1, off.saturating_add(lim)),
    }
}

fn split_prefix(prefix: &str, cwd: &Path) -> (PathBuf, String, bool) {
    let is_absolute = Path::new(prefix).is_absolute();
    let p = Path::new(prefix);
    let (parent, name) = if prefix.is_empty() || prefix.ends_with('/') {
        (p.to_path_buf(), String::new())
    } else {
        let parent = p.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (parent, name)
    };
    let dir = if is_absolute {
        parent
    } else if parent.as_os_str().is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(parent)
    };
    (dir, name, is_absolute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_engine::ContentPart;
    use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
    use tempfile::TempDir;

    fn fs_for(dir: &TempDir) -> ScopedFs {
        ScopedFs::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        )
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

    #[test]
    fn slice_lines_handles_offset_and_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, 0, None), "a\nb\nc\nd");
        assert_eq!(slice_lines(text, 1, Some(2)), "b\nc");
        assert_eq!(slice_lines(text, 10, None), "");
    }

    #[test]
    fn render_auto_read_emits_system_messages_with_range_label() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        let view = PodFsView::new(fs_for(&dir));
        let items = view.render_auto_read(&[ReadRequirement {
            path: file.clone(),
            offset: Some(1),
            limit: Some(1),
        }]);

        assert_eq!(items.len(), 1);
        let rendered = format!("{:?}", items[0]);
        assert!(rendered.contains("Auto-read file"));
        assert!(rendered.contains(":2-2"));
        assert!(rendered.contains("beta"));
        assert!(!rendered.contains("alpha"));
    }

    #[test]
    fn resolve_file_ref_emits_system_message_with_path_header() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("hello.txt", 1024).unwrap();
        let text = format!("{item:?}");
        assert!(text.contains("[File: hello.txt]"));
        assert!(text.contains("hello world"));
        assert!(!text.contains("truncated"));
    }

    #[test]
    fn resolve_file_ref_truncates_with_hint_when_over_cap() {
        let dir = TempDir::new().unwrap();
        let body = "x".repeat(2048);
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("big.txt", 256).unwrap();
        let text = format!("{item:?}");
        assert!(text.contains("[File: big.txt]"));
        assert!(text.contains("truncated"));
        assert!(text.contains("2048 bytes total"));
    }

    #[test]
    fn resolve_file_ref_lists_directory_shallow_entries() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("docs/sub")).unwrap();
        touch(&dir.path().join("docs/.hidden"), "hidden");
        touch(&dir.path().join("docs/.gitignore"), "ignored.txt\n");
        touch(
            &dir.path().join("docs/ignored.txt"),
            "not ignored for FileRef",
        );
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).unwrap();
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

    #[test]
    fn resolve_file_ref_directory_listing_filters_unreadable_entries() {
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
        let fs = ScopedFs::new(scope, dir.path().to_path_buf());
        let view = PodFsView::new(fs);

        let item = view.resolve_file_ref("docs", 4096).unwrap();
        let text = system_text(&item);
        assert!(text.contains("visible.txt"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("hidden.txt"));
    }

    #[test]
    fn resolve_file_ref_directory_listing_uses_upload_byte_cap() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        touch(&dir.path().join("docs/very-long-file-name.txt"), "");
        touch(&dir.path().join("docs/another-long-file-name.txt"), "");
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 10).unwrap();
        let text = system_text(&item);
        assert!(text.starts_with("[Dir: docs]\n"));
        assert!(text.contains("truncated"));
        assert!(text.contains("bytes total"));
        assert!(text.contains("use Glob or Read for more"));
    }

    #[test]
    fn resolve_file_ref_directory_listing_uses_completion_entry_limit() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        for i in 0..(DIR_FILE_REF_ENTRY_LIMIT + 5) {
            touch(&dir.path().join(format!("docs/file-{i:03}.txt")), "");
        }
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).unwrap();
        let text = system_text(&item);
        assert!(text.contains("105 readable entries total"));
        assert!(text.contains("file-099.txt"));
        assert!(!text.contains("file-100.txt"));
        assert!(text.contains("use Glob for more"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_file_ref_directory_listing_marks_readable_symlink_entries() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        touch(&dir.path().join("docs/target.txt"), "target");
        symlink("target.txt", dir.path().join("docs/link.txt")).unwrap();
        let view = PodFsView::new(fs_for(&dir));

        let item = view.resolve_file_ref("docs", 4096).unwrap();
        let text = system_text(&item);
        assert!(text.contains("link.txt@"));
    }

    #[test]
    fn resolve_file_ref_rejects_binary_with_binary_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("blob.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let view = PodFsView::new(fs_for(&dir));

        let err = view.resolve_file_ref("blob.bin", 1024).unwrap_err();
        assert!(matches!(err, ResolveError::Binary { .. }));
    }

    #[test]
    fn resolve_file_ref_returns_fs_error_for_out_of_scope() {
        let outer = TempDir::new().unwrap();
        let inner = outer.path().join("scoped");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(outer.path().join("secret.txt"), "nope").unwrap();
        let scope = Scope::writable(&inner).unwrap();
        let fs = ScopedFs::new(scope, inner.clone());
        let view = PodFsView::new(fs);

        // Absolute path outside of scope.
        let outside = outer.path().join("secret.txt");
        let err = view
            .resolve_file_ref(outside.to_str().unwrap(), 1024)
            .unwrap_err();
        assert!(matches!(err, ResolveError::Fs(_)));
    }

    #[test]
    fn render_auto_read_skips_unreadable_targets() {
        let dir = TempDir::new().unwrap();
        let view = PodFsView::new(fs_for(&dir));
        let items = view.render_auto_read(&[ReadRequirement {
            path: dir.path().join("missing.txt"),
            offset: None,
            limit: None,
        }]);
        assert!(items.is_empty());
    }

    #[test]
    fn list_file_completions_lists_pwd_when_prefix_empty() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("alpha.rs"), "");
        touch(&dir.path().join("beta.rs"), "");
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let view = PodFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("");
        // ディレクトリ first
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names, vec!["subdir", "alpha.rs", "beta.rs"]);
        assert!(cands[0].is_dir);
    }

    #[test]
    fn list_file_completions_filters_by_name_prefix() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("alpha.rs"), "");
        touch(&dir.path().join("beta.rs"), "");
        let view = PodFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("al");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].path, "alpha.rs");
    }

    #[test]
    fn list_file_completions_descends_into_subdir_with_trailing_slash() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("sub/x.rs"), "");
        touch(&dir.path().join("sub/y.rs"), "");
        let view = PodFsView::new(fs_for(&dir));

        let cands = view.list_file_completions("sub/");
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(names, vec!["sub/x.rs", "sub/y.rs"]);
    }

    #[test]
    fn list_file_completions_filters_out_non_readable_under_scope() {
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
        let fs = ScopedFs::new(scope, dir.path().to_path_buf());
        let view = PodFsView::new(fs);

        let cands = view.list_file_completions("");
        let names: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert!(names.contains(&"visible.rs"));
        assert!(!names.contains(&"secret"));
    }

    #[test]
    fn list_file_completions_supports_absolute_prefix() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("a.rs"), "");
        let view = PodFsView::new(fs_for(&dir));

        let prefix = format!("{}/", dir.path().display());
        let cands = view.list_file_completions(&prefix);
        assert_eq!(cands.len(), 1);
        assert!(cands[0].path.starts_with('/'));
        assert!(cands[0].path.ends_with("a.rs"));
    }
}
