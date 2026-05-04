# Review: Pod: memory の post-run 同期実行を解消し、Busy 状態を撤廃する

## 前提・要件の確認

- **Phase 1 (extract) の detach**: post-run から detached spawn に移行。`crates/pod/src/pod.rs:236` `spawn_post_run_memory_jobs` で `tokio::spawn` 経由に切り替わり、`extract_in_flight` の CAS は `try_post_run_extract` 内 (`crates/pod/src/pod.rs:1758`) でそのまま維持されている。
- **Phase 2 (consolidate) の独立化**: controller の post-run チェーンから外れ、spawn したタスク内で extract 完了直後に呼ばれる (`crates/pod/src/pod.rs:253`)。Phase 1 直後の連鎖実行は「staging 変化を契機とした独立タスク」の最小実装として doc 要件と整合する。
- **compact の post-run 切り離し**: 旧 `try_post_run_compact` が `try_pre_run_compact` にリネームされ、`run` / `resume` / `run_for_notification` 冒頭の `ensure_session_head` 直後で呼ばれる (`crates/pod/src/pod.rs:906, 1080, 1097`)。doc (`docs/compaction.md`) も同期して更新済み。
- **`PodStatus::Busy` 撤廃**: `crates/protocol/src/lib.rs:440` で enum から削除。controller / TUI / tests から Busy 経路が削除されている。
- **shutdown 時の wait**: `pod.wait_for_memory_jobs()` がメインループ break 後に呼ばれ (`crates/pod/src/controller.rs:684`)、in-flight な extract / consolidate を join する。compact は inline 実行のため、turn 完了で必ず終わっており別途待つ必要なし。

未達の要件:
- **compact 開始時に in-flight extract を待つ同期**（ticket 要件「compact を次ターン冒頭に移すなら、その冒頭で『現在 in-flight な extract があれば完了を待つ』一段の同期を挟む」）が `try_pre_run_compact` に組み込まれていない。`crates/pod/src/pod.rs:1239` を見ると compact_state チェック直後に `compact()` を呼ぶだけで、`extract_in_flight` も `memory_task` join もない。

## アーキテクチャ・スコープ

- レイヤ境界・クレート命名・依存追加方法はチケット範囲内で逸脱なし。
- `crates/llm-worker/src/llm_client/client.rs` への `impl Clone for Box<dyn LlmClient>` 追加は、Pod を `clone_for_memory_task` で複製するために必要となった補助。スコープ境界としては「LLM 抽象に Clone を許す」付随変更で、低レベル基盤の責務範囲内であり許容範囲。
- `crates/tools/src/task.rs` は doc コメントの API 名変更追従のみ。
- `Pod::clone_for_memory_task` で「memory ジョブ用に Pod をまるごと複製してデタッチ」というアプローチを取ったことが、本実装の最大の構造的判断。これは下記 Blocking で挙げる持続性の問題を生んでいる: **session-store 側に並走する writer を入れても安全な排他層が無い**ため、controller と memory タスクが同一 jsonl に同時に追記する設計はアーキテクチャ的に破綻している。

## 指摘事項

### Blocking

- **memory タスクと controller が同一 session log に競合書き込みする (head_hash race)** — `crates/pod/src/pod.rs:193` `clone_for_memory_task` は `session_id` を値で、`head_hash` を `Option<EntryHash>::clone()` で持ち出すだけで、`store` (FsStore は `append` モード書き) の排他は何もない。`run_extract_once` 内 `session_store::save_extension` (`crates/pod/src/pod.rs:1911`) は cloned Pod 自身の `head_hash` を進めるが、controller 側の `head_hash` は据え置きのまま。
  - 帰結 1: memory タスクが完了して save_extension を書いた直後に次ターンを始めると、`ensure_session_head` の `ensure_head_or_fork` (`crates/session-store/src/session.rs:117`) が「store_head ≠ self.head_hash」を検知して **毎回 spurious fork** が走る。意図しないセッション ID 切り替わりが extract 完了ごとに発生する重大な機能リグレッション。
  - 帰結 2: 同じターン内で controller が `save_user_input` / `save_delta` を書いた後に memory タスクが save_extension を書くと、両者とも stale な `prev_hash: X` を載せたエントリが jsonl に並ぶ (chain が分岐する)。`collect_state` は線形読みなのでクラッシュは起きないが、`read_head_hash` が直前の Extension の hash を返すため次ターン頭で fork が起きる。
  - 帰結 3: fork した側 (新 session) には Extension エントリが残らないため、resume 時に extract pointer が None で復元される。staging との整合は壊れていなくても、復元した Pod は無駄に entry 0 から再走査する。
  - 旧実装が inline `.await` だったときは、save_extension が完了してから次ターンが始まるので head_hash が常に進んだ状態で観測でき、この race は構造的に発生しなかった。detach するなら head_hash / session_id の所有権を 1 本化する (memory タスクから writeback、または extract の永続化を別経路に切り出す) 等の対応が必要。

