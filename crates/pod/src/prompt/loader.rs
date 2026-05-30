//! Prefix-addressed prompt asset loader used by [`crate::SystemPromptTemplate`].
//!
//! Three prefixes address three physical libraries:
//!
//! | prefix       | location                                                |
//! |--------------|---------------------------------------------------------|
//! | `$insomnia`  | builtin, baked into the binary via `include_dir!`       |
//! | `$user`      | `<config_dir>/prompts/` (resolved by `manifest::paths`) |
//! | `$workspace` | `<project>/.insomnia/prompts/`                          |
//!
//! A reference is `$<prefix>/<path>` where `<path>` is a `/`-separated
//! name without the `.md` extension (e.g. `$insomnia/common/header`).
//! Unqualified names (no `$prefix/` at the front) are resolved relative
//! to an optional current reference — typically the file that issued
//! the `{% include %}` — so a prompt library can be authored as a
//! self-contained directory.
//!
//! Missing files produce a [`LoaderError::NotFound`]; there is no
//! fallthrough between layers.

use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};
use thiserror::Error;

static BUILTIN_PROMPTS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../resources/prompts");

const PREFIX_INSOMNIA: &str = "$insomnia";
const PREFIX_USER: &str = "$user";
const PREFIX_WORKSPACE: &str = "$workspace";

/// Prefix-resolved reference to a prompt asset. Produced by
/// [`PromptLoader::parse_ref`] from a user-supplied string such as
/// `"$insomnia/default"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRef {
    prefix: Prefix,
    /// Relative path under the prefix root, without the `.md` extension.
    /// `/`-separated, never empty, never starts with `/`.
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefix {
    Insomnia,
    User,
    Workspace,
}

impl Prefix {
    fn as_str(self) -> &'static str {
        match self {
            Self::Insomnia => PREFIX_INSOMNIA,
            Self::User => PREFIX_USER,
            Self::Workspace => PREFIX_WORKSPACE,
        }
    }
}

impl PromptRef {
    /// Produce a canonical `$prefix/path` string.
    pub fn to_qualified_string(&self) -> String {
        format!("{}/{}", self.prefix.as_str(), self.path)
    }

    /// Directory portion (leading prefix segments minus the file name),
    /// joined with `/`. Returns an empty string when the ref points at
    /// a file directly under the prefix root.
    fn dir(&self) -> &str {
        match self.path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        }
    }
}

/// Errors produced when resolving a [`PromptRef`].
#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("invalid prompt reference '{raw}': {reason}")]
    InvalidRef { raw: String, reason: String },
    #[error("unknown prompt prefix '{prefix}' in reference '{raw}'")]
    UnknownPrefix { raw: String, prefix: String },
    #[error(
        "unqualified prompt reference '{raw}' requires a current prefix \
         (include it from inside another template, or use an explicit \
         $prefix/path form)"
    )]
    UnqualifiedWithoutCurrent { raw: String },
    #[error("prompt prefix '{prefix}' is not configured for this loader")]
    PrefixNotConfigured { prefix: &'static str },
    #[error("prompt asset not found: '{}'", .reference.to_qualified_string())]
    NotFound { reference: PromptRef },
    #[error("failed to read prompt asset '{}': {source}", .reference.to_qualified_string())]
    Io {
        reference: PromptRef,
        #[source]
        source: std::io::Error,
    },
}

/// Loader that resolves [`PromptRef`]s against the configured prompt
/// libraries. Cheap to clone.
///
/// Also carries the auto-discovered `prompts.toml` pack file paths so
/// [`crate::prompt::catalog::PromptCatalog`] can read the same user/workspace
/// layers without a separate plumbing channel. These fields do not
/// affect `$prefix` asset resolution — they are purely metadata
/// consulted by the catalog loader.
#[derive(Debug, Clone)]
pub struct PromptLoader {
    user_dir: Option<PathBuf>,
    workspace_dir: Option<PathBuf>,
    user_pack_file: Option<PathBuf>,
    workspace_pack_file: Option<PathBuf>,
}

impl PromptLoader {
    /// Loader with only the builtin `$insomnia` library available.
    /// `$user` / `$workspace` references fail with
    /// [`LoaderError::PrefixNotConfigured`].
    pub fn builtins_only() -> Self {
        Self {
            user_dir: None,
            workspace_dir: None,
            user_pack_file: None,
            workspace_pack_file: None,
        }
    }

    /// Loader with optional user and workspace prompt directories.
    pub fn new(user_dir: Option<PathBuf>, workspace_dir: Option<PathBuf>) -> Self {
        Self {
            user_dir,
            workspace_dir,
            user_pack_file: None,
            workspace_pack_file: None,
        }
    }

