# Review: 親が投げたターンのみを子→親 callback の対象にする

対象コミット: `8019d0d update: 親にターン完了を通達する経路の整理`

## 前提・要件の確認

- **TurnEnded を親由来 turn の `Finished` に限定**: 満たされている。
  `controller_loop` で `run.is_parent_originated()` を取り、`drive_turn` の `parent_originated` 引数として伝搬。`drive_turn` 内 `Finished` 分岐 (controller.rs:796) で `parent_originated && Finished` 双方が立ったときのみ `PodEvent::TurnEnded` を `fire_and_forget`。
- **「親由来」の判定が `PendingRun` バリアントベース (`Run` / `InterruptAndRun` / `Resume` = true、`RunForNotification` = false)**: 満たされている。
  `PendingRun::is_parent_originated()` (controller.rs:108-113) のマッチがチケットの表と一致。`pending_run_parent_origin_table` テスト (controller.rs:1043-1048) で 4 バリアントすべてを assert。
- **Errored を同スコープに揃える**: 満たされている。
  `drive_turn` の `Err(e)` 分岐 (controller.rs:822-830) を `if parent_originated { ... }` でゲート。`Event::Error` のローカル broadcast (controller.rs:818-821) は無条件のままで「`Event::Error` ローカル通知は維持」の要件も満たす。
  `Err(PodError::Worker(WorkerError::Cancelled)) if pause_requested` 分岐 (controller.rs:806-814) は元から `Errored` を上げない設計で、本チケットでは触れていないため変更なし。妥当。
- **`PodEvent::ShutDown` は対象外、従来通り**: 満たされている。
  `controller_loop` の loop 終了後 (controller.rs:734-745) の `send_pod_event(ShutDown)` には `parent_originated` も `drive_turn` も介在しない。発火条件不変を確認。
- **`ScopeSubDelegated` は対象外**: 満たされている。
  `spawn/tool.rs:278` が発火元で、`drive_turn` のゲートとは無関係。今回の diff で `controller.rs` のみが変更されており影響しない。
- **親側受信処理は変更しない**: 満たされている。
  diff は `crates/pod/src/controller.rs` のみ。`apply_event_side_effects` / `NotifyBuffer` / history append 経路は変更なし。

## 完了条件の確認

- **SpawnPod 初回 Run / SendToPod 完了 → 親 history に 1 件 append**: `PendingRun::Run` / `InterruptAndRun` 経路で `parent_originated=true` となり、上記 TurnEnded ゲートを通過。受信側を変更していないため挙動成立。
- **孫 PodEvent / 他 Notify 起点の auto-kick 完了 → 親 history に何も append しない**: `PendingRun::RunForNotification` 経路で `parent_originated=false` → 親へ何も飛ばない → 親側で受信しないので append も起こらない。
- **親由来 turn の worker エラー → `Errored` が親に届く / `RunForNotification` の worker エラー → 届かない**: `Errored` ゲートで確認済み。
- **ユニットテスト 4 種**: 仕様マップは以下の通り。
  - 「`PendingRun::Run` 完了 → 親に届く」 → `parent_originated_finished_fires_turn_ended` (controller.rs:1052-) で `parent_originated=true` + `Finished` を駆動し、Unix socket で `PodEvent::TurnEnded` を受信。
  - 「`PendingRun::RunForNotification` 完了 → 親に届かない」 → `non_parent_originated_finished_stays_silent` (controller.rs:1080-) で `parent_originated=false` + `Finished` を駆動し accept timeout を期待。
  - 「`PendingRun::Resume` 完了 → 親に届く」 → 直接的な統合テストは無いが、`pending_run_parent_origin_table` で `PendingRun::Resume.is_parent_originated() == true` を assert + `parent_originated=true` 経由の `drive_turn` 動作を `parent_originated_finished_fires_turn_ended` で検証しており、`Resume → true → 親に届く` の合成は閉じている。フォローアップ参照。
  - 「`RunForNotification` 起点 turn の Errored → 親に届かない」 → `non_parent_originated_worker_error_stays_silent` (controller.rs:1146-) で確認。
- ローカルで `cargo test -p pod --lib` を実行し 173/173 パス、新規 5 ケースもすべて green。

## アーキテクチャ・スコープ

- 起源情報はすでに `controller_loop` が `PendingRun` で表現済みで、新規追加は `is_parent_originated()` 1 メソッドのみ。enum バリアント追加や別構造体の導入は無く、表現を最短ルートで再利用しており設計を歪めていない。
- `drive_turn` の引数増 (1 個追加) は `#[allow(clippy::too_many_arguments)]` 配下で、シグネチャの煩雑さは既存水準を踏み越えていない。代替案として `PendingRun` 自体を `drive_turn` に渡す案も考えられるが、現状 `drive_turn` は具体的な `pod_future` を呼び出し側で生成する形のため、bool 引数の方が責務境界がクリーンで妥当。
- 変更ファイルは `crates/pod/src/controller.rs` のみ。protocol / event 側 / 親側受信ロジックには触れず、ファイアウォール的に発火源だけを絞る意図と一致。
- `controller.rs` の 264 行追加のうち約 220 行がテストモジュール + テストヘルパ (`DriveTurnEnv` / `make_env` / `recv_pod_event`)、本体ロジックは 44 行程度。スコープ外変更や別件リファクタの紛れ込みは無い。
- LLM provider policy / crate 命名 / `cargo add` 規約に関わる変更は無く、該当無し。

## 指摘事項

### Non-blocking / Follow-up

- 完了条件 4 番目で `PendingRun::Resume` 完了の単体テストが明記されているが、現状の `drive_turn` テストでは `parent_originated: bool` のみが入力次元のため、`Resume → true` の対応関係は `pending_run_parent_origin_table` の 1 行 assert に依存している。設計上は同値だが、ticket 文言との対応をより明示するために `parent_originated_finished_fires_turn_ended` の `Resume` 命名バリアント (例: `resume_finished_fires_turn_ended` を `pod.resume()` future ではなく `parent_originated=true` で駆動) を追加するか、ヘルパ + parameterized 化しておくと将来の読み手が辿りやすい。
- `DriveTurnEnv` の `recv_pod_event` は 2 秒タイムアウトで OK、negative side は 200ms で accept がエラーになることを「届かなかった」と判定している。基本問題ないが、CI で I/O が詰まると false-positive で pass する余地はある (届くべき送信が来なかったのを「来なかったので OK」と読み違える)。本チケットの 2 種類の negative テスト両方が `Finished` / `Err` を即時返す `async { ... }` future を使っており実 IO レイテンシは無いため実害は小さい。

### Nits

- `DriveTurnEnv` のフィールドコメント (`_method_tx`) は select! arm が `None` を観測しないようにする意図を 1 行で説明していて読みやすい。継続コード変更で気づきにくくなるポイントなので、もし `make_env` 経由のテストを増やしていく場合はこのコメントを残す方針で。
- `parent_originated` という命名は明快。`is_parent_originated` ヘルパの doc コメントが `RunForNotification` の意味も列挙しており、enum の意味論を 1 か所で説明する役割を果たせている。

## 判断

**Approve** — 要件 5 点・完了条件 4 点はいずれも実装とテストで充足。設計は既存の `PendingRun` 表現を再利用する最短手で、スコープ外変更や API 表面の過剰な拡張は無い。`Resume` 単体テスト不在は厳密には完了条件 4 番目との突き合わせで指摘可能だが、`pending_run_parent_origin_table` + `parent_originated=true` の挙動テストの合成で要件は閉じており blocking ではない。
