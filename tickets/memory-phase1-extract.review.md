# Review: メモリ機構 Phase 1 活動抽出

## 前提・要件の確認

- **Trigger (input tokens 累積閾値、tool call カウント不採用)**: `Pod::cumulative_input_tokens_since` (`crates/pod/src/pod.rs:1254`) で `usage_history` の `input_total_tokens` を集計し、`memory.extract_threshold` 未設定時は no-op (`pod.rs:1280`)。tool call は不参照で要件適合。閾値の単位については後述の Non-blocking 参照
- **Trigger (compact より前)**: Controller の post-run ブロック 4 箇所すべてで `try_post_run_extract` → `try_post_run_compact` の順に呼ばれる (`crates/pod/src/controller.rs:335,393,448,545`)。要件どおり
- **実行主体 (compact と同じ Worker spawn 機構を再利用、Pod は立てない)**: `run_extract_once` が `llm_worker::Worker` を直接組んで `system_prompt`/`temperature`/`max_tokens`/usage callback/interceptor を貼る構成 (`pod.rs:1378-1404`)。compact 経路 (`pod.rs:1046-1077`) と同じ素の Worker spawn パターン。Pod は立てていない
- **入出力 (前回 Phase 1 以降の session log 範囲)**: `processed_through_history_len..current_history_len` を `Worker::history()` から切り出し (`pod.rs:1369`)、`source.range = [start_entry, end_entry]` は session-store の entry index で機械付与 (`pod.rs:1417-1420`)
- **出力 schema (4 種候補配列、空配列許容、自由文不可)**: `ExtractedPayload` (`crates/memory/src/extract/payload.rs:13`) が `decisions/discussions/attempts/requests` を保持し全 default 空。EXTRACT_SYSTEM_PROMPT (`crates/memory/src/extract/prompt.rs:7`) は schema 遵守と空配列許容、自由文禁止を明示。`write_extracted` ツール 1 本のみ提供で「ツールで提出して終わる」枠組みに閉じている
- **pointer 永続化 (session-store 拡張点 `LogEntry::Extension`)**: `LogEntry::Extension { ts, domain, payload }` を新設 (`crates/session-store/src/session_log.rs:175`)、`RestoredState.extensions: Vec<(String, Value)>` を追加 (`session_log.rs:223`)、`save_extension` ヘルパー (`crates/session-store/src/session.rs:333`) を export。session-store 側に `"memory.extract"` 定数も `payload` 構造の知識も持たせていない (replay は順番に積むだけ)。fold 責務は `memory::extract::fold_pointer` (`crates/memory/src/extract/pointer.rs:26`) に閉じている。ドメイン純度の要件を満たす
- **並走防止 (`extract_in_flight: AtomicBool`、完了時再評価ループ)**: `Pod::extract_in_flight: Arc<AtomicBool>` (`pod.rs:131`) を `compare_exchange` でガード (`pod.rs:1287`)、完了時は `loop` で再評価 (`pod.rs:1289-1314`)。`pending` 状態は持たず、完了時に閾値が落ちていれば自然に Skipped で抜けるため coalesce 相当が成立。要件どおり
- **書き込み (`memory/_staging/<id>.json`、source 機械付与、UUIDv7 可)**: `write_staging` (`crates/memory/src/extract/staging.rs:40`) が UUIDv7 ファイル名で `StagingRecord { source, payload }` を pretty JSON 書き出し。`source` は Pod 側で `session_id` + `range` を付与し、LLM には推論させない (system prompt にも明示)
- **モデル設定 (`memory.extract_model` + 副次設定)**: `MemoryConfig` に `extract_model` / `extract_threshold` / `extract_worker_max_input_tokens` の 3 つを追加 (`crates/manifest/src/lib.rs:75-86`)、cascade merge も対称 (`crates/manifest/src/config.rs:215-219`)、`extract_worker_max_input_tokens` の default 定数も追加 (`crates/manifest/src/defaults.rs:50`)。`extract_model` 未設定時は main client の `clone_boxed()` にフォールバック (`pod.rs:1238-1247`)
- **prompt 要件 (`docs/plan/memory-prompts.md` §Phase 1)**: `EXTRACT_SYSTEM_PROMPT` は §共通原則 (source 推論禁止、空出力許容) と §Phase 1 (派生物作成禁止、4 種限定、自由文禁止、shallow 除外) を網羅。`#共通原則` の「既存 docs と重複保存しない」も "Do not duplicate content already captured by static project docs" として反映済み

## 完了条件の確認

