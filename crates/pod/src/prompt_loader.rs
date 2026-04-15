//! Three-layer prompt asset loader used by [`crate::SystemPromptTemplate`].
//!
//! Layers (highest priority first):
//! 1. **Project prompts** — `<project>/.insomnia/prompts/`
//! 2. **User prompts** — `$XDG_CONFIG_HOME/insomnia/prompts/`
//! 3. **Builtin prompts** — baked into the binary from `resources/prompts/`
//!    via [`include_dir!`].
//!
//! A prompt name is its path stem without the `.md` extension.
//! Subdirectories are supported: `common/tool-usage` maps to
//! `common/tool-usage.md` under whichever layer provides it first.

use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

static BUILTIN_PROMPTS: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../resources/prompts");

/// Lookup table for prompt assets across the three cascade layers.
#[derive(Debug, Clone)]
pub struct PromptLoader {
    user_dir: Option<PathBuf>,
    project_dir: Option<PathBuf>,
}

impl PromptLoader {
    /// Builtins-only loader. Used for direct `Pod::from_manifest`
    /// calls that skip the factory cascade (tests, examples, simple
    /// callers).
    pub fn builtins_only() -> Self {
        Self {
            user_dir: None,
            project_dir: None,
        }
    }

    /// Loader with optional user and project prompts directories. Both
    /// are consulted before falling back to builtins; `None` on either
    /// skips that layer.
    pub fn new(user_dir: Option<PathBuf>, project_dir: Option<PathBuf>) -> Self {
        Self {
            user_dir,
            project_dir,
        }
    }

    /// Look up the raw template source for `name`. Returns `None` if
    /// no layer provides it.
    pub fn lookup(&self, name: &str) -> Option<String> {
        if let Some(ref dir) = self.project_dir {
            if let Some(s) = read_from_dir(dir, name) {
                return Some(s);
            }
        }
        if let Some(ref dir) = self.user_dir {
            if let Some(s) = read_from_dir(dir, name) {
                return Some(s);
            }
        }
        read_from_include_dir(&BUILTIN_PROMPTS, name)
    }
}

fn read_from_dir(dir: &Path, name: &str) -> Option<String> {
    let path = dir.join(format!("{name}.md"));
    std::fs::read_to_string(path).ok()
}

fn read_from_include_dir(dir: &Dir<'static>, name: &str) -> Option<String> {
    let path = format!("{name}.md");
    dir.get_file(&path)
        .and_then(|f| f.contents_utf8())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn builtin_coder_prompt_present() {
        let loader = PromptLoader::builtins_only();
        let coder = loader.lookup("coder").expect("coder builtin missing");
        assert!(coder.contains("software engineering agent"));
    }

    #[test]
    fn builtin_subdirectory_lookup() {
        let loader = PromptLoader::builtins_only();
        let tu = loader
            .lookup("common/tool-usage")
            .expect("common/tool-usage missing");
        assert!(tu.contains("tool"));
    }

    #[test]
    fn unknown_name_returns_none() {
        let loader = PromptLoader::builtins_only();
        assert!(loader.lookup("definitely-not-a-prompt").is_none());
    }

    #[test]
    fn user_layer_overrides_builtin() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().to_path_buf();
        std::fs::write(user_dir.join("coder.md"), "user-coder").unwrap();

        let loader = PromptLoader::new(Some(user_dir), None);
        assert_eq!(loader.lookup("coder").as_deref(), Some("user-coder"));
    }

    #[test]
    fn project_layer_overrides_user_and_builtin() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(user_dir.join("coder.md"), "user-coder").unwrap();
        std::fs::write(project_dir.join("coder.md"), "project-coder").unwrap();

        let loader = PromptLoader::new(Some(user_dir), Some(project_dir));
        assert_eq!(loader.lookup("coder").as_deref(), Some("project-coder"));
    }

    #[test]
    fn falls_through_to_builtin_when_user_missing_name() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().to_path_buf();
        // user layer only defines "only-user", not "coder"
        std::fs::write(user_dir.join("only-user.md"), "x").unwrap();
        let loader = PromptLoader::new(Some(user_dir), None);
        assert!(loader.lookup("coder").is_some()); // from builtin
    }
}
