---
title: 'Implement MCP 2025-11-25 local stdio server-feature bridge'
state: 'planning'
created_at: '2026-06-10T07:48:49Z'
updated_at: '2026-06-10T07:48:49Z'
assignee: null
readiness: 'blocked'
risk_flags: ['mcp', 'authority-boundary', 'prompt-context', 'permission-scope', 'secrets', 'process-exec', 'feature-api']
---

## Background

Yoi に MCP integration を追加する。対象は現時点の latest MCP specification `2025-11-25` を基準にした local stdio MCP server integration。

この Ticket は MCP 実装を担当し、`pod::feature` / Worker / ToolRegistry の拡張そのものは別 Ticket `00001KTR81P9X` に分離する。MCP 実装は、その拡張 API に乗り、Yoi の通常 ToolRegistry / permission / history / bounded result path を迂回しない。

Objective context: `00001KTR80WMN`。

## Requirements

- MCP specification `2025-11-25` を基準にする。
- local stdio transport の MCP server を explicit Profile/config から有効化できる。
- stdio subprocess lifecycle を実装する。
  - stdin/stdout newline-delimited JSON-RPC。
  - stdout は MCP messages として扱う。
  - stderr は bounded diagnostics/logging として扱い、即 protocol failure にはしない。
  - shutdown は graceful close / terminate / kill を安全に扱う。
- MCP lifecycle を実装する。
  - `initialize`
  - capability negotiation
  - `notifications/initialized`
  - protocol/startup/disconnect diagnostics
- server features を Yoi の明示 tool operations として expose する。
  - `tools/list`
  - `tools/call`
  - `resources/list`
  - `resources/read`
  - `prompts/list`
  - `prompts/get`
- `resources` / `prompts` は direct context injection ではなく、通常の tool call result として history に記録される model-visible operation として扱う。
- MCP tools は discovered tool を namespaced stable Yoi tool として expose する。
- tool/resource/prompt metadata、schemas、annotations、content は untrusted data として扱う。
- `tools/list` / `resources/list` / `prompts/list` pagination と list-changed notification を扱う。
- result content を bounded serialization する。
  - `content[]`
  - `structuredContent`
  - `isError`
  - `_meta`
  - text / image / audio / resource_link / embedded resource
- roots を実装する場合は Yoi authorized scope 由来の root のみに限定する。
- sampling / elicitation は初期実装では client capability として宣言せず、要求された場合は fail-closed diagnostic とする。
- Streamable HTTP transport / OAuth / remote auth / MCP Registry distribution / automatic package execution はこの Ticket の対象外。
- secret refs / env values / command args containing secrets を diagnostics / logs / model context に plaintext で出さない。
- local mock MCP server を使った focused tests を追加する。
- docs に local stdio MCP server の設定例、trust model、permission/scope/secret guidance を追加する。

## Acceptance criteria

- docs/tests/config で MCP spec baseline `2025-11-25` が明記されている。
- 少なくとも1つの local stdio MCP server を Profile/config で有効化できる。
- configured server の initialize 成功/失敗が観測可能で、失敗時に server 名と phase が分かる。
- discovered MCP tools が namespaced stable name で Yoi の model-visible tool schema に現れる。
- registered MCP tool を呼ぶと `tools/call` が実行され、normal result / `isError: true` / JSON-RPC protocol error が区別される。
- `resources/list` / `resources/read` と `prompts/list` / `prompts/get` が明示 tool operations として使え、結果が通常 tool result として history に残る。
- resource/prompt content は history に残らない形で context に注入されない。
- `structuredContent` / output schema / rich content blocks が bounded serialization される。
- list-changed notification が silent stale にならず、safe refresh または restart/reinitialize-required diagnostic として扱われる。
- PreToolCall permission denial が通常 Yoi tool denial と同じ経路で動く。
- server-provided metadata/content が system/developer instruction や Yoi scope/permission を弱めない。
- secrets が diagnostics/logs/model context に plaintext で出ないことを focused test または review で確認できる。
- sampling / elicitation / Streamable HTTP / remote auth / distribution は disabled, fail-closed, or explicitly out-of-scope として実装・docs に反映されている。
- Validation: focused MCP mock server tests、related crate tests、`cargo fmt --check`、`cargo check --workspace --all-targets`、`nix build .#yoi`。

## Binding decisions / invariants

- MCP server は untrusted external capability provider として扱う。
- MCP integration は `pod::feature` / Worker / ToolRegistry の通常 authority path に乗せ、private/ad-hoc bypass を作らない。
- `resources/read` / `prompts/get` の結果を hidden context injection しない。明示 tool result としてのみ model-visible にする。
- local process 起動は explicit config と explicit process authority に限る。
- filesystem roots を公開する場合は Yoi authorized scope 由来に限定する。
- unsupported client features, including sampling and elicitation, are fail-closed。

## Implementation latitude

- exact Profile/config syntax、crate/module placement、namespacing format、diagnostic surface は invariants を満たす範囲で実装側に委ねる。
- `notifications/*/list_changed` の live refresh が current run の tool schema consistency を壊す場合は、next-turn refresh または restart/reinitialize-required diagnostic でよい。
- MCP `2025-11-25` の experimental tasks / `execution.taskSupport` は、実装時に API 拡張 Ticket の結果を踏まえて ordinary call fallback / unsupported diagnostic / focused support の範囲を決める。`required` task-only tool を silent mis-expose しない。

## Escalation conditions

- API extension Ticket `00001KTR81P9X` の完了前に private bypass が必要になりそうな場合。
- MCP tasks / task-augmented tool calls を full support するために Yoi Task / approval / resume model との統合判断が必要になる場合。
- server-provided resource/prompt content を tool result 以外の context path に入れる必要が出た場合。
- Streamable HTTP、remote auth、sampling、elicitation、workspace-provided executable/package auto-start に踏み込む必要が出た場合。

## Related work

- Objective: `00001KTR80WMN`
- API prerequisite: `00001KTR81P9X`
- Decomposed from broad Ticket: `00001KST8H4M0`
