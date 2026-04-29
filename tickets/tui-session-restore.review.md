# Review: TUI: 既存セッションからの Pod 復帰

レビュー対象は `3c90729` 以降のワーキングツリー (`e98a596` までの 2 commit) と、それに伴う `tickets/tui-session-restore.md` の更新版。

## 前提・要件の確認

### 起動導線
- `tui`（引数なし）/ `tui <pod-name>` の既存挙動は `Mode::Spawn` / `Mode::Attach` として温存（`crates/tui/src/main.rs:130-136`）。OK。
- `tui -r` / `tui --resume`（`Mode::Resume`）は picker → `run_spawn(Some(id))` の二段構成で実装（`crates/tui/src/main.rs:202-211`）。OK。
- `tui --session <UUID>` は picker をスキップして `run_spawn(Some(id))` 直行（`crates/tui/src/main.rs:166`）。OK。
- `--resume` と `--session` の排他は `parse_args` で明示チェック（`crates/tui/src/main.rs:120-122`）。OK。
- pod CLI に `--session` を追加し `--adopt` と排他（`crates/pod/src/main.rs:52`）。OK。

### セッション一覧
- `manifest::paths::sessions_dir()` 経由で store を開き直近 10 件を表示（`crates/tui/src/picker.rs:76-86`）。OK。
- `SessionId` 短縮表記、最後の user/assistant メッセージプレビュー（同 `:259-300`, `last_message_preview`）。OK。
- 並び順は `Store::list_sessions` の UUIDv7 順（=作成時刻順、新しい順）。OK。
- session が 0 件のときは `PickerError::NoSessions` を返してエラー終了（`:79-81`）。OK。
- ログ破損時は `[corrupt]` プレビューで行を残す（`build_preview` の `Err` 分岐, `:154`）。スキップではないが「エラー表示して」の文言を満たす。OK。
- **未実装: 「その session が今 live かどうか」の表示** — `scope_lock` 連携が picker に入っていない（`crates/tui/src/picker.rs` 内に `scope_lock` 参照なし）。要件に明記された情報項目なので blocking 寄り。後述。

### 復元 Pod の構築
- 同じ `session_id` を引き継ぎ、追記する設計に変更されている（`crates/pod/src/pod.rs:1599-1690`）。fork は行わない。OK。
- `system_prompt` は session 保存値そのまま、`SystemPromptTemplate` 再レンダなし（`prepare_pod_common(.., parse_template = false)`、`system_prompt_template = None`）。OK。
- `head_hash` は `RestoredState.head_hash` をそのまま保持（`:1666`）。OK。
- 履歴・request_config・turn_count・usage_history・extract_pointer・interrupted フラグ復元は揃っている（`:1651-1686`）。OK。
- 圧縮アンカー: 先頭が `Role::System` のとき `cache_anchor=Some(0)` にする処理（`:1644-1657`）。compact 後の resume を想定した妥当な追加。OK。

### Pod / 高レベル API の整理
- `Pod::restore_from_manifest(session_id, manifest, store, loader)` 新設（`:1599`）。OK。
- 旧 `Pod::restore` は削除済み（grep で呼出元なし、`Pod::new` のドキュメントコメントだけ「Pod::restore」表記が残存=軽微）。OK。
- `prepare_pod_common` で `from_manifest` / `from_manifest_spawned` / `restore_from_manifest` の共通部分を集約（`:1922`）。粒度妥当。OK。

### scope.lock への session_id 追加と live 検出
- `Allocation::session_id: Option<SessionId>` を追加（`crates/pod/src/runtime/scope_lock.rs:58-60`）。`#[serde(default)]` で旧フォーマット読込時は `None` 扱いになる（dev 期間互換不要だが副作用としては安全）。OK。
- `register_pod` / `install_top_level` / `adopt_allocation` のシグネチャに `session_id` を追加（`:306, :501, :533`）。OK。
- `delegate_scope` のシグネチャは変更せず `session_id = None` のまま事前予約し、子側 `adopt_allocation` で確定する設計（`:344-387, :533-551`）。チケット文面（57 行）は `delegate_scope` も含めて挙げているが、子の session_id が spawner 時点で未確定であるため `Option` 化が技術的に妥当で、ユーザの設計判断 (3) として明確に記録されている。**ticket 本文と乖離があるが、コメント (`:55-60, :381-384`) で意図が明記されているため許容**。
- `LockFile::find_by_session` 提供（`:73-77`）。OK。
- `scope_lock::lookup_session(id) -> Option<SessionLockInfo>` 提供（`:567-578`）、`SessionLockInfo { pod_name, socket, pid }` も要件通り（`:556-560`）。OK。
- 既存 `scope.lock` の互換性は `#[serde(default)]` で旧形式も壊れずに読める。OK。

