---
title: 'Implement event-driven WorkerConfigBundle sync for Runtime worker creation'
state: 'planning'
created_at: '2026-07-07T20:51:35Z'
updated_at: '2026-07-07T21:18:00Z'
assignee: null
---

## 背景

Backend -> Runtime の config 境界は既に `ConfigBundle` の sync/check/store の外枠を持っているが、現状の bundle は profile selector 宣言が中心であり、Worker 作成時の実際の Profile discovery / manifest resolution は Runtime 側の `ProfileRuntimeWorkerFactory` が host filesystem から再実行している。

これは正規経路ではない。Workspace は Browser/product の論理単位であり、Runtime は Workspace filesystem を profile/config discovery source として読まない。Backend が Workspace / project / builtin Profile を解決し、Runtime へ同期した Worker 起動用 bundle だけを Worker creation の config authority にする。

この Ticket では既存の一般名 `ConfigBundle` を Worker 起動用途に限定した `WorkerConfigBundle` として整理する。実装上の rename 範囲は code review で確定してよいが、外向きの意味は「Backend-resolved, content-addressed Worker launch config snapshot」で固定する。

同期したい Workspace 情報は主に Profile と、その Profile から Worker 起動に必要になる non-secret launch config である。Ticket / Objective / repository contents / Worker catalog などの Workspace product state 全体を Runtime に同期するものではない。

## 実装目的

- Backend が Profile discovery / resolution を行い、Runtime は通常 Worker creation 経路で Profile filesystem discovery を行わない。
- `WorkerConfigBundle` は profile selector allowlist ではなく、Worker 作成に必要な resolved launch config snapshot を持つ。
- Backend は設定変更時、Profile 変更時、Runtime 登録/再接続時、Backend 起動時 reconcile、または明示 resync 時に bundle を Runtime へ同期する。
- Worker launch 時は既に Backend が生成済みの `WorkerConfigBundle` を参照し、通常経路で bundle generation / Profile discovery を行わない。
- Runtime は同期済み `WorkerConfigBundleRef`、WorkingDirectory、initial input から Worker を作る。
- Browser-facing API には `WorkerConfigBundleRef`、bundle digest、Runtime config store、raw path、Runtime endpoint を出さない。

## 同期モデル

主経路:

```text
Workspace/Profile/config change
  -> Backend resolves profiles into WorkerConfigBundle
  -> Backend computes content digest
  -> Backend syncs bundle to registered/reachable Runtimes

Worker launch
  -> Backend selects an already-generated WorkerConfigBundleRef
  -> Backend checks Runtime availability
  -> Runtime creates Worker from stored bundle + WorkingDirectory + initial input
```

launch 時に許すのは availability check と missing repair sync だけである。launch 時に Profile discovery や bundle generation を行わない。missing repair sync は Backend が既に持っている bundle artifact を Runtime に再送するだけに限定する。repair できない場合は typed diagnostic で止める。

同期トリガー:

- Backend startup reconcile。
- Workspace / Profile / config record change。
- builtin Profile version change。
- Runtime registration / reconnect。
- manual resync。
- Worker launch-time missing repair sync。

## 実装内容

### 1. `WorkerConfigBundle` payload を resolved launch config に拡張する

既存 `ConfigBundle` を `WorkerConfigBundle` として整理し、Backend-resolved profile payload を追加する。新しい payload は selector 宣言だけではなく、Runtime が Worker manifest/config を構築するための typed data を持つ。

payload に含めるもの:

- profile selector。
- display label / role metadata。
- `WorkerManifestConfig` 相当の resolved launch config。
- prompt / resource / workflow を解決するための resource table または digest 付き resource entry。
- non-secret provider/model config refs。
- tool / hook / plugin declaration と policy declaration。
- language / memory / ticket / runtime policy のうち Worker manifest 作成に必要な non-secret config。

payload に含めないもの:

- secret values。
- host absolute path。
- Runtime endpoint / token / socket path / session path。
- runtime-local mount actual path。
- Browser request 由来の raw cwd / raw filesystem scope。
- Ticket / Objective / repository contents / Worker catalog など、Worker 起動 config ではない Workspace product state。

bundle digest は metadata だけではなく、この resolved payload を含む canonical content identity にする。

### 2. Backend に `WorkerConfigBundle` builder と change-driven sync を実装する

Workspace Backend に、Profile/config 変更から `WorkerConfigBundle` を構築・更新する経路を追加する。

実装すること:

- 既存の builtin / workspace / project Profile discovery を Backend 側で呼び出す。
- selector ambiguity / missing profile / invalid profile を Backend diagnostic にする。
- Profile を resolved launch config payload に変換する。
- bundle digest を計算し、変更がある場合だけ Runtime sync 対象にする。
- Backend startup reconcile と Runtime registration/reconnect 時に必要 bundle を sync/check する。
- manual resync を可能にする。
- launch options は Browser に profile/role candidates だけ返し、bundle identity は返さない。

