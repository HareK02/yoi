# Review: メモリ機構 Phase 2 consolidation + 整理

## 前提・要件の確認

### Trigger
- 累積ファイル数 / bytes 閾値で発火: `MemoryConfig::consolidation_threshold_files` / `consolidation_threshold_bytes` を導入し、`try_post_run_consolidate` が両方 `None` なら早期 return、いずれかが満たされれば発火。`crates/pod/src/pod.rs:1533-1537`、`crates/pod/src/pod.rs:1588-1592` で OR 条件評価。要件通り。
- compact 同期発火を持たない: `controller.rs` で extract → consolidate → compact の独立順序、compact からの相互呼び出しなし。要件通り。

### 実行主体と入力
- Phase 1 を終えた pod が consolidation Worker を spawn: `try_post_run_extract` → `try_post_run_consolidate` → `try_post_run_compact` の順。`crates/pod/src/controller.rs:397-420`（Run）/ `464-487`（Notify）/ `528-551`（Resume）。要件通り。
- 起動時スナップショットで consumed ID list 確定: `run_consolidate_once` が `list_staging_entries` で snapshot を取り、そのまま `consumed_ids` に渡している。`crates/pod/src/pod.rs:1581-1599`。要件通り。
- 入力 4 種:
  - consumed staging エントリ: `render_staging_records` が JSON 整形で全文渡し（`crates/memory/src/consolidate/input.rs:88-101`）。
  - 既存 `memory/*` 全文: `render_existing_memory_records` で summary / decisions / requests を render（`input.rs:105-123`）。
  - Knowledge 候補レポート: `KnowledgeCandidateReport::empty()` を渡しており、prompt 上は「(empty — usage metrics pipeline not populated. Do not create new Knowledge records this run.)」と明示（`input.rs:162-166`）。`tickets/memory-usage-metrics.md` 完了までは空入力という ticket §背景 の記述に合致。
  - 整理材料: `collect_tidy_hints` が Linter Warn 既存ルール（replaced chain / sources overflow / similar slug clusters）と同じ閾値で集計（`crates/memory/src/consolidate/tidy.rs:25-28`、`SOURCES_OVERFLOW_THRESHOLD = 10` / `SIMILAR_SLUG_DISTANCE = 2` を Linter と揃える明示コメントあり）。
- 既存 `knowledge/*` 全文を prompt に埋めない: `render_existing_memory_records` は `RecordKind::Knowledge` を弾いている（`input.rs:129`）。`KnowledgeQuery` ツール経由で agent が引く方針と整合。

### 渡すツール
- memory read / write / edit / memory_query / knowledge_query: 全て登録（`crates/pod/src/pod.rs:1643-1647`）。
- Linter Hook: memory tool の write/edit が pre-write 検証で `ToolError::InvalidArgument` を返す既存実装にそのまま乗せている。`docs/plan/memory.md` §書き込み経路と Linter の「LLM は通常の tool error フローで違反内容を読み、自己修正する」と整合。
- 「汎用 CRUD」: ticket は「file read / write / edit」と書いているが、`docs/plan/memory.md` §書き込み経路と Linter の更新版で memory 専用 Tool 一本化が確定している。本実装は generic ScopedFs CRUD を載せていない（worker は `Worker::new` 直接構築、Pod の Scope/汎用 tool 群を継承しない）ため、結果的に memory tool 経由のみ。これがアーキテクチャ的に正しい選択。

### 常駐注入の無効化
- ticket §常駐注入の無効化 は「Pod 起動直後に `set_resident_knowledge_injection(false)` を呼ぶ」と書いているが、本実装は Pod ではなく `llm_worker::Worker` を直接 spawn する経路（Phase 1 / compact と同一パターン）。
- `set_resident_knowledge_injection` は `Pod::prepare_system_prompt` の resident_knowledge 収集分岐（`crates/pod/src/pod.rs:559-576`）にのみ作用し、disposable Worker のシステムプロンプトは `CONSOLIDATION_SYSTEM_PROMPT` を `Worker::new(client).system_prompt(...)` で直接設定（`pod.rs:1619`）。Pod 経路を通らないため、resident-injection は構造的に発生しない。
- `pod.rs:1635-1641` のコメントで「Resident knowledge injection (`Pod::set_resident_knowledge_injection`) is a Pod-level concern; this disposable Worker is built without it by construction」と明記。`docs/plan/memory.md` §retrieval 経路 の "Phase 2 prompt には入れない" 規定とも整合。
- 結論: ticket 文面と実装が乖離しているが、要件の **意図**（resident injection を載せない / Knowledge 検索ツール経由で引かせる）は構造的に達成されている。ticket 側の記述が古い前提（Pod を立てる想定）に引きずられているだけ。

