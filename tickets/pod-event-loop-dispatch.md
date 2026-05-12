# Pod: イベントハンドラからターン起動を分離する

## 背景

`crates/pod/src/controller.rs` の controller タスクは、 outer loop の Method ハンドラ内に **イベント処理とターン起動を同居** させている。

該当する箇所:

- `Method::Run` → outer arm 内で `run_with_cancel_support(pod.run(...)).await` と `finish_controller_run.await` を呼ぶ
- `Method::Notify` (Idle) → outer arm 内で `set_controller_status(Running).await` のあと `run_with_cancel_support(pod.run_for_notification()).await` / `finish_controller_run.await`
- `Method::PodEvent` (Idle) → 同上 (副作用処理 `apply_event_side_effects.await` も同じ arm 内)
- `Method::Resume` → 同上

ハンドラ自身が「ターンを丸ごと await する長時間処理」 を抱えているため、 以下が起きる:

- 同じターン起動コード (`set_controller_status` → `run_with_cancel_support` → `finish_controller_run`) が 4 箇所に重複する
- ターン起動の発火元 (Run / Notify / PodEvent / Resume) と起動本体が分離されていないため、 起動条件の変更が複数 arm に波及する
- ターン起動に必要な前処理 (history への user message append / NotifyBuffer への push / status 遷移) も各 arm に分散している

ターン起動を outer loop の周回トップに集約し、 各イベントハンドラは「次のターンに渡す入力をその場で確定 (history append / NotifyBuffer push) + `needs_run` フラグセット」 までに留めることで、 起動コードの重複と入力経路の分散をなくす。

加えて、 `PodController::spawn` 自体が約 615 行に膨らんでおり、「組み立て (channel/runtime_dir/Worker hook 配線/ツール登録/PodHandle 構築)」と「実行ループ (controller タスク)」が単一関数に同居している。 outer loop を上記の形に書き直すタイミングで、 spawn の構造分解も同時に行う方が、 重複コードの統合先が見やすくなる。

なお、 別途観測されている「auto-kick されたターンの内部で controller がデッドロックする現象」 はこの整理では解決しない (inner select! のアーム内 await が pending する経路は構造上残る)。 根本原因の特定は別チケットの対象。

## 要件

- イベントハンドラは **「状態更新と `needs_run` フラグ立て」 まで** にとどめる。ハンドラ内で `run_with_cancel_support.await` / `finish_controller_run.await` を呼ばない
- outer loop の各周回はまず `needs_run` を評価し、 立っていればターン起動 (`run_with_cancel_support` → `finish_controller_run`) を実行してフラグを降ろす。 立っていなければ `method_rx.recv().await` で次の Method を待つ
- 既存の inner select! によるターン中の Method 並行受信は維持する。 ターン本体の借用構造 (`&mut Pod` を Worker が抱える) も変更しない
- `needs_run` を立てる契機は最低限以下を含める (= 現状 auto-kick している経路):
  - `Method::Run`
  - `Method::Resume`
  - `Method::Notify` (Idle 時のみ)
  - `Method::PodEvent` (Idle 時のみ)
- 「次のターンの起動意図と入力」 は `needs_run` を起動意図を持つ enum として表現する:
  - `Run { input }` / `InterruptAndRun { input }` / `RunForNotification` / `Resume`
  - `Method::Notify` / `Method::PodEvent` の Idle 経路は NotifyBuffer に push したうえで `RunForNotification` を立てる (現状の `pod.run_for_notification()` が NotifyBuffer から自動取得する挙動に乗るため、 enum に入力を載せる必要はない)
- `Worker::run` / `Pod::run` の入力受け取り API には触らない (interceptor の `on_prompt_submit` cancel / 書き換え / extras ordering の invariant に踏み込まない)
- Pause / Shutdown / Cancel はハンドラ内で完結する (フラグ化しない、 既存通り即時処理)
- Running 中に来た `Method::Notify` / `Method::PodEvent` の挙動 (NotifyBuffer に積むだけ、 副作用は実行) は変えない
- 上記改修に合わせて `PodController::spawn` を以下の責務単位に分解する:
  - 初期化 (channel 群 / `RuntimeDir` / pod-immutable snapshot / `SpawnedPodRegistry` / `alerter` 装着 / bash-output scope)
  - Worker への event bridge コールバック配線 (`on_turn_*` / `on_*_block` / `on_tool_result` / `on_usage` / `on_warning` / `on_history_append` 等)
  - ツール登録 (builtin / memory / spawn orchestration)
  - 初期ファイル書き出し + `PodSharedState` / `PodHandle` 構築 + `SocketServer` 起動
  - `controller_loop` — Idle/Paused 状態の event loop + 後処理 (現 `tokio::spawn(async move { loop { ... } })` の本体)
  - `drive_turn` — Running 状態の event loop。 現 `run_with_cancel_support` を改名し、 `controller_loop` と同格の関数として並べる (「cancel support」 という実装詳細名から役割ベースの名前に改める)
- 分解後の `spawn` はこの順序で各責務を呼び出す薄いフローになる。 Idle/Paused 側 (`controller_loop`) と Running 側 (`drive_turn`) の event loop が同格の関数として並ぶ形に揃える

## 完了条件

- `Method::Run` / `Method::Resume` / `Method::Notify(Idle)` / `Method::PodEvent(Idle)` のいずれも、 ターン起動が outer loop の周回トップ 1 箇所に集約されており、 ハンドラ側にはターン起動コードが残っていない
- 各イベントハンドラの async body 内に「ターン丸ごと」 や `finish_controller_run` 等の長時間 await が無い
- 既存挙動が変わらない (どの Method で auto-kick されるか、 ターン中の Cancel / Pause / Shutdown が効くか、 NotifyBuffer に積まれた内容が次ターンで反映されるか、 ターン起動順序)
- `PodController::spawn` が責務単位の関数列を順に呼ぶ薄いフローになっており、 単一関数の中に組み立てと実行ループが同居していない
- Idle/Paused 状態の event loop (`controller_loop`) と Running 状態の event loop (`drive_turn`) が同格の関数として並んでおり、 名前が役割を表している (`run_with_cancel_support` の名前は残らない)
- `crates/pod/tests/controller_test.rs` の既存テストが通る

## 範囲外

- 観測されたデッドロックの根本原因特定 (別チケット予定)
- `Pod` 構造体の `&mut self` borrow を分解して outer loop を完全 dispatcher 化する大改修 (将来検討)
- Cancel / Shutdown の専用チャネル化 (controller が固まっている時には別経路でも効かないため、 メリット薄と判断して見送り)
- `Worker::run` / `Pod::run` の入力なし API 化 (controller の dispatch を bool に圧縮するメリットに対し、 worker interceptor invariant を壊すコストが見合わない)
