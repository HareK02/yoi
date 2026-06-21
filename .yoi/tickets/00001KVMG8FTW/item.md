---
title: 'Plugin: host_api.https を廃止して URL 権限ベースの host_api.request に統合する'
state: 'inprogress'
created_at: '2026-06-21T07:10:30Z'
updated_at: '2026-06-21T07:18:48Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'host-api', 'public-api', 'permissions', 'security', 'local-network', 'breaking-change']
queued_by: 'workspace-panel'
queued_at: '2026-06-21T07:15:41Z'
---

## User claims / request snapshot

- 現行 `host_api.https` は localhost/private address を拒否するため、Plugin とローカル外部プロセスの通信に合わない。
- `host_api.https` は廃止し、one-shot request/response 型通信は `host_api.request` に統合する。
- WebSocket は `request` に混ぜず、別 capability として扱う。
- 対象 host / URL は Plugin 側が権限として要求する前提にする。
- 任意 URL access は便利だが大きすぎる権限であり、導入時に「何が可能になる権限を要求しているか」がわかりやすいことを優先する。
- Regex を許してもよいが、permission review の可読性を壊さないことが重要。

## Confirmed facts / sources

- `docs/development/plugin-development.md` は `https` host API を outbound-only / grant-gated とし、WebSocket/Gateway/inbound HTTP surface ではないと説明している。
- 同 docs は manifest permission `host_api.https` と enablement grant `grants.https` による host/method/path allowlist を説明している。
- 同 docs は `http://`, localhost/private/link-local targets, disallowed hosts/methods, oversize requests/responses, missing grants を reject すると説明している。
- `crates/manifest/src/plugin.rs` には `PluginGrantConfig.https`, `PluginHttpsGrant`, `PluginHostApi::Https` がある。
- `crates/pod/src/feature/plugin.rs` の `validate_plugin_https_request` は URL scheme を `https` に限定し、static HTTPS target validation と allowlist authorization を行っている。
- Closed Ticket `00001KVFDX9AF` は localhost/private/link-local rejection と WebSocket/SSE non-goals を含む HTTPS host API 実装だった。
- Closed Ticket `00001KVJHYP4Q` は Plugin Service/Ingress lifecycle 基盤を入れたが、実外部 socket/event source は scope 外だった。

## Unverified hypotheses

- 初期 schema は named request permission として `id`, `schemes`, `hosts`, `ports`, `methods`, `path_prefixes` を持たせると、導入時表示と runtime authorization の両方を扱いやすい。
- Arbitrary URL access は `broad = "any_url"` のような明示的な broad permission として扱うと、人間がレビューしやすい。
- Regex は初期 non-goal でもよい。入れる場合も opaque regex ではなく、normalized display / warning / label を伴う必要がある。

## Undecided points / open questions

- 最終的な TOML/Rust 型名は実装時に決めてよい。
- Regex support を初回に含めるかは実装裁量。ただし、入れる場合は導入時に可能範囲が読める表示と broad/opaque pattern の診断が必要。
- Private LAN targets を loopback targets と同じ category にするか、より大きい broad/local-network permission として表示するかは実装時に整理してよい。

## Background

現行 `host_api.https` は public outbound HTTPS API 用の安全な最小能力として実装された。一方で、Plugin と local bridge process / 外部プロセスを連携させる用途では、`https` 固定・localhost/private 拒否・WebSocket 非対応という名前と境界が合わない。

この Ticket では、one-shot request/response 型通信を `host_api.request` に統合し、Plugin package manifest が必要な target URL 権限を静的に要求し、workspace/user enablement grant がそれを明示承認する model に変更する。WebSocket / persistent streaming / bidirectional connection は別 Ticket の対象であり、`request` に混ぜない。

## Requirements

- `host_api.https` naming/API を廃止し、one-shot request/response 型通信を `host_api.request` に統合する。
- Active public/model/config-facing API から `host_api.https`, `PluginHttps*`, `grants.https` naming を除去し、`request` naming に統一する。
- `PluginPackageManifest` 側に、Plugin が必要とする request target permissions を静的に宣言できる schema を追加する。
- `PluginEnablementConfig.grants` 側に、manifest-declared request target permission を実際に許可する grant schema を追加する。
- Runtime authorization は、原則として「Plugin が manifest で要求した request target」かつ「enablement grant で承認された request target」だけを許可する。
- `request` target permission は URL を前提にし、少なくとも scheme, host, optional port, method, path prefix を人間が読める形で表現できること。
- 任意 URL / broad network access は許可可能にしてもよいが、通常 host grant と区別して「大きい権限」として導入時に明示する。
- localhost/loopback/private/local targets を許す場合も ambient に開けず、manifest-declared URL permission と enablement grant の両方がある場合だけ許可する。
- `request` は WebSocket, persistent streaming, background event handling, Service lifecycle を含めない。
- Embedded credentials, credential-like headers, oversize request/response, missing grants, untrusted external content handling などの既存 safety は維持する。
- Manifest resolution / `yoi plugin show` / diagnostics で以下を区別して表示する:
  - Plugin が要求している request permissions;
  - workspace/user が grant している request permissions;
  - requested だが未 grant のため拒否された request permissions;
  - arbitrary URL / broad network access 相当の request permissions。
