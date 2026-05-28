---
id: 20260528-001748-compact-session-log-exploration
slug: compact-session-log-exploration
title: Compact: session log 探索型の要約入力に変更する
status: open
kind: task
priority: P2
labels: [compact, session-log]
created_at: 2026-05-28T00:17:48Z
updated_at: 2026-05-28T00:58:00Z
assignee: null
legacy_ticket: null
---

# Compact: session log 探索型の要約入力に変更する

## 背景

`insomnia-troubleshoot` Pod の手動 compact で、Compact Worker が入力トークン上限に到達して停止した。現行実装は `Pod::compact` で retained tail より前の `items_to_summarise` を `build_summary_input()` に渡し、`build_summary_prompt()` が user / assistant / system message と tool result summary を `## Conversation` に連結して Compact Worker の初回 input に載せている。

raw tool output や reasoning は落としているが、長い session では pruned transcript だけでも `compact_worker_max_input_tokens` を超える。Compact の目的は「全履歴を読ませる」ことではなく、次セッションに必要な構造化要約と file auto-read/reference を作ることなので、初期 input は軽量 overview に留め、必要箇所は Compact Worker が session log / workspace file を探索して確認できる形にする。

また、Compact Worker の健全性は「初期 input が小さいこと」だけでは保証できない。探索 tool の結果、assistant 出力、`write_summary` 呼び出しまでを含む Compact Worker 全体の context と、compact 後に作られる新 session 初期 context を別々に制御する必要がある。

## 方針

Compact Worker の初期 context は、全文 transcript ではなく決定的に生成した session overview / index を渡す。LLM には探索空間を狭めた上で、必要な session log 範囲や workspace file を tool で読む権限を与える。

基本方針:

- 初期 input は User / Assistant / System の継続に必要な情報を中心に、target size 内の overview として生成する。
- 初期 overview が target を超えた程度で compact を失敗させない。warning / trace に記録して続行する。
- 初期 overview deadline は通常運用の調整値ではなく、想定外の入力生成バグを検出する最悪ケースの安全網とする。deadline 超過時は、可能ならより粗い overview へ fallback し、それでも最低限の入力を作れない場合だけ失敗する。
- ToolCall / ToolResult は初期 input では本文を展開しない。
  - tool 名、summary、対象 path、成否、大きな出力の有無、session log 上の位置などの index に留める。
- Compact Worker は session log の必要箇所を探索・再読できる。
- Compact Worker の探索量は、session-log/file-read 個別の総量 budget ではなく、Compact Worker session 全体の context budget で制御する。
- Compact Worker context が上限に近づいたら、`mark_read_required` とは独立に「探索を切り上げて `write_summary` へ進め」という勧告を Worker に渡し、人間にも警告を出す。
- 最終 summary と closing turn のための reserve を確保し、reserve を食い潰すほど大きい tool result は残 budget に合わせて抑制・切り詰め・再読指示にする。
- AutoRead 判断のため、workspace file は現行通り `read_file` で確認し、必要なものだけ `mark_read_required` / `add_reference` する。
- AutoRead budget は Compact Worker の探索 budget ではなく、compact 後の新 session 初期 context に注入される file content の合計上限として扱う。
- Compact Worker の出力は現行と同じく structured summary + auto-read + references を生成する。

## Compact Worker / compaction parameters

`[compaction]` 配下では `compact_` prefix を新規 parameter 名につけない。既存の `compact_*` key は、この ticket の実装時に同じ意味の prefix なし key へ整理する。

必要な parameter:

- `retained_tokens`
  - compact 後に verbatim で残す history tail の token budget。
- `overview_target_tokens`
  - 初期 overview / index 生成器が目指す通常サイズ。超過しても即失敗しない。
- `overview_warning_tokens`
  - 初期 overview が想定より大きいことを記録・警告する閾値。compact は続行する。
- `overview_deadline_tokens`
  - 初期 overview の最悪ケース deadline。超過時はより粗い overview へ deterministic fallback し、それでも無理な場合だけ compact を失敗させる。
- `worker_context_max_tokens`
  - Compact Worker session 全体の context hard limit。system prompt、overview、assistant output、tool calls/results、session-log/file read results、`write_summary` 周辺の蓄積を含む。
- `finish_warning_remaining_tokens`
  - 残り context がこの値以下になったら、Compact Worker に探索切り上げと `write_summary` を促す勧告を入れる。
- `final_reserve_tokens`
  - 最終 summary と closing turn のために残す reserve。これを割り込みそうな tool result は full content を返さず、range 縮小や summary への移行を促す。
- `worker_max_turns`
  - Compact Worker の tool-loop 最大 turn 数。budget 制御とは別の runaway guard。
- `summary_target_tokens`
  - `write_summary` text の目標サイズ。prompt / nudge に使う。
- `summary_max_tokens`
  - `write_summary` text の hard validation。超過した summary は縮約を促すか compact 成功扱いにしない。
- `auto_read_budget_tokens`
  - `mark_read_required` によって compact 後の新 session に注入される file content の合計 token budget。
- `result_context_max_tokens`
  - compact 成功前に dry-run する新 session 初期 context の上限。summary、auto-read contents、references、task snapshot、retained tail を含む。
