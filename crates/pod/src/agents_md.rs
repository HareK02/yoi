//! `AGENTS.md` ingestion for system-prompt templates.
//!
//! Reads `AGENTS.md` directly under the Pod cwd and exposes its body
//! to the template engine through `SystemPromptContext.files.agents_md`.
//! Nested / parent-directory AGENTS.md files are intentionally ignored;
//! subproject context is expressed by launching a Pod with that
//! directory as cwd.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

use tracing::warn;

/// Hard cap on the bytes exposed to the template. Roughly 20-25k tokens,
/// well within typical provider rate limits.
pub(crate) const AGENTS_MD_LIMIT: usize = 64 * 1024;

const TRUNCATION_NOTICE: &str = "\n\n[truncated: AGENTS.md exceeded 64KB limit]";

/// Read `AGENTS.md` from `cwd` if present. Returns `None` for "absent or
/// unreadable"; all non-fatal problems are logged via `tracing::warn!`.
///
/// - Absent: `None`, no warn.
/// - Over limit: first 64KB (UTF-8 char boundary) + truncation notice, warn.
/// - Non-UTF-8 or I/O error: `None`, warn.
pub(crate) fn read_agents_md(cwd: &Path) -> Option<String> {
    let path = cwd.join("AGENTS.md");

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to open AGENTS.md");
            return None;
        }
    };

    // Read one extra byte beyond the limit so we can detect oversize
    // regardless of what `metadata()` claims (pipes/procfs may lie).
    let mut buf = Vec::new();
    let read_limit = (AGENTS_MD_LIMIT as u64) + 1;
    if let Err(e) = file.take(read_limit).read_to_end(&mut buf) {
        warn!(path = %path.display(), error = %e, "failed to read AGENTS.md");
        return None;
    }

    let truncated = buf.len() > AGENTS_MD_LIMIT;
    if truncated {
        buf.truncate(AGENTS_MD_LIMIT);
    }

    // UTF-8 decoding must not depend on whether the file exceeded the
    // size limit: the same "genuinely non-UTF-8" file should be rejected
    // regardless of its size. The only case in which we tolerate an
    // invalid tail is when truncation itself sliced through a multi-byte
    // char — at most 3 bytes of the final (4-byte) code point can be
    // orphaned that way. Anything worse means the file was already
    // non-UTF-8 before truncation, and we reject it.
    let text = match std::str::from_utf8(&buf) {
        Ok(_) => {
            // SAFETY path: buf is valid UTF-8 in its entirety.
            String::from_utf8(buf).expect("validated above")
        }
        Err(e) if truncated && e.valid_up_to() >= AGENTS_MD_LIMIT - 3 => {
            let valid_len = e.valid_up_to();
            buf.truncate(valid_len);
            String::from_utf8(buf).expect("valid_up_to prefix is valid UTF-8")
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "AGENTS.md is not valid UTF-8");
            return None;
        }
    };

    let mut text = text;
    if truncated {
        warn!(
            path = %path.display(),
            limit = AGENTS_MD_LIMIT,
            "AGENTS.md exceeded size limit; truncating"
        );
        text.push_str(TRUNCATION_NOTICE);
    }

    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn absent_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_agents_md(dir.path()).is_none());
    }

    #[test]
    fn reads_small_file_verbatim() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# hello\nworld").unwrap();
        assert_eq!(
            read_agents_md(dir.path()).as_deref(),
            Some("# hello\nworld"),
        );
    }

    #[test]
    fn oversized_file_is_truncated_with_notice() {
        let dir = TempDir::new().unwrap();
        let body = "a".repeat(AGENTS_MD_LIMIT + 1024);
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();

        let got = read_agents_md(dir.path()).expect("some");
        assert!(got.ends_with(TRUNCATION_NOTICE));
        let prefix = got.strip_suffix(TRUNCATION_NOTICE).unwrap();
        assert_eq!(prefix.len(), AGENTS_MD_LIMIT);
        assert!(prefix.chars().all(|c| c == 'a'));
    }

    #[test]
    fn exact_limit_is_not_truncated() {
        let dir = TempDir::new().unwrap();
        let body = "a".repeat(AGENTS_MD_LIMIT);
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();

        let got = read_agents_md(dir.path()).expect("some");
        assert_eq!(got.len(), AGENTS_MD_LIMIT);
        assert!(!got.contains("truncated"));
    }

    #[test]
    fn truncation_respects_utf8_char_boundary() {
        let dir = TempDir::new().unwrap();
        // Fill up to just under the limit with ASCII, then append a
        // multi-byte char that straddles the boundary.
        let mut body = "a".repeat(AGENTS_MD_LIMIT - 1);
        body.push('あ'); // 3 bytes → pushes total past the limit
        body.push_str(&"b".repeat(128));
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();

        let got = read_agents_md(dir.path()).expect("some");
        assert!(got.ends_with(TRUNCATION_NOTICE));
        let prefix = got.strip_suffix(TRUNCATION_NOTICE).unwrap();
        // The partial 'あ' must have been dropped, leaving only the ASCII prefix.
        assert_eq!(prefix.len(), AGENTS_MD_LIMIT - 1);
        assert!(prefix.chars().all(|c| c == 'a'));
    }

    #[test]
    fn oversized_non_utf8_is_still_rejected() {
        // Regression: a file that is genuinely non-UTF-8 must be rejected
        // regardless of its size. Previously the truncation-recovery pop
        // loop would silently accept a partial prefix of such files once
        // they exceeded the limit.
        let dir = TempDir::new().unwrap();
        let body = vec![0xffu8; AGENTS_MD_LIMIT + 1024];
        fs::write(dir.path().join("AGENTS.md"), body).unwrap();
        assert!(read_agents_md(dir.path()).is_none());
    }

    #[test]
    fn non_utf8_returns_none() {
        let dir = TempDir::new().unwrap();
        // Invalid UTF-8 start byte.
        fs::write(dir.path().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).unwrap();
        assert!(read_agents_md(dir.path()).is_none());
    }
}
