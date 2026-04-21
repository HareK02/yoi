# Review: Protocol の設計

対象コミット: working tree (未コミット)
レビュー日: 2026-04-21

## 前提・要件の確認

チケット「本チケットで実装するもの」1〜5 を実装と照合した。

1. **`protocol` クレートに `uuid` 追加 + 3 バリアント追加**
   - `crates/protocol/Cargo.toml:11` に `uuid = { version = "1.23.1", features = ["serde"] }` を追加済み。
   - `crates/protocol/src/lib.rs:138-149` に `Event::CompactStart` / `Event::CompactDone { new_session_id: uuid::Uuid }` / `Event::CompactFailed { error: String }` を追加。決定事項 5 の型方針（`uuid::Uuid` 直接）に一致。
   - ただしチケット本文は `uuid = { workspace = true, features = ["serde"] }` と書いているが、ルート `Cargo.toml` には `[workspace.dependencies]` が存在せず、そのままでは解決不能。既存の `session-store` と同じく直接バージョン指定にしているのは実装として合理的（コードベース流儀に整合）。チケット文言との齟齬は残っているので、どちらかを合わせるのが望ましい（後述）。
2. **`Pod` が `event_tx` を保持 / Controller が `Notifier` と同タイミングで渡す**
   - `crates/pod/src/pod.rs:124-128` にフィールド追加、`attach_event_tx` は `pod.rs:358-361`。全 4 箇所のコンストラクタで `event_tx: None` 初期化済み（`pod.rs:185, 251, 1182, 1242`）。
   - `crates/pod/src/controller.rs:104-107` で `attach_notifier` の直後に `attach_event_tx(event_tx.clone())`。決定事項 6 通り。
3. **compact 発火 2 箇所で start/成功/失敗を broadcast**
   - `do_compact_and_resume`: `pod.rs:711` (start) / `pod.rs:718` (done) / `pod.rs:726-728` (failed)。
   - `try_post_run_compact`: `pod.rs:757` (start) / `pod.rs:764` (done) / `pod.rs:770-772` (failed)。
   - 既存 `notify(...)` の失敗通知（Compactor Warn/Error）も残っており、決定事項 2 の「初期移行では重複して出しても害はない」に整合。
4. **TUI `handle_pod_event` の 3 分岐**
   - `crates/tui/src/app.rs:192-214` で 3 バリアントを `NoticeWarn`/`NoticeError` 枠に追加。
   - 表示文言もチケット指定通り（`[compact] starting` / `[compact] done (new session <先頭8字>)` / `[compact error] <message>`）。
5. **テスト**
   - protocol クレート: JSON roundtrip 3 本（`event_compact_start_roundtrip` / `event_compact_done_roundtrip` / `event_compact_failed_roundtrip`）— `new_session_id` の shape（ハイフン付き UUID 文字列）も `serde_json::Value` 経由で検証済み。
   - pod クレート統合テスト: `crates/pod/tests/compact_events_test.rs` に `post_run_compact_success_broadcasts_start_and_done` / `post_run_compact_failure_broadcasts_start_and_failed` の 2 本。
   - **mid-turn (`do_compact_and_resume`) のテストは欠落**。ファイル先頭コメントで「`send_event` ヘルパが共通のため inspection で担保」と明記しているが、チケット要件 5 は「成功/失敗/mid-turn の各パスで Event が発行されることを確認する統合テスト」と明示的に 3 パスを要求している。要件に対して実装側の判断で削ったことになるので Follow-up 以上。

`cargo build -p protocol -p pod -p tui` と `cargo test -p protocol --lib` / `cargo test -p pod --test compact_events_test` はローカルで通過。ビルドが通り機能が実行可能であるという全体不変条件は保たれている。

## アーキテクチャ・スコープ