- `model`
  - compactor model。未指定なら main Worker の client を clone する。

Compact 発火条件の `threshold` / `request_threshold` は Compact Worker の健全性 parameter ではないが、既存の `compact_threshold` / `compact_request_threshold` を整理する場合は `[compaction]` 内の prefix なし key として扱う。

## 要件

- `build_summary_input()` / compact 入力生成を、prefix 全体の pruned transcript 一括投入から、bounded overview + index 生成に変更する。
  - overview は `overview_target_tokens` を目指して生成する。
  - `overview_warning_tokens` 超過時は警告・trace を記録しつつ続行する。
  - `overview_deadline_tokens` 超過時はより粗い deterministic overview に fallback する。通常ケースの user-facing hard error にしない。
  - User / Assistant / System message を優先し、古い detail は落としてよい。
  - Tool output content は初期 input に載せない。
- Compact Worker 用の session log 探索 tool を追加する。
  - 例: `search_session_log(query, filters, range)`。
  - 例: `read_session_items(range | item_ids, mode = compact/full)`。
  - 必要なら large tool result を個別に読む tool を追加する。
- 探索 tool は session-store の現在 segment / compact 対象 range を正本として読む。
  - Compact 対象外の future/retained tail と混ざらないよう range 境界を明示する。
  - tool result full content を返す場合は Compact Worker の残り context / `final_reserve_tokens` を守る。
  - session-log/file-read 個別の総量 budget を user-facing parameter として増やさず、主制御は `worker_context_max_tokens` に寄せる。
- Compact Worker の context occupancy を request 前に見積もり、`worker_context_max_tokens` を最後の hard stop として扱う。
- Compact Worker の残り context が `finish_warning_remaining_tokens` 以下になったら、追加探索を切り上げて `write_summary` に進むよう Worker に勧告し、人間向け warning も出す。
- `final_reserve_tokens` を割り込む可能性がある tool result は、full content を返さず bounded/truncated result とし、range 縮小または `write_summary` への移行を促す。
- `write_summary` 後に `summary_max_tokens` を validation する。超過時は縮約を促し、改善できない場合は compact 成功扱いにしない。
- compact 成功前に、`summary + auto-read + references + retained tail + task snapshot` の新 session 初期 context を dry-run 見積もりし、`result_context_max_tokens` を超えないことを確認する。
- `mark_read_required` / `add_reference` の意味論は維持する。
  - AutoRead は session log 上の過去 tool output ではなく、現在の workspace file を `read_file` で確認してから選ぶ。
  - `auto_read_budget_tokens` は新 session 初期 context への file content 注入上限であり、Compact Worker の探索 budget ではない。
- `resources/prompts/internal/compact_system.md` の summary target は `summary_target_tokens` から反映する。
- 手動 compact / auto compact の双方で同じ経路を使う。
- 巨大 session でも Compact Worker が初回 input 上限で即停止しない。

## 完了条件

- 長い session で compact 初期 overview が transcript 全体を載せず、`overview_target_tokens` を目指して生成される unit test がある。
- `overview_warning_tokens` 超過時に compact が続行し、警告・trace が記録される test がある。
- `overview_deadline_tokens` 超過時に粗い deterministic overview へ fallback する test がある。
- Tool result content が初期 compact input に混入しないことを test で確認している。
- Compact Worker が session log overview から必要 range を tool で読み、`write_summary` まで到達できる test がある。
- `finish_warning_remaining_tokens` 到達時に Compact Worker へ探索切り上げ勧告が入り、人間向け warning も出る test がある。
- `final_reserve_tokens` を守るため、過大な tool result が bounded/truncated される test がある。
- `summary_max_tokens` 超過 summary が compact 成功扱いにならない、または縮約 nudge を受ける test がある。
- compact 後の新 session 初期 context が `result_context_max_tokens` で dry-run validation される test がある。
- `mark_read_required` / `add_reference` 既存 test が通り、auto-read budget の挙動が維持されている。
- `[compaction]` の新 parameter 名が docs / manifest schema / defaults に反映されている。
- `docs/compaction.md` と `resources/prompts/internal/compact_system.md` が新しい探索型 flow と budget/warning semantics に更新されている。
- `cargo fmt --check` と関連 crate の compact/session-store/pod/manifest tests が通る。

## 範囲外

- Compact summary 自体を deterministic summarizer に置き換えること。
- Memory extract / consolidation の入力方式変更。
- 過去の壊れた session log の migration。
- Compact 後の retained tail token policy の再設計。
- session-log/file-read ごとの user-facing 総量 budget を増やすこと。

## 実装メモ

現行コード上の主な起点:

- `crates/pod/src/pod.rs::compact`
- `crates/pod/src/pod.rs::build_summary_input`
- `crates/pod/src/pod.rs::build_summary_prompt`
- `crates/pod/src/compact/worker.rs`
- `crates/manifest/src/lib.rs::CompactionConfig`
- `crates/manifest/src/config.rs::CompactionConfigPartial`
- `crates/manifest/src/defaults.rs`
- `resources/prompts/internal/compact_system.md`
- `docs/compaction.md`
