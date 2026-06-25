//! `AGENTS.md` ingestion for system-prompt templates.
//!
//! Reads `AGENTS.md` directly under the Worker cwd and exposes its body
//! to the template engine through `SystemPromptContext.agents_md`.
//! Nested / parent-directory AGENTS.md files are intentionally ignored;
//! subproject context is expressed by launching a Worker with that
//! directory as cwd.
//!
//! No size cap is applied here — the whole file is read and embedded.
//! System-prompt-size policing is the responsibility of a higher layer
//! (Usage-driven warning after the first LLM round-trip).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use tracing::warn;

/// Outcome of an `AGENTS.md` ingestion attempt.
///
/// `body` carries the text that should be handed to the template
/// engine (if any); `warnings` are short human-readable messages that
/// Worker forwards to the user-facing notification channel. The caller
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
/// - Non-UTF-8 or I/O error: `body = None`, warning.
pub(crate) fn read_agents_md(cwd: &Path) -> AgentsMdResult {
    let path = cwd.join("AGENTS.md");
    let mut warnings = Vec::new();

    match fs::read_to_string(&path) {
        Ok(body) => AgentsMdResult {
            body: Some(body),
            warnings,
        },
        Err(e) if e.kind() == ErrorKind::NotFound => AgentsMdResult {
            body: None,
            warnings,
        },
        Err(e) if e.kind() == ErrorKind::InvalidData => {
            warn!(path = %path.display(), error = %e, "AGENTS.md is not valid UTF-8");
            warnings.push(format!(
                "AGENTS.md ({}) is not valid UTF-8: {}",
                path.display(),
                e
            ));
            AgentsMdResult {
                body: None,
                warnings,
            }
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to read AGENTS.md");
            warnings.push(format!(
                "failed to read AGENTS.md ({}): {}",
                path.display(),
                e
            ));
            AgentsMdResult {
                body: None,
                warnings,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn reads_large_file_verbatim() {
        // Previously truncated at 64KB; now read whole. Size-policing
        // is deferred to the Usage-driven warning layer.
        let dir = TempDir::new().unwrap();
        let body = "a".repeat(128 * 1024);
        fs::write(dir.path().join("AGENTS.md"), &body).unwrap();
        let result = read_agents_md(dir.path());
        assert_eq!(result.body.as_ref().map(String::len), Some(128 * 1024));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn non_utf8_surfaces_warning() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let result = read_agents_md(dir.path());
        assert!(result.body.is_none());
        assert_eq!(result.warnings.len(), 1);
    }
}
