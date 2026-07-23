//! Yoi のホームディレクトリ配下のパス解決を一元化するモジュール。
//!
//! 用途別に三つの base directory を持つ:
//!
//! - **`config_dir`** — 人が手で書く / 編集する設定。`profiles.toml`,
//!   `providers.toml`, `models.toml`, `prompts/`, `prompts.toml` 等
//! - **`data_dir`** — プログラムが書く永続データ。`sessions/` 等
//! - **`runtime_dir`** — 再起動で消えてよいランタイム状態。socket,
//!   `workers.json`, `pid` ファイル等
//!
//! ## 解決順 (優先順位高 → 低)
//!
//! | base | 1. `YOI_<KIND>_DIR` | 2. `YOI_HOME` | 3. `XDG_*` | 4. 既定 |
//! |---|---|---|---|---|
//! | config  | `YOI_CONFIG_DIR`  | `$YOI_HOME/config` | `$XDG_CONFIG_HOME/yoi` | `$HOME/.config/yoi` |
//! | data    | `YOI_DATA_DIR`    | `$YOI_HOME`        | `$XDG_DATA_HOME/yoi`   | `$HOME/.local/share/yoi` |
//! | runtime | `YOI_RUNTIME_DIR` | `$YOI_HOME/run`    | `$XDG_RUNTIME_DIR/yoi` | `$HOME/.yoi/run` |
//!
//! `YOI_HOME=$X` のとき config は `$X/config`、data は `$X` 直下、
//! runtime は `$X/run` に集約される。テストや sandbox 利用ではこれ一本
//! で全部 tempdir に向けられる。
//!
//! 解決された各 base が存在するか / ディレクトリかは保証しない —
//! 呼び出し側がファイル操作の前に作成 / 検査する。

use std::path::PathBuf;

/// 設定ディレクトリ。`profiles.toml`, `providers.toml`, `models.toml`,
/// `prompts/` などが置かれる。
pub fn config_dir() -> Option<PathBuf> {
    resolve_config_dir_from_parts(
        env_path("YOI_CONFIG_DIR"),
        env_path("YOI_HOME"),
        env_path("XDG_CONFIG_HOME"),
        env_path("HOME"),
    )
}

/// データディレクトリ。`sessions/` などプログラムが書く永続データの
/// 置き場。
pub fn data_dir() -> Option<PathBuf> {
    resolve_data_dir_from_parts(
        env_path("YOI_DATA_DIR"),
        env_path("YOI_HOME"),
        env_path("XDG_DATA_HOME"),
        env_path("HOME"),
    )
}

/// ランタイムディレクトリ。socket, `workers.json`, Worker ごとの `pid` /
/// `status.json` 等が置かれる。再起動で消えて構わない。
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

/// `<config_dir>/prompts/` — user prompts ライブラリ。
pub fn user_prompts_dir() -> Option<PathBuf> {
    user_prompts_dir_from_config_dir(config_dir())
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

/// `<runtime_dir>/workers.json` — machine-wide Worker allocation table。
pub fn worker_allocation_path() -> Option<PathBuf> {
    worker_allocation_path_from_runtime_dir(runtime_dir())
}

/// `<runtime_dir>/<worker_name>/` — Worker ごとのランタイムディレクトリ。
pub fn worker_runtime_dir(worker_name: &str) -> Option<PathBuf> {
    worker_runtime_dir_from_runtime_dir(runtime_dir(), worker_name)
}

/// `<runtime_dir>/<worker_name>/sock` — Worker の Unix socket パス。
///
/// Worker プロセス内で実際に socket を作成するのは `worker` crate の
/// `RuntimeDir::socket_path()` で、Worker 名が分かっている外部 (TUI の
/// attach フロー等) からの**予測**はこの関数で行う。両者は同じパス
/// を返すことが期待される。
pub fn worker_socket_path(worker_name: &str) -> Option<PathBuf> {
    worker_socket_path_from_runtime_dir(runtime_dir(), worker_name)
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

fn worker_allocation_path_from_runtime_dir(runtime_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(runtime_dir?.join("workers.json"))
}

fn worker_runtime_dir_from_runtime_dir(
    runtime_dir: Option<PathBuf>,
    worker_name: &str,
) -> Option<PathBuf> {
    Some(runtime_dir?.join(worker_name))
}

fn worker_socket_path_from_runtime_dir(
    runtime_dir: Option<PathBuf>,
    worker_name: &str,
) -> Option<PathBuf> {
    Some(worker_runtime_dir_from_runtime_dir(runtime_dir, worker_name)?.join("sock"))
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
            worker_allocation_path_from_runtime_dir(runtime_dir.clone()).unwrap(),
            PathBuf::from("/sand/run/workers.json")
        );
        assert_eq!(
            worker_runtime_dir_from_runtime_dir(runtime_dir.clone(), "foo").unwrap(),
            PathBuf::from("/sand/run/foo")
        );
        assert_eq!(
            worker_socket_path_from_runtime_dir(runtime_dir, "foo").unwrap(),
            PathBuf::from("/sand/run/foo/sock")
        );
    }
}