### 処理内容（統合 phase / 整理 phase）
- prompt: `crates/memory/src/consolidate/prompt.rs` で `docs/plan/memory-prompts.md` §共通原則 / §Phase 2 を縮約。`# Consolidation phase` と `# Tidy phase` の 2 セクション + 共通ルールで「1 セッション内で順に走る」縛りを明示。実装側は単一 sub-Worker 1 run で 2 phase を走らせる構造（独立 trigger なし、`docs/plan/memory.md` §整理 の方針と整合）。
- 統合 phase の sources コピー指示、`replaced_by` 運用、knowledge 新規作成 gate（候補レポート空時の作成禁止）、整理 phase の評価カテゴリ 4 種、保護閾値の保守 default 動作 — すべて prompt に明示。
- 完了条件「整理 phase が同じ agent セッション内で続けて走る」: prompt 縛りで担保。実装での機械的 2 phase 強制はない（単一 Worker run）。`docs/plan/memory.md` §整理「同じ Worker 同じツール surface で済むため、別 Agent / 別 spawn 経路は設けない」と整合。

### 並走防止
- staging 配下に `_staging/.consolidation.lock`（JSON: `pid` / `pod_name` / `started_at` / `consumed_ids`）。`crates/memory/src/consolidate/lock.rs:23-35`。
- in-process は `consolidation_in_flight: AtomicBool` の CAS（`pod.rs:1540-1546`）、cross-process は `kill(pid, 0)` 判定（`lock.rs:170-191`、`pid == 0` と `pid > i32::MAX` を死亡扱い）。2 段防御。
- stale lock は `kill` で ESRCH なら上書き取得、JSON parse 失敗も stale 扱い（`lock.rs:97-113`）。
- cleanup は consumed ID list のみ削除、追加分は残す（`release_with_cleanup`、`lock.rs:130-147`）。失敗時は `release_only` で staging を保全（`pod.rs:1660-1664`）。要件「実行中に Phase 1 が追加した staging は触らず」を達成。
- Coalesce: `try_post_run_consolidate` の loop で `Completed` 後に閾値再評価。Phase 1 の `try_post_run_extract` と同パターン（`pod.rs:1539-1565`）。

### モデル / 設定
- `memory.consolidation_model`（reasoning 系）: `MemoryConfig::consolidation_model` 追加、`build_consolidator_client` で `provider::build_client` 経由構築。`pod.rs:1504-1514`。
- `consolidation_worker_max_input_tokens`: default 80,000（`MEMORY_CONSOLIDATION_WORKER_MAX_INPUT_TOKENS`）。compact の 50,000 / extract の 30,000 と比べて妥当な reasoning 寄り上限。

## アーキテクチャ・スコープ

- **layer 配置**: 助力モジュール群（prompt / staging / lock / tidy / input）は `crates/memory/src/consolidate/` に集約、spawn 経路（Worker 構築 / interceptor / 在荷フラグ / decision enum）は `crates/pod/src/pod.rs` 内 inline。Phase 1 の `extract` モジュール + `try_post_run_extract` と完全に対称な分離で、低レベル基盤と Pod-level orchestration の境界が明快。`feedback_llm_worker_scope` の指針に整合。
- **依存追加**: `cargo add` で memory に `libc = "0.2.186"` / pod に `libc = "0.2.185"` および `uuid` を追加。手動編集ではない（`feedback_cargo_add` 準拠）。ただし libc バージョンが微妙に異なる（後述、Nit）。
- **module split**: pod.rs は既に肥大化しているが、本チケット追加分（约 200 行）は `try_post_run_extract` / `run_extract_once` と並行配置で、構造的不整合を増やさない。`MemoryExtractWorkerInterceptor` と並べて `MemoryConsolidationWorkerInterceptor` を separate struct で定義しているのも一貫。
- **prompt 配置**: prompt は memory crate 側 (`consolidate/prompt.rs`)。compact prompt は `pod` crate 側 / extract prompt は memory crate 側（`extract::EXTRACT_SYSTEM_PROMPT`）と既存実装で揺れているが、**memory プロンプト系統は memory crate 側に揃える** 方向で extract と統一されており、Phase 2 もそれに従っている。
- **テスト分離**: 各モジュール inline `#[cfg(test)]`（lock / staging / tidy / input）+ pod 統合テスト `consolidation_test.rs` の 2 段で、layer ごとに責務切り分けあり。
- **過剰実装の有無**: 5 つの helper module は最小構成。daemon 化や DB 化など `docs/plan/memory.md` §将来検討 に列挙された未確定項目は手を出していない。`KnowledgeCandidateReport::empty()` placeholder も usage-metrics チケット完成までの埋め草として最小。

