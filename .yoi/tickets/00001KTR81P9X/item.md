---
title: 'Extend pod::feature API for external protocol-backed capability providers'
state: 'planning'
created_at: '2026-06-10T07:48:14Z'
updated_at: '2026-06-10T07:48:14Z'
assignee: null
readiness: 'requirements_sync_needed'
risk_flags: ['authority-boundary', 'feature-api', 'tool-registry', 'permission-scope', 'process-exec', 'prompt-context']
---

## Background

MCP integration を concrete work item として実装する前に、Yoi の `pod::feature` / Worker / ToolRegistry 境界が外部 protocol-backed capability provider を自然に扱える必要がある。

現行の `pod::feature` API は static descriptor / static tool contribution / host authority grant を中心にしており、MCP のように startup 時の protocol negotiation と discovery によって tools/resources/prompts surface が決まる provider には不足がある。MCP 実装側でこれを ad-hoc に迂回すると、permission、history、bounded result、service lifecycle、feature/plugin boundary が歪む。

Objective context: `00001KTR80WMN`。

## Requirements

- `pod::feature` が external protocol-backed capability provider を表現できるようにする。
- local subprocess を起動する authority を明示的に表現する。
  - 既存の filesystem/network/service authority に押し込まない。
  - command / args / cwd / env / secret refs は explicit config/grant に基づく。
- Pod lifetime に紐づく feature-provided service / connection manager の lifecycle を表現できるようにする。
- startup / initialize / discovery 後に tool contribution を登録または更新できる host-mediated API を設計する。
- dynamic contribution は通常の ToolRegistry / PreToolCall permission / history / bounded tool-result path を迂回しない。
- tool metadata に、MCP など外部 provider が持つ追加 metadata を安全に保持・変換できる余地を作る。
  - title
  - output schema
  - annotations
  - icons or display metadata
  - execution/task-support metadata
- rich / structured tool result を bounded serialization できる共通 path を用意する。
- capability metadata / schemas / descriptions / results は untrusted data として扱う。
- live list-changed / registry refresh が current run の tool schema consistency を壊さない方針を決める。
- API 拡張は MCP 固有名に寄せすぎず、将来の external plugin / bridge provider にも使える feature boundary として設計する。

## Acceptance criteria

- external protocol-backed provider を built-in feature として表現できる API がある。
- subprocess execution authority が `HostAuthority` または同等の明示 grant として型で表現されている。
- feature-owned long-running service lifecycle を Pod lifetime に接続できる。
- discovery 後の dynamic tool contribution が通常 ToolRegistry と permission path に統合される。
- external metadata / schema / result content が untrusted data として扱われる設計になっている。
- `list_changed` 相当の dynamic registry change について、safe refresh / next-turn refresh / restart-required diagnostic のいずれかが明示され、silent stale にならない。
- MCP 実装 Ticket が private/ad-hoc API ではなく、この拡張 API に乗って実装できる見通しがある。
- focused tests で authority grant、dynamic registration、permission denial、bounded result、service diagnostics を確認できる。
- Validation: relevant crate tests、`cargo fmt --check`、`cargo check --workspace --all-targets`、`nix build .#yoi`。

## Binding decisions / invariants

- ToolRegistry / permission / history / bounded result の既存経路を迂回する API は作らない。
- external provider 由来の schema / description / annotation / content は instruction ではなく untrusted data として扱う。
- process execution は explicit authority として分離し、filesystem/network authority から暗黙に派生させない。
- dynamic registry update は prompt/tool schema consistency と cache behavior を壊さないよう、turn boundary または restart/reinitialize diagnostic を持つ。
- MCP specific shortcut ではなく、`pod::feature` の長期的な extension surface として成立させる。

## Implementation latitude

- exact type names、crate placement、service handle の形、dynamic registration timing は実装側に委ねる。
- 初期実装では rich content を provider-native multimodal block として渡さず、bounded structured text/JSON serialization に寄せてもよい。
- live registry refresh が大きい場合は、まず restart/reinitialize-required diagnostic でもよい。ただし silent stale は避ける。

## Escalation conditions

- current Worker / ToolRegistry architecture では dynamic contribution を安全に扱えない場合。
- process execution authority と profile/config authority の責務境界が曖昧になる場合。
- rich content を model-provider native content block に渡す必要が出た場合。
- hook / feature / plugin contribution boundary の既存設計と矛盾する場合。

## Related work

- Objective: `00001KTR80WMN`
- Decomposed from broad Ticket: `00001KST8H4M0`
- Follow-up implementation Ticket: MCP local stdio implementation Ticket created by this intake split.
