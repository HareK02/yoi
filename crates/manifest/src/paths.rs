//! Centralized path resolution under Yoi's home directory.
//!
//! Separates base directories by purpose:
//!
//! - **`config_dir`** — Handwritten/edited configuration: `profiles.toml`,
//!   `providers.toml`, `models.toml`, etc.
//! - **`data_dir`** — Program-written persistent data, such as `sessions/`.
//! - **`secret_data_dir`** — Base for local secret-store reads/writes. The existing
//!   store uses path-derived keys, so it defaults to legacy `$HOME/.yoi`, separate
//!   from regular data.
//! - **`runtime_dir`** — Runtime state disposable on restart: sockets,
//!   `workers.json`, `pid` files, etc.
//!
//! ## Resolution order (highest to lowest priority)
//!
//! | base | 1. `YOI_<KIND>_DIR` | 2. `YOI_HOME` | 3. `XDG_*` | 4. default |
//! |---|---|---|---|---|
//! | config  | `YOI_CONFIG_DIR`  | `$YOI_HOME/config` | `$XDG_CONFIG_HOME/yoi` | `$HOME/.config/yoi` |
//! | data    | `YOI_DATA_DIR`    | `$YOI_HOME`        | `$XDG_DATA_HOME/yoi`   | `$HOME/.local/share/yoi` |
//! | secrets | `YOI_DATA_DIR`    | `$YOI_HOME`        | —                      | `$HOME/.yoi` |
//! | runtime | `YOI_RUNTIME_DIR` | `$YOI_HOME/run`    | `$XDG_RUNTIME_DIR/yoi` | `$HOME/.yoi/run` |
//!
//! With `YOI_HOME=$X`, config uses `$X/config`, data uses `$X` directly, and
//! runtime uses `$X/run`. Tests and sandboxes can redirect all of them to a
//! temporary directory with this single variable.
//!
//! Resolved bases are not guaranteed to exist or be directories; callers must
//! create/check them before file operations.

use std::path::PathBuf;

/// Configuration directory for `profiles.toml`, `providers.toml`, `models.toml`,
/// `prompts/`, etc.
pub fn config_dir() -> Option<PathBuf> {
    resolve_config_dir_from_parts(
        env_path("YOI_CONFIG_DIR"),
        env_path("YOI_HOME"),
        env_path("XDG_CONFIG_HOME"),
        env_path("HOME"),
    )
}

/// Data directory for program-written persistent data, such as `sessions/`.
pub fn data_dir() -> Option<PathBuf> {
    resolve_data_dir_from_parts(
        env_path("YOI_DATA_DIR"),
        env_path("YOI_HOME"),
        env_path("XDG_DATA_HOME"),
        env_path("HOME"),
    )
}

/// Data directory for the secret store.
///
/// SecretStore derives decryption keys from the base path, so it does not follow
/// the regular data directory's XDG fallback. Without an explicit override, it
/// reads/writes legacy `$HOME/.yoi`.
pub fn secret_data_dir() -> Option<PathBuf> {
    resolve_secret_data_dir_from_parts(
        env_path("YOI_DATA_DIR"),
        env_path("YOI_HOME"),
        env_path("HOME"),
    )
}

/// Runtime directory for sockets, `workers.json`, per-Worker `pid` /
/// `status.json`, etc. Its contents may be discarded on restart.
pub fn runtime_dir() -> Option<PathBuf> {
    resolve_runtime_dir_from_parts(
        env_path("YOI_RUNTIME_DIR"),
        env_path("YOI_HOME"),
        env_path("XDG_RUNTIME_DIR"),
        env_path("HOME"),
    )
}

// ---- well-known file getters ------------------------------------------------

/// `<config_dir>/profiles.toml` — user profile registry/default configuration.
///
/// This is application/profile selection configuration, not a Worker manifest
/// layer.
pub fn user_profiles_path() -> Option<PathBuf> {
    user_profiles_path_from_config_dir(config_dir())
}