## 指摘事項

### Blocking

なし。ticket §要件 / §完了条件 の主要項目は満たされ、build / test も全緑。

### Non-blocking / Follow-up

- **`Method::PodEvent` 経路のみ consolidate が抜けている**: `crates/pod/src/controller.rs:635-650` は `pod.run_for_notification()` を介して child pod event 由来の auto-kick turn の post-run hook を持つが、この箇所は `try_post_run_extract` → `try_post_run_compact` のみで `try_post_run_consolidate` を呼ばない（他 3 経路 `Method::Run` `Method::Notify` `Method::Resume` は揃っている）。事前討議で「Method::Run と Method::Notify」と表現されたが、Resume が wired されている事実から考えると PodEvent も同様に追加するのが対称的。子 Pod の event だけで生きているような pod が長時間 Phase 2 を踏まなくなるが、次の手動 Run / Notify で吸収される性質のため Blocking ではない。次回 PR で 4 経路統一推奨。
- **in-process 重複起動防止のテスト不在**: `consolidation_in_flight: AtomicBool` の CAS は試験コードでは触られていない。`live_lock_held_by_other_pod_skips` は cross-process（lock file）側のみカバー。Phase 1 (`extract_in_flight`) でも同様のギャップなので、本チケットだけの責務ではないが、ticket 完了条件「並走防止ファイルが想定通り機能し、複数 Phase 2 の重複起動が防げる」の **重複起動** 部分は in-process 同時呼び出し（再入）への耐性も含むと読めるため、後追いで concurrent test を 1 件足すと安心。
- **Coalesce loop の test 不在**: `Completed` → 再評価 → `Skipped` の round-trip を試すテストがない。`fires_on_threshold_and_cleans_up_consumed_entries` は 1 回 fire の cleanup だけ確認。loop 自体は Phase 1 と同パターンなので回帰確率は低いが、`release_with_cleanup` の後に再 snapshot して staging が残っていれば 2 周目を発火するという ticket §並走防止 の Coalesce 期待値を直接見てはいない。

### Nits

- `memory` crate の `libc = "0.2.186"`、`pod` crate の `libc = "0.2.185"` でパッチ番号が揃っていない。Cargo は semver 互換で 0.2.186 に解決するため動作上の問題はないが、いずれか片方を追加した時点でもう一方も合わせると依存ロックが綺麗。
- `crates/memory/src/consolidate/lock.rs` の `pid_is_alive` で `kill(0, ...)` / `kill(-1, ...)` を弾く防御コメントが正確で良い。`u32::MAX` を死亡扱いするテスト（`acquire_overwrites_stale_lock`）も合理。
- `tidy.rs` の `cluster_similar` で union-find を再実装している。Linter 側の `similar_slugs` と同じアルゴリズムなので、将来は共通化候補（ただし Linter 側はパス引数や報告型が違うため引き剥がしのコストもあり、本チケット範囲では現状維持で問題ない）。

## 判断

**Approve with follow-up** — ticket §要件 / §完了条件 の主要項目はすべて satisfied。アーキテクチャは Phase 1 / compact の既存パターンと完全対称で、`docs/plan/memory.md` §自動化メカニズム / §並走防止 / §整理 のセマンティクスを破っていない。`set_resident_knowledge_injection` の ticket 文面 vs 実装の divergence は、コード側コメントで明示されており、結果的に意図（Phase 2 prompt に常駐 injection を載せない）を達成しているため許容範囲。`Method::PodEvent` 経路の consolidate 抜けと、in-process 並走防止 / Coalesce loop の test 不在は次回タスクで詰める価値あり。

## 参考: 確認した関連箇所

- `tickets/memory-phase2-consolidation.md`
- `docs/plan/memory.md` §自動化メカニズム / §整理（GC 相当）の扱い / §Compact との関係
- `docs/plan/memory-prompts.md` §共通原則 / §Phase 2
- `crates/memory/src/consolidate/{mod,prompt,staging,lock,tidy,input}.rs`
- `crates/memory/src/lib.rs`
- `crates/pod/src/pod.rs` （`try_post_run_consolidate` / `run_consolidate_once` / `build_consolidator_client` / `MemoryConsolidationWorkerInterceptor` / `ConsolidateDecision` / `PodError::ConsolidationLock`）
- `crates/pod/src/controller.rs` 4 つの post-run 分岐
- `crates/pod/tests/consolidation_test.rs`
- `crates/manifest/src/{lib,config,defaults}.rs`
- `crates/memory/Cargo.toml`、`crates/pod/Cargo.toml`
