//! Insomnia のホームディレクトリ配下のパス解決を一元化するモジュール。
//!
//! 用途別に三つの base directory を持つ:
//!
//! - **`config_dir`** — 人が手で書く / 編集する設定。`manifest.toml`,
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

use std::ffi::OsString;
use std::path::PathBuf;

/// Environment variable that points at an explicit user manifest.
///
/// Pod CLI treats a non-empty value as an explicit manifest path. Empty values
/// are treated the same as an unset variable, so callers fall back to the
/// auto-discovered user manifest path.
pub const USER_MANIFEST_ENV: &str = "INSOMNIA_USER_MANIFEST";

/// 設定ディレクトリ。`manifest.toml`, `providers.toml`, `models.toml`,
/// `prompts/` などが置かれる。
pub fn config_dir() -> Option<PathBuf> {
    if let Some(p) = env_path("INSOMNIA_CONFIG_DIR") {
        return Some(p);
    }
    if let Some(p) = env_path("INSOMNIA_HOME") {
        return Some(p.join("config"));
    }
    if let Some(p) = env_path("XDG_CONFIG_HOME") {
        return Some(p.join("insomnia"));
    }
    Some(env_path("HOME")?.join(".config").join("insomnia"))
}

/// データディレクトリ。`sessions/` などプログラムが書く永続データの
/// 置き場。
pub fn data_dir() -> Option<PathBuf> {
    if let Some(p) = env_path("INSOMNIA_DATA_DIR") {
        return Some(p);
    }
    if let Some(p) = env_path("INSOMNIA_HOME") {
        return Some(p);
    }
    Some(env_path("HOME")?.join(".insomnia"))
}

/// ランタイムディレクトリ。socket, `pods.json`, Pod ごとの `pid` /
/// `status.json` 等が置かれる。再起動で消えて構わない。
pub fn runtime_dir() -> Option<PathBuf> {
    if let Some(p) = env_path("INSOMNIA_RUNTIME_DIR") {
        return Some(p);
    }
    if let Some(p) = env_path("INSOMNIA_HOME") {
        return Some(p.join("run"));
    }
    if let Some(p) = env_path("XDG_RUNTIME_DIR") {
        return Some(p.join("insomnia"));
    }
    Some(env_path("HOME")?.join(".insomnia").join("run"))
}

// ---- well-known file getters ------------------------------------------------

/// `<config_dir>/manifest.toml` — user manifest の既定位置。
///
/// This deliberately ignores [`USER_MANIFEST_ENV`]. Use
/// [`user_manifest_path_with_env_override`] when mirroring the Pod CLI cascade
/// resolution rules.
pub fn user_manifest_path() -> Option<PathBuf> {
    Some(config_dir()?.join("manifest.toml"))
}

