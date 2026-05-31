//! Insomnia のホームディレクトリ配下のパス解決を一元化するモジュール。
//!
//! 用途別に三つの base directory を持つ:
//!
//! - **`config_dir`** — 人が手で書く / 編集する設定。`profiles.toml`,
//!   `providers.toml`, `models.toml`, `prompts/`, `prompts.toml` 等
//! - **`data_dir`** — プログラムが書く永続データ。`sessions/` 等
//! - **`runtime_dir`** — 再起動で消えてよいランタイム状態。socket,
//!   `pods.json`, `pid` ファイル等
//!
//! ## 解決順 (優先順位高 → 低)
//!
//! | base | 1. `INSOMNIA_<KIND>_DIR` | 2. `INSOMNIA_HOME` | 3. `XDG_*` | 4. 既定 |
//! |---|---|---|---|---|
//! | config  | `INSOMNIA_CONFIG_DIR`  | `$INSOMNIA_HOME/config` | `$XDG_CONFIG_HOME/insomnia` | `$HOME/.config/insomnia` |
//! | data    | `INSOMNIA_DATA_DIR`    | `$INSOMNIA_HOME`        | —                            | `$HOME/.insomnia` |
//! | runtime | `INSOMNIA_RUNTIME_DIR` | `$INSOMNIA_HOME/run`    | `$XDG_RUNTIME_DIR/insomnia` | `$HOME/.insomnia/run` |
//!
//! `INSOMNIA_HOME=$X` のとき config は `$X/config`、data は `$X` 直下、
//! runtime は `$X/run` に集約される。テストや sandbox 利用ではこれ一本
//! で全部 tempdir に向けられる。
//!
//! 解決された各 base が存在するか / ディレクトリかは保証しない —
//! 呼び出し側がファイル操作の前に作成 / 検査する。

use std::path::PathBuf;

/// Environment variable that points at installed project resources.
pub const RESOURCE_DIR_ENV: &str = "INSOMNIA_RESOURCE_DIR";

/// 設定ディレクトリ。`profiles.toml`, `providers.toml`, `models.toml`,
/// `prompts/` などが置かれる。
pub fn config_dir() -> Option<PathBuf> {
    resolve_config_dir_from_parts(
        env_path("INSOMNIA_CONFIG_DIR"),
        env_path("INSOMNIA_HOME"),
        env_path("XDG_CONFIG_HOME"),
        env_path("HOME"),
    )
}

/// データディレクトリ。`sessions/` などプログラムが書く永続データの
/// 置き場。
pub fn data_dir() -> Option<PathBuf> {
    resolve_data_dir_from_parts(
        env_path("INSOMNIA_DATA_DIR"),
        env_path("INSOMNIA_HOME"),
        env_path("HOME"),
    )
}

/// ランタイムディレクトリ。socket, `pods.json`, Pod ごとの `pid` /
/// `status.json` 等が置かれる。再起動で消えて構わない。
pub fn runtime_dir() -> Option<PathBuf> {
    resolve_runtime_dir_from_parts(
        env_path("INSOMNIA_RUNTIME_DIR"),
        env_path("INSOMNIA_HOME"),
        env_path("XDG_RUNTIME_DIR"),
        env_path("HOME"),
    )
}

// ---- well-known file getters ------------------------------------------------

/// `<config_dir>/profiles.toml` — user profile registry/default configuration.
///
/// This is application/profile selection configuration, not a Pod manifest
/// layer.
pub fn user_profiles_path() -> Option<PathBuf> {
    user_profiles_path_from_config_dir(config_dir())
}

/// `<config_dir>/prompts/` — user prompts ライブラリ。
pub fn user_prompts_dir() -> Option<PathBuf> {
    user_prompts_dir_from_config_dir(config_dir())
}

/// Root resource directory used for bundled prompts, profiles, catalogs, and docs.
pub fn resource_dir() -> Option<PathBuf> {
    if let Some(p) = env_path(RESOURCE_DIR_ENV) {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let installed = prefix.join("share").join("insomnia").join("resources");
            if installed.exists() {
                return Some(installed);
            }
        }
    }
    Some(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("resources"),
    )
}

/// Bundled Lua profile registry directory. Missing directories are treated as
/// an empty builtin registry by discovery.
pub fn builtin_profiles_dir() -> Option<PathBuf> {
    Some(resource_dir()?.join("profiles"))
}

/// `<config_dir>/prompts.toml` — user prompt pack。
pub fn user_pack_file() -> Option<PathBuf> {
    user_pack_file_from_config_dir(config_dir())
}

/// `<config_dir>/<file_name>` — providers.toml / models.toml 等の
/// user override ファイル。
pub fn user_catalog_override(file_name: &str) -> Option<PathBuf> {
    user_catalog_override_from_config_dir(config_dir(), file_name)
}

/// `<data_dir>/sessions/` — session store のデフォルト位置。
pub fn sessions_dir() -> Option<PathBuf> {
    sessions_dir_from_data_dir(data_dir())
}

/// `<runtime_dir>/pods.json` — machine-wide Pod allocation registry。
pub fn pod_registry_path() -> Option<PathBuf> {
    pod_registry_path_from_runtime_dir(runtime_dir())
}

/// `<runtime_dir>/<pod_name>/` — Pod ごとのランタイムディレクトリ。
pub fn pod_runtime_dir(pod_name: &str) -> Option<PathBuf> {
    pod_runtime_dir_from_runtime_dir(runtime_dir(), pod_name)
}

