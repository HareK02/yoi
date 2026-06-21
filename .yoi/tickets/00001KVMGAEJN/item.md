---
title: 'Plugin: URL 権限ベースの WebSocket host API を実装する'
state: 'inprogress'
created_at: '2026-06-21T07:11:34Z'
updated_at: '2026-06-21T13:20:53Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'host-api', 'websocket', 'service', 'ingress', 'lifecycle', 'permissions', 'security', 'persistence']
queued_by: 'workspace-panel'
queued_at: '2026-06-21T11:34:07Z'
---

## User claims / request snapshot

- WebSocket / bidirectional communication は `host_api.request` に混ぜず、別 capability としてサポートする。
- WebSocket も、対象 URL を Plugin 側が権限として要求する前提でよい。
- 任意 URL access は便利だが大きすぎる権限であり、導入時に何が可能になる権限を要求しているかがわかりやすいことを重視する。
- Service/Plugin instance は状態を持てるため、WebSocket API 自体は「WebSocket を扱う host API」として提供すればよい。
- WebSocket 接続後の処理、incoming message の解釈、Ingress 発火、Service 状態更新などは Plugin instance 側が既存 Service/Ingress lifecycle と host-mediated action path を使って組み立てる。

## Confirmed facts / sources

- Closed Ticket `00001KVMG8FTW` は one-shot request/response を `host_api.request` に統合し、WebSocket / persistent streaming / bidirectional connection は別 capability とする方針で完了した。
- Closed Ticket `00001KVJHYP4Q` は Plugin Service/Ingress component lifecycle surface を実装し、Plugin instance が lifecycle と state を持てる前提を作った。
- `docs/development/plugin-development.md` と Plugin design docs は Tool call の中に long-lived connection を隠さず、Service/Ingress surface に分ける方針を持つ。
- 現行 code map では WebSocket 専用 host API / persistent bidirectional Plugin transport はまだ active API として確認できていない。

## Background

`host_api.request` は one-shot request/response の authority として設計し、WebSocket / persistent connection / bidirectional event handling を含めない方針になった。一方で、Plugin と外部プロセス・Gateway・bridge service が双方向通信するには WebSocket 等の persistent transport が必要になる。

この Ticket は、WebSocket を `request` とは別の `host_api.websocket` capability として追加する実装 work item である。WebSocket は Service/Plugin instance が使う host API であり、Yoi が独自に ingress routing policy を抱え込むものではない。Plugin instance は接続 handle と内部状態を保持し、受信メッセージをどう扱うか、Ingress/action/status にどう変換するかを Plugin の Service/Ingress logic として実装する。

## Binding decisions / invariants

- WebSocket は `host_api.request` に含めない。
- WebSocket authority は URL permission を前提にする。
- Public/config-facing name は `host_api.websocket` とする。
- Plugin package manifest は必要な WebSocket URL targets を静的に要求する。
- Workspace/user enablement grant は、manifest-declared WebSocket URL target を明示的に承認する。
- WebSocket 接続は Plugin instance / Service lifecycle の中で使う host API resource とする。
  - Yoi host は permission check、resource bounds、redaction、shutdown/cancellation cleanup を担当する。
  - Plugin instance は connection handle を使い、send/receive/close と自身の state/lifecycle を管理する。
- Incoming message は Yoi が自動的に history/model context に注入しない。
- Incoming message の解釈、Ingress 発火、SystemItem/Notify/diagnostic 等への変換は Plugin instance が既存の host-mediated API / action path を使って行う。
- Long-lived connection を Tool call の中に隠さない。Tool は必要なら Service instance に command/query を投げるだけにする。
- Package discovery / static inspection は socket connection や process startup を意味しない。
- External content is untrusted and bounded.
- Broad/arbitrary WebSocket URL access は大きい権限として表示・診断する。

## Requirements

- `host_api.websocket` capability / host API を追加する。
- WebSocket URL permission は `host_api.request` の URL target model と整合させる。
  - 少なくとも scheme, host, optional port, path prefix を人間が読める形で表現できること。
  - `ws` / `wss` を扱う。HTTP request の method permission とは分ける。
- Plugin manifest 側に WebSocket target permission declaration を追加する。
- Plugin enablement grant 側に WebSocket target grant を追加する。
- Runtime authorization は「manifest で要求された target」かつ「enablement で grant された target」だけを許可する。
- Manifest で要求されていない target への WebSocket 接続は grant だけがあっても fail closed する、または明示 override として安全に診断される。
- WebSocket host API は Plugin instance が保持できる connection handle/resource を返す。
- Host API は最低限の操作を提供する。
  - connect/open
  - send text/binary
  - receive next message with bounds/timeout/cancellation
  - close
  - status/diagnostic where needed
