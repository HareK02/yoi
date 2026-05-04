# Review: TUI Pod 状態同期と Ctrl 系操作の安定化

レビュー対象: `develop` の作業ツリー（staged + unstaged）。22 files / +641 -311。

## 前提・要件の確認

### 状態同期 (要件 1)
- **達成**。`PodStatus` を `protocol` クレートに昇格させ、`Event::Status` と `Event::History.status` を新設（`crates/protocol/src/lib.rs:317-328, 437-447`）。
  - GetHistory 応答に現在 status を埋め込み (`crates/pod/src/ipc/server.rs:162-168`)。
  - controller の status 遷移ごとに `Event::Status` を broadcast する単一経路を `set_controller_status` に集約 (`crates/pod/src/controller.rs:68-77`)。
  - TUI 側は `App.pod_status` を `Event::Status` / `Event::History.status` から駆動し、`set_pod_status` が `running` / `paused` / `busy` を一括更新する (`crates/tui/src/app.rs:110-118, 669-679`)。
- attach 時の history 取得時点での status 同期は `attach_history_includes_current_status` テストでカバー (`crates/pod/tests/controller_test.rs:217-238`)。

### post-run 処理の扱い (要件 2)
- **達成**。`PodStatus::Busy` を新設し、`PodRunResult::Finished` / `LimitReached` で controller が一旦 Busy に入ってから post-run jobs (extract / consolidate / compact) を実行、完了後に Idle へ遷移する設計 (`crates/pod/src/controller.rs:79-135, 786-836`)。
- Busy 中の `Event::Status::Busy` broadcast はテスト `run_end_enters_busy_until_post_run_finishes_and_broadcasts_status` で確認 (`crates/pod/tests/controller_test.rs:175-214`)。
- TUI status line に `busy — finishing post-run work` 表示 (`crates/tui/src/ui.rs:880-890`)、Ctrl-X は Busy 中はガード (`crates/tui/src/main.rs:438-442`)。
- `submit_input` も Busy 中は早期リターンし、誤って新しい turn を送信しない (`crates/tui/src/app.rs:298-302`)。

### Ctrl-C / Ctrl-X の分岐 (要件 4)
- **達成**。`crates/tui/src/main.rs:434-442, 581-595` で `app.pod_status` の値ごとに明示的に分岐:
  - Ctrl-C: Running → Pause / それ以外 → 2-tap quit (TUI exit のみ、Pod は維持)。
  - Ctrl-X: Running → Cancel / Paused, Idle → Shutdown / Busy → ガード (押し戻し)。
- 古い `running` / `paused` ローカルフラグも `pod_status` から派生するため二重情報源にはなっていない (`crates/tui/src/app.rs:110-118`)。

### Input polling (要件 3)
- **達成**。`run_loop` を3段階に再構成 (`crates/tui/src/main.rs:281-338`):
  1. Pod event を最大 32 件まで非ブロッキングでドレイン (バウンド付きで starvation 防止)。
  2. `event::poll(ZERO)` で溜まった key event を全て処理、何かあれば再描画して即 `continue`。
  3. (1)(2) 共に空ならばはじめて `select!` で blocking poll または `client.next_event` を待つ。
- これにより Pod event 大量受信時でも各イテレーションで key 入力チェックが必ず走るため、Ctrl-C/Ctrl-X 取りこぼしは構造的に解消されている。
- `client.try_next_event` (新設) はバウンドドレイン用 (`crates/tui/src/client.rs:38-40`)。

### テスト (要件 5)
- **達成**。`controller_test.rs` に 3本の状態遷移テストを追加:
  - `run_end_enters_busy_until_post_run_finishes_and_broadcasts_status` (要件 2)。
  - `attach_history_includes_current_status` (要件 1)。
  - `pause_while_busy_is_idempotent_not_not_running` (Pause が Busy 中に NotRunning エラーを出さないこと)。

### CLAUDE.md コンテキスト加工原則 (観点 6)
- **違反なし**。`PodEvent` / `Notify` の系統は従来どおり `pod.push_notify(...)` で history append → 次の `pre_llm_request` で context へ流れる経路を踏襲 (`crates/pod/src/controller.rs:537, 683, 863, 883`)。
- 状態 (Running / Busy 等) は context には差し込まれず、`Event::Status` / runtime_dir/status.json と Pod handle の `shared_state` のみで配信。

## アーキテクチャ・スコープ

### 良い点
- `PodStatus` が「Pod 内部状態」から「protocol レベルの値」へ昇格したのは妥当。`Event::Status` を明示すれば外側の TUI/GUI/CLI が同じ source of truth を読める。
- post-run jobs の実行を `run_post_run_jobs` ヘルパに集約 (`crates/pod/src/controller.rs:79-108`)、各 method 分岐 (Run / Notify / Resume / PodEvent) に重複していたロジックを `finish_controller_run` 1本にまとめている。本チケット前は同じ post-run 三段階が4箇所に重複していたため、純粋なリファクタとしても健全。
- Busy → Idle の遷移を `finish_controller_run` 内で完結させ、controller の outer loop は `run_with_cancel_support` の戻り値だけ見れば良い構造。
- TUI input polling のリファクタは "drain bounded → poll terminal → fall back to select" の3段で、よくある正攻法。

