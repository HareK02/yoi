//! File content cache for the Edit renderer.
//!
//! Holds `path → content` for every file the TUI has observed via a
//! successful Read (where `ToolResult.output` contains the file body),
//! Write (where `args.content` is the new body), or Edit (which mutates
//! the cached body in-place). The cache is purely a display-side view —
//! it has no opinion on what the filesystem actually contains.

use std::collections::HashMap;

#[derive(Default)]
pub struct FileCache {
    contents: HashMap<String, String>,
}

impl FileCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.contents.insert(path.into(), content.into());
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.contents.get(path).map(String::as_str)
    }

    /// Apply an Edit-style substitution. When `old` is unique in the
    /// cached content we swap it for `new`; otherwise we leave the
    /// cache untouched (the TUI can't reliably reconstruct the new
    /// state in that case).
    pub fn apply_edit(&mut self, path: &str, old: &str, new: &str) {
        let Some(current) = self.contents.get(path) else {
            return;
        };
        // Only swap when `old` appears exactly once — mirrors the
        // tool's own precondition and keeps the cache from diverging
        // when ambiguity would otherwise force a guess.
        let mut occurrences = current.match_indices(old);
        let first = occurrences.next();
        let second = occurrences.next();
        if let (Some((idx, matched)), None) = (first, second) {
            let end = idx + matched.len();
            let mut buf = String::with_capacity(current.len() - old.len() + new.len());
            buf.push_str(&current[..idx]);
            buf.push_str(new);
            buf.push_str(&current[end..]);
            self.contents.insert(path.to_owned(), buf);
        }
    }
}
