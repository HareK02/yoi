# Review: Client crate の切り出し

## 前提・要件の確認

- `crates/client/` 新設、`crates/tui/src/client.rs` 相当の機能を提供 (`tickets/client-crate.md:28`)
  - 満たされている。`crates/tui/src/client.rs` は `crates/client/src/pod_client.rs` へ純粋 rename されており (`PodClient::{connect, send, try_next_event, next_event}` の 4 API は完全に同一)、`Drop` で reader タスクが mpsc 切断を契機に終了する graceful shutdown 挙動も同じ。
- TUI が新 crate に依存して動作・既存テストが通る (`tickets/client-crate.md:29`)
  - 満たされている。`crates/tui/Cargo.toml:8` で `client = { workspace = true }` を追加、`crates/tui/src/main.rs:30` で `use client::PodClient`、`crates/tui/src/spawn.rs:19` で `use client::{SpawnConfig, spawn_pod}`。`cargo test -p tui` 91 件 pass、ローカルで `cargo build --workspace` も完走。
- API は GUI / E2E から呼べる粒度で公開 (`tickets/client-crate.md:30`)
  - 満たされている。`crates/client/src/lib.rs:14-15` で `PodClient` と `spawn::{SpawnConfig, SpawnError, SpawnReady, spawn_pod}` が re-export されており、接続 / Method 送信 / Event 購読 (`next_event` / `try_next_event`) / shutdown (Drop) と subprocess spawn が独立して呼び出せる粒度で揃う。
- pod subprocess を spawn する経路の決定 (`tickets/client-crate.md:31`)
  - 満たされている。client crate に同梱しつつ `spawn_pod` と `PodClient::connect` を分離。GUI が「自身で spawn せず attach する」「自身で spawn する」のいずれを選んでも一段の関数呼び出しで済む。

## 完了条件 (`tickets/client-crate.md:33-37`)

- 上記要件を満たす点は OK。
- TUI 既存挙動・テスト通過は OK (`spawn::tests::*` 6 件含めて 91 件 pass)。
- E2E ハーネス (`tickets/e2e-harness.md`) は本 crate に依存して protocol を喋れる状態 — 公開 API として `PodClient::connect(&Path) -> Result<Self, io::Error>` と `spawn_pod(SpawnConfig, FnMut(&str)) -> Result<SpawnReady, SpawnError>` が揃ったため、依存追加だけで E2E が protocol を直接駆動できる。OK。

## アーキテクチャ・スコープ

- 範囲外 (`tickets/client-crate.md:39-44`) を侵していない:
  - GUI バイナリ実装には踏み込んでいない。
  - protocol 互換破壊なし (`pod_client.rs` は `JsonLineReader/Writer` をそのまま使用)。
  - TUI のリファクタは責務移動のみ。`Form` / `draw_form` / `build_overlay_toml` / `load_resume_scope` 等の UI / manifest 解決ロジックは TUI に残置。`crates/tui/src/spawn.rs:541-653` の単体テストも手付かず。
  - daemon 層には触れていない。
- crate 命名: `client` (プレフィックス無し)。`feedback_crate_naming.md` 方針に準拠。
- 依存追加方法: `Cargo.toml` の `[workspace.dependencies]` への登録は workspace 規約上必須なため手動編集が正当 (`cargo add` の対象外)。`crates/client/Cargo.toml` も `protocol` / `manifest` / `tokio` / `uuid` を `workspace = true` 経由で取得しており方針通り。
- レイヤ整合: `client` は `protocol` / `manifest` / `tokio` のみに依存し、`session-store` や上位レイヤを引きずり込んでいない。`session_store::StoreError` 関連の error は TUI 側の `SpawnError::{Store, MissingResumeScope}` に残置されており、責務分割が綺麗に出来ている (`crates/tui/src/spawn.rs:46-85`)。
- `SpawnError` の Display 移送: 旧 TUI の `SpawnError` のうち `PodLaunchFailed` / `PodExitedEarly` / `Timeout` / runtime dir 解決失敗 / `Io` を `client::SpawnError` 側へ移動し、TUI 側は `Spawn(client::SpawnError)` でラップして `{e}` で透過表示。重複なくユーザー向けメッセージが保持されている。
- 公開 API 境界: 生 socket / `JsonLineReader` を露出せず、薄いラッパーに留めた。チケットの検討事項 (`tickets/client-crate.md:22`) に対する妥当な選択で、既存挙動を壊さない最小範囲。

## 指摘事項

### Non-blocking / Follow-up

- `crates/client/src/spawn.rs` に単体テストが無い (`INSOMNIA_POD_COMMAND` で mock pod を差し込めば ready / timeout / 早期 exit の各パスをテスト可能)。E2E ハーネス側で間接的にカバーされる予定なら不要だが、E2E チケット先行よりこの crate の自己完結テストが入っていると将来回帰検出が容易。
- `SpawnConfig::cwd` が `PathBuf` フィールドで強制される一方、TUI 側 `wait_for_ready` (`crates/tui/src/spawn.rs:275`) は `run()` の冒頭で取った cwd と独立にもう一度 `current_dir()` を呼んでいる。動作上は等価だが、責務切り出しの完了を機に `run()` で取得した cwd を form に持たせて一度だけ参照する形にできる (今回スコープ外でも可)。
- `crates/client/src/spawn.rs:21-22` の `READY_PREFIX` / `READY_TIMEOUT` は pod 側 (`crates/pod/`) の出力フォーマットと結合した contract だが、両者の同期は型/定数共有でなく目視確認のまま。protocol crate に const として置く方が長期的に安全だが、これも今回の責務切り出しスコープを超える。

### Nits

- `crates/client/src/lib.rs:5` の docstring "pod バイナリをサブプロセスとして起動し" は OK だが、`spawn_pod` が `kill_on_drop = false` + `process_group(0)` で detach する点を 1 行追加するとライフサイクル不変条件が見えやすい (本体側 `crates/client/src/spawn.rs:9-11` には記載されているのでそちらへの参照でもよい)。
- `SpawnReady` の同名構造体が `crates/tui/src/spawn.rs:35-38` にも残っており、`client::SpawnReady` を unwrap して TUI 側で再構築している (`crates/tui/src/spawn.rs:288-291`)。TUI 側の `SpawnReady` は外部公開されないため `pub use client::SpawnReady` に置換可能だが、TUI 内 API 表面の安定性に関わるので任意。

## 判断

Approve — 要件・完了条件・範囲外の規定に正しく収まっており、責務移動以上のリファクタは行われていない。新 crate は他 crate から再利用できる粒度で公開され、TUI 既存挙動は 91 件のテストで保たれている。Follow-up の指摘は本チケットの完了を妨げない。