/// `<config_dir>/<file_name>` — user override files such as providers.toml /
/// models.toml.
pub fn user_catalog_override(file_name: &str) -> Option<PathBuf> {
    user_catalog_override_from_config_dir(config_dir(), file_name)
}

/// `<data_dir>/sessions/` — default session store location.
pub fn sessions_dir() -> Option<PathBuf> {
    sessions_dir_from_data_dir(data_dir())
}

/// `<runtime_dir>/workers.json` — machine-wide Worker allocation table.
pub fn worker_allocation_path() -> Option<PathBuf> {
    worker_allocation_path_from_runtime_dir(runtime_dir())
}

// ---- internals --------------------------------------------------------------

fn resolve_config_dir_from_parts(
    yoi_config_dir: Option<PathBuf>,
    yoi_home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = yoi_config_dir {
        return Some(p);
    }
    if let Some(p) = yoi_home {
        return Some(p.join("config"));
    }
    if let Some(p) = xdg_config_home {
        return Some(p.join("yoi"));
    }
    Some(home?.join(".config").join("yoi"))
}

fn resolve_data_dir_from_parts(
    yoi_data_dir: Option<PathBuf>,
    yoi_home: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = yoi_data_dir {
        return Some(p);
    }
    if let Some(p) = yoi_home {
        return Some(p);
    }
    if let Some(p) = xdg_data_home {
        return Some(p.join("yoi"));
    }
    Some(home?.join(".local").join("share").join("yoi"))
}

fn resolve_secret_data_dir_from_parts(
    yoi_data_dir: Option<PathBuf>,
    yoi_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = yoi_data_dir {
        return Some(p);
    }
    if let Some(p) = yoi_home {
        return Some(p);
    }
    Some(home?.join(".yoi"))
}

fn resolve_runtime_dir_from_parts(
    yoi_runtime_dir: Option<PathBuf>,
    yoi_home: Option<PathBuf>,
    xdg_runtime_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = yoi_runtime_dir {
        return Some(p);
    }
    if let Some(p) = yoi_home {
        return Some(p.join("run"));
    }
    if let Some(p) = xdg_runtime_dir {
        return Some(p.join("yoi"));
    }
    Some(home?.join(".yoi").join("run"))
}

fn user_profiles_path_from_config_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join("profiles.toml"))
}

fn user_catalog_override_from_config_dir(
    config_dir: Option<PathBuf>,
    file_name: &str,
) -> Option<PathBuf> {
    Some(config_dir?.join(file_name))
}

fn sessions_dir_from_data_dir(data_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(data_dir?.join("sessions"))
}

fn worker_allocation_path_from_runtime_dir(runtime_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(runtime_dir?.join("workers.json"))
}

/// Treats empty environment values as unset. `std::env::var` distinguishes
/// `Ok("")` from `Err(NotPresent)`, but path resolution treats both as unset
/// for intuitive behavior.
fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var(name).ok()?;
    path_from_env_value(Some(value.as_str()))
}