- **compact 開始時に in-flight extract を待つ同期がない** — チケット要件 §Phase 1 detach 末尾に明記された「compact を次ターン冒頭に移すなら、その冒頭で『現在 in-flight な extract があれば完了を待つ』一段の同期」が未実装。`crates/pod/src/pod.rs:1239` の `try_pre_run_compact` は `extract_in_flight` も `memory_task` も観測しない。
  - extract が走っている間に compact がセッションを fork すると、extract が `save_extension` を書く先は旧セッション側になり、共有 `Arc<Mutex<extract_pointer>>` の値は新セッションに対して意味をなさない `processed_through_history_len` を持つことになる。
  - 単純対処は `try_pre_run_compact` の頭で `wait_for_memory_jobs` 相当 (もしくは `extract_in_flight` の spinning wait) を入れること。chickeness よりは、clone-detach 設計そのものを見直したほうが早い可能性がある。

### Non-blocking / Follow-up

- `crates/tui/src/app.rs:54, 92, 114, 299` および `crates/tui/src/ui.rs:880` に `App::busy: bool` が残っている。Busy 撤廃後は常時 `false` が代入されるだけのデッドフィールドで、`submit_input` の guard と ui の `busy` 表示分岐は到達不能。チケット §`PodStatus::Busy` の撤廃 で「Busy を観測している経路を整理する」が要件に挙がっているので、この残骸も併せて消す方針が筋。
- `crates/pod/src/pod.rs:236` `spawn_post_run_memory_jobs` の二段 `if` は読みづらい。`is_finished` 後の `handle.abort()` は no-op だがロジックが冗長。`if let Some(handle) = self.memory_task.take_if(|h| h.is_finished()) { drop(handle); }` 相当に整理できる。
- `clone_for_memory_task` で `worker.set_cache_key(Some(self.session_id.to_string()))` を埋めているが、もしこの後 compact が新 session_id を割り当てると memory タスク側の cache_key と合わなくなる。prompt cache 効率の観点では現状でも問題ないが、上記 Blocking 修正と一緒に再評価したい。
- `crates/pod/src/pod.rs:248` で worker の system_prompt を `unwrap_or_default().to_string()` しているのは、抽出 Worker 側で別 prompt を貼り直すから問題はないが、`expect("worker present")` と組み合わさると意図が読みづらい。コメントで「extract worker は `try_post_run_extract` 内で Worker を使い捨て生成するため、ここでの worker は実は使われない」と明示しておくと将来の事故が防げる。
- detached path のテストが無い。要件に直結する観点 (Busy が観測されないこと、shutdown が wait すること、extract 完了後に save_extension が走っても次ターンが fork しないこと) にカバレッジが無いので、Blocking を直したら回帰テストとして追加したい。
- TODO.md に追加された `tui-context-usage-indicator` 行と新規 `tickets/tui-context-usage-indicator.md` は本チケットの diff に紛れているが、別件。コミット切り分けの確認推奨。

### Nits

- `crates/pod/src/pod.rs:1735-1738` のコメントに「Called by the Controller before spawning the background memory task」とあるが、実際に呼んでいるのは `spawn_post_run_memory_jobs` 内の spawn 後タスク。文言が紛らわしい。
- `crates/pod/src/pod.rs:1232-1238` のドックで「Best-effort: failures are logged and surfaced, but do not abort the user turn that triggered the check」とあるが、関数自体は `Result<(), PodError>` を返し常に `Ok(())`。コメントと整合。問題はないが、`run` 側で `?` を付けている (`crates/pod/src/pod.rs:906`) ので将来 `Err` を返すようになった瞬間にユーザーターンを abort してしまう。コメントの宣言通り `Ok` 確定なら戻り値は `()` でよい。

