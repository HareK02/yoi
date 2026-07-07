---
title: 'Implement Backend-resolved ConfigBundle sync for Runtime worker creation'
state: 'planning'
created_at: '2026-07-07T20:51:35Z'
updated_at: '2026-07-07T21:08:00Z'
assignee: null
---

## 背景

Backend -> Runtime の ConfigBundle 境界は既に型と sync/check/store の外枠を持っているが、現状の bundle は profile selector 宣言が中心であり、Worker 作成時の実際の Profile discovery / manifest resolution は Runtime 側の `ProfileRuntimeWorkerFactory` が host filesystem から再実行している。

これは正規経路ではない。Workspace は Browser/product の論理単位であり、Runtime は Workspace filesystem を profile/config discovery source として読まない。Backend が Workspace / project / builtin Profile を解決し、Runtime へ同期した ConfigBundle だけを Worker creation の config authority にする。

既存の closed Ticket `00001KVZQHPNY` は ConfigBundle sync の初期設計、`00001KW7726H9` は Runtime Worker creation の正規経路を扱った。本 Ticket はその follow-up として、selector-only bundle を廃止し、Backend-resolved ConfigBundle で Worker を作る実装 slice にする。

## 実装目的

- Backend が Profile discovery / resolution を行い、Runtime は通常 Worker creation 経路で Profile filesystem discovery を行わない。
- ConfigBundle は profile selector allowlist ではなく、Worker 作成に必要な resolved launch config を持つ。
- Backend は Worker launch 前に対象 Runtime の ConfigBundle availability を ensure/sync/check する。
- Runtime は同期済み ConfigBundleRef、WorkingDirectory、initial input から Worker を作る。
- Browser-facing API には ConfigBundleRef、bundle digest、Runtime config store、raw path、Runtime endpoint を出さない。

## 実装内容

### 1. ConfigBundle payload を resolved launch config に拡張する

`ConfigBundle` に Backend-resolved profile payload を追加する。新しい payload は selector 宣言だけではなく、Runtime が Worker manifest/config を構築するための typed data を持つ。

実装する payload:

- profile selector。
- display label / role metadata。
- `WorkerManifestConfig` 相当の resolved launch config。
- prompt / resource / workflow 参照を解決するための resource table。
- non-secret provider/model config refs。
- tool / hook / plugin declaration と policy declaration。
- language / memory / ticket / runtime policy のうち Worker manifest 作成に必要な non-secret config。

payload に含めないもの:

- secret values。
- host absolute path。
- Runtime endpoint / token / socket path / session path。
- runtime-local mount actual path。
- Browser request 由来の raw cwd / raw filesystem scope。

ConfigBundle digest は metadata だけではなく、この resolved payload を含む canonical content identity にする。

### 2. Backend に ConfigBundle builder を実装する

Workspace Backend に、Browser launch の profile/role selector から ConfigBundle を構築する経路を追加する。

実装すること:

- 既存の builtin / workspace / project Profile discovery を Backend 側で呼び出す。
- selector ambiguity / missing profile / invalid profile を Backend diagnostic にする。
- Profile を resolved launch config payload に変換する。
- ConfigBundle digest を計算する。
- Runtime ごとの bundle availability を確認し、missing / digest mismatch の場合は sync する。
- launch options は Browser に profile/role candidates だけ返し、ConfigBundle identity は返さない。

### 3. Runtime の ConfigBundle store / sync / check を resolved payload 対応にする

Runtime 側の ConfigBundle store は resolved payload を保存・検証できるようにする。

実装すること:

- `store_config_bundle` / check API が payload digest を検証する。
- unsupported declaration / unsupported host policy / missing secret ref を typed diagnostic にする。
- embedded Runtime direct call と remote Runtime REST path が同じ ConfigBundle contract を使う。
- retry / duplicate launch で同じ bundle content は同じ identity として扱える。

### 4. Worker creation を ConfigBundle payload から構築する

Runtime Worker creation は、通常経路で Runtime-local filesystem から Profile discovery しない。

実装すること:

- `CreateWorkerRequest` は Backend が ensure した ConfigBundleRef を参照する。
- Runtime は ConfigBundleRef が保存済み bundle と一致することを検証する。
- Runtime は bundle payload から Worker manifest/config を構築する。
- WorkingDirectory は Worker execution root/cwd としてのみ使う。
- WorkingDirectory を Profile discovery source として使わない。
- ConfigBundle missing / digest mismatch / profile not in bundle / unsupported declaration は typed rejection にする。

### 5. Browser WorkingDirectory launch を ConfigBundle 経由に接続する

既存 Browser WorkingDirectory launch は、Backend-resolved ConfigBundle を必ず経由して Worker を作る。

実装すること:

- Browser request は profile/role selector、working_directory_id、relative cwd / initial input など product-level fields だけを送る。
- Backend は request を ConfigBundleRef + WorkingDirectory + initial input に落とし込む。
- Browser-facing response / error に ConfigBundleRef、bundle digest、Runtime store path、Runtime endpoint、raw path を含めない。
- diagnostic は Browser に必要な sanitized message/code だけを返す。

### 6. compatibility fallback を通常経路から外す

`config_bundle = None` や runtime-local profile discovery fallback が残る場合は、direct worker-runtime CLI / tests / builtin debugging の compatibility path として明示的に隔離する。

実装すること:

- Browser / Workspace Backend launch では fallback を使わない。
- fallback を使う code path には diagnostic または test name で compatibility-only であることを残す。
- `profile_base_dir` / legacy `workspace_root` 呼称が残る場合、通常 launch 経路から参照されていないことを test で確認する。

## 受け入れ条件

- ConfigBundle に resolved launch config payload が実装されている。
- ConfigBundle digest が resolved payload を含む canonical content identity になっている。
- ConfigBundle validation が secret values、raw host paths、Runtime endpoint、socket/session paths、runtime-local mount actual paths を拒否する。
- Backend が profile/role selector から ConfigBundle を構築し、対象 Runtime に ensure/sync/check できる。
- Runtime Worker creation は通常経路で Workspace filesystem / profile base / WorkingDirectory root から Profile discovery を行わない。
- Runtime Worker creation は保存済み ConfigBundleRef と bundle payload から Worker manifest/config を構築する。
- Existing Browser WorkingDirectory launch が ConfigBundle 経由で Worker を作れる。
- Browser-facing API に ConfigBundleRef / bundle digest / Runtime config store / raw path / Runtime endpoint が露出しない。
- Embedded Runtime と remote Runtime の bundle check/sync/create behavior が同じ contract を使う。
- Missing bundle、digest mismatch、profile missing、unsupported declaration、unsupported host policy、missing secret ref は typed diagnostic になる。
- Focused tests が Backend bundle build、Runtime bundle validation、Worker create without Runtime FS profile discovery、remote sync/check path、Browser payload redaction、WorkingDirectory launch success を確認する。
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
- WorkingDirectory materializer の新しい dirty snapshot / cache / remote clone policy。
