# メモリ機構: summary.md の resident 注入

## 背景

`memory/summary.md` は durable memory の圧縮サマリとして設計されているが、現状の通常 Pod では system prompt に自動注入されていない。

現在 `summary.md` が使われる経路は主に以下である。

- `MemoryRead(kind=summary)` / `MemoryQuery` などの tool 経由。
- memory consolidation worker の入力。既存 `summary.md` / `decisions/*` / `requests/*` が consolidation prompt に渡され、必要なら rewrite される。
- linter / audit / usage metrics の対象。

一方で、通常 Pod の resident injection は `.insomnia/knowledge/*.md` のうち `model_invokation: true` な Knowledge description と resident Workflow description に限られている。`summary.md` は LLM が自発的に `MemoryRead` / `MemoryQuery` しない限り参照されない。

memory は「思い出してほしい情報」であり、短く保たれた summary は tool discovery に任せるより、通常 Pod の context に常駐させた方が活用されやすい。オン/オフ可能にした上で、`summary.md` を resident context として注入する。

## 方針

`summary.md` の本文を、通常 Pod の system prompt trailing section に resident memory summary として注入する。

- `[memory]` が有効な Pod だけが対象。
- `memory/summary.md` が存在し、valid な summary record として読める場合だけ注入する。
- frontmatter は注入しない。body のみを注入する。
- 空 body は注入しない。
- consolidation / compaction などの disposable internal Worker には注入しない。
  - 既存の `set_resident_knowledge_injection(false)` 相当の opt-out と同じ考え方に揃える。
- resident Knowledge / resident Workflow と同じく、system prompt materialization 時に一度だけ読む。
  - turn ごとに history へ system item を append しない。
  - context だけに揮発的に差し込む実装は禁止。system prompt の materialized text として扱う。

## 設定

オン/オフは manifest で制御する。

候補名:

```toml
[memory]
inject_summary = true
```

- default は `true`。
- `[memory]` が無い場合は無効。
- `inject_summary = false` で明示的に無効化できる。

既存の `inject_resident_knowledge` は Pod の内部 opt-out API として残し、manifest の `inject_summary` は memory summary の注入だけを制御する。

## 注入形式

System prompt の resident section に、Knowledge / Workflow とは別の明確な block として入れる。

例:

```text
## Resident memory summary

The following is the workspace-local durable memory summary. Treat it as helpful context, not as authoritative over current user instructions, tickets, files, or git state.

<summary body>
```

注意:

- summary は stale になり得る。現行の user instruction / tickets / files / git / session log を上書きする権威として扱わせない。
- summary が大きすぎる場合は hard error にしない。初期実装では soft cap で切り詰めるか、既存 linter / consolidation prompt の「1-5k tokens 目安」に依存してよい。
- ただし注入時に上限を設ける場合は、切り詰めたことが分かる注記を入れる。

## usage metrics

resident summary injection は「受動的露出」であり、`MemoryRead(kind=summary)` のような明示使用とは区別する。

- 既存の memory usage metrics が明示 tool use を測っている場合、resident injection を `use_count` として混ぜない。
- 必要なら resident exposure として別イベント / 別 source に記録する。
- 既存の resident Knowledge / resident Workflow exposure 記録があるなら、それに合わせる。

## 要件

- `[memory]` が有効で `memory/summary.md` が存在する場合、通常 Pod の system prompt に summary body が注入される。
- summary frontmatter は注入されない。
- `inject_summary = false` の場合は注入されない。
- `[memory]` が無い Pod では注入されない。
- summary が存在しない / parse 不能 / body 空の場合、Pod 起動や run は失敗しない。
- internal disposable Worker には summary resident injection されない。
- resident injection は `worker.history` へ新しい item を append しない。
- summary resident exposure は明示 tool read usage と混同されない。
- stale summary が current project authority を上書きしないよう、注入文言で優先順位を明示する。

## 完了条件

- system prompt render test で summary body が resident block として入ることを確認する。
- frontmatter が入らない test がある。
- `inject_summary = false` で入らない test がある。
- `[memory]` 無しで入らない test がある。
- malformed summary でも run / render が失敗しない test がある。
- internal Worker opt-out の既存経路を壊さない test、または該当コード確認がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- summary の自動圧縮 / token budget 再設計。
- summary を turn ごとに動的 refresh する仕組み。
- `MemoryRead(kind=summary)` の挙動変更。
- global memory / project local memory の store 分離。
- memory の git 運用方針変更。