## 判断

**Request changes** — Busy 撤廃 / detach / compact 前倒しという外形は要件通りだが、(1) cloned Pod が共有 `head_hash` 抜きで session log に直書きする構造的レース、(2) compact 前に in-flight extract を待つ同期の欠落、の 2 点が要件・運用上の正しさを破っている。とくに (1) は extract が完了するたびに spurious session fork を引き起こす機能リグレッションで、merge 前の修正が必須。

---

## Round 2 (再レビュー)

### 前提・要件の確認 (差分)

- **Blocking 1 (head_hash race) 解消**: `Pod` から個別 `head_hash` フィールドを削除し、`SessionHead { session_id, head_hash }` を `Arc<AsyncMutex<_>>` で controller / cloned-memory-task 双方が共有する構造に変更 (`crates/pod/src/pod.rs:39-43, 71, 306-309`)。`save_user_input` / `save_delta` / `save_turn_end` / `save_usage` / `save_run_completed` / `save_run_errored` / `save_pod_scope` / `save_extension` / `record_metric` が全て同じ mutex 越しに head_hash を読み書きする (`crates/pod/src/pod.rs:441-447, 468-475, 540-548, 1207-1236, 1378, 1398-1407, 1422-1438, 1445-1457, 1467-1478, 2014-2024`)。compact (`crates/pod/src/pod.rs:1703-1736`) も lock 内で session_id 切り替えを行う。`extract_pointer` も `Arc<Mutex<_>>` 化されてコピー先と共有 (`crates/pod/src/pod.rs:182-187, 247`)。これで cloned Pod の `save_extension` が書いた head_hash は controller 側にも即座に反映され、`ensure_head_or_fork` での spurious fork は構造的に発生しない。設計上の race は解消。
- **Blocking 2 (pre-run extract wait) 実装**: `run` / `run_for_notification` / `resume` の冒頭で `compact_state.exceeds_post_run(...)` を満たす場合のみ `memory_task.take().await` で in-flight ジョブを join してから `try_pre_run_compact` へ進む (`crates/pod/src/pod.rs:926-946, 1123-1143, 1159-1179`)。閾値を超えていない通常ターンでは join せず detach のままなので、要件の「compact 直前にだけ wait」が成立。`memory_task` は `JoinHandle<()>` 一本で持たれ、必ず完了する (内部に LLM 呼び出しタイムアウトは Worker 側に委ねるが、追加の無限ループは無い) ので `await` が永久 block するパスは見えない。
- **デッドコード掃除**: `App::busy` フィールドおよびその参照 (`submit_input` の早期 return / `ui::draw_status` の busy 表示 / `handle_key` Ctrl-X 分岐) を完全削除 (`crates/tui/src/app.rs`, `crates/tui/src/ui.rs`, `crates/tui/src/main.rs`)。`grep PodStatus::Busy` も TUI / Pod / protocol 全域で commentary 1 件のみ (`crates/pod/src/pod.rs:1325` のコメント、過去経緯の説明として妥当)。
- **テスト追加**: `controller_test.rs` の `run_end_enters_busy_until_post_run_finishes_and_broadcasts_status` を `run_end_returns_to_idle_without_busy_status` にリネーム + 期待値変更。`pause_while_busy_is_idempotent_not_not_running` を削除 (Busy が無いので意味を失った)。`compact_events_test.rs` は `try_post_run_compact` → `try_pre_run_compact` 変名追従のみ。**detach 経路に固有のテスト (spawn 後の次ターンが fork しないこと、shutdown で memory_task が join されること、pre-run の extract 待ち合わせ) は依然として未追加**。

### 残っている指摘事項

#### Blocking
- なし。前回ラウンドの両 Blocking は解消。

#### Non-blocking / Follow-up

