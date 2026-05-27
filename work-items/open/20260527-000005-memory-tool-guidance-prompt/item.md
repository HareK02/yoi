---
id: 20260527-000005-memory-tool-guidance-prompt
slug: memory-tool-guidance-prompt
title: プロンプト: memory / knowledge tool 利用タイミングのガイダンス
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:05Z
updated_at: 2026-05-27T00:00:05Z
assignee: null
legacy_ticket: tickets/memory-tool-guidance-prompt.md
---

## Migration reference

- legacy_ticket: tickets/memory-tool-guidance-prompt.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# プロンプト: memory / knowledge tool 利用タイミングのガイダンス

## 背景

通常 Pod には `MemoryQuery` / `MemoryRead` / `KnowledgeQuery` / `MemoryWrite` 等の memory / knowledge tools が提供されているが、現状の通常 system prompt はそれらを「いつ使うべきか」をほとんど説明していない。

現在の `resources/prompts/common/tool-usage.md` は、既知パスなら Read、検索なら Grep/Glob、並列可能ならまとめる、という汎用 tool 方針に留まる。memory / knowledge tools の description には操作方法はあるが、モデルが自発的に memory lookup すべき状況は明示されていない。

このため、過去の決定・ユーザー嗜好・以前の経緯を問われても、モデルが `MemoryQuery` / `MemoryRead` を自発的に使わない可能性が高い。`summary.md` resident injection により短い durable context は常時見えるようになるが、詳細な過去判断や request を探すには query guidance が必要である。

## 方針

通常 Pod の system prompt に、memory / knowledge tools の利用タイミングを短く追加する。

目的は「必要な時に過去情報を探す」ことであり、毎 turn memory query を強制することではない。memory / knowledge は helpful context だが stale になり得るため、現在の user instruction / files / tickets / git state / session log を上書きする権威として扱わせない。

## 推奨する追加文言

`resources/prompts/common/tool-usage.md` に新しい小節を足すか、`resources/prompts/common/memory.md` を作って `default.md` から include する。

例:

```md
## Memory and knowledge

Use memory and knowledge tools when the user asks about past decisions, prior requests, durable preferences, project history, or why something was done. Do not guess from vague recollection when a targeted memory lookup would answer the question.

- Use `MemoryQuery` for durable memory records: summary, decisions, and requests.
- Use `KnowledgeQuery` for project knowledge records.
- Use `MemoryRead(kind=summary)` when you need the full workspace memory summary.
- Use `MemoryRead` on returned slugs when query excerpts are insufficient.

Resident memory and knowledge are helpful context but may be stale. Current user instructions, repository files, tickets, git history, and session logs are more authoritative for exact current state.

Do not query memory on every turn. Prefer it when past context, user preferences, or prior rationale materially affects the answer or implementation.
```

文言は実装時に自然に調整してよいが、以下の意味は維持する。

- 過去判断 / 過去依頼 / ユーザー嗜好 / project history / why 系では memory lookup を促す。
- `MemoryQuery`, `KnowledgeQuery`, `MemoryRead(kind=summary)`, slug read の役割を明示する。
- resident context は stale になり得ると明示する。
- current user instruction / files / tickets / git / session logs の方が exact current state では強いと明示する。
- 毎 turn query しないと明示する。

## 要件

- 通常 Pod の default prompt に memory / knowledge tool 利用タイミングの guidance が入る。
- internal prompts (`memory_extract_system`, `memory_consolidation_system`, `compact_system`) の挙動を変えない。
- guidance は短く、通常 turn の token overhead を過度に増やさない。
- guidance は memory / knowledge を current authority より上に置かない。
- guidance は毎 turn memory query を促さない。
- `MemoryWrite` / `MemoryEdit` / `MemoryDelete` の自発的利用を安易に促さない。
  - 通常作業では read/query を促し、write/edit/delete は明示的な依頼または memory maintenance worker に寄せる。

## 完了条件

- `resources/prompts/default.md` から memory guidance が render される。
- prompt render / catalog 関連 test があれば更新されている。
- internal worker prompt には不要な memory guidance が混ざらない。
- `cargo fmt --check` と関連 test が通る。

## 範囲外

- `summary.md` resident injection の実装。これは `memory-summary-resident-injection.md` で扱う。
- memory tool descriptions の大幅変更。
- memory usage metrics の設計変更。
- global memory / project local memory の store 分離。
