# Pod: memory の post-run 同期実行を解消し、Busy 状態を撤廃する

## 背景

`docs/plan/memory.md` の memory 機構（Phase 1 extract / Phase 2 consolidate）は **設計時点から非同期実行を前提** にしている。doc から読める設計意図は以下:

- **Phase 1 trigger は「activity tokens の累積閾値」**（§Phase 1）。post-run 固定ではなく、閾値超過がイベント。
- **Phase 2 は明示的に独立 trigger**: 「Phase 2 を compact に同期させる義務はなく、staging 累積閾値で独立に発火する」（§Compact との関係）。
- **並走防止は doc レベルで設計済み**: extract は `extract_in_flight` フラグ、consolidate は staging 配下の `StagingLock` + `consolidation_in_flight`（§Phase 1 並走防止 / §Phase 2 並走防止）。inline `.await` でなくても排他は壊れない。
- **将来検討に「Phase 2 を担う常駐 daemon 化」が明記**（§将来検討）。最初から Pod プロセス外でも動く前提で設計されている。

ところが現状の `crates/pod/src/controller.rs::run_post_run_jobs` は extract → consolidate → compact を逐次 `.await` しており、その間 controller task は `PodStatus::Busy` を発行してターン受付を止める。これは doc の非同期設計と乖離した実装由来の挙動で、Busy という Pod status が存在することそのものが **memory の設計を素直に実装していれば原理的に発生しない** 状態を可視化している。

加えて compact も post-run 直後に inline 実行する必然性は薄い。compact threshold は次ターン開始時の冒頭で評価しても要件を満たせる（compact 後の history で次ターンを始めるという順序が崩れない）。compact を Running の一部として組み込めば、post-run に controller が握りこむ時間そのものがゼロになり、Busy 状態は丸ごと不要になる。

要するに **`PodStatus::Busy` は現状の post-run inline 実装の副作用としてしか存在しておらず、設計レベルで必要な状態ではない**。本チケットは memory を doc の設計通り非同期化し、その帰結として Busy を enum から撤廃する。

## 要件

### Phase 1（extract）の detach

- post-run 経路では「閾値判定 → 超えていれば detached spawn」までで controller を解放する。
- spawn 先は Pod が所有する単一の background task。同 Pod 内での並走は引き続き `extract_in_flight` AtomicBool の CAS で skip（既存挙動維持）。
- compact との順序要件（extract が compact より前）は崩さない。compact を次ターン冒頭に移すなら、その冒頭で「現在 in-flight な extract があれば完了を待つ」一段の同期を挟む。

### Phase 2（consolidate）の独立化

- controller の post-run チェーンから完全に外し、staging 変化を契機とする独立タスクへ移す。
- 監視の具体仕様（変更 watch / 一定間隔のポーリング / Pod プロセス内のどこに常駐させるか）は実装で詰める。doc §並走防止 の `StagingLock` がそのまま生きる。

### compact の post-run 切り離し

- compact threshold 判定と実行を post-run から **次ターン開始の冒頭** に移す。compact は Running 状態の前段として走る（ユーザーから見れば Run を投げた直後にコンパクションが入って通常ターンが続く）。
- これにより post-run の controller 直列処理がゼロ化する。

### `PodStatus::Busy` の撤廃

- `crates/protocol/src/lib.rs` の enum から `Busy` を削除する。
- 現状 Busy を観測している経路（TUI 表示、external observer、tests、`finish_controller_run` の分岐）を `Idle / Running / Paused` のみに整理する。
- compact 中の表示文言が必要なら `Running` 内のサブ表示で TUI が出す（status enum を増やさない）。

### shutdown 時の挙動

- shutdown 時は in-flight な extract / consolidate / compact が完了するまで待ってから終了する。途中放棄しない（staging への半端書き込みや history 半端書き換えを避けるため）。
- 中断したい場合の方針（cancel 経路）は本チケット範囲外。

## 範囲外

- consolidate の常駐 daemon 化（doc §将来検討）。本チケットは Pod プロセス内の独立タスクまで。
- `extract_in_flight` / `StagingLock` 仕様変更。既存の並走防止機構をそのまま使う。
- 使用頻度メトリクス（別チケット `tickets/memory-usage-metrics.md`）。
- compact 自体のアルゴリズム変更。実行タイミングを post-run から次ターン冒頭に移すのみ。

## 完了条件

- post-run で controller がブロックする時間がほぼ消える（spawn のセットアップのみ）。
- `PodStatus` enum から `Busy` が削除され、Pod の status 遷移は `Idle / Running / Paused` のみで完結する。
- compact threshold は次ターン冒頭で判定・実行され、post-run 経路から消えている。
- extract / consolidate は閾値超過で独立に発火し、Pod の `Method` 受付に直接影響しない。
- shutdown で in-flight ジョブを待ち切ってから終了する。
- doc（`memory.md`）と実装が「memory は非同期」で整合する。

## 参照

- `docs/plan/memory.md` §Phase 1 / §Phase 2 / §Compact との関係 / §並走防止 / §将来検討
- `crates/pod/src/controller.rs` `run_post_run_jobs` / `finish_controller_run`
- `crates/pod/src/pod.rs` `try_post_run_extract` / `try_post_run_consolidate` / `try_post_run_compact`
- `crates/protocol/src/lib.rs` `PodStatus` 定義

## Review

- 状態: Approve
- レビュー詳細: [./pod-post-run-detach.review.md](./pod-post-run-detach.review.md)
- 日付: 2026-05-04 (Round 3)
