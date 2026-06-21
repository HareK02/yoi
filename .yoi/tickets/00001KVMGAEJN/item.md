---
title: 'Plugin: URL 権限ベースの別 capability として WebSocket support を設計する'
state: 'ready'
created_at: '2026-06-21T07:11:34Z'
updated_at: '2026-06-21T07:14:05Z'
assignee: null
readiness: 'requirements_sync_needed'
risk_flags: ['plugin', 'host-api', 'websocket', 'service', 'ingress', 'lifecycle', 'permissions', 'security', 'persistence']
---

## User claims / request snapshot

- WebSocket / bidirectional communication は `host_api.request` に混ぜず、別 capability としてサポートする。
- WebSocket も、対象 URL を Plugin 側が権限として要求する前提でよい。
- 任意 URL access は便利だが大きすぎる権限であり、導入時に何が可能になる権限を要求しているかがわかりやすいことを重視する。
- Regex を許す余地はあるが、本来の権限ニーズは permission review の可読性にある。

## Confirmed facts / sources

- `docs/development/plugin-development.md` は現行 `https` host API を outbound-only / grant-gated とし、WebSocket/Gateway/inbound HTTP surface ではないと説明している。
- Closed Ticket `00001KVFDX9AF` は WebSocket / SSE / timer host APIs, Service surface lifecycle, Ingress surface, Discord Gateway bridge, Inbound HTTP server を non-goals として HTTPS host API を実装した。
- Closed Ticket `00001KVJHYP4Q` は Plugin Service/Ingress component lifecycle surface を実装し、PluginInstance lifecycle と in-process ingress dispatch path の基盤を入れたが、実外部 socket/event source は scope 外だった。
- `docs/design/plugin-component-model.md` / `docs/design/plugin-packages.md` は Plugin instance lifecycle、Service/Ingress、future MCP/plugin bridge、external process authority/lifecycle/permission/diagnostics/trust model の必要性に触れている。
- 現行 code map では WebSocket 専用 host API / persistent bidirectional Plugin transport はまだ active API として確認できていない。

## Unverified hypotheses

- WebSocket capability は `host_api.websocket` のような host API になるか、Service surface 専用 transport になる可能性がある。
- Incoming messages は PluginInstance / Service / Ingress lifecycle と接続するのが自然だが、connection ownership と dispatch route は設計判断が必要。
- URL permission model は `request` と共通概念にできるが、WebSocket は persistent lifecycle / reconnect / shutdown / inbound events を持つため、grant と runtime management は別 schema になる可能性が高い。

## Undecided points / open questions

- WebSocket connection を Yoi host が所有するのか、Plugin instance が host API を通じて所有するのか。
- Incoming message を Ingress に dispatch するのか、Service event/status stream として扱うのか。
- Reconnect / backoff / heartbeat / shutdown / cancellation / restore 時の扱いをどこまで初回に含めるか。
- WebSocket auth/headers/secrets を URL permission とどう分けて表示・grant するか。
- 初回 work item を design/spec のみで閉じるか、最小実装 slice まで含めるか。

## Background

`host_api.request` は one-shot request/response の authority として設計し、WebSocket / persistent connection / bidirectional event handling を含めない方針になった。一方で、Plugin と外部プロセス・Gateway・bridge service が双方向通信するには、WebSocket 等の persistent transport が必要になる。

この Ticket は、WebSocket を `request` とは別 capability として扱い、URL permission を前提にした権限表示・grant・runtime lifecycle・Service/Ingress 接続を設計するための concrete design/spec work item である。実装に進む場合も、Tool call の中に長時間 connection や background daemon を隠さないことを前提にする。

## Requirements

- WebSocket は `host_api.request` に混ぜず、別 capability / host API / transport として設計する。
- WebSocket も URL permission を前提にし、Plugin package manifest 側が必要な WebSocket URL targets を静的に要求できること。
- Workspace/user enablement grant は、manifest-declared WebSocket URL permission を明示的に承認する model にすること。
- Arbitrary WebSocket URL / broad network access は通常 target grant と区別して「大きい権限」として表示・診断すること。
- Regex を許す場合も、導入時に可能範囲が読める normalized display / warning / label を持つこと。
- Tool call の中に long-lived WebSocket connection や background daemon を隠さないこと。
- Service / Ingress / PluginInstance lifecycle との関係を設計すること。
- Connection ownership, shutdown, cancellation, reconnect/backoff, heartbeat, diagnostics, bounds を設計対象に含めること。
- Incoming messages が history/model context に入る場合は hidden context injection にせず、durable/visible/routable path を通すこと。
- Secrets/credentials/auth headers は ambient env ではなく、explicit config / SecretRef / grant model に乗せること。
- Package discovery / `yoi plugin list/show` は Plugin code 実行、socket connection、external process startup を行わないこと。