### 3. Runtime の bundle store / sync / check を resolved payload 対応にする

Runtime 側の bundle store は resolved payload を保存・検証できるようにする。

実装すること:

- `store_worker_config_bundle` / check API が payload digest を検証する。
- unsupported declaration / unsupported host policy / missing secret ref を typed diagnostic にする。
- embedded Runtime direct call と remote Runtime REST path が同じ bundle contract を使う。
- retry / duplicate sync で同じ bundle content は同じ identity として扱える。

### 4. Worker creation を `WorkerConfigBundle` payload から構築する

Runtime Worker creation は、通常経路で Runtime-local filesystem から Profile discovery しない。

実装すること:

- `CreateWorkerRequest` は Backend が ensure した `WorkerConfigBundleRef` を参照する。
- Runtime は `WorkerConfigBundleRef` が保存済み bundle と一致することを検証する。
- Runtime は bundle payload から Worker manifest/config を構築する。
- WorkingDirectory は Worker execution root/cwd としてのみ使う。
- WorkingDirectory を Profile discovery source として使わない。
- bundle missing / digest mismatch / profile not in bundle / unsupported declaration は typed rejection にする。

### 5. Browser WorkingDirectory launch を `WorkerConfigBundle` 経由に接続する

既存 Browser WorkingDirectory launch は、Backend-resolved `WorkerConfigBundle` を必ず経由して Worker を作る。

実装すること:

- Browser request は profile/role selector、working_directory_id、relative cwd / initial input など product-level fields だけを送る。
- Backend は request を `WorkerConfigBundleRef` + WorkingDirectory + initial input に落とし込む。
- Worker launch 時は bundle availability check を行う。
- missing の場合は Backend が持つ既存 bundle artifact の repair sync を試すか、typed diagnostic で止める。
- Browser-facing response / error に `WorkerConfigBundleRef`、bundle digest、Runtime store path、Runtime endpoint、raw path を含めない。
- diagnostic は Browser に必要な sanitized message/code だけを返す。

### 6. compatibility fallback を通常経路から外す

`worker_config_bundle = None` や runtime-local profile discovery fallback が残る場合は、direct worker-runtime CLI / tests / builtin debugging の compatibility path として明示的に隔離する。

実装すること:

- Browser / Workspace Backend launch では fallback を使わない。
- fallback を使う code path には diagnostic または test name で compatibility-only であることを残す。
- `profile_base_dir` / legacy `workspace_root` 呼称が残る場合、通常 launch 経路から参照されていないことを test で確認する。

## 受け入れ条件

- `WorkerConfigBundle` に resolved launch config payload が実装されている。
- bundle digest が resolved payload を含む canonical content identity になっている。
- bundle validation が secret values、raw host paths、Runtime endpoint、socket/session paths、runtime-local mount actual paths を拒否する。
- Backend が Workspace/Profile/config change を契機に `WorkerConfigBundle` を生成・更新できる。
- Backend startup reconcile、Runtime registration/reconnect、manual resync、launch-time missing repair sync が実装されている。
- Worker launch 時の通常経路で Profile discovery / bundle generation を行わない。
- Runtime Worker creation は通常経路で Workspace filesystem / profile base / WorkingDirectory root から Profile discovery を行わない。
- Runtime Worker creation は保存済み `WorkerConfigBundleRef` と bundle payload から Worker manifest/config を構築する。
- Existing Browser WorkingDirectory launch が `WorkerConfigBundle` 経由で Worker を作れる。
- Browser-facing API に `WorkerConfigBundleRef` / bundle digest / Runtime config store / raw path / Runtime endpoint が露出しない。
- Embedded Runtime と remote Runtime の bundle check/sync/create behavior が同じ contract を使う。
- Missing bundle、digest mismatch、profile missing、unsupported declaration、unsupported host policy、missing secret ref は typed diagnostic になる。
- Focused tests が Backend bundle build、change-driven sync、Runtime bundle validation、Worker create without Runtime FS profile discovery、remote sync/check path、Browser payload redaction、WorkingDirectory launch success を確認する。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Secret value synchronization。
- Ticket / Objective / repository contents / Worker catalog の Runtime 同期。
- Plugin package manager / signature policy の完成。
- Multi-tenant auth model の完成。
- Browser-facing bundle editor UI。
- Runtime が Workspace filesystem を profile/config discovery source として読む経路の延命。
- WorkingDirectory materializer の新しい dirty snapshot / cache / remote clone policy。
