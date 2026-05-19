//! `_staging/.consolidation.lock` による consolidation 占有ファイル。
//!
//! `docs/plan/memory.md` §並走防止 に従い:
//!
//! - ファイルが存在し、記録された Pod が動作している間、その Pod が排他占有
//! - クラッシュで残った stale lock は、所有者 PID が死んでいれば次回 spawn
//!   時に上書き取得できる
//! - cleanup は consumed ID の staging エントリのみ削除し、実行中に extract
//!   が追加した分は残す
//!
//! 占有判定は Linux/macOS の `kill(pid, 0)` 経由で行う（`ESRCH` で死亡判定）。
//! Windows は対象外: INSOMNIA は POSIX 環境を前提にしている。

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::workspace::WorkspaceLayout;

const LOCK_FILE: &str = ".consolidation.lock";

/// 占有ファイルの中身。`pid` で stale 判定し、`pod_name` / `started_at` /
/// `consumed_ids` は診断とクラッシュ復旧時の参照に使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockRecord {
    pub pid: u32,
    pub pod_name: String,
    pub started_at: DateTime<Utc>,
    /// この consolidation run が起動時スナップショットで確定した consumed staging
    /// entry の UUIDv7 列。完了時はこの列のみ削除し、追加分は残す。
    pub consumed_ids: Vec<Uuid>,
}

/// 占有取得 / 解放のエラー。
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// 占有ファイルが既にあり、所有者 PID が生きているのでスキップ。
    #[error("consolidation lock held by live pid {pid} (pod {pod_name:?})")]
    InUse { pid: u32, pod_name: String },
    #[error("io error at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to (de)serialize lock record: {0}")]
    Serde(#[from] serde_json::Error),
}

impl LockError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// consolidation が走っている間 RAII で持つ占有ハンドル。`Drop` では何もしない —
/// 完了時の cleanup は consumed ID 列削除と一緒に行う必要があるため、明示
/// 解放 [`StagingLock::release_with_cleanup`] を使う。明示解放しないまま
/// drop された場合は占有ファイルがそのまま残り、次回 spawn 時に PID が
/// 死んでいれば stale 上書きされる。
#[derive(Debug)]
pub struct StagingLock {
    path: PathBuf,
    record: LockRecord,
}