- 閾値超過で発火し staging file 作成: `run_extract_once` の最後で `write_staging` → pointer save。空 payload でも pointer は前進し、staging 書き込みのみ skip という分岐が pointer.rs:19 のコメントと整合
- schema 準拠 + `source` 機械付与: `WriteExtractedTool` が `from_str::<ExtractedPayload>` で受け、Pod 側で `SourceRef` を wrap (`pod.rs:1417-1422`)。LLM 側に source は渡らない
- 抽出対象なしは空配列 / skip どちらでも可: 現実装は空配列でも skip (空時は staging file を作らず、pointer だけ前進)。仕様上どちらでも良いとある通り
- session 側 pointer 更新で続きから走る: `save_extension` で `EXTRACT_DOMAIN` 永続化 + 同期して in-memory `extract_pointer` 更新 (`pod.rs:1442-1445`)。restore 時は `fold_pointer` で最新値を取り出す (`pod.rs:254`)
- 既存 compact の動作に回帰なし: pod 141 / session-store 16 / memory 82 / manifest 75 全 pass、compact のテストは無改変

## アーキテクチャ・スコープ

- **session-store ドメイン純度**: `LogEntry::Extension` は domain 文字列を不透明に扱い、payload は `serde_json::Value` のまま積むだけ。`"memory.extract"` 定数も `ExtractPointerPayload` の構造も memory crate に閉じている。session-store 単体テスト (`session_log.rs:644-700`) も memory ドメインを知らない汎用テストになっており、設計通りの分離が取れている
- **memory crate 内のモジュール分割**: `extract/{mod,input,payload,pointer,prompt,staging,tool}.rs` で関心ごとに 1 ファイルずつ切れている。`mod.rs` の re-export と doc が綺麗。`pod.rs` に直書きせず memory 側に責務が寄っており、好ましい
- **Pod の責務**: trigger 判定・worker spawn・source 機械付与・pointer 永続化のみ。`memory::extract::*` を呼ぶだけで自分は schema/prompt/tool を知らない。範囲外（Phase 2 / staging cleanup / compact spawn 機構の共通化 / Phase 2 並走防止ファイル）には手を出していない
- **compact との並列性**: `MemoryExtractWorkerInterceptor` は `CompactWorkerInterceptor` のミラー実装。共通化していないが、ticket §範囲外で「compact Worker spawn 機構自体の拡張」を明確に除外しているので OK。ただし将来 3 個目が出るなら共通化候補
- **manifest 設定の cascade**: `MemoryConfig::merge_with_upper` に追加 3 フィールド全てを忘れず追記済み (`config.rs:215-219`)、defaults.rs に定数化、lib.rs に `Option` の意図 doc を追加とパターン遵守
- **依存追加**: `memory/Cargo.toml` に `uuid = { version = "1.23.1", features = ["v7", "serde"] }` 追加。バージョンとフォーマットは `cargo add` の出力体裁

## 指摘事項

### Blocking

- **Compact 後に extract_pointer が陳腐化して Phase 1 が止まる** — `crates/pod/src/pod.rs:1180-1216` の `compact()` は `session_id` を新しいコンパクト後セッションへ swap し `usage_history` を `clear()` しているが、`extract_pointer` (in-memory) は古いセッションの `processed_through_history_len`（典型的には 50+）を保持したまま。次回以降 `cumulative_input_tokens_since(processed_history_len)` の filter `r.history_len > history_len_pointer` は新しいセッションの低い `history_len` をすべて除外し、永久に 0 を返す。結果としてプロセス継続中に compact が走ると、その後 Phase 1 は二度と発火しない。fix は `compact()` の swap 直後（`usage_history.clear()` の隣）で `*self.extract_pointer.lock() = None` を行うのが最小。新セッション側の log には Extension entry がまだ無いので、cold restore でも `fold_pointer = None` になっており、cold restore と挙動を一致させられる
  - **対応済み (2026-04-28)**: `Pod::compact()` (`crates/pod/src/pod.rs:1217-1226`) で `usage_history.clear()` の直後に `*self.extract_pointer.lock() = None` を実行。意図と cold restore 一致を doc コメントに明記。
- **完了条件「session 側の処理済み pointer が更新され、次回 Phase 1 は続きから走る」が compact を挟むと崩れる** — 上記 Blocking の直接の帰結。テストでは現れていないが、compact + extract の組合せシナリオを想定したテストがあれば検出できる
  - **対応済み (2026-04-28)**: `Pod::extract_pointer() -> Option<ExtractPointerPayload>` accessor を追加し、回帰テスト `compact_resets_extract_pointer_so_phase1_can_fire_again` (`crates/pod/tests/compact_events_test.rs:319-381`) を追加。extract → compact の連続シナリオで pointer が None に戻ることを検証。

### Non-blocking / Follow-up