fn path_from_env_value(value: Option<&str>) -> Option<PathBuf> {
    value.filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        assert_eq!(
            resolve_config_dir_from_parts(None, None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.config/yoi")
        );
    }

    #[test]
    fn config_dir_uses_xdg_when_set() {
        assert_eq!(
            resolve_config_dir_from_parts(
                None,
                None,
                Some(PathBuf::from("/x")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/x/yoi")
        );
    }

    #[test]
    fn config_dir_yoi_home_outranks_xdg() {
        assert_eq!(
            resolve_config_dir_from_parts(
                None,
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/x")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/sand/config")
        );
    }

    #[test]
    fn config_dir_explicit_wins_over_yoi_home() {
        assert_eq!(
            resolve_config_dir_from_parts(
                Some(PathBuf::from("/explicit-cfg")),
                Some(PathBuf::from("/sand")),
                None,
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/explicit-cfg")
        );
    }

    #[test]
    fn data_dir_falls_back_to_home_local_share() {
        assert_eq!(
            resolve_data_dir_from_parts(None, None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.local/share/yoi")
        );
    }

    #[test]
    fn data_dir_uses_xdg_when_set() {
        assert_eq!(
            resolve_data_dir_from_parts(
                None,
                None,
                Some(PathBuf::from("/xdg-data")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/xdg-data/yoi")
        );
    }

    #[test]
    fn data_dir_yoi_home_outranks_xdg() {
        assert_eq!(
            resolve_data_dir_from_parts(
                None,
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/xdg-data")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/sand")
        );
    }

    #[test]
    fn data_dir_explicit_wins_over_yoi_home() {
        assert_eq!(
            resolve_data_dir_from_parts(
                Some(PathBuf::from("/explicit-data")),
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/xdg-data")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/explicit-data")
        );
    }

    #[test]
    fn secret_data_dir_falls_back_to_home_dot_yoi() {
        assert_eq!(
            resolve_secret_data_dir_from_parts(None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.yoi")
        );
    }

    #[test]
    fn secret_data_dir_yoi_home_wins() {
        assert_eq!(
            resolve_secret_data_dir_from_parts(
                None,
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/h"))
            )
            .unwrap(),
            PathBuf::from("/sand")
        );
    }

    #[test]
    fn secret_data_dir_explicit_wins_over_yoi_home() {
        assert_eq!(
            resolve_secret_data_dir_from_parts(
                Some(PathBuf::from("/explicit-data")),
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/explicit-data")
        );
    }

    #[test]
    fn runtime_dir_prefers_xdg_runtime_dir() {
        assert_eq!(
            resolve_runtime_dir_from_parts(
                None,
                None,
                Some(PathBuf::from("/xdg-runtime")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/xdg-runtime/yoi")
        );
    }

    #[test]
    fn runtime_dir_falls_back_to_dot_yoi_run() {
        assert_eq!(
            resolve_runtime_dir_from_parts(None, None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.yoi/run")
        );
    }

    #[test]
    fn runtime_dir_yoi_home_is_run_subdir() {
        assert_eq!(
            resolve_runtime_dir_from_parts(
                None,
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/run/user/1000")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/sand/run")
        );
    }

    #[test]
    fn runtime_dir_explicit_wins_over_yoi_home() {
        assert_eq!(
            resolve_runtime_dir_from_parts(
                Some(PathBuf::from("/explicit-run")),
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/run/user/1000")),
                Some(PathBuf::from("/h")),
            )
            .unwrap(),
            PathBuf::from("/explicit-run")
        );
    }

    #[test]
    fn empty_env_value_treated_as_unset_before_path_resolution() {
        let xdg_config_home = path_from_env_value(Some(""));

        assert_eq!(
            resolve_config_dir_from_parts(None, None, xdg_config_home, Some(PathBuf::from("/h")))
                .unwrap(),
            PathBuf::from("/h/.config/yoi")
        );
    }

    #[test]
    fn returns_none_when_nothing_set() {
        assert!(resolve_config_dir_from_parts(None, None, None, None).is_none());
        assert!(resolve_data_dir_from_parts(None, None, None, None).is_none());
        assert!(resolve_runtime_dir_from_parts(None, None, None, None).is_none());
    }

    #[test]
    fn well_known_files_compose_off_base_dirs() {
        let config_dir = Some(PathBuf::from("/sand/config"));
        let data_dir = Some(PathBuf::from("/sand"));
        let runtime_dir = Some(PathBuf::from("/sand/run"));

        assert_eq!(
            user_profiles_path_from_config_dir(config_dir.clone()).unwrap(),
            PathBuf::from("/sand/config/profiles.toml")
        );
        assert_eq!(
            user_catalog_override_from_config_dir(config_dir, "providers.toml").unwrap(),
            PathBuf::from("/sand/config/providers.toml")
        );
        assert_eq!(
            sessions_dir_from_data_dir(data_dir).unwrap(),
            PathBuf::from("/sand/sessions")
        );
        assert_eq!(
            worker_allocation_path_from_runtime_dir(runtime_dir).unwrap(),
            PathBuf::from("/sand/run/workers.json")
        );
    }
}