/// `<runtime_dir>/<pod_name>/sock` — Pod の Unix socket パス。
///
/// Pod プロセス内で実際に socket を作成するのは `pod` crate の
/// `RuntimeDir::socket_path()` で、Pod 名が分かっている外部 (TUI の
/// attach フロー等) からの**予測**はこの関数で行う。両者は同じパス
/// を返すことが期待される。
pub fn pod_socket_path(pod_name: &str) -> Option<PathBuf> {
    pod_socket_path_from_runtime_dir(runtime_dir(), pod_name)
}

// ---- internals --------------------------------------------------------------

fn resolve_config_dir_from_parts(
    insomnia_config_dir: Option<PathBuf>,
    insomnia_home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = insomnia_config_dir {
        return Some(p);
    }
    if let Some(p) = insomnia_home {
        return Some(p.join("config"));
    }
    if let Some(p) = xdg_config_home {
        return Some(p.join("insomnia"));
    }
    Some(home?.join(".config").join("insomnia"))
}

fn resolve_data_dir_from_parts(
    insomnia_data_dir: Option<PathBuf>,
    insomnia_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = insomnia_data_dir {
        return Some(p);
    }
    if let Some(p) = insomnia_home {
        return Some(p);
    }
    Some(home?.join(".insomnia"))
}

fn resolve_runtime_dir_from_parts(
    insomnia_runtime_dir: Option<PathBuf>,
    insomnia_home: Option<PathBuf>,
    xdg_runtime_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = insomnia_runtime_dir {
        return Some(p);
    }
    if let Some(p) = insomnia_home {
        return Some(p.join("run"));
    }
    if let Some(p) = xdg_runtime_dir {
        return Some(p.join("insomnia"));
    }
    Some(home?.join(".insomnia").join("run"))
}

fn user_profiles_path_from_config_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join("profiles.toml"))
}

fn user_prompts_dir_from_config_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join("prompts"))
}

fn user_pack_file_from_config_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join("prompts.toml"))
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

fn pod_registry_path_from_runtime_dir(runtime_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(runtime_dir?.join("pods.json"))
}

fn pod_runtime_dir_from_runtime_dir(
    runtime_dir: Option<PathBuf>,
    pod_name: &str,
) -> Option<PathBuf> {
    Some(runtime_dir?.join(pod_name))
}

fn pod_socket_path_from_runtime_dir(
    runtime_dir: Option<PathBuf>,
    pod_name: &str,
) -> Option<PathBuf> {
    Some(pod_runtime_dir_from_runtime_dir(runtime_dir, pod_name)?.join("sock"))
}

/// 空文字列の env は未設定として扱う。`std::env::var` は `Ok("")` と
/// `Err(NotPresent)` を区別するが、パス解決においては両者を未設定と
/// 同等に扱うのが直感的。
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
            PathBuf::from("/h/.config/insomnia")
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
            PathBuf::from("/x/insomnia")
        );
    }

    #[test]
    fn config_dir_insomnia_home_outranks_xdg() {
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
    fn config_dir_explicit_wins_over_insomnia_home() {
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
    fn data_dir_default_is_dot_insomnia() {
        assert_eq!(
            resolve_data_dir_from_parts(None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.insomnia")
        );
    }

    #[test]
    fn data_dir_insomnia_home_is_data_dir_itself() {
        assert_eq!(
            resolve_data_dir_from_parts(
                None,
                Some(PathBuf::from("/sand")),
                Some(PathBuf::from("/h"))
            )
            .unwrap(),
            PathBuf::from("/sand")
        );
    }

    #[test]
    fn data_dir_explicit_wins_over_insomnia_home() {
        assert_eq!(
            resolve_data_dir_from_parts(
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
            PathBuf::from("/xdg-runtime/insomnia")
        );
    }

    #[test]
    fn runtime_dir_falls_back_to_dot_insomnia_run() {
        assert_eq!(
            resolve_runtime_dir_from_parts(None, None, None, Some(PathBuf::from("/h"))).unwrap(),
            PathBuf::from("/h/.insomnia/run")
        );
    }

    #[test]
    fn runtime_dir_insomnia_home_is_run_subdir() {
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
    fn runtime_dir_explicit_wins_over_insomnia_home() {
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
            PathBuf::from("/h/.config/insomnia")
        );
    }

    #[test]
    fn returns_none_when_nothing_set() {
        assert!(resolve_config_dir_from_parts(None, None, None, None).is_none());
        assert!(resolve_data_dir_from_parts(None, None, None).is_none());
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
            user_prompts_dir_from_config_dir(config_dir.clone()).unwrap(),
            PathBuf::from("/sand/config/prompts")
        );
        assert_eq!(
            user_pack_file_from_config_dir(config_dir.clone()).unwrap(),
            PathBuf::from("/sand/config/prompts.toml")
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
            pod_registry_path_from_runtime_dir(runtime_dir.clone()).unwrap(),
            PathBuf::from("/sand/run/pods.json")
        );
        assert_eq!(
            pod_runtime_dir_from_runtime_dir(runtime_dir.clone(), "foo").unwrap(),
            PathBuf::from("/sand/run/foo")
        );
        assert_eq!(
            pod_socket_path_from_runtime_dir(runtime_dir, "foo").unwrap(),
            PathBuf::from("/sand/run/foo/sock")
        );
    }
}