- **detach 経路の動作テストが未追加**: 前回 follow-up に挙げた以下が引き続き未カバー。Round 1 では Blocking に隠れていたが、今回は単独の品質課題として残る。
  - 同 turn 内の `save_extension` と次ターンの `save_user_input` が同じ jsonl に並ぶケースで fork しないこと (`session_head` mutex の serialization が機能していること)。
  - `wait_for_memory_jobs` が shutdown 時に in-flight な extract / consolidate を join し切ること。
  - `compact_state.exceeds_post_run(...)` を満たす状態で memory_task が走行中に次ターンを投げると、compact が memory_task の完了を待ってから走ること。
  - 推奨は `crates/pod/tests/controller_test.rs` に `MockClient` を遅延つきで返す形のテストを 1〜2 本追加。
- **`run` / `run_for_notification` / `resume` で同じ memory_task 整理 + pre-compact wait ブロックが 3 回コピペされている** (`crates/pod/src/pod.rs:926-946, 1123-1143, 1159-1179`)。ヘルパ (`async fn ensure_pre_run_state(&mut self) -> Result<(), PodError>` 等) に 1 本化したほうが、将来 wait 条件を変更したときの抜け漏れを防げる。
- **`spawn_post_run_memory_jobs` の二段 `if` が依然冗長** (`crates/pod/src/pod.rs:254-264`)。「未完了なら何もしない、完了済みなら take して drop してから新しく spawn」という意図を `take_if(|h| h.is_finished())` 等で 1 段に書ける。Round 1 の指摘そのまま。
- **`clone_for_memory_task` の `worker.set_cache_key(Some(self.session_id.to_string()))`** (`crates/pod/src/pod.rs:216`) は clone 時点の session_id を貼っている。compact が走って session_id が変わる前に memory_task は join されるよう pre-run wait が入ったので race にはならないが、cloned worker は extract worker 用に作っているだけで実体は `run_extract_once` 内で `extract_worker = Worker::new(...).system_prompt(...)` (`crates/pod/src/pod.rs:1956`) として作り直される。**したがって cloned Pod の `worker` は実際には使われていない**。`clone_for_memory_task` で `worker` をビルドしている処理 (`crates/pod/src/pod.rs:211-216`) はメモリ・初期化コストの無駄であり、`Option::None` で十分か、せめてコメントで「使われない / placeholder」と明示すべき。
- **`pending_attachments` を空 Arc で再生成、`pending_scope_snapshot` は共有という非対称** (`crates/pod/src/pod.rs:237, 244`)。memory タスクは scope 変更も attachment も生まないので実害は無いが、意図がコメントに無いと将来事故る。
- **スコープ外混入が継続**: `TODO.md` の `tui-context-usage-indicator` 行追加と `tickets/tui-context-usage-indicator.md` 新規が本チケットの未コミット差分に依然として残っている。前回 follow-up でコミット切り分けを指摘済みだが解消されていない。コミット時に別チケットとして分離する判断を再度推奨。

#### Nits

- `crates/pod/src/pod.rs:1834-1839` の doc コメント「Called by the Controller before spawning the background memory task」は誤り。実際に呼んでいるのは `spawn_post_run_memory_jobs` が `tokio::spawn` した async block の内部 (`crates/pod/src/pod.rs:268`)。「Spawned background memory task が compact 確定前に呼び出す Phase 1 entry point」程度の文言に。
- `crates/pod/src/pod.rs:1322-1328` `try_pre_run_compact` の戻り値は依然 `Result<(), PodError>` だが本体は `Ok(())` 確定。`run` 側 (`crates/pod/src/pod.rs:946, 1143, 1179`) で `?` で受けているので、将来 `Err` を返すようになった瞬間にユーザーターンを abort してしまう。「best-effort で abort しない」という設計意図と整合させるなら戻り値を `()` にするか、本体側で必ず `Ok(())` に正規化するコメントを残すか。
- `crates/pod/src/pod.rs:253` 直後の空白 2 行 + 関数定義は 1 行多い。

### Round 2 判断

