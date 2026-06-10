---
title: "MCP integration as external capability provider"
state: "active"
created_at: "2026-06-10T07:47:48Z"
updated_at: "2026-06-10T07:47:48Z"
linked_tickets: ["00001KST8H4M0", "00001KTR81P9X", "00001KTR82RB7"]
---

## Goal

Yoi が MCP (Model Context Protocol) server を external capability provider として安全に扱えるようにする。

到達点は、MCP server が提供する tools / resources / prompts を、Yoi の既存の Pod / Feature / ToolRegistry / permission / scope / history / bounded result 境界に乗せて利用できる状態にすること。MCP は Yoi の plugin model そのものではなく、protocol-bound bridge / external provider runtime として扱う。

## Motivation / background

MCP は AI application と外部 system を接続する標準 protocol になりつつあり、server は tools、resources、prompts などを提供する。Yoi にとって MCP support は有用だが、外部 server が返す schema、description、annotation、resource content、prompt template はすべて untrusted data であり、Yoi の instruction hierarchy、scope、permission、history persistence、prompt-context 加工原則を弱めてはいけない。

現行の `pod::feature` API は static descriptor / static tool contribution を中心にしており、MCP のように startup 時の initialize / capability negotiation / discovery によって surface が決まる provider には不足がある。そのため、MCP 実装だけを先に ad-hoc に入れるのではなく、まず外部 protocol-backed provider を扱える Feature API boundary を整え、その上に MCP implementation を載せる。

## Strategy / design direction

- Broad MCP integration Ticket は progress-container として使わず、この Objective に中期方針を移す。
- 実装 work item は concrete Ticket に分ける。
  - `00001KTR81P9X`: `pod::feature` / Worker / ToolRegistry API を external protocol-backed provider に耐える形へ拡張する。
  - `00001KTR82RB7`: MCP `2025-11-25` local stdio server-feature bridge を実装する。
- MCP 実装は local stdio transport から始める。
  - Streamable HTTP、remote auth/OAuth、MCP Registry distribution、workspace-provided package auto-start は後続判断とする。
- tools / resources / prompts は無理に分けず、MCP server features として扱う。
  - ただし resources/prompts は direct context injection ではなく、明示 tool operation の結果として history に残す。
  - `resources/read` / `prompts/get` の結果を history に残らない形で context に注入しない。
- MCP server は untrusted external capability provider として扱う。
  - server-provided schema/description/annotation/content/error は instruction ではなく data。
  - ToolRegistry / PreToolCall permission / history / bounded result path を迂回しない。
- local process execution は explicit authority として扱う。
  - filesystem/network authority から暗黙に subprocess 起動権限を派生させない。
  - secrets は explicit secret/env references で扱い、diagnostics / logs / model context / project records に plaintext として残さない。
- dynamic discovery / list_changed は prompt/tool schema consistency を壊さない範囲で扱う。
  - live refresh が危険なら next-turn refresh または restart/reinitialize-required diagnostic を選ぶ。
  - silent stale state は避ける。
- Sampling / elicitation は external server が Yoi 側の LLM/user interaction を要求する強い authority なので、初期段階では fail-closed とし、必要なら別 Ticket で approval/resume/UI path を設計する。

## Success criteria / exit conditions

- `pod::feature` が external protocol-backed capability provider を表現できる。
- local subprocess execution authority が明示的に型・config・grant として扱われる。
- Feature-provided long-running service / connection manager を Pod lifetime に安全に接続できる。
- discovery 後の dynamic tool contribution が通常 ToolRegistry と permission path に統合される。
- MCP spec baseline `2025-11-25` に基づく local stdio server integration がある。
- MCP tools が namespaced stable Yoi tool として使える。
- MCP resources/prompts が明示 tool operation として使え、取得結果は通常 tool result として history に残る。
- server-provided data が system/developer instruction、scope、permission、prompt-context、history persistence rules を弱めない。
- secrets / env values / command args containing secrets が diagnostics、logs、model context に plaintext で出ない。
- Streamable HTTP、remote auth、distribution、sampling、elicitation の扱いが out-of-scope / fail-closed / follow-up として明示されている。
- local mock MCP server による focused tests と、関連 crate tests、`nix build .#yoi` による packaging validation が定義されている。

## Decision context

- `00001KST8H4M0` は broad MCP integration Ticket として作られていたが、今後はこの Objective の背景 context として扱い、実装 authority は concrete Tickets に置く。
- `00001KTR81P9X` は API/architecture prerequisite。MCP 実装が ToolRegistry / permission / history path を迂回しないための土台を作る。
- `00001KTR82RB7` は MCP implementation Ticket。API 拡張の結果に乗って local stdio MCP server features を実装する。
- Objective-to-Ticket links は context であり、dependency/scheduling authority ではない。実際の routing / readiness / blockers は各 Ticket の body/thread/artifacts と Orchestrator 判断に置く。
- `resources/prompts` は scope 外に固定しない。安全条件は「direct hidden context injection をしない」ことであり、明示 tool result として扱うなら MCP integration の対象に含める。
