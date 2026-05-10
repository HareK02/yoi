# Memory: extract / consolidation 呼称を extract / consolidation に統一

## 背景

メモリサブシステムは公開 API 側では既に `extract` / `consolidate` (consolidation) を識別子として採用している:

- モジュール: `crates/memory/src/extract/`, `crates/memory/src/consolidate/`
- Manifest フィールド: `extract_model`, `extract_threshold`, `extract_worker_max_input_tokens`, `consolidation_model`, `consolidation_threshold_files`, `consolidation_threshold_bytes` (`crates/manifest/src/lib.rs:99-131`)
- `pub fn` / `pub struct` で `phase` を含む識別子は 0 件、共通の `Phase` enum / trait も存在しない

一方、コメント・ログ・エラー文字列・LLM プロンプト・ドキュメントには「extract」「consolidation」という呼称が広く残っている。設計的に "phase" という抽象が不要であるにもかかわらず二重の語彙が並走している状態で、新規読者にとって「extract = extract、consolidation = consolidation」というマッピングを暗黙に要求している。

特に LLM プロンプト (`resources/prompts/internal/memory_*_system.md`, `crates/memory/src/consolidate/input.rs:57-59`) に "consolidation" が出現しているのは、モデルの行動を機能名ではなく序数で誘導している形になり、置き換えるとついでに改善になる。

## 方針

"extract" を "extract"、"consolidation" を "consolidation" に置き換える。コードに `phase` という共通抽象が無いことを名前の上でも明示する。

「consolidation 内部の `consolidation phase` / `tidy phase`」のように phase という語が **階層的に再利用されている表現**は単純置換すると壊れるので、その箇所だけ語彙ごと整理する（例: `consolidation step` / `tidy step`、または「consolidation の統合パート / 整理パート」など、文脈に応じて）。

## 要件

- Rust コード (`crates/`) 内の doc comment / 通常コメント / ログメッセージ / `thiserror` の `#[error]` 文字列 / テスト関数名・コメントから "extract" / "consolidation" を排除し、`extract` / `consolidation` に置き換える。
- LLM プロンプト用の固定文字列 (`crates/memory/src/consolidate/input.rs` の `consolidation input. Run the consolidation phase first ...`) を `consolidation` 系の語彙のみで再構成する。`resources/prompts/internal/memory_extract_system.md` と `resources/prompts/internal/memory_consolidation_system.md` の `phase` 言及も同様に置換する。
- `docs/plan/memory.md` と関連 plan / ref 文書 (`docs/plan/memory-effectiveness.md`, `docs/plan/memory-prompts.md`, `docs/ref/memory-systems.md`, `docs/ref/opencode-comparison.md` の memory 周辺) の "extract / consolidation" を改稿する。章立てや見出しに使われている場合は「## Extract」「## Consolidation」の構成に置き換える。
- `phase` という語を残してよいのは「(memory と無関係な) tool dispatch / TUI / worker 内部の局所的なフェーズ表現」（例: `crates/llm-worker/src/worker.rs:747,795`、`crates/tui/src/spawn.rs:148,182`、`crates/tui/src/main.rs:209`）。これは memory の話ではないので対象外。
- 後方互換 shim は不要（公開 API 名は変更しない、テキストの置き換えのみ）。

## 完了条件

- `git grep -i 'extract\|consolidation\|phase1\|phase2'` の結果から、memory サブシステムに紐づく hit が 0 になる（TUI / tool dispatch / 一般語 "thinking phase" は除く）。
- LLM プロンプト 3 ファイル (`memory_extract_system.md`, `memory_consolidation_system.md`, `consolidate/input.rs` の inline テキスト) で序数表現が消え、機能名のみで指示が成立している。
- `docs/plan/memory.md` の見出し構造と本文が `Extract` / `Consolidation` ベースに揃っている。

## 範囲外

- 公開 API 名 (`extract_*`, `consolidation_*`) の rename。既に統一されているので変更しない。
- メモリの設計・挙動変更。純粋に呼称の整理のみ。
- TUI / worker / tool dispatch などメモリ外の "extract / 2" 言及。
- ファイル名・モジュール名の変更（既に `extract/` / `consolidate/`）。

## 影響範囲

- `crates/manifest/src/lib.rs`, `crates/manifest/src/defaults.rs`: 該当 doc comment。
- `crates/memory/src/extract/{mod,input,payload,pointer,staging,tool}.rs`: doc comment。
- `crates/memory/src/consolidate/{mod,input,lock,staging,tidy}.rs`: doc comment、ログ、`#[error]`、LLM プロンプト inline 文字列。
- `crates/memory/src/workspace.rs:186`: コメント 1 件。
- `crates/llm-worker/src/token_counter.rs:11`, `crates/pod/src/compact/token_counter.rs:154`: doc comment。
- `crates/pod/src/prompt/catalog.rs:64,66`: doc comment。
- `crates/pod/tests/compact_events_test.rs`: テスト関数名 (`compact_resets_extract_pointer_so_phase1_can_fire_again` 他) とコメント。
- `crates/pod/tests/consolidation_test.rs`: コメント。
- `resources/prompts/internal/memory_extract_system.md`, `resources/prompts/internal/memory_consolidation_system.md`: プロンプト本文。
- `docs/plan/memory.md`, `docs/plan/memory-effectiveness.md`, `docs/plan/memory-prompts.md`, `docs/ref/memory-systems.md`, `docs/ref/opencode-comparison.md` の該当箇所。
- `docs/manifest.toml`: 該当があれば。
