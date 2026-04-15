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

/// Outcome of an `AGENTS.md` ingestion attempt.
///
/// `body` carries the text that should be handed to the template
/// engine (if any); `warnings` are short human-readable messages that
/// Pod forwards to the user-facing notification channel. The caller
/// also gets `tracing::warn!` lines for the developer log.
pub(crate) struct AgentsMdResult {
    pub body: Option<String>,
    pub warnings: Vec<String>,
}

/// Read `AGENTS.md` from `cwd` if present. All non-fatal problems are
/// both logged via `tracing::warn!` (developer-facing) and surfaced
/// via `AgentsMdResult::warnings` (user-facing).
///
/// - Absent: `body = None`, no warning.
/// - Over limit: first 64KB (UTF-8 char boundary) + truncation notice, warning.
/// - Non-UTF-8 or I/O error: `body = None`, warning.
pub(crate) fn read_agents_md(cwd: &Path) -> AgentsMdResult {
    let path = cwd.join("AGENTS.md");
    let mut warnings = Vec::new();

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return AgentsMdResult {
                body: None,
                warnings,
            };
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to open AGENTS.md");
            warnings.push(format!("failed to open AGENTS.md ({}): {}", path.display(), e));
            return AgentsMdResult {
                body: None,
                warnings,
            };
        }
    };

    // Read one extra byte beyond the limit so we can detect oversize
    // regardless of what `metadata()` claims (pipes/procfs may lie).
    let mut buf = Vec::new();
    let read_limit = (AGENTS_MD_LIMIT as u64) + 1;
    if let Err(e) = file.take(read_limit).read_to_end(&mut buf) {
        warn!(path = %path.display(), error = %e, "failed to read AGENTS.md");
        warnings.push(format!("failed to read AGENTS.md ({}): {}", path.display(), e));
        return AgentsMdResult {
            body: None,
            warnings,
        };
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
            warnings.push(format!(
                "AGENTS.md ({}) is not valid UTF-8: {}",
                path.display(),
                e
            ));
            return AgentsMdResult {
                body: None,
                warnings,
            };
        }
    };

    let mut text = text;
    if truncated {
        warn!(
            path = %path.display(),
            limit = AGENTS_MD_LIMIT,
            "AGENTS.md exceeded size limit; truncating"
        );
        warnings.push(format!(
            "AGENTS.md ({}) exceeded {} bytes; the tail was truncated",
            path.display(),
            AGENTS_MD_LIMIT
        ));
        text.push_str(TRUNCATION_NOTICE);
    }

    AgentsMdResult {
        body: Some(text),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn absent_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_agents_md(dir.path()).body.is_none());
    }

    #[test]
    fn reads_small_file_verbatim() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# hello\nworld").unwrap();
        let result = read_agents_md(dir.path());
        assert_eq!(result.body.as_deref(), Some("# hello\nworld"));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn oversized_file_is_truncated_with_notice() {
        let dir = TempDir::new().unwrap();
        let body = "a".repeat(AGENTS_MD_LIMIT + 1024);
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();

        let result = read_agents_md(dir.path());
        let got = result.body.expect("some");
        assert!(got.ends_with(TRUNCATION_NOTICE));
        let prefix = got.strip_suffix(TRUNCATION_NOTICE).unwrap();
        assert_eq!(prefix.len(), AGENTS_MD_LIMIT);
        assert!(prefix.chars().all(|c| c == 'a'));
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn exact_limit_is_not_truncated() {
        let dir = TempDir::new().unwrap();
        let body = "a".repeat(AGENTS_MD_LIMIT);
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();

        let result = read_agents_md(dir.path());
        let got = result.body.expect("some");
        assert_eq!(got.len(), AGENTS_MD_LIMIT);
        assert!(!got.contains("truncated"));
        assert!(result.warnings.is_empty());
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

        let result = read_agents_md(dir.path());
        let got = result.body.expect("some");
        assert!(got.ends_with(TRUNCATION_NOTICE));
        let prefix = got.strip_suffix(TRUNCATION_NOTICE).unwrap();
        // The partial 'あ' must have been dropped, leaving only the ASCII prefix.
        assert_eq!(prefix.len(), AGENTS_MD_LIMIT - 1);
        assert!(prefix.chars().all(|c| c == 'a'));
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn non_utf8_surfaces_warning() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let result = read_agents_md(dir.path());
        assert!(result.body.is_none());
        assert_eq!(result.warnings.len(), 1);
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
        assert!(read_agents_md(dir.path()).body.is_none());
    }

    #[test]
    fn non_utf8_returns_none() {
        let dir = TempDir::new().unwrap();
        // Invalid UTF-8 start byte.
        fs::write(dir.path().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).unwrap();
        assert!(read_agents_md(dir.path()).body.is_none());
    }
}
