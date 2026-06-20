---
title: 'Plugin Service/Ingress component lifecycle surface'
state: 'ready'
created_at: '2026-06-20T13:01:37Z'
updated_at: '2026-06-20T13:02:36Z'
assignee: null
---

## 背景

現行の外部 Plugin 実装は Tool surface のみを提供しており、`yoi plugin new rust-component-tool` で作れるものは「モデルが明示的に呼ぶ Tool」に限られる。Discord Bridge のような Gateway/WebSocket 接続、常駐 lifecycle、外部イベント受信、Yoi 内部への通知/配送を持つ Plugin には Service / Ingress surface が必要だが、まだ外部 Wasm Plugin runtime として実装されていない。

方針として、Service は専用の Component interface を定義し、Wasm 側がそれを実装する。Yoi host は enabled package / grants / digest を確認したうえで Service instance を生成・保持し、start/stop/status/event delivery を host-managed lifecycle として管理する。Wasm component が勝手に常駐 daemon や unmanaged socket を持つのではなく、host が instance と権限付き host API を所有し、外部イベントや host-managed I/O event に応じて component export を呼び出す。

Ingress は外部から Yoi に入る event boundary として扱う。Discord message、Webhook、file watcher event などは Ingress event として bounded/typed data に正規化され、Service instance または handler export に渡される。Plugin が返す action は host が grant と policy を再確認して、Notify / SystemItem / Companion message / Tool-visible diagnostics など既存の durable/visible path に適用する。

既存前提:

- `00001KT6Q08R9` で Feature contribution registry は Tool / Hook / BackgroundTask / Service provider descriptor 境界を導入済み。
- 現行 Plugin package/runtime は Tool registration/execution まで実装済み。
- Plugin output は hidden context injection ではなく、Tool result、committed history、explicit notification/history path へ流す原則を維持する。

## 設計方針

- Service は Tool call の延長ではなく、host-managed lifecycle を持つ separate surface とする。
- ただし stateful Plugin では、Service instance が Plugin の機能上の主 instance であり、同じ Plugin package が提供する Tool はその instance にアクセスできる必要がある。
  - Minecraft Plugin 的に、Plugin instance とその state は Tool/Service/Ingress から不可分な場合がある。
  - Tool は transient worker state ではなく、host が保持する Plugin/Service instance へ command/query として dispatch される。
  - Tool schema/model-visible registration と Service lifecycle は別 surface だが、実行先 instance は同一であり得る。
- Wasm component は Service interface の export を実装する。
  - 例: `start(config)`, `handle_event(event)`, `handle_tool(tool, input)`, `status()`, `stop(reason)` 相当。
  - exact schema / naming は実装時に詰めるが、Tool-only handler と stateful Service-backed Tool dispatch は区別する。
- Yoi host は Plugin/Service instance を保持し、lifecycle / restart / diagnostics / resource bounds / call serialization を管理する。
- 長寿命 network/socket/process ownership は原則 host 側に置く。
  - Wasm に raw ambient network/WASI socket を渡して unmanaged runtime loop を作らせない。
  - 必要な outbound/inbound は host API と grant で明示する。
- Ingress は外部 event を Yoi 内部 event/action に変換する境界とする。
  - external event は bounded, typed, untrusted input として扱う。
  - event delivery は session history / prompt context を直接改ざんしない。
- Plugin action application は host-mediated とする。
  - Notify / SystemItem / Companion message / diagnostics 等、既存または明示的に追加した durable/visible path にだけ反映する。
  - arbitrary UI channel や hidden context injection は作らない。
- Service state は短期状態なら instance 内保持を許すが、永続状態は明示的な persisted state API / scoped fs 等に逃がす。
- package discovery は inventory only、enablement/grants/digest approval が Service/Ingress 登録の前提。

## 要件

- Plugin manifest / static inspection が Service / Ingress contribution を表現できる。
  - `surfaces = ["service", "ingress"]` または同等の明示 surface。
  - service id/version, ingress event kinds, required host APIs, side effects, secrets/network requirements を reviewable にする。
- Component Service runtime が Tool runtime と分離される。
  - Tool schema/model-visible registration と Service lifecycle registration を混同しない。
  - Tool-only Plugin は従来通り bounded Tool execution でよい。
  - Service-backed Plugin では、Tool execution が host-managed Plugin/Service instance の command/query export へ dispatch できる。
  - Service component artifact / interface compatibility を static check できる。
- Host-managed Service instance registry を追加する。
  - enabled Service を start/stop/status できる。
  - 同一 Plugin package の Tool / Ingress / Service 呼び出しが同じ stateful instance を参照できる。
  - instance への同時呼び出し、reentrancy、timeout、crash isolation の方針を定義する。
  - failure/restart/backoff/diagnostics を bounded に記録/表示できる。
  - Pod shutdown / restore / config change 時の lifecycle を明確化する。
- Ingress event delivery path を追加する。
  - external event を bounded typed input として Service export に渡す。
  - Plugin response action は grant/policy check 後に既存 Yoi path へ適用する。
- Permission/grant model を Service/Ingress 用に拡張する。
  - host API, network host, secret refs, event sources, output actions を個別に grant-gated にする。
  - SDK/PDK helper availability と authority grant を混同しない。
- Rust PDK/template は Tool 用と Service/Ingress 用を分ける。
  - Tool authoring docs が Service/Ingress capability を誤って約束しないようにする。
  - Service/Ingress authoring docs は host-managed lifecycle と event-driven export 呼び出しを明記する。

## Non-goals

- Discord Bridge そのものの実装。
- raw ambient WASI network/socket を Plugin に渡して常駐 daemon 化すること。
- arbitrary Plugin UI channel。
- hidden prompt/context injection。
- public registry/install/update/signature tooling。
- Tool surface を Service 代替として拡張すること。

## 受け入れ条件

- Service/Ingress surface の design record または implementation design が repository 内に追加され、Tool surface との違い、host-managed instance lifecycle、Ingress event boundary、permission/grant model が明文化されている。
- `plugin.toml` / package inspection / enablement model に Service/Ingress contribution をどう表現するかが決まっている。
- Wasm 側が実装する Service interface と、host が保持・呼び出す lifecycle の責務境界が決まっている。
- Service-backed Tool が同じ stateful Plugin/Service instance に command/query としてアクセスする方式が決まっている。
- Ingress event がどの durable/visible Yoi path に変換され得るか、どの path が禁止かが決まっている。
- 既存 Tool Plugin runtime/docs が「現時点では Tool surface」であることを誤解なく示す。
- 実装を一度に詰め込まず、必要なら follow-up Ticket に分割できる状態になっている。
- 既存 Feature contribution registry (`00001KT6Q08R9`) との接続方針が明記されている。

## Validation

- design/doc diff review。
- `git diff --check`。
- docs/resource を変更した場合は `nix build .#yoi --no-link`。
- Ticket 整合性確認: `yoi ticket doctor`。
