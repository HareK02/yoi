---
title: 'RuntimeへProfile/config bundleを同期する'
state: 'queued'
created_at: '2026-06-25T15:49:30Z'
updated_at: '2026-06-25T16:56:06Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:44:39Z'
---

## 背景

Runtime は Worker を動かす環境であり、Worker creation 時には Profile / prompt resources / tool policy / plugin declarations / host-local policy を使って最終的な WorkerSpec を作る必要がある。Backend が Profile を完全解決して巨大な WorkerSpec を毎回 Runtime に渡す設計にすると、remote Runtime / Plugin / secret / mount / host-specific policy と相性が悪い。

一方で、Runtime に `profile = "builtin:companion"` の selector だけを送っても、remote Runtime が同じ Profile / prompt / plugin resource を持っている保証はない。したがって、Backend は workspace/project で有効な Profile/config bundle を Runtime に同期し、Worker creation では profile selector + bundle digest + intent を送る形にしたい。

この Ticket は `worker-runtime` core の後続として、Runtime への Profile/config bundle sync と Runtime-side profile resolution を実装する。初期 `worker-runtime` core では `config_bundle = None` による builtin/default fallback で動作確認できるため、この同期機能は別実装粒度とする。

## 要件

### Config bundle model

- Runtime に同期可能な Profile/config bundle model を定義する。
- Bundle は digest / revision / workspace id / created_at / source metadata を持つ。
- Bundle は少なくとも以下を表現できる。
  - Profile definitions。
  - prompt resources。
  - workflow definitions or references。
  - tool declarations / tool policy。
  - plugin descriptors / package refs / digests。
  - non-secret model/provider config refs。
  - language settings。
  - workspace/project metadata。
  - grants / policy declarations。
- Secret values、runtime-local mount actual path、local cache path、raw socket/session path は bundle に含めない。
- Secret は secret ref / grant / policy として表現し、値は Runtime host-local secret store が解決する。

### Runtime sync API

- Runtime は config bundle を受け取り、digest で保存・照合できる。
- Embedded Runtime では direct lib API で bundle sync できる。
- Networked Runtime では REST API で bundle sync できる shape を定義する。
  - 例: `PUT /v1/config-bundles/{digest}`。
  - 例: `GET /v1/config-bundles/{digest}` or status endpoint。
- Runtime は create worker 時に指定された bundle digest を持っているか検証する。
- Bundle digest mismatch / missing bundle / invalid profile selector / unsupported declaration を typed error にする。

### Worker creation integration

- `CreateWorkerRequest` は profile selector + config bundle ref を受ける。
- Runtime は bundle 内の Profile を最終解決する。
- Runtime は host-local policy / capability / secret / mount / plugin grant enforcement を適用して `ResolvedWorkerSpec` を作る。
- Backend は Profile を完全解決した巨大 WorkerSpec を送らず、intent / profile selector / bundle ref / required capabilities を送る。
- Runtime-local builtin/default fallback は残してよいが、remote Runtime / plugin use では bundle が必要になる policy を設定できる。

### Backend responsibility

- Backend は workspace/project の有効 config bundle を作成・選択し、対象 Runtime に同期する。
- Backend はどの Runtime にどの bundle を同期してよいかを policy / workspace visibility で判断する。
- Backend は Browser に Runtime credential / direct endpoint / raw bundle storage path を渡さない。
- Backend RuntimeRegistry は Runtime の bundle availability / digest status を確認できる。

### Plugin / host policy boundary

- Plugin package bytes を bundle に含めるか package ref + digest にするかは実装時に決める。
- Runtime は plugin descriptor / digest / grants を検証してから tool/service/ingress surface を登録する。
- Runtime host が保護したい secret / mount / network egress / shell/git availability は host-local policy として最終判断する。
- Bundle sync は Plugin execution を直接許可するものではなく、Runtime-side grant enforcement が必要である。

## Non-goals

- `worker-runtime` core crate の作成。
- FS store feature の実装。
- REST command server の実装そのもの。ただし API shape は定義してよい。
- Full Plugin package manager / registry / signature policy。
- Secret value synchronization。
- Workspace mount actual path synchronization without host policy。
- Backend internal Companion Web Console completion。

## 受け入れ条件

- Config bundle domain type が定義され、digest / revision / provenance を持つ。
- Runtime は bundle を保存・一覧/確認・digest 検証できる。
- `CreateWorkerRequest` が profile selector + config bundle ref を扱える。
- Runtime は bundle 内 Profile を解決し、host-local policy を適用する境界を持つ。
- Missing bundle / digest mismatch / invalid profile / unsupported declaration が typed error になる。
- Bundle は secret values / raw socket path / raw session path / runtime-local mount actual path を含まない。
- Backend は Runtime へ bundle sync し、Runtime の bundle availability を確認できる。
- Remote Runtime 用 REST sync API shape または実 endpoint がある。
- Builtin/default fallback と synced bundle mode の責務が docs/tests で区別されている。
- `cargo test -p worker-runtime` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