    /// Override pack file paths supplied by the caller's profile/manifest
    /// resolution context.
    pub fn with_pack_files(
        mut self,
        user_pack_file: Option<PathBuf>,
        workspace_pack_file: Option<PathBuf>,
    ) -> Self {
        self.user_pack_file = user_pack_file;
        self.workspace_pack_file = workspace_pack_file;
        self
    }

    /// Root of the `$user` prompt library, if configured.
    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    /// Root of the `$workspace` prompt library, if configured.
    pub fn workspace_dir(&self) -> Option<&Path> {
        self.workspace_dir.as_deref()
    }

    /// Auto-discovered path to the user-layer `prompts.toml` pack, if any.
    pub fn user_pack_file(&self) -> Option<&Path> {
        self.user_pack_file.as_deref()
    }

    /// Auto-discovered path to the workspace-layer `prompts.toml` pack, if any.
    pub fn workspace_pack_file(&self) -> Option<&Path> {
        self.workspace_pack_file.as_deref()
    }

    /// Parse a string reference into a [`PromptRef`]. Unqualified
    /// references (no leading `$prefix/`) are resolved against
    /// `current`: the prefix is inherited, and the path is joined to
    /// the current ref's directory.
    pub fn parse_ref(
        &self,
        raw: &str,
        current: Option<&PromptRef>,
    ) -> Result<PromptRef, LoaderError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(LoaderError::InvalidRef {
                raw: raw.to_string(),
                reason: "reference must not be empty".into(),
            });
        }
        if let Some(prefix) = trimmed.strip_prefix('$') {
            let (prefix_name, rest) =
                prefix
                    .split_once('/')
                    .ok_or_else(|| LoaderError::InvalidRef {
                        raw: raw.to_string(),
                        reason: "prefix must be followed by '/'".into(),
                    })?;
            let prefix = parse_prefix(raw, prefix_name)?;
            let path = normalize_path(raw, rest)?;
            Ok(PromptRef { prefix, path })
        } else {
            let Some(current) = current else {
                return Err(LoaderError::UnqualifiedWithoutCurrent {
                    raw: raw.to_string(),
                });
            };
            let dir = current.dir();
            let joined = if dir.is_empty() {
                trimmed.to_string()
            } else {
                format!("{dir}/{trimmed}")
            };
            let path = normalize_path(raw, &joined)?;
            Ok(PromptRef {
                prefix: current.prefix,
                path,
            })
        }
    }

    /// Resolve a [`PromptRef`] to its raw template source. Hard-errors
    /// when the prefix is not configured or the file does not exist.
    pub fn load(&self, reference: &PromptRef) -> Result<String, LoaderError> {
        match reference.prefix {
            Prefix::Insomnia => load_from_include_dir(&BUILTIN_PROMPTS, reference),
            Prefix::User => match self.user_dir.as_deref() {
                Some(dir) => load_from_dir(dir, reference),
                None => Err(LoaderError::PrefixNotConfigured {
                    prefix: PREFIX_USER,
                }),
            },
            Prefix::Workspace => match self.workspace_dir.as_deref() {
                Some(dir) => load_from_dir(dir, reference),
                None => Err(LoaderError::PrefixNotConfigured {
                    prefix: PREFIX_WORKSPACE,
                }),
            },
        }
    }

    /// Parse `raw` against `current`, then load the resulting ref.
    /// Convenience wrapper for the minijinja loader hook.
    pub fn resolve(
        &self,
        raw: &str,
        current: Option<&PromptRef>,
    ) -> Result<(PromptRef, String), LoaderError> {
        let reference = self.parse_ref(raw, current)?;
        let source = self.load(&reference)?;
        Ok((reference, source))
    }
}

fn parse_prefix(raw: &str, prefix_name: &str) -> Result<Prefix, LoaderError> {
    match prefix_name {
        "insomnia" => Ok(Prefix::Insomnia),
        "user" => Ok(Prefix::User),
        "workspace" => Ok(Prefix::Workspace),
        _ => Err(LoaderError::UnknownPrefix {
            raw: raw.to_string(),
            prefix: format!("${prefix_name}"),
        }),
    }
}

fn normalize_path(raw: &str, rest: &str) -> Result<String, LoaderError> {
    let cleaned = rest.trim_matches('/').trim();
    if cleaned.is_empty() {
        return Err(LoaderError::InvalidRef {
            raw: raw.to_string(),
            reason: "path component must not be empty".into(),
        });
    }
    if cleaned.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(LoaderError::InvalidRef {
            raw: raw.to_string(),
            reason: "path must not contain '.' or '..' segments".into(),
        });
    }
    Ok(cleaned.to_string())
}

