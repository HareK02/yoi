# Known Issues

Ticket を切るほどではないが、次に近所を触るときに合わせて拾いたい小粒な所見の置き場。

## 運用

- 1 項目 = 出典 (file:line) + 症状 (一文) + トリガー (いつ拾うか、一文)
- 関連 ticket があれば `→ [tickets/foo.md]` でリンク
- 修正したら同じコミットで該当エントリを削除する (履歴は git)
- ここに溜める基準: 「ticket は重い」「だが忘れたら次の触り手が踏む」もの。明確に作業すべきものは ticket 化する

## エントリ

- `crates/pod/src/controller.rs:703-718` / `crates/tui/src/app.rs:837-845` — bad workflow slug を含む `Method::Run` 送信時、`Event::UserMessage` の早期 broadcast で `turn_index += 1` されターンヘッダだけ残る ("ghost turn header")。次に TUI のターンヘッダ / エラー表示周りを触るときに整理。
- `crates/pod/src/controller.rs:1256-1265` — `worker_error_code` で `PodError::WorkflowResolve(_) => InvalidRequest` が post-commit な resolve エラー (`KnowledgeNotFound` 等) にも適用される。意味論的には妥当方向だが、resolve 系のエラー粒度を分けたくなったタイミングで再評価。
- `crates/session-store/src/fs_store.rs:200-210` — `FsStore::read_entry_count` が `fs::read_to_string` で全文ロードしてから行数カウントするため O(n)。`ensure_head_or_fork` は run-start でしか呼ばれず現状は許容範囲だが、長期セッションが普通になった時点で `\n` バイト数の cheap count か末尾 seek に置き換える。
- `crates/session-store/src/segment.rs:143-172` `ensure_head_or_fork` (free fn, test 専用・本番 caller ゼロ) と `crates/pod/src/pod.rs:1943-2006` `Pod::ensure_segment_head` (本番 inline) に live auto-fork の検知 + forked_from 記録が二重実装されている。entry-hash-abolish 以前からの重複で、両方独立にテスト済みだが drift 必至。session-store 側を本番から呼ぶ形に寄せるか free fn を畳むかは要設計判断。Pod state / fork 周辺を次に触るときに統合を検討。
- `crates/pod/src/pod.rs:4074` / `crates/pod/src/spawn/registry.rs:83` — restore 時の spawned child prune/reclaim が Pod restore path と spawned registry load path の両方に残っている。現状は安全側の重複チェックだが、Pod state / spawned registry 周辺を次に触るときに責務境界を再整理。