- **閾値の単位（cumulative `input_total_tokens` の解釈）** — `UsageRecord.input_total_tokens` は「そのリクエスト送信時のプロンプト全長」(`session-store/src/session_log.rs:147-149`) で増分ではない。pointer 以降の records を素直に sum すると、長い 1 turn 内の連続 LLM call では各 prompt が前 prompt の super-set なので合計は実トークン消費の数倍に膨らむ。ticket / 設計の「cumulative input tokens since last pointer」をどう取るかは複数解釈あるが、現実装は最もリベラル（=最も発火しやすい）解釈になっており、頻繁発火を許容する仕様前提と整合はする。今後 threshold の運用値を tune するときに「3 turn で hit する」程度の感覚と乖離するので、doc に "billed input cumulative" と明記するか、`input_total_tokens - cache_read` の差分版や、最後の record だけを見る (= 現プロンプト長) など別解釈に切り替えるか、いずれ運用観察で決める
  - **対応済み (2026-04-28)**: `Pod::tokens_added_since(history_len_pointer)` を `Pod::total_tokens_at(now) - Pod::total_tokens_at(pointer)` の差分計算に切り替え (`crates/pod/src/pod.rs:1284-1294`)。compact 側と同じ accounting (measured/interpolated/extrapolated) に乗るので、threshold 値は「pointer 以降に追加されたプロンプト全長」と素直に解釈できる。`Pod::total_tokens_at(history_len)` を `compact::token_counter` に追加 (`pub` API、将来別チケットで llm-worker に下ろす予定)。
- **`build_extract_input` が ToolCall 引数と ToolResult 本文を落とす** — `crates/memory/src/extract/input.rs:40-45`。compact 用 `build_summary_prompt` のミラーだが、Phase 1 の `attempts` 抽出は「何を試したか」が tool 引数（書き換えた箇所、実行コマンド）に集中することが多く、tool 名だけだと "ran read_file" レベルの粒度になる。tool 結果 summary は残るので致命ではないが、Phase 1 prompt の `attempts.action` 品質を観察してから decide。共通ヘルパー化したいなら memory crate へ持って行く整理も将来検討
- **空 payload で pointer だけ前進する設計の妥当性** — `pod.rs:1414-1424` で `payload.is_empty()` なら staging を作らず pointer だけ進める。spec §完了条件の「空配列で書き出す または skip、どちらでもよい」に合致。一方 `LogEntry::Extension` は payload を増やし続けるので、空 turn が連続すると Extension entry が積み上がる。session 寿命なので Phase 1 の頻度なら許容範囲だが、log size 監視のときに見える要素として把握しておく
- **`write_extracted` が呼ばれずに worker が終了したケースの取り扱い** — `pod.rs:1406-1412` で warn を出して空 payload 扱い、pointer は前進。再実行で同 range を再抽出できないので、ここで pointer を前進させる選択は LLM 不安定時に情報を取りこぼす可能性がある一方、無限ループ防止という意味では合理的。仕様上は "skip でも空配列でもよい" なので問題ないが、運用で `write_extracted` 呼び忘れが頻発するなら spec 変更（pointer 据え置きで再 trigger）が選択肢
- **Controller 側の `if let Err(e)` 分岐は到達不能** — `try_post_run_extract` は最後に internal `self.alert` した上で常に `Ok(())` を返すので、controller の `if let Err(e) = pod.try_post_run_extract()` は dead branch。`try_post_run_compact` も同様で先例を踏襲しているだけだが、両方とも内部 alert だけに統一するか、controller-only alert にするかの整理は別 PR で

### Nits

- `pointer.rs:80-90` の malformed entry テスト: コメント「壊れた最新を黙って無視すると意図しない再抽出を招く」は妥当な保守姿勢。ただし複数 Extension entries がある中で**たまたま最新だけ malformed** の場合は古い良 entry に fallback したほうが安全とも言える。現状で良いが、Phase 2 の stagings 消費と接続するときに再度 visit
- `tool.rs:42-48` の `call_count` は debug 用のみで Pod 側からは未使用。残しておくならいずれ logging で警告に繋ぐ
- `pod.rs:1339` 直前の comment「`history_len_pointer == 0` means everything so far」は「history_len > 0 のすべての record」という意味で正しいが、説明が `cumulative_input_tokens_since(0)` と読むと若干誤読しやすい。doc に「pointer 0 = 未抽出」と書き換えると親切
- `MEMORY_EXTRACT_WORKER_MAX_INPUT_TOKENS = 30_000` の default 値はコメントに根拠を 1 行付記しても良い (compact が 50_000 なので、抽出は context 圧縮後の slice であってより小さくて足りる、という意図か)

## 判断

**Request changes → 対応済み (2026-04-28)** — Blocking 指摘事項 (compact 後の extract_pointer リセット漏れ) は `crates/pod/src/pod.rs:1217-1226` で fix し、`crates/pod/tests/compact_events_test.rs:319-381` に回帰テストを追加して再発防止を担保した。workspace 全テスト pass。Non-blocking / Nits は仕様準拠と保守姿勢の範囲で別チケット / 運用観察に委ねる方針で完了とできる。