fn load_from_dir(dir: &Path, reference: &PromptRef) -> Result<String, LoaderError> {
    let path = dir.join(format!("{}.md", reference.path));
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(LoaderError::NotFound {
            reference: reference.clone(),
        }),
        Err(source) => Err(LoaderError::Io {
            reference: reference.clone(),
            source,
        }),
    }
}

fn load_from_include_dir(dir: &Dir<'static>, reference: &PromptRef) -> Result<String, LoaderError> {
    let path = format!("{}.md", reference.path);
    dir.get_file(&path)
        .and_then(|f| f.contents_utf8())
        .map(|s| s.to_string())
        .ok_or_else(|| LoaderError::NotFound {
            reference: reference.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn builtin_default_resolves() {
        let loader = PromptLoader::builtins_only();
        let (r, source) = loader.resolve("$insomnia/default", None).unwrap();
        assert_eq!(r.to_qualified_string(), "$insomnia/default");
        assert!(!source.is_empty());
    }

    #[test]
    fn builtin_subdirectory_lookup() {
        let loader = PromptLoader::builtins_only();
        let (_, source) = loader.resolve("$insomnia/common/tool-usage", None).unwrap();
        assert!(source.contains("tool"));
    }

    #[test]
    fn user_prefix_resolves() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().to_path_buf();
        std::fs::write(user_dir.join("my.md"), "user-body").unwrap();
        let loader = PromptLoader::new(Some(user_dir), None);
        let (_, source) = loader.resolve("$user/my", None).unwrap();
        assert_eq!(source, "user-body");
    }

    #[test]
    fn workspace_prefix_resolves() {
        let tmp = TempDir::new().unwrap();
        let ws_dir = tmp.path().to_path_buf();
        std::fs::write(ws_dir.join("custom.md"), "ws-body").unwrap();
        let loader = PromptLoader::new(None, Some(ws_dir));
        let (_, source) = loader.resolve("$workspace/custom", None).unwrap();
        assert_eq!(source, "ws-body");
    }

    #[test]
    fn missing_file_is_hard_error() {
        let loader = PromptLoader::builtins_only();
        let err = loader
            .resolve("$insomnia/definitely-missing", None)
            .unwrap_err();
        assert!(matches!(err, LoaderError::NotFound { .. }));
    }

    #[test]
    fn user_prefix_not_configured_errors() {
        let loader = PromptLoader::builtins_only();
        let err = loader.resolve("$user/my", None).unwrap_err();
        assert!(matches!(
            err,
            LoaderError::PrefixNotConfigured { prefix: "$user" }
        ));
    }

    #[test]
    fn unknown_prefix_errors() {
        let loader = PromptLoader::builtins_only();
        let err = loader.resolve("$bogus/x", None).unwrap_err();
        assert!(matches!(err, LoaderError::UnknownPrefix { .. }));
    }

    #[test]
    fn unqualified_ref_without_current_errors() {
        let loader = PromptLoader::builtins_only();
        let err = loader.resolve("default", None).unwrap_err();
        assert!(matches!(err, LoaderError::UnqualifiedWithoutCurrent { .. }));
    }

    #[test]
    fn unqualified_ref_resolves_relative_to_current() {
        let loader = PromptLoader::builtins_only();
        let current = loader
            .parse_ref("$insomnia/common/tool-usage", None)
            .unwrap();
        // Sibling lookup under the same prefix and directory.
        let sibling = loader.parse_ref("workspace", Some(&current)).unwrap();
        assert_eq!(sibling.to_qualified_string(), "$insomnia/common/workspace");
    }

    #[test]
    fn unqualified_ref_from_root_file_has_empty_dir() {
        let loader = PromptLoader::builtins_only();
        let current = loader.parse_ref("$insomnia/default", None).unwrap();
        let sibling = loader.parse_ref("other", Some(&current)).unwrap();
        assert_eq!(sibling.to_qualified_string(), "$insomnia/other");
    }

    #[test]
    fn explicit_prefix_overrides_current() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().to_path_buf();
        std::fs::write(user_dir.join("custom.md"), "user-body").unwrap();
        let loader = PromptLoader::new(Some(user_dir), None);

        let current = loader.parse_ref("$insomnia/default", None).unwrap();
        // Even with an $insomnia-rooted current, an explicit $user
        // prefix must win.
        let (reference, source) = loader.resolve("$user/custom", Some(&current)).unwrap();
        assert_eq!(reference.to_qualified_string(), "$user/custom");
        assert_eq!(source, "user-body");
    }

    #[test]
    fn traversal_segments_rejected() {
        let loader = PromptLoader::builtins_only();
        let err = loader.resolve("$insomnia/../etc/passwd", None).unwrap_err();
        assert!(matches!(err, LoaderError::InvalidRef { .. }));
    }
}