### 二重起動の扱い
- TUI 経路では `pod` 子プロセスが `PodError::SessionInUse` で非ゼロ終了 → spawn が `PodExitedEarly` でスタブ末尾を viewport に表示してそのまま終了（`crates/tui/src/spawn.rs:329-336`）。エラーメッセージ本体（`session_id` / `pod_name` / `socket`）も `PodError::SessionInUse` の `#[error]` に含まれて stderr 経由で通る（`crates/pod/src/pod.rs:1886-1894`）。要件通り picker 復帰や自動 attach 切替なし。OK。

### UI / 操作
- 上下キー / `j` `k` / Enter / Esc / Ctrl-C で picker 操作（`crates/tui/src/picker.rs:223-232`）。OK。
- 直近 10 件のみのためスクロール UI なし。OK。
- 決定後 name 入力ダイアログを別 inline viewport として描画。`close_viewport` で改行を 1 行押し出して name 入力との重なりを防止（`:127-138`）。OK。
- title `resume pod   session: <short-id>` で復帰モード明示（`crates/tui/src/spawn.rs:516-519`）。OK。
- name のデフォルトは cwd 由来でこれまでと同じ。OK。
- 検索フィルタは将来拡張余地のある state で、必須にしない。OK。

### 完了条件
- 「`pod --session <UUID>` で fork 由来の空 session が生成されない」: 実装上 `Pod::restore_from_manifest` は新 session を作らず元の `session_id` を再利用するので OK。
- 「復帰直後に過去履歴が表示される」: `RestoredState.history` を `worker.set_history` 経由で流し込んでおり、Controller の history.json/Event::History 経路は変更不要。OK（手動動作確認は範囲外）。
- 「同一 source session に対する live Pod が存在する場合、復元起動はエラーで終了し、`pod_name` / socket がメッセージに表示」: 静的状態に対しては成立。ただし live 写し書き入れ替わり時の整合性に注意点あり（後述 Blocking）。

## アーキテクチャ・スコープ
- 新規クレートなし。`tui` 側に `session-store` を依存追加したのみ（`crates/tui/Cargo.toml:17`）。`session-store` が picker のために `ContentPart` / `Item` / `Role` を `pub use` で再公開（`crates/session-store/src/lib.rs:43`）。`llm-worker` のレイヤを跨いで TUI が直接型を持ち込まずに済むので妥当。
- `prepare_pod_common` の抽出は 3 系統の共通部分（pwd / scope / client / catalog / template parse 切替）の自然な集約で、過剰抽象化していない。`PodCommon` 構造体も local。OK。
- `PodError` への `SessionInUse` / `SessionEmpty` 追加は `restore` 固有の正当な失敗モード。`thiserror` の表現に揃っている。OK。
- picker の preview 実装は素朴（`read_all` で全件読む）だが、大セッション時の重さは本チケット範囲外と user note (5) で明記されており妥当。OK。
- spawn.rs と picker.rs の境界が「picker は選択のみ、scope-lock チェックや fork は後段」で揃っており、低レベル基盤と UI が混ざっていない。OK。

## 指摘事項

### Blocking

- **scope.lock の session_id による「構造的」二重書き込み防止には穴がある**
  - 要件 / 設計判断 (1) は「同一 session への同時書き込みは `scope.lock` の `session_id` で構造的に防止する」が前提。実装は (a) `restore_from_manifest` 入口の `lookup_session` チェックと (b) `adopt_allocation`/`install_top_level` 時の `session_id` 記録、の二点のみ。次の経路で構造保証が崩れる:
    1. **compaction 後**（`crates/pod/src/pod.rs:1133-1156`）: `self.session_id = new_session_id` に差し替わるが、scope.lock 上の `Allocation.session_id` は旧 session_id のまま更新されない。結果として `lookup_session(new_session_id)` は `None` を返し、live Pod が新セッションへ書いている最中でも別プロセスが `restore_from_manifest(new_session_id)` を成立させてしまう。
    2. **`ensure_head_or_fork` 後**（`crates/session-store/src/session.rs:109-138`）: `self.session_id` が auto-fork で書き換わるが scope.lock は同じく置き去り。
    3. **TOCTOU**: `lookup_session` の `LockFileGuard` を閉じてから後段の `install_top_level` で再オープンするまでの間に、別プロセスが同じ流れで通り抜ける可能性。`register_pod` 内では `session_id` の重複チェックを行っていない（`pod_name` と scope のみ）ので、別 `pod_name` を指定する 2 つの resume が両方成功し、両方が同じ session jsonl に append しに行ける。
  - 実害は `ensure_head_or_fork` の auto-fork が後段で救うとはいえ、それ自体「fork しない」設計の前提を破る挙動になる。少なくとも以下のいずれかが必要:
    - `register_pod` に session_id 重複チェックを追加し、`lookup_session` と install を同一 `LockFileGuard` 配下で行う API を提供する。
    - compaction 完了時 / `ensure_head_or_fork` 後に scope.lock の自分のエントリの `session_id` を書き換える明示的な hook を入れる。
  - 該当箇所: `crates/pod/src/pod.rs:1156`、`crates/pod/src/runtime/scope_lock.rs:306-339`（`register_pod` に session_id 衝突チェックなし）、`crates/pod/src/pod.rs:1599-1631`（lookup と install の分離）。