**Approve with follow-up** — 前回ラウンドの 2 件の Blocking はいずれも構造的に解消されている。`SessionHead` の `Arc<AsyncMutex<_>>` 共有は cloned Pod / controller 双方の head_hash / session_id 進行を直列化し、ticket 要件の「detach しても session log の整合性を壊さない」を満たす。compact 前 wait も threshold 一致時にだけ join する形で必要十分。残課題は (a) detach 経路の回帰テスト追加、(b) `run` / `resume` / `run_for_notification` のコピペ整理、(c) スコープ外コミットの切り分け、で、いずれも本チケットを閉じてから別チケット ないし follow-up commit として処理可能。

---

## Round 3 (再レビュー)

### 前提・要件の確認 (差分)

Round 2 の follow-up 6 点に対する解消状況を確認した。

- **#1 detach 経路の固有テスト追加**: `crates/pod/tests/compact_events_test.rs:528-581` に 2 本追加。
  - `spawn_and_wait_drives_extract_to_completion` (line 529) — `pod.spawn_post_run_memory_jobs() + wait_for_memory_jobs().await` の round-trip で `extract_pointer` が `None → Some` に進むことを assert。`spawn`/`wait` の API contract と extract の完了条件をカバー。
  - `detached_extract_does_not_fork_session_log` (line 553) — Round 1 Blocking 1 の構造的不変条件 (`SessionHead` 共有が `save_extension` と `save_user_input` の chain を直列化する) を行動レベルで検証。`session_id_before == session_id_after` を比較するので、cloned Pod が独立 head_hash を持っていた場合の `ensure_head_or_fork` 経由の spurious fork を検知できる。
  - 残りの 2 観点 (shutdown wait の正常完了 / pre-run compact の memory_task 待ち合わせのタイミング) は本ラウンドでも未追加だが、ユーザーのコメントで「テスト容易性の限界」として明示されている。後述の判断参照。
- **#2 ヘルパ集約**: `prepare_for_run` (`crates/pod/src/pod.rs:938-948`) に 1 本化。`run` (line 963)、`run_for_notification` (line 1138)、`resume` (line 1152) の prelude が全て `self.prepare_for_run().await?;` の 1 行で済むように整理。同期順序は「`ensure_interceptor_installed` → `ensure_system_prompt_materialized` → `cleanup_finished_memory_task` → `ensure_session_head` → (compact 必要時のみ `join_memory_task`) → `try_pre_run_compact`」(`crates/pod/src/pod.rs:939-947`) で、Round 2 で個別に書かれていた順序と意味的に等価。`cleanup_finished_memory_task` を `ensure_session_head` の前に移したのは finished handle を早めに drop するだけで semantics に影響なし。
- **#3 `clone_for_memory_task` の worker 縮約**: `set_system_prompt` / `set_request_config` / `set_cache_key` の copy をすべて削除し、`Worker::new(client.clone()).set_history(...)` の最小 snapshot に縮約 (`crates/pod/src/pod.rs:216-218`)。コメント (line 210-215) で「extract/consolidate は内部で worker を作り直すので system_prompt / request_config / cache_key は不要」と意図を明示。`run_extract_once` 内 (line 1922) で `extract_worker = Worker::new(client).system_prompt(extract_system_prompt)` として作り直されるという既存挙動と整合する。
- **#4 `spawn_post_run_memory_jobs` のフラット化**: 二段 `if` を `cleanup_finished_memory_task() + if self.memory_task.is_some() { return; }` (`crates/pod/src/pod.rs:259-262`) に整理。`take_if` での 1 段化案より「先に finished を掃除 → in-flight があれば skip」という 2 ステップの構造が読み取りやすく、結果的に意図がはっきりした。
- **#5 戻り値整理**: `try_pre_run_compact` (`crates/pod/src/pod.rs:1296`) と `wait_for_memory_jobs` (`crates/pod/src/pod.rs:199`) の戻り値を `()` に変更。`prepare_for_run` 内 (line 946) と controller の shutdown 直前 (`crates/pod/src/controller.rs:684`) の呼び出し側からも `?` / `if let Err(...)` が消えた。「best-effort で user turn を abort しない」設計意図がシグネチャレベルで保証される。
- **#6 スコープ外混入の切り分け**: 既コミット (`632d63d docs(tickets): 追加：タスクリストの表示とコンテキスト長インジケータ`) で `tui-context-usage-indicator.md` / `tui-task-display.md` および `TODO.md` 行が別チケットとして分離済み。本チケットの差分は本体実装 + 本 review.md + ticket 末尾の Review 状態のみ。