- **層分離**: `protocol` は wire 型に専念し、Pod 層は `broadcast::Sender<Event>` を保持して直接 push。`Notifier` は `Event::Notification` の replay 専用として残り、compact 系は通していない。決定事項 6/7 通りで、Notifier の責務を汚さない良い切り分け。
- **send_event の書き方**: `pod.rs:370-374` の `fn send_event(&self, event: Event)` は `if let Some(tx) = self.event_tx.as_ref() { let _ = tx.send(event); }`。subscriber 不在時の `SendError` を潰す挙動（決定事項 7 の「late subscriber には届かない」）と一致。late subscriber への再配信をしないのも合意通り。
- **依存追加規律**: `uuid` は `cargo add -p protocol uuid --features serde` で追加された形跡（バージョン番号 `1.23.1` が精密指定で入っている）。`Cargo.lock` の patch-level 更新も自然。`cargo add` 利用ポリシーを満たす。
- **クレート命名・モジュール分割**: 新規クレート追加なし。`Pod` 本体への追加は `attach_event_tx` / `send_event` の 2 メソッド + 1 フィールドに留め、compact ロジックは既存関数内の差し込みに限定。機能別ファイル分離のポリシーを破っていない（今回は既存メソッド内への数行挿入なので妥当）。
- **プロトコルの健全性**: wire path は `broadcast::Sender<Event>` → `socket_server::handle_connection` の `rx.recv()` → `writer.write(&event)` で既存と同じルート。新バリアントを足すだけで TUI まで透過的に届く（`crates/pod/src/socket_server.rs:83-91`）。
- **代替案の妥当性**: `Notifier` に compact を相乗りさせる案は、「Notification は Warn/Error レベル付き replay バッファで意味が噛み合わない」ため避けた判断は正しい。`Notifier` は今後も compact 系を通さないことを前提にできるようになった。
- **YAGNI 方針**: `SessionChanged` の一般化や `request_id` フィールドの先取り導入を見送り、compact に絞った最小変更。コードベースを歪めていない。

## 指摘事項

### Blocking
- なし。

### Non-blocking / Follow-up
- **mid-turn のテスト省略はチケット要件 5 に対する未達**（`crates/pod/tests/compact_events_test.rs:1-6` のファイル冒頭で省略理由を明記済み）。`do_compact_and_resume` は `just_compacted` フラグと `Box::pin` の async 再帰、`resume()` 呼び出しが挟まるため、`try_post_run_compact` とは制御フローが別物。共通化しているのは `send_event` のみで、発火点の有無・順序・失敗時の `notify + Err(e)` 伝播は別のテスト対象である。後続チケットに流してよい程度の軽い欠落だが、要件の文言通り 1 本（例: 成功時 + `PodError::CompactThrash` での挙動、または Yielded からの compact success）追加するのが望ましい。
- **ticket 文言と Cargo.toml の表現齟齬**: ticket では `uuid = { workspace = true, ... }` と書いているのに、実装は直接バージョン指定（`session-store` と同じ書き方）。どちらが正解かはプロジェクト方針だが、ワークスペースに `[workspace.dependencies]` が未定義なので直接指定の方が現状整合する。チケットの文言を「`session-store` と同じく直接バージョン指定で追加する」に修正する、あるいは `[workspace.dependencies]` を用意して両方揃える、のどちらか。後続対応で構わない。
- **CompactThrash 時は Start/Failed のいずれも発火しない**（`pod.rs:698-702`）。thrash 検出は compact を実行前に弾くので `CompactStart` が出ないのは正しいが、TUI 側は「compact が必要だったのに thrash で諦めた」ことを知る手段がない。現状は `PodError::CompactThrash` が `run` の Result として返り、その先で誰かがエラー Event 化する想定と思われるが、今回のスコープでは扱わなくてよい。将来 compact 周りの可視化を強化するときに再考を。
- **`CompactDone` の短縮 UUID 表現** (`tui/src/app.rs:199-203`): `to_string().chars().take(8)` は UTF-8 上 ASCII なので実質 `&uuid_str[..8]` と等価。実害はないが、`new_session_id.as_simple().to_string()[..8].to_string()` か `format!("{new_session_id:.8}")` 相当にするとアロケーション 1 回で済む。nit に近い。

### Nits
- `crates/protocol/src/lib.rs:138-149` の docstring が `CompactStart`/`CompactDone`/`CompactFailed` で個別に説明されていて読みやすい。`CompactStart` のコメントに「Broadcast to all clients; not replayed to late subscribers.」まで書いているので、決定事項 7 が wire-type 側に滲み出して次のレビューアの手助けになる。このレベルで継続されると良い。
- `compact_events_test.rs:121-131` の `drain` ヘルパは `loop { match ... { Err(_) => break } }` で lagged/closed を区別していない。このテストでは channel 容量 64 で十分なので問題ないが、将来テストを増やす際には `try_recv` の `TryRecvError::Lagged(n)` を拾って assert するとより堅牢。

## 判断

**Approve with follow-up** — 要件 1〜4 と アーキテクチャ方針（決定事項 5/6/7）は全て達成。ビルド・テストも通過。要件 5 のうち mid-turn テストの省略がチケット文言に対してわずかに未達だが、インスツルメント自体は 1 箇所共通 (`send_event`) で呼び出し側の差分も小さいため blocker ではなく follow-up として処理可能。チケット側の `uuid` 依存表記（`workspace = true`）と実装の直接指定の齟齬も合わせて、どちらかを揃える後続対応を推奨する。