## Acceptance criteria

- WebSocket responsibility が `host_api.request` から明確に分離される。
- WebSocket URL permission の manifest declaration と enablement grant の関係が定義される。
- Plugin inspection で WebSocket URL permission 要求と grant 状態が bounded / human-readable に表示される設計になる。
- Arbitrary URL / broad WebSocket access が通常 target grant と区別して表示される。
- Connection ownership と lifecycle boundary が明確になる。
- Incoming messages の dispatch path が Service / Ingress / PluginInstance / host routing のどこに属するか決まる。
- Hidden context injection を避ける durable/visible event path が定義される。
- Cancellation, shutdown, reconnect/backoff, timeout, message size bounds, diagnostics, redaction の方針が定義される。
- Auth/secrets handling が explicit config / SecretRef / grant に限定される。
- 最小実装 slice と non-goals が明確になる。

## Binding decisions / invariants

- WebSocket は `host_api.request` に含めない。
- WebSocket authority は URL permission を前提にする。
- Plugin 側が対象 WebSocket URL scope を権限として要求し、user/workspace enablement がそれを承認する二段階 model にする。
- 任意 URL / broad WebSocket access は大きい権限として明示されなければならない。
- Long-lived connection を Tool call の中に隠さない。
- External incoming messages を hidden context injection しない。
- Package discovery / static inspection は socket connection や process startup を意味しない。
- External content is untrusted and bounded.

## Implementation latitude

- 初回は design/spec Ticket として閉じてもよい。
- `host_api.websocket` として設計するか、Service surface 専用 transport として設計するかは比較して決めてよい。
- URL permission expression は `host_api.request` と共通の exact scheme/host/port/path model を流用してもよい。
- Regex support は入れても入れなくてもよいが、入れる場合は permission review の可読性を守ること。
- Reconnect/backoff は初回実装 non-goal にしてもよいが、その場合も明示的な failure/diagnostic behavior を定義すること。

## Readiness

- readiness: requirements_sync_needed
- risk_flags: [plugin, host-api, websocket, service, ingress, lifecycle, permissions, security, persistence]

## Escalation conditions

- WebSocket を `host_api.request` に混ぜたくなる場合。
- Connection lifecycle が Tool call / Tool result に隠れそうな場合。
- Incoming message が history/context に非永続・非可視に注入されそうな場合。
- URL permission が broad なのに導入時表示で目立たない場合。
- Secret/auth handling が ambient env や raw config leakage に寄る場合。
- Restore / cancellation / shutdown / reconnect 方針が決まらないまま実装に進みそうな場合。

## Validation

Design/spec の場合:

- Docs/design update review.
- Existing Plugin Service/Ingress design との整合性確認。
- Ticket body の open questions が resolution / follow-up Ticket に変換されていること。

実装を含める場合:

- Focused WebSocket capability tests.
- Manifest-declared WebSocket URL permission parsing/resolution tests.
- Grant allow/deny tests.
- Broad/arbitrary URL display/diagnostic tests.
- Lifecycle shutdown/cancellation tests.
- Message bounds/redaction diagnostics tests.
- No hidden context injection tests.
- `cargo fmt --check`
- relevant `cargo test`
- `cargo check`
- `git diff --check`

## Related work

- `00001KVFDX9AF` — Plugin HTTPS host API, closed.
- `00001KVJHYP4Q` — Plugin Service/Ingress component lifecycle surface, closed.
- `00001KSXRQ4G8` — Plugin runtime/surface/host API design record, closed/superseded.
- `00001KVMG8FTW` — Plugin: host_api.https を廃止して URL 権限ベースの host_api.request に統合する。
- `docs/development/plugin-development.md`
- `docs/design/plugin-component-model.md`
- `docs/design/plugin-packages.md`
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