- Host は message size bounds、timeout/cancellation、shutdown cleanup、diagnostics、redaction を実装する。
- Reconnect/backoff/heartbeat policy は初回 host API の必須機能にしない。
  - Plugin Service が state と timer/lifecycle で組み立ててよい。
  - Host は failed/closed/cancelled を bounded diagnostic として返す。
- Auth/headers/secrets は ambient env ではなく explicit config / SecretRef / grant model に乗せる。
  - 初回で SecretRef header injection が未実装の場合は non-goal として fail closed / future follow-up にする。
- `yoi plugin list/show` / static inspection は WebSocket URL permission 要求と grant 状態を bounded / human-readable に表示する。
- Arbitrary URL / broad WebSocket access は通常 target grant と区別して表示・診断する。
- Docs/templates/tests を `host_api.request` と WebSocket 別 capability 方針に更新する。

## Acceptance criteria

- `host_api.websocket` が `host_api.request` とは別 capability として定義される。
- Plugin manifest だけを見れば、その Plugin がどの WebSocket URL target 権限を要求しているか分かる。
- Enablement grant が manifest-declared WebSocket target と照合される。
- Grant されていない requested target への WebSocket connect は fail closed する。
- Manifest で要求されていない target への WebSocket connect は fail closed する、または明示 override として安全に診断される。
- `yoi plugin show` 相当の inspection で requested/granted/denied/broad WebSocket permission が bounded / human-readable に表示される。
- Plugin instance / Service lifecycle 内で WebSocket connection handle を保持し、send/receive/close できる。
- Incoming messages は自動で history/model context に入らず、Plugin instance の処理を通る。
- Hidden context injection が導入されていない。
- Tool call の中に long-lived WebSocket connection を隠さない設計・テストになっている。
- Shutdown/cancellation で open connection が cleanup される。
- Message size bounds / timeout / redaction / diagnostics のテストがある。
- Existing `host_api.request` behavior は壊れない。

## Implementation latitude

- Internal type names are implementation latitude, but public/config-facing names should use `websocket`.
- URL permission expression は `host_api.request` と共通の exact scheme/host/port/path model を流用してよい。
- Regex support は入れても入れなくてもよい。入れる場合は permission review の可読性を守ること。
- Initial implementation may support only text messages if binary support would expand scope too much, but binary handling must then fail closed and be documented.
- Reconnect/backoff/heartbeat は host API ではなく Plugin Service layer の responsibility としてよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [plugin, host-api, websocket, service, ingress, lifecycle, permissions, security, persistence]

## Escalation conditions

- WebSocket を `host_api.request` に混ぜたくなる場合。
- WebSocket runtime が Tool call / Tool result に隠れそうな場合。
- Incoming message が history/context に非永続・非可視に注入されそうな場合。
- URL permission が broad なのに導入時表示で目立たない場合。
- Secret/auth handling が ambient env や raw config leakage に寄る場合。
- Host 側が Plugin Service の reconnect/application protocol policy まで抱え込みそうな場合。

## Validation

- Focused WebSocket host API tests.
- Manifest-declared WebSocket URL permission parsing/resolution tests.
- Grant allow/deny tests.
- Requested-but-ungranted and granted-but-unrequested denial tests.
- Broad/arbitrary URL display/diagnostic tests.
- Lifecycle shutdown/cancellation tests.
- Message bounds/redaction diagnostics tests.
- No hidden context injection tests.
- `host_api.request` regression tests.
- Docs/template updates.
- `cargo fmt --check`
- relevant `cargo test`
- `cargo check`
- `git diff --check`
- `nix build .#yoi --no-link`

## Related work

- `00001KVFDX9AF` — Plugin HTTPS host API, closed.
- `00001KVJHYP4Q` — Plugin Service/Ingress component lifecycle surface, closed.
- `00001KSXRQ4G8` — Plugin runtime/surface/host API design record, closed/superseded.
- `00001KVMG8FTW` — Plugin: host_api.https を廃止して URL 権限ベースの host_api.request に統合する, closed.
- `docs/development/plugin-development.md`
- `docs/design/plugin-component-model.md`
- `docs/design/plugin-packages.md`
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
