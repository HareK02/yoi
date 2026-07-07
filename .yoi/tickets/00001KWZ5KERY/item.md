---
title: 'Design Backend-to-Runtime ConfigBundle synchronization strategy'
state: 'planning'
created_at: '2026-07-07T20:51:35Z'
updated_at: '2026-07-07T20:51:35Z'
assignee: null
---

## 背景

Backend -> Runtime の ConfigBundle 境界は既に型と sync/check/store の外枠を持っているが、現状の bundle は profile selector 宣言が中心であり、Worker 作成時の実際の Profile discovery / manifest resolution は Runtime 側の `ProfileRuntimeWorkerFactory` が host filesystem から再実行している。

これは長期設計と合わない。Workspace は Browser/product の論理単位であり、Runtime は Workspace filesystem を profile discovery source として読むべきではない。Backend が Workspace / project の有効 Profile と policy を解決し、Runtime には検証済み ConfigBundle として同期する。Runtime は同期済み bundle と WorkingDirectory / execution backend の入力だけで Worker を作れる必要がある。

既存の closed Ticket `00001KVZQHPNY` は ConfigBundle sync の初期設計、`00001KW7726H9` は Runtime Worker creation の正規経路を扱った。本 Ticket はその follow-up として、selector-only bundle から resolved launch config bundle へ進める同期戦略を確定し、実装可能な slice に分ける。

## 目的

- Backend が Profile discovery / resolution の authority を持つことを明確にする。
- Runtime が Worker creation 時に Workspace filesystem / profile base を読まない設計にする。
- ConfigBundle に何を含め、何を含めないかを確定する。
- Backend が Runtime ごとに bundle availability を ensure/sync/check する戦略を定義する。
- Worker creation は synced ConfigBundle identity と WorkingDirectory / execution backend の入力で成立するようにする。

## 設計方針

### 1. ConfigBundle は selector allowlist ではなく resolved launch config を運ぶ

現状の `ConfigProfileDescriptor { selector, label }` だけでは Runtime が Worker manifest を作れない。ConfigBundle には少なくとも Backend-resolved Profile artifact または WorkerManifestConfig 相当の launch recipe を含める。

候補:

- `ResolvedProfileBundle` / `ResolvedWorkerLaunchConfig` のような新型を bundle に入れる。
- 既存 `WorkerManifestConfig` を直接入れる場合は、runtime-bound / authority-bearing field が混入しない validation を強化する。
- prompt/resource/workflow は inline bytes、embedded resource ref + digest、または explicit resource table のどれにするかを決める。

### 2. Runtime は bundle を検証・保存・参照するが discovery はしない

Runtime の責務:

- ConfigBundle digest / revision / provenance / workspace_id を検証する。
- `CreateWorkerRequest` の ConfigBundleRef が保存済み bundle と一致することを確認する。
- bundle 内の resolved launch config から Worker を作る。
- host-local secret / plugin / mount / network / shell policy は Runtime host policy と照合する。
- WorkingDirectory は Worker execution root/cwd として使うが、Profile discovery source にはしない。

Runtime がしてはいけないこと:

- Workspace filesystem を profile discovery のために読む。
- Browser-facing request 由来の raw path / cwd / secret / endpoint を信頼する。
- selector だけを受け取り、Runtime-local FS から暗黙解決する。

### 3. Backend は bundle identity を content-addressed に扱う

Backend は launch 時に、対象 Runtime に必要な bundle があるかを確認し、なければ sync する。

想定 flow:

```text
Browser / Web Console
  -> Backend: launch worker(profile/role, working_directory_id, initial input)

Backend
  -> resolve Profile/config/prompt/resource/policy into ConfigBundle
  -> compute digest / stable bundle id
  -> Runtime: check bundle availability
  -> Runtime: sync bundle if missing or digest mismatch
  -> Runtime: create worker with ConfigBundleRef + WorkingDirectory/claim + initial input
```

### 4. Browser-facing API に ConfigBundle を出さない

Browser は Profile selector / role / display information を選ぶだけにする。ConfigBundleRef、Runtime internal config store、bundle digest、secret refs の詳細、Runtime endpoint は Browser-facing payload に出さない。

### 5. 互換 bridge は明示的に一時扱いにする

実装途中に builtin/default fallback や `profile_base_dir` が必要なら、runtime-local compatibility path として明示し、Browser WorkingDirectory launch の通常経路では使わない。

## 実装分割案

1. Field audit / current boundary report
   - `ConfigBundle`、`CreateWorkerRequest`、`ProfileRuntimeWorkerFactory`、Backend launch API、remote Runtime sync API の現状を整理する。
   - Runtime が FS profile discovery をしている箇所を列挙する。

2. Bundle payload model
   - ConfigBundle に resolved launch config を表す typed payload を追加する。
   - digest 計算に payload を含める。
   - secret values / raw host path / runtime endpoint / socket/session path を拒否する validation を追加する。

3. Backend bundle builder
   - Backend が Profile selector / role から resolved bundle を作る経路を実装する。
   - builtin role profiles と workspace/project profile discovery を Backend 側に寄せる。
   - launch options は Browser に selector candidates だけ見せる。

4. Runtime create integration
   - Worker creation が ConfigBundle payload から Worker manifest/config を構築する。
   - Runtime-side profile filesystem discovery を Browser launch 経路から外す。
   - missing/mismatch/unsupported bundle は typed diagnostic にする。

5. Remote/embedded sync behavior
   - embedded Runtime direct call と remote Runtime REST client の sync/check behavior を揃える。
   - retries は content identity で idempotent にする。

6. Cleanup / compatibility removal plan
   - selector-only bundle や runtime-local profile fallback が残る場合、その用途と削除条件を記録する。

## 受け入れ条件

- ConfigBundle の payload strategy が Ticket thread または artifact に明文化されている。
- Backend が Profile/config を解決して ConfigBundle を作る責務を持つことが code/docs/tests 上で明確になっている。
- Runtime Worker creation は通常経路で Workspace filesystem / profile base から Profile discovery を行わない。
- ConfigBundle digest は resolved payload を含む content identity になっている。
- ConfigBundle validation が secret values、raw host paths、Runtime endpoint、socket/session paths、runtime-local mount actual paths を拒否する。
- Browser-facing API に ConfigBundleRef / bundle digest / Runtime config store / raw path が露出しない。
- Embedded Runtime と remote Runtime の bundle check/sync/create behavior が同じ contract を使う。
- Missing bundle、digest mismatch、unsupported declaration、unsupported host policy、missing secret ref は typed diagnostic になる。
- Existing WorkingDirectory launch が ConfigBundle 経由で Worker を作れる。
- Focused tests が Backend bundle build、Runtime bundle validation、create without Runtime FS profile discovery、remote sync/check path、Browser payload redactionを確認する。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Secret value synchronization。
- Plugin package manager / signature policy の完成。
- Multi-tenant auth model の完成。
- Browser-facing ConfigBundle editor UI。
- Runtime が Workspace filesystem を profile/config discovery source として読む経路の延命。
