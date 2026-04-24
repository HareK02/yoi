# Review: pod クレートのモジュール分割

## 前提・要件の確認

- **目標レイアウト**: ほぼチケット通り。`crates/pod/src/` 直下は `pod.rs` / `controller.rs` / `factory.rs` / `shared_state.rs` / `hook.rs` / `interrupt_and_run.rs` / `lib.rs` / `main.rs` のみとなり、`ipc/` / `spawn/` / `prompt/` / `compact/` / `runtime/` の 5 つのサブディレクトリに責務別に移動されている（`crates/pod/src/` ls 結果 + 各 `mod.rs` で確認）。
- **`ipc/` と `spawn/` の分離**: 自 Pod 対外 socket 系は `ipc/` (`server`, `event`, `interceptor`, `notifier`, `notification_buffer`)、親子関係系は `spawn/` (`tool`, `registry`, `comm_tools`) に分かれており、対向別の軸が保たれている。
- **`compact/` への会計系集約**: `state` / `worker` / `token_counter` / `usage_tracker` / `prune` がすべて `compact/` に収まっている。
- **`pod_` prefix 全廃**: `pod_events` → `ipc::event`、`pod_interceptor` → `ipc::interceptor`、`pod_comm_tools` → `spawn::comm_tools` に改名済み。`spawn_pod` / `spawned_pod_registry` も `spawn::tool` / `spawn::registry` に短縮。ソース・テストともに旧名は残っていない（grep 確認済み）。
- **公開 API パスの変化**: チケット表通りに移動済み。`crates/pod/tests/` 側も 3 本（`pod_comm_tools_test.rs` / `pod_events_test.rs` / `spawn_pod_test.rs`）を新パスに一斉更新。`pod::spawn::tool::spawn_pod_tool` 等は `pub mod` を経由して到達可能。
- **スコープ外（ファイル内部分割なし）**: `pod.rs` / `scope_lock.rs` の中身には手が入っておらず、差分は `use crate::...` の書き換えに限定されている（`git diff` 確認）。
- **完了条件**: `cargo check -p pod` 緑、`cargo test --workspace` が 558 件緑との申告とも整合（依存側・ビルド側でも stale な参照なし）。

## アーキテクチャ・スコープ

- **レイヤ境界**: 新しい軸は「自 Pod の外界（ipc）」「他 Pod（spawn）」「プロンプト」「会計・圧縮」「ランタイム配置」という独立した関心事で、相互依存は必要最小限（`ipc::event` → `runtime::scope_lock`、`ipc::interceptor` → `compact::token_counter` / `compact::state` など）。責務軸が立ち、`llm-worker` の `llm_client/scheme/` 等と釣り合う粒度になった。
- **純粋な再配置か**: 差分は (1) 物理移動、(2) `use crate::...` の書き換え、(3) `pod.rs` 内 1 箇所の `runtime_dir::default_base` → `dir::default_base`、(4) `interrupt_and_run.rs` の `#[cfg(test)] use` のパス、(5) `prompts.rs` の `include_str!("../../../...")` を 1 階層ぶんだけ `../../../../...` に延長（`realpath` で同一ファイルを指すことを確認）— の範囲に留まっている。振る舞い変化は混ざっていない。
- **hook/ サブディレクトリの取りやめ**: 現状 `hook.rs` が単体で ~200 行程度、かつ `interrupt_and_run.rs` は `impl Pod` の拡張メソッドであり `hook` の仲間ではない、という判断は妥当。無理にディレクトリを作ると「中身 1 ファイル＋意味の異なる隣人」という歪な構造になる。ticket 本文の現行版（行 51–52）は新判断と整合しており、乖離はない。
- **コードベースを歪める変更**: なし。新構造のために本体のロジックを曲げた箇所や、無駄な中間層の追加は見当たらない。
- **依存追加**: 本チケットでは外部 crate の追加はなく、`cargo add` ルールの対象外。

## 指摘事項

### Non-blocking / Follow-up

- **`pub mod` / `mod` の区別が一部で広がっている** — `crates/pod/src/lib.rs` と各 `mod.rs`。旧 `lib.rs` では次の 5 個は `mod ...;`（crate-private）だった:
  - `agents_md`, `prompt_loader`, `prompts`, `system_prompt`, `token_counter`

  新レイアウトではそれぞれ `prompt/mod.rs` の `pub mod agents_md;` / `pub mod loader;` / `pub mod catalog;` / `pub mod system;` と `compact/mod.rs` の `pub mod token_counter;` になっており、`pod::prompt::catalog::PromptCatalog` のような深いパスが外部から到達可能になった（ルート `pub use` で代表型は既に公開されているため、副作用は「外から深いパスが見える」だけ）。

  実害は発見できなかった（`grep -rn "pod::prompt\|pod::compact::token"` は 0 ヒット、利用側は全てルート再エクスポート経由）が、ユーザが質問 #2 で述べている「旧 lib.rs での `pub mod` / `mod` 区別を保持」という意図に厳密に従うなら、上記 5 個は `pub(crate) mod ...;` に落とす方が素直。現に `ipc::notification_buffer` / `ipc::interceptor` / `compact::state` / `compact::worker` / `compact::usage_tracker` / `compact::prune` については `pub(crate) mod` で区別を維持できており、プロンプト系だけが揃っていない。テスト側も `pod::PromptLoader` 等のルート経由で参照しているので、`pub(crate) mod` に絞っても既存テストは影響を受けないはず。

  別チケット or 本チケット内の微修正のどちらでも対応可能。

### Nits

- **`lib.rs` の `pub use` ブロック内で `manifest`/`protocol`/`provider` の外部 crate 再エクスポートが自 crate モジュールの間に挟まっている** — `crates/pod/src/lib.rs:14-30`。旧版でも同様に混在していたので新規に悪化したわけではないが、責務軸で整理する今回のタイミングで「自 crate → 外部 crate」の順にグルーピングしても読みやすい。強い指摘ではない。
- **`pod.rs` 先頭 `use` の順序** — `crates/pod/src/pod.rs:16-30`。`crate::prompt::agents_md`, `crate::compact::state`, `crate::ipc::...`, `crate::prompt::...`, `crate::runtime::...`, `crate::compact::usage_tracker` がサブモジュール別にまとまっておらず行きつ戻りつしている。機能的には無害だが、rustfmt では並び替わらない範囲なので任意で整頓する余地がある。

## 判断

**Approve with follow-up** — チケット本文の要件（配置・命名・prefix 全廃・スコープ外保持）と完了条件（ビルド緑・テスト緑・挙動不変）はすべて満たされている。hook/ 取りやめの判断変更も合理的で本文と整合。唯一、旧 `lib.rs` での `mod`（crate-private）を保持するなら `prompt/` と `compact::token_counter` の 5 モジュールを `pub(crate) mod` に狭める余地があるが、これはブロッキングではない。