- 不要な backward compatibility alias は追加しない。必要になった場合は escalation する。

## Acceptance criteria

- `host_api.https` / `PluginHttps*` / `grants.https` naming が active API から消え、`host_api.request` / request grant naming に統一される。
- Plugin manifest だけを見れば、その Plugin がどの URL/request target 権限を要求しているか分かる。
- `yoi plugin show` 相当の inspection で request 権限要求と grant 状態が bounded / human-readable に表示される。
- Enablement grant が manifest-declared request target と照合される。
- Manifest で要求されていない target への request は、grant だけがあっても原則 fail closed する、または明示 override として安全に診断される。
- Grant されていない requested target への runtime request は fail closed する。
- 既存の HTTPS request use case は `host_api.request` として動く。
- `http://localhost` / loopback request は、manifest-declared URL permission と enablement grant がある場合だけ許可される。
- Arbitrary URL / broad access は通常 host grant と区別して表示・診断される。
- Regex を入れる場合、broad/opaque pattern を review で見落とさない表示・テストがある。
- WebSocket URL / upgrade / persistent stream は `request` では拒否または非対応として明示される。
- Docs / templates / tests / diagnostics が `host_api.request` と WebSocket 別サポート方針に更新される。

## Binding decisions / invariants

- `host_api.https` は残さない。
- `request` は one-shot request/response 用であり、WebSocket / SSE / persistent connection を含めない。
- Request authority は URL permission を前提にする。
- Plugin 側が対象 URL/host scope を権限として要求し、user/workspace enablement がそれを承認する二段階 model にする。
- 任意 URL access は大きい権限として明示されなければならない。
- Local/private communication is not ambient; it requires explicit manifest declaration and explicit grant.
- Hidden context injection はしない。
- External content is untrusted and bounded.
- Backward compatibility alias は追加しない unless explicitly reapproved.

## Implementation latitude

- Type names are free to choose, but model-facing/config-facing names should be `request`.
- Internal implementation may reuse existing HTTPS request code paths after renaming/refactoring.
- Request permission schema は exact host / scheme / port / method / path prefix を基本にしてよい。
- Regex support は入れても入れなくてもよいが、入れる場合は permission review の可読性を守ること。
- Private LAN target は loopback より広い権限として表示してよい。
- `yoi plugin show` の表示形式は実装裁量だが、requested/granted/denied/broad の区別は維持する。

## Readiness

- readiness: implementation_ready
- risk_flags: [plugin, host-api, public-api, permissions, security, local-network, breaking-change]

## Escalation conditions

- `request` に WebSocket / SSE / daemon lifecycle を混ぜたくなる場合。
- Local/private target policy が manifest declaration + grant model なしに広がる場合。
- 任意 URL access が通常権限として目立たなくなる場合。
- Secret-bearing headers/env/config を guest memory から直接渡す設計になりそうな場合。
- Compatibility alias を追加したくなる場合。
- Regex が導入時 review で理解不能な opaque permission になりそうな場合。

## Validation

- Focused plugin host API tests.
- Manifest-declared request permission parsing/resolution tests.
- Grant allow/deny tests.
- Requested-but-ungranted and granted-but-unrequested denial tests.
- Loopback/local target allow/deny tests.
- Broad/arbitrary URL display/diagnostic tests.
- Docs/template updates.
- `cargo fmt --check`
- relevant `cargo test`
- `cargo check`
- `git diff --check`

## Related work

- `00001KVFDX9AF` — Plugin HTTPS host API, closed.
- `00001KVJHYP4Q` — Plugin Service/Ingress component lifecycle surface, closed.
- `00001KSXRQ4G8` — Plugin runtime/surface/host API design record, closed/superseded.
- `docs/development/plugin-development.md`
- `docs/design/plugin-component-model.md`
- `docs/design/plugin-packages.md`
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
- `crates/yoi/src/plugin_cli.rs`