/// Resolve an explicit user manifest override from an env value.
///
/// Non-empty values are paths. `None` and empty strings are both treated as no
/// override, matching the Pod CLI's `INSOMNIA_USER_MANIFEST` handling.
pub fn user_manifest_path_from_env(value: Option<OsString>) -> Option<PathBuf> {
    value.and_then(|value| {
        if value.as_os_str().is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

/// User manifest path using the same env override rule as the Pod CLI cascade.
///
/// A non-empty [`USER_MANIFEST_ENV`] value wins. If the variable is unset or
/// empty, this falls back to [`user_manifest_path`]. The returned path is not
/// guaranteed to exist.
pub fn user_manifest_path_with_env_override() -> Option<PathBuf> {
    user_manifest_path_from_env(std::env::var_os(USER_MANIFEST_ENV)).or_else(user_manifest_path)
}

/// `<config_dir>/prompts/` — user prompts ライブラリ。
pub fn user_prompts_dir() -> Option<PathBuf> {
    Some(config_dir()?.join("prompts"))
}

/// `<config_dir>/prompts.toml` — user prompt pack。
pub fn user_pack_file() -> Option<PathBuf> {
    Some(config_dir()?.join("prompts.toml"))
}

/// `<config_dir>/<file_name>` — providers.toml / models.toml 等の
/// user override ファイル。
pub fn user_catalog_override(file_name: &str) -> Option<PathBuf> {
    Some(config_dir()?.join(file_name))
}

/// `<data_dir>/sessions/` — session store のデフォルト位置。
pub fn sessions_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("sessions"))
}

/// `<runtime_dir>/pods.json` — machine-wide Pod allocation registry。
pub fn pod_registry_path() -> Option<PathBuf> {
    Some(runtime_dir()?.join("pods.json"))
}

/// `<runtime_dir>/<pod_name>/` — Pod ごとのランタイムディレクトリ。
pub fn pod_runtime_dir(pod_name: &str) -> Option<PathBuf> {
    Some(runtime_dir()?.join(pod_name))
}

/// `<runtime_dir>/<pod_name>/sock` — Pod の Unix socket パス。
///
/// Pod プロセス内で実際に socket を作成するのは `pod` crate の
/// `RuntimeDir::socket_path()` で、Pod 名が分かっている外部 (TUI の
/// attach フロー等) からの**予測**はこの関数で行う。両者は同じパス
/// を返すことが期待される。
pub fn pod_socket_path(pod_name: &str) -> Option<PathBuf> {
    Some(pod_runtime_dir(pod_name)?.join("sock"))
}

// ---- internals --------------------------------------------------------------

/// 空文字列の env は未設定として扱う。`std::env::var` は `Ok("")` と
/// `Err(NotPresent)` を区別するが、パス解決においては両者を未設定と
/// 同等に扱うのが直感的。
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// プロセス全体で env を弄るテスト同士が並行に走らないように保護
    /// する。Cargo の test harness はファイル単位で別プロセスにせず
    /// マルチスレッドで実行するため、env を読む全テストはこの lock を
    /// 取ってから操作する。
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// テスト中だけ env を上書きし、drop 時に元の値に戻す RAII guard。
    struct EnvGuard {
        vars: Vec<(&'static str, Option<String>)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(overrides: &[(&'static str, Option<&str>)]) -> Self {
            let lock = env_lock();
            let names = [
                "INSOMNIA_CONFIG_DIR",
                "INSOMNIA_DATA_DIR",
                "INSOMNIA_RUNTIME_DIR",
                "INSOMNIA_USER_MANIFEST",
                "INSOMNIA_HOME",
                "XDG_CONFIG_HOME",
                "XDG_RUNTIME_DIR",
                "HOME",
            ];
            let saved: Vec<_> = names.iter().map(|n| (*n, std::env::var(n).ok())).collect();
            // SAFETY: env_lock() 取得済みなので env への並行アクセスは
            // この test バイナリ内では発生しない。
            unsafe {
                for (n, _) in &saved {
                    std::env::remove_var(n);
                }
                for (n, v) in overrides {
                    if let Some(v) = v {
                        std::env::set_var(n, v);
                    }
                }
            }
            Self {
                vars: saved,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: lock を握ったまま元に戻す。
            unsafe {
                for (n, v) in &self.vars {
                    match v {
                        Some(v) => std::env::set_var(n, v),
                        None => std::env::remove_var(n),
                    }
                }
            }
        }
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let _g = EnvGuard::new(&[("HOME", Some("/h"))]);
        assert_eq!(config_dir().unwrap(), PathBuf::from("/h/.config/insomnia"));
    }

    #[test]
    fn config_dir_uses_xdg_when_set() {
        let _g = EnvGuard::new(&[("HOME", Some("/h")), ("XDG_CONFIG_HOME", Some("/x"))]);
        assert_eq!(config_dir().unwrap(), PathBuf::from("/x/insomnia"));
    }

    #[test]
    fn config_dir_insomnia_home_outranks_xdg() {
        let _g = EnvGuard::new(&[
            ("HOME", Some("/h")),
            ("XDG_CONFIG_HOME", Some("/x")),
            ("INSOMNIA_HOME", Some("/sand")),
        ]);
        assert_eq!(config_dir().unwrap(), PathBuf::from("/sand/config"));
    }

    #[test]
    fn config_dir_explicit_wins_over_insomnia_home() {
        let _g = EnvGuard::new(&[
            ("HOME", Some("/h")),
            ("INSOMNIA_HOME", Some("/sand")),
            ("INSOMNIA_CONFIG_DIR", Some("/explicit-cfg")),
        ]);
        assert_eq!(config_dir().unwrap(), PathBuf::from("/explicit-cfg"));
    }

    #[test]
    fn data_dir_default_is_dot_insomnia() {
        let _g = EnvGuard::new(&[("HOME", Some("/h"))]);
        assert_eq!(data_dir().unwrap(), PathBuf::from("/h/.insomnia"));
    }

    #[test]
    fn data_dir_insomnia_home_is_data_dir_itself() {
        let _g = EnvGuard::new(&[("HOME", Some("/h")), ("INSOMNIA_HOME", Some("/sand"))]);
        assert_eq!(data_dir().unwrap(), PathBuf::from("/sand"));
    }

    #[test]
    fn runtime_dir_prefers_xdg_runtime_dir() {
        let _g = EnvGuard::new(&[
            ("HOME", Some("/h")),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        assert_eq!(runtime_dir().unwrap(), PathBuf::from("<runtime-dir>"));
    }

    #[test]
    fn runtime_dir_falls_back_to_dot_insomnia_run() {
        let _g = EnvGuard::new(&[("HOME", Some("/h"))]);
        assert_eq!(runtime_dir().unwrap(), PathBuf::from("/h/.insomnia/run"));
    }

    #[test]
    fn runtime_dir_insomnia_home_is_run_subdir() {
        let _g = EnvGuard::new(&[
            ("HOME", Some("/h")),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            ("INSOMNIA_HOME", Some("/sand")),
        ]);
        assert_eq!(runtime_dir().unwrap(), PathBuf::from("/sand/run"));
    }

    #[test]
    fn empty_env_treated_as_unset() {
        let _g = EnvGuard::new(&[("HOME", Some("/h")), ("XDG_CONFIG_HOME", Some(""))]);
        assert_eq!(config_dir().unwrap(), PathBuf::from("/h/.config/insomnia"));
    }

    #[test]
    fn returns_none_when_nothing_set() {
        let _g = EnvGuard::new(&[]);
        assert!(config_dir().is_none());
        assert!(data_dir().is_none());
        assert!(runtime_dir().is_none());
    }

    #[test]
    fn user_manifest_env_override_wins_when_non_empty() {
        let _g = EnvGuard::new(&[
            ("HOME", Some("/h")),
            ("INSOMNIA_USER_MANIFEST", Some("/tmp/user.toml")),
        ]);
        assert_eq!(
            user_manifest_path_with_env_override().unwrap(),
            PathBuf::from("/tmp/user.toml")
        );
    }

    #[test]
    fn empty_user_manifest_env_falls_back_to_default_path() {
        let _g = EnvGuard::new(&[("HOME", Some("/h")), ("INSOMNIA_USER_MANIFEST", Some(""))]);
        assert_eq!(
            user_manifest_path_with_env_override().unwrap(),
            PathBuf::from("/h/.config/insomnia/manifest.toml")
        );
    }

    #[test]
    fn user_manifest_path_from_env_treats_empty_as_unset() {
        assert_eq!(user_manifest_path_from_env(None), None);
        assert_eq!(user_manifest_path_from_env(Some(OsString::from(""))), None);
        assert_eq!(
            user_manifest_path_from_env(Some(OsString::from("/tmp/u.toml"))).unwrap(),
            PathBuf::from("/tmp/u.toml")
        );
    }

    #[test]
    fn well_known_files_compose_off_base_dirs() {
        let _g = EnvGuard::new(&[("INSOMNIA_HOME", Some("/sand"))]);
        assert_eq!(
            user_manifest_path().unwrap(),
            PathBuf::from("/sand/config/manifest.toml")
        );
        assert_eq!(
            user_prompts_dir().unwrap(),
            PathBuf::from("/sand/config/prompts")
        );
        assert_eq!(
            user_pack_file().unwrap(),
            PathBuf::from("/sand/config/prompts.toml")
        );
        assert_eq!(
            user_catalog_override("providers.toml").unwrap(),
            PathBuf::from("/sand/config/providers.toml")
        );
        assert_eq!(sessions_dir().unwrap(), PathBuf::from("/sand/sessions"));
        assert_eq!(
            pod_registry_path().unwrap(),
            PathBuf::from("/sand/run/pods.json")
        );
        assert_eq!(
            pod_runtime_dir("foo").unwrap(),
            PathBuf::from("/sand/run/foo")
        );
        assert_eq!(
            pod_socket_path("foo").unwrap(),
            PathBuf::from("/sand/run/foo/sock")
        );
    }
}