impl StagingLock {
    pub fn record(&self) -> &LockRecord {
        &self.record
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 占有取得を試みる。既に live な lock があれば
    /// [`LockError::InUse`]、stale 判定なら上書き取得する。
    /// staging dir が無ければ作成する。
    pub fn acquire(
        layout: &WorkspaceLayout,
        pid: u32,
        pod_name: impl Into<String>,
        consumed_ids: Vec<Uuid>,
    ) -> Result<Self, LockError> {
        let staging_dir = layout.staging_dir();
        fs::create_dir_all(&staging_dir).map_err(|e| LockError::io(&staging_dir, e))?;
        let path = staging_dir.join(LOCK_FILE);

        if path.exists() {
            let raw = fs::read_to_string(&path).map_err(|e| LockError::io(&path, e))?;
            // 壊れた lock は stale とみなして上書き許可。
            if let Ok(existing) = serde_json::from_str::<LockRecord>(&raw) {
                if pid_is_alive(existing.pid) {
                    return Err(LockError::InUse {
                        pid: existing.pid,
                        pod_name: existing.pod_name,
                    });
                }
                tracing::warn!(
                    stale_pid = existing.pid,
                    stale_pod = %existing.pod_name,
                    "consolidation stale lock detected, taking over"
                );
            } else {
                tracing::warn!(path = %path.display(), "consolidation lock unparseable, treating as stale");
            }
        }

        let record = LockRecord {
            pid,
            pod_name: pod_name.into(),
            started_at: Utc::now(),
            consumed_ids,
        };
        let json = serde_json::to_string_pretty(&record)?;
        fs::write(&path, json).map_err(|e| LockError::io(&path, e))?;
        Ok(Self { path, record })
    }

    /// 占有を解放しつつ consumed ID 列の staging エントリを削除する。
    /// 削除対象が見当たらない場合は黙ってスキップ（既に外部で消えていた等）。
    /// 占有ファイル自体の削除も best-effort: 失敗時は warn を出すだけで
    /// エラーは伝播しない（次回 spawn 時に stale 判定で上書きされる）。
    pub fn release_with_cleanup(self, layout: &WorkspaceLayout) {
        let staging_dir = layout.staging_dir();
        for id in &self.record.consumed_ids {
            let target = staging_dir.join(format!("{id}.json"));
            match fs::remove_file(&target) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        path = %target.display(),
                        error = %e,
                        "failed to clean up consumed staging entry"
                    );
                }
            }
        }
        self.unlink_lock_only();
    }

    /// 占有ファイルだけ削除し、staging エントリには触らない。consolidation
    /// sub-Worker が途中で失敗した場合に使う: 入力 staging を残したまま
    /// 次回再評価で再処理させる（`docs/plan/memory.md` §並走防止 の
    /// 「重複作成は同一 slug update に自然収束」運用）。
    pub fn release_only(self) {
        self.unlink_lock_only();
    }

    fn unlink_lock_only(&self) {
        if let Err(e) = fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "failed to remove consolidation lock"
                );
            }
        }
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // `kill(0, 0)` and `kill(-1, 0)` are POSIX-special (process group / all
    // signalable processes) and would yield false positives. Reject pids
    // that don't fit a positive `pid_t` so a corrupted lock file with a
    // u32::MAX-ish value is treated as stale instead of magically alive.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: `kill` with sig 0 only probes whether the target pid exists
    // and the caller has permission to signal it. No signal is delivered.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    // EPERM means the process exists but we can't signal it — still alive
    // for our purposes. ESRCH means it's gone.
    let errno = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL);
    errno != libc::ESRCH
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    // Unsupported platforms: assume the lock is live so we never overwrite
    // someone else's claim. consolidation will skip and try again next post-run.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractedPayload, write_staging};
    use crate::schema::SourceRef;

    fn make_layout() -> (tempfile::TempDir, WorkspaceLayout) {
        let dir = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        std::fs::create_dir_all(layout.staging_dir()).unwrap();
        (dir, layout)
    }

    #[test]
    fn acquire_writes_lock_file() {
        let (_dir, layout) = make_layout();
        let lock = StagingLock::acquire(&layout, std::process::id(), "pod", Vec::new()).unwrap();
        let path = layout.staging_dir().join(LOCK_FILE);
        assert!(path.exists());
        assert_eq!(lock.record().pid, std::process::id());
        assert_eq!(lock.record().pod_name, "pod");
    }

    #[test]
    fn acquire_rejects_when_live_pid_holds_lock() {
        let (_dir, layout) = make_layout();
        // Use this test process's pid — it's definitely alive.
        let _first =
            StagingLock::acquire(&layout, std::process::id(), "pod-a", Vec::new()).unwrap();
        let err = StagingLock::acquire(&layout, std::process::id(), "pod-b", Vec::new())
            .expect_err("expected InUse");
        assert!(matches!(err, LockError::InUse { .. }));
    }

    #[test]
    fn acquire_overwrites_stale_lock() {
        let (_dir, layout) = make_layout();
        // pid 1 is init on linux but for arbitrarily-large pids we'd need
        // `kill(pid, 0)` to return ESRCH. Use u32::MAX which is guaranteed
        // dead on every platform we target.
        let stale = LockRecord {
            pid: u32::MAX,
            pod_name: "ghost".into(),
            started_at: Utc::now(),
            consumed_ids: Vec::new(),
        };
        std::fs::write(
            layout.staging_dir().join(LOCK_FILE),
            serde_json::to_string_pretty(&stale).unwrap(),
        )
        .unwrap();

        let lock = StagingLock::acquire(&layout, std::process::id(), "pod", Vec::new())
            .expect("stale lock must be overwritable");
        assert_eq!(lock.record().pid, std::process::id());
    }

    #[test]
    fn release_drops_consumed_entries_and_unlinks_lock() {
        let (_dir, layout) = make_layout();
        let (id_a, _) = write_staging(
            &layout,
            SourceRef {
                segment_id: "s".into(),
                range: [0, 0],
            },
            ExtractedPayload::default(),
        )
        .unwrap();
        let (id_b, _) = write_staging(
            &layout,
            SourceRef {
                segment_id: "s".into(),
                range: [1, 1],
            },
            ExtractedPayload::default(),
        )
        .unwrap();

        let lock = StagingLock::acquire(&layout, std::process::id(), "pod", vec![id_a]).unwrap();
        let lock_path = lock.path().to_path_buf();
        lock.release_with_cleanup(&layout);

        assert!(!lock_path.exists(), "lock file must be removed");
        assert!(
            !layout.staging_dir().join(format!("{id_a}.json")).exists(),
            "consumed entry must be deleted"
        );
        assert!(
            layout.staging_dir().join(format!("{id_b}.json")).exists(),
            "non-consumed entry must remain"
        );
    }

    #[test]
    fn release_is_resilient_to_missing_consumed_entries() {
        let (_dir, layout) = make_layout();
        let phantom = uuid::Uuid::now_v7();
        let lock = StagingLock::acquire(&layout, std::process::id(), "pod", vec![phantom]).unwrap();
        let lock_path = lock.path().to_path_buf();
        // No file at <staging>/<phantom>.json — release must not panic.
        lock.release_with_cleanup(&layout);
        assert!(!lock_path.exists());
    }
}
