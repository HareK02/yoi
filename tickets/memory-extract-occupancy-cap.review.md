# Review: Memory: extract worker のサーキットブレーカーを占有量ベースに統一

## 前提・要件の確認

- 要件1「`MemoryExtractWorkerInterceptor::pre_llm_request` を `total_tokens(context, &records).tokens > max_input_tokens` ベースに置換、`input_so_far` 廃止」: 達成。`crates/pod/src/pod.rs:2192-2208` で `usage_tracker.records()` → `llm_worker::token_counter::total_tokens(context, &records)` の経路に切り替わり、`fetch_add` ベースの累積パスは消滅。リポジトリ全体を `grep` しても `input_so_far` の残滓なし。
- 要件2「extract worker にも `UsageTracker` を持たせ、`note_request` / `record_usage` で配線」: 達成。`pod.rs:1934-1945` の wiring が compact 側 (`crates/pod/src/pod.rs:1537-1548`) と完全に対称。`extract_usage_tracker` というローカル独立 tracker（main Pod の persist 用 `UsageTracker` とは別インスタンス）になっており、compact 側の `summary_usage_tracker` と同じ分離方針。`pre_llm_request` 末尾で `usage_tracker.note_request(context.len())` も入っている (`pod.rs:2206`)。
- 要件3「`extract_worker_max_input_tokens` の doc を新セマンティクスに更新」: 達成。`crates/manifest/src/lib.rs:111-115`、`crates/manifest/src/defaults.rs:53-56`、`docs/manifest.toml:260-262` がいずれも「現在占有 token cap」表現に書き換わっている。
- 要件4「`MemoryConfig::extract_worker_max_turns: Option<u32>` を新設、cascade と `set_max_turns` 連動」: 達成。`MemoryConfig` (`lib.rs:116-121`)、`MemoryConfig::merge` (`config.rs:255-257`)、`MEMORY_EXTRACT_WORKER_MAX_TURNS` 定数 (`defaults.rs:58-60`) が揃い、`pod.rs:1918-1920` で cascade、`pod.rs:1945` で `extract_worker.set_max_turns(extract_worker_max_turns)` を `Option<u32>` ごと無条件に渡している。compact 側レビューで撤去された `if .is_some()` 分岐の轍は踏んでいない。
- 要件5「`Mirror of CompactWorkerInterceptor` コメントを独立 doc に書き換え」: 達成。`pod.rs:2178-2185` の doc は「Kept separate from `compact::worker::CompactWorkerInterceptor` so each subsystem can tune its own cancel message and budget」と独立性を明示する形に書き換え済み。リポジトリ全体に `Mirror of CompactWorkerInterceptor` の残骸なし。
- 要件6「後方互換 shim 無し」: 達成。`extract_worker_max_input_tokens` はフィールド名そのままで意味だけ差し替え。新フィールド `extract_worker_max_turns` も `#[serde(default)]` の Option で導入され、過渡互換用のエイリアスや旧名フォールバックは入っていない。
- 完了条件「3 者が同じ算出経路 (`llm_worker::token_counter::total_tokens`) を使う」: 達成。`Pod::total_tokens()` (`compact/token_counter.rs:146-149`)、`CompactWorkerInterceptor::pre_llm_request` (`compact/worker.rs:261-273`)、`MemoryExtractWorkerInterceptor::pre_llm_request` (`pod.rs:2192-2208`) の 3 者がいずれも `llm_worker::token_counter::total_tokens(history/context, &records)` 一本に揃っている。
- 完了条件「ユニットテスト 2 パターン」: 達成。`crates/pod/src/pod.rs:3093-3158` の `memory_extract_interceptor_tests` モジュールに `extract_interceptor_uses_occupancy_not_cumulative_usage` と `extract_interceptor_cancels_when_occupancy_exceeds_cap` の 2 本があり、compact 側 `compact/worker.rs:301-349` の 2 本と入力値（cap=150, 100×3 / cap=99, 100×1）まで対称。

## アーキテクチャ・スコープ

- 占有量計算は main Pod / compact / extract の 3 系統で一本化された。設計上の歪み（要約: 「extract worker だけ別ロジック」）は解消。
- 共通 `OccupancyInterceptor` 抽象化に踏み込まず、サブシステム個別の cancel メッセージと cap を別構造体として保持。チケットの範囲外指定通り。
- `extract_threshold` (`docs/manifest.toml:256`) の累積セマンティクスは触らずに残されており、これは「ポインタ→now の差分」という別概念なので正解。
- 新規依存追加なし、新規クレートなし、`llm-worker` の責務範囲は変えていない。クレート命名や cargo add ポリシーに抵触する変更なし。
- `extract_worker_max_turns` のデフォルト `Some(8)` は、extract が write_extracted_tool 1 本で 1-2 ターンで終わる軽量 worker であることを踏まえると compact (20) より小さく合理的。runaway リカバリのみを担う上限として妥当。
- cascade パターンは `MemoryConfig::merge` 内の他フィールドと同じ `upper.x.or(self.x)` 形。`CompactionConfig` 側が serde-time デフォルト関数で埋めている方式とは流儀が異なるが、これは既存 `MemoryConfig` 内部の慣習（他フィールドも `#[serde(default)]` Option + 呼び出し側 `unwrap_or(defaults::...)`）と一致しており、内部一貫性の観点で問題なし。

## 指摘事項

特になし。compact 側で確立した方式の対称適用が、コメント・テスト・cascade・wiring まで含めて綺麗に完遂されている。docs 側の「累積」表現も新規導入箇所には残っていない。

## 判断

Approve (完了可) — チケットの全要件と完了条件が明示的な根拠付きで満たされており、compact 側との対称性が wiring・doc・テストの 3 階層で揃っている。コードベースの歪みを増やす要素もなし。
