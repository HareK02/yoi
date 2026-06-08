# Known Issues

Ticket を切るほどではないが、次に近所を触るときに合わせて拾いたい小粒な所見の置き場。

- `crates/pod/src/controller.rs:1269-1278` — `worker_error_code` で `PodError::WorkflowResolve(_) => InvalidRequest` が post-commit な resolve エラー (`KnowledgeNotFound` 等) にも適用される。意味論的には妥当方向だが、resolve 系のエラー粒度を分けたくなったタイミングで再評価。
- `crates/session-store/src/fs_store.rs:200-210` — `FsStore::read_entry_count` が `fs::read_to_string` で全文ロードしてから行数カウントするため O(n)。`ensure_head_or_fork` は run-start でしか呼ばれず現状は許容範囲だが、長期セッションが普通になった時点で `\n` バイト数の cheap count か末尾 seek に置き換える。
- `crates/session-store/src/segment.rs:143-172` `ensure_head_or_fork` (free fn, test 専用・本番 caller ゼロ) と `crates/pod/src/pod.rs:1941-2006` `Pod::ensure_segment_head` (本番 inline) に live auto-fork の検知 + forked_from 記録が二重実装されている。entry-hash-abolish 以前からの重複で、両方独立にテスト済みだが drift 必至。session-store 側を本番から呼ぶ形に寄せるか free fn を畳むかは要設計判断。Pod state / fork 周辺を次に触るときに統合を検討。
- `crates/pod/src/pod.rs:4100-4147` / `crates/pod/src/spawn/registry.rs:84-174` — restore 時の spawned child prune/reclaim が Pod restore path と spawned registry load path の両方に残っている。現状は安全側の重複チェックだが、Pod state / spawned registry 周辺を次に触るときに責務境界を再整理。
