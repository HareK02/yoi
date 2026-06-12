---
title: 'Ticket language guidance must apply to all Ticket tool users'
state: 'inprogress'
created_at: '2026-06-11T14:48:44Z'
updated_at: '2026-06-12T14:55:42Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['prompt-context', 'tool-description', 'feature-boundary', 'ticket-language', 'companion']
queued_by: 'workspace-panel'
queued_at: '2026-06-12T14:49:39Z'
---

## Background

`ticket.language` は durable Ticket record の言語設定であり、Ticket を書くすべての Pod に適用されるべき情報である。

現状の実装では、Ticket record language guidance が主に Ticket role launch prompt に入っている。しかし Companion など Ticket role ではない Pod も Ticket tools を受け取り、Ticket の `item.md` / `thread.md` / `resolution.md` や Ticket tool body を書く可能性がある。

そのため、Ticket record language guidance を Ticket role 専用の launch prompt に置くだけでは不十分である。

## Requirements

- Ticket-writing tools を使える Pod には、Ticket role かどうかに関係なく、configured `ticket.language` に従って durable Ticket record / Ticket tool body を書く guidance が model-visible になること。
- Companion など non-Ticket-role Pod が Ticket tools を扱う場合にも同じ guidance が届くこと。
- 既存の言語境界を維持すること。
  - `worker.language`: 通常の会話 prose。
  - `memory.language`: memory / Knowledge generation。
  - `ticket.language`: durable Ticket records / Ticket tool bodies。
- `ticket.language` が設定されていても、protocol literals、file paths、commands、logs、identifiers、quoted external text は不要に翻訳しないこと。
- prompt-context principle を守ること。モデルの挙動根拠が history / prompt / tool surface に残らない hidden context-only injection を作らないこと。
- Ticket role prompt だけを唯一の伝達経路にしないこと。

## Acceptance criteria

- Ticket tools を持つ non-Ticket-role Pod、特に Companion-style context でも、Ticket tool bodies を configured `ticket.language` で書く guidance が model-visible になる。
- Ticket role Pods でも同等の guidance が引き続き届き、既存挙動が退行しない。
- guidance の source は universal な Ticket capability / tool surface、または feature-scoped system prompt path に置かれ、Ticket role launch prompt 専用ではない。
- `worker.language` が `ticket.language` を override しない。
- 既存 Ticket records は翻訳・一括 rewrite しない。
- focused test または snapshot-style verification で、Ticket role と generic / Companion-style Ticket-capable context の両方に guidance が届くことを確認する。
- runtime prompt / tool behavior に関わるため、完了前に `nix build .#yoi` を通す。

## Binding decisions / invariants

- `ticket.language` は Ticket record writing の policy であり、Ticket role 固有の policy ではない。
- Companion や他の non-role Pods が、conversation language から Ticket record language を推測する状態にしてはならない。
- `worker.language` / `memory.language` / `ticket.language` の責務を混同しない。
- hidden context-only language injection を実装しない。

## Implementation latitude

実装方式は coder がよりきれいな architecture を選んでよい。

候補:

- `ticket.language` が設定されている場合に Ticket tool descriptions / schema text へ language instruction を入れる。
- Ticket capability / feature が有効な Pod に対して、feature-scoped system prompt guidance を追加する。

どちらの場合も、guidance は Ticket tools を扱うすべての model context に durable / model-visible な形で届く必要がある。

## Readiness

- readiness: implementation_ready
- risk_flags: [prompt-context, tool-description, feature-boundary, ticket-language, companion]

## Escalation conditions

- Tool descriptions が現在の構造では configured Ticket language に依存できず、広い ToolRegistry redesign が必要になる場合。
- feature-scoped prompt guidance が history に残らない context mutation を必要とする場合。
- Companion の Ticket capability path から Ticket config にアクセスできない場合。
- 提案実装が Ticket record language と worker response language を混同する場合。

## Validation

- Ticket role prompt / context の focused test。
- generic / Companion-style Ticket-capable context の guidance を確認する focused test または snapshot-style test。
- relevant focused `cargo test`。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi`。

## Related work

- `00001KTJMDWTR` — Separate Ticket record language from worker response language