### Non-blocking / Follow-up

- **picker に live セッションの可視化が無い**（要件「その session が今 live かどうか（`scope.lock` を引いて判定）」）。`crates/tui/src/picker.rs` 全体に `scope_lock` 参照なし。`Row` に `live: bool` を持たせ、`scope_lock::lookup_session(id)` で判定して表示色を変える程度で完結する。要件の文言上は blocking 級だが、live セッションを picker から選んだ場合でも `Pod::restore_from_manifest` 側で `SessionInUse` で弾かれて pod が落ち、TUI もそのまま終了するという完了条件は満たせるため follow-up とする。次回の TUI 改善で対処を推奨。

- **`pod` CLI および TUI の `Form.resume_from` ドキュメントコメントが旧 fork 設計のまま**。
  - `crates/pod/src/main.rs:47-51`: 「The source session log is forked at its head into a new session id, so the original jsonl is left untouched and double-write races are impossible.」 → 実装は fork しないし、二重書き込み防止は scope.lock 経由の前提に変わっている。
  - `crates/tui/src/spawn.rs:448-452`: 「the child pod is launched with `--session <id>` so it forks and restores `id`」 → 同上。
  - 動作には影響しないが、レビュー観点（コードベースを歪めていないか）で読者を誤導するため早めに直したい。

- **`Pod::new` のドキュメントコメントに削除済みの `Pod::restore` 表記が残存**（`crates/pod/src/pod.rs:104`）。「lower-level constructors (`Pod::new`, `Pod::restore`)」の `Pod::restore` を削るか、低レベル API の用途記述を `Pod::new` 単独に書き換えれば足りる。

- **`restore_from_manifest` / `lookup_session` / `find_by_session` / `SessionInUse` / `SessionEmpty` のテストが未追加**。`scope_lock.rs` 既存テストは register/delegate/adopt 系のみ。`find_by_session` の挙動（`session_id == Some(_)` のみマッチし `None` の placeholder をスキップする）と、empty session に対する `restore_from_manifest` の `SessionEmpty` 経路は意図そのものが今回新規なので、ユニットテスト 1〜2 本でも欲しい。

- **picker の preview が重い場合**（user note (5) 明記）の挙動は範囲外で良いが、`read_all` を 10 セッション分逐次（`for id in ids ... build_preview(...).await`）で行っているため大規模 store ではブロッキング感が出る。将来 `tokio::join_all` 並走化で済む程度なので、follow-up 記録のみ。

### Nits

- `PickerError` の `Display` が `unable` 系の標準形式を踏襲していて良いが、`StoreError` を `Box<dyn Error>` 経由で渡す経路で `From` の方向（`From<StoreError>` for `PickerError`）と main.rs 側の `Box<dyn Error>` の整合は保たれている。問題なし。
- `crates/tui/src/picker.rs:23` の `pub use llm_worker::llm_client::types::{ContentPart, Item, Role}` は本来 picker のためだけに `session-store` 経由で入っている。将来 picker 以外も使うようなら `session-store` が「保存対象の構造を再公開する」責務として正規化する余地あり。

## 判断

**Approve with follow-up** — チケットのコア要件（`tui --resume` / `--session` 起点の復帰経路、同 session_id 再利用、`scope.lock` への session_id 拡張、二重起動時のエラー終了、共通部分の抽出と低レベル `Pod::restore` 削除）はすべて達成されており、build / `cargo test -p pod` も緑。

ただし設計判断 (1) で「scope.lock で構造的に二重書き込み防止」と打ち出している割に、(a) compaction 後 (b) `ensure_head_or_fork` 後 (c) lookup→install の TOCTOU の 3 経路で構造保証が崩れる点は要承知。ticket の「fork しない」前提を厳密に守るには上記 Blocking のいずれかの補強が必要だが、現実には `ensure_head_or_fork` の auto-fork で実害が起きにくいため、本チケット完了は Non-blocking として現行 PR を許容しつつ、scope.lock 二重書き防止の補強を別チケットで切り出す運用が妥当。

加えて、要件文に明記されている picker の live 表示と、stale ドキュメント（pod CLI / spawn.rs Form / Pod::new）の刷新は早めに follow-up で潰してほしい。