### 範囲外との切り分け (観点 7)
- 入力キューイング (`tui-input-queue.md`) には踏み込んでいない。`tui-input-queue.md` は **"in-flight 中に submit された run の保留"** が主題で、本チケットは状態同期と入力 polling 構造のみ。サブミット中の Run は依然として `submit_input` の Busy ガード以外は controller 側 `AlreadyRunning` で拒否される (`crates/pod/src/controller.rs:464-470, 851-856`)。OK。

### 巻き込みの整理 (観点 8, 9)
- 確認した周辺ファイル (`crates/manifest/src/scope.rs`, `crates/memory/src/consolidate/{input,lock,tidy}.rs`, `crates/tools/src/task.rs`, `crates/session-metrics/src/lib.rs`, `crates/pod/src/compact/prune.rs`, `crates/pod/src/ipc/interceptor.rs`, `crates/pod/tests/{consolidation_test,pod_comm_tools_test,session_metrics_test}.rs`) はすべて **rustfmt 由来の表面整形のみ** で、セマンティクスは変わっていない。`git diff HEAD` で確認済み。
  - これらは本チケットでは不要だが、`cargo fmt` の副産物として一緒に乗ってしまったタイプ。レビュー的には "別 PR ならクリーン" だが、blocking ではない。
- `crates/pod/src/lib.rs` の `pub use` 変更は `PodStatus` が `protocol` 側に移動したことに伴う必須の追従。
- `crates/pod/src/runtime/dir.rs` の変更も同上 (test の import パスを `protocol::PodStatus` へ）。
- `crates/pod/tests/consolidation_test.rs` 等の `+/-` も大半が rustfmt とインポートパス更新。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up
- **エラー経路の Status::Busy broadcast 抜け** — `run_with_cancel_support` の `Err(e)` 分岐 (`crates/pod/src/controller.rs:820-835`) は `(PodStatus::Busy, ...)` を返すが、成功側 (`crates/pod/src/controller.rs:794-800`) と違って `Event::Status::Busy` を明示的に broadcast していない。post-run jobs はエラー時にも走るので、その間 TUI 側の `pod_status` は最後に受信した `Running` のままで、Busy バッジは出ない。Idle 遷移は `finish_controller_run` で必ず broadcast されるので致命的ではないが、UX としては "エラー後の数秒間 status が Running のまま見える" 可能性がある。`finish_controller_run` の冒頭 (Busy だった場合) にもう一度 Status::Busy を打つ等の整合化を検討してよい。
- **`runtime_dir.write_status` が Busy では呼ばれない** — 成功分岐 (`crates/pod/src/controller.rs:794-800`) は `shared_state.set_status` と event broadcast はやるが `runtime_dir.write_status` は呼ばない。disk 上の `status.json` を外部プロセスが読む用途では Busy が見えない。`Event::Status` を購読していない CLI / 監視スクリプトには影響しうる。本チケットの完了条件としては Event 経路が満たせていれば足りるので blocking ではない。
- **rustfmt の巻き込み** — `crates/manifest/`, `crates/memory/consolidate/`, `crates/tools/task.rs`, `crates/session-metrics/`, `crates/pod/src/compact/prune.rs`, `crates/pod/src/ipc/interceptor.rs` 等の整形変更は本チケットの目的外。次回以降は事前に `cargo fmt` 単独でコミットするか、関係ないファイルへのフォーマット差分は除外することを推奨。今回は意味的に害はないので blocking ではない。
- **attach 直後の初期 status は `Idle`** — `App::new` で `pod_status: PodStatus::Idle` を初期値に置く (`crates/tui/src/app.rs:89-108`)。`GetHistory` 応答が届く前 (数十ms) に Ctrl-C を押すと "Idle 扱い" の 2-tap quit 経路に流れる。実害は小さく要件 1 に違反しないが、コメントに記すか `History` 受信前は Ctrl 系を抑制する手もある。Follow-up 候補。
- **post-run 中は method_rx が pump されない** — `finish_controller_run` 内で `run_post_run_jobs` を逐次 await しているため、その間 `Method::Run` / `Notify` / `Pause` / `Shutdown` は mpsc キューで待たされる。Pause/Shutdown を Busy 中に押した場合、後続の Idle 遷移後に処理される (Pause は idempotent no-op、Shutdown は遅れて反映)。要件は満たすが「Ctrl-X (Shutdown) 押下から数秒遅延しうる」点はドキュメント or follow-up 検討の余地。

### Nits
- `crates/pod/src/controller.rs:117-135` `finish_controller_run` の `new_status == PodStatus::Busy` 比較は2回出る。enum 拡張時の保守性のため、ローカル変数 `is_busy` で1回にまとめてもよい。
- `crates/pod/src/ipc/interceptor.rs:18-22` の `tracing::warn` import 順は `cargo fmt` がやった整形だが、本チケットの差分から外せれば望ましい。

## 判断

**Approve with follow-up** — チケットの完了条件 (Running/Paused 同期、Busy の表示、starvation 解消、ユニットテスト) はすべて満たされており、設計の方向性 (PodStatus を protocol の一級型へ昇格、post-run jobs の集約) は健全。コードベースを歪めるような複雑化は見られず、範囲外の入力キューイングにも踏み込んでいない。エラー経路の Busy 表示と rustfmt 巻き込みは follow-up でフォローすればよく、blocking ではない。