### 動作確認

- `cargo check --workspace --tests` clean (warning 1 件は既存の `end_scope` dead-code、本チケット無関係)。
- `cargo test -p pod --test compact_events_test` 8 本全て pass (新規 2 本含む)。
- `cargo test -p pod --test controller_test` 21 本全て pass (renamed `run_end_returns_to_idle_without_busy_status` 含む)。
- ユーザー報告の `cargo test --workspace` 50 group pass を裏取り済み。

### 残っている指摘事項

#### Blocking
- なし。

#### Non-blocking / Follow-up
- **未追加の detach テスト 2 観点 (shutdown wait の正常完了 / pre-run compact の memory_task 待ち合わせ)**: ユーザー側で「テスト容易性の限界による断念」として明示。前者は `controller` task の終了パス (`crates/pod/src/controller.rs:684`) を `wait_for_memory_jobs` の代入前後で観測する必要があるが、現在の `controller_test.rs` フレームには in-flight job を確実に「shutdown 時に未完了」状態に置く手段がない (MockClient が同期に response を返すため)。後者は `compact_state` 閾値を超えた状態で memory_task が走行中という条件をテスト 1 本で再現する必要があり、`MockClient` のレスポンス遅延フックがない現状では確定的に作れない。**判断**: 構造的不変条件 (`SessionHead` mutex / `extract_in_flight` CAS) は Round 2 で型レベル + 既存テストで検証済みであり、`detached_extract_does_not_fork_session_log` がその不変条件を行動レベルで cover している。残り 2 観点はテスト基盤側の課題 (e.g. `MockClient` への delay hook 追加) として `tickets/` 別件で扱える性質であり、本チケットの merge を block しない。
- **`prepare_for_run` の名前**: prelude / pre-run setup として汎用名なので将来「pre_run_X」系の helper が増えた時に衝突しうる。本チケット範囲外の整理候補として留意。
- **`should_pre_run_compact` と `try_pre_run_compact` の二重判定**: `should_pre_run_compact` は `disabled / just_compacted / exceeds_post_run` を見るが (`crates/pod/src/pod.rs:927-931`)、`try_pre_run_compact` は `disabled / just_compacted` 早期 return + `exceeds_post_run` 二度判定で同じ判断ロジックを再評価する (`crates/pod/src/pod.rs:1297-1304`)。コメント (line 922-925) で「defensive に重複している」と明示してあるので意図的だが、片方を `should_pre_run_compact()` の呼び出しに置き換える形で 1 本化しても同じ defensive 性は保てる。可読性向上の余地。

#### Nits
- `crates/pod/src/pod.rs:1802` の `try_post_run_extract` doc コメント「Called by the Controller before spawning the background memory task」が依然として残っている。Round 2 の Nits として既に指摘済み。実際の呼び出しは `spawn_post_run_memory_jobs` が tokio::spawn した async block 内 (`crates/pod/src/pod.rs:266`)。修正コストは小さい。

### Round 3 判断

**Approve** — Round 2 で挙げた follow-up #1〜#5 はすべて妥当に解消され、新規 regression は確認できない。`prepare_for_run` への集約で 3 経路の prelude 順序が単一の真理になり、将来「compact 前に何を待つか」を変更したい時の抜け漏れリスクが消えた。`clone_for_memory_task` の worker 縮約は無駄な init を削るだけでなく「memory タスクから見える worker は client + history のみが意味を持つ」という設計境界を明示している。`try_pre_run_compact` / `wait_for_memory_jobs` の `()` 返却は best-effort セマンティクスを型で保証する。新規テスト 2 本のうち `detached_extract_does_not_fork_session_log` は Round 1 Blocking 1 の構造的不変条件 (`SessionHead` 共有) を行動でカバーする実効的な assertion になっている。スコープ外混入も切り分け済み。残った Non-blocking 項目はいずれもテスト基盤拡張 / 命名整理レベルで、別チケットで個別に拾える性質。本チケットは閉じて良い。
