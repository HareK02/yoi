---
title: '旧Pod関連クレートを削除しWorker/Runtime storeへ整理する'
state: 'closed'
created_at: '2026-06-28T13:53:21Z'
updated_at: '2026-06-28T20:10:27Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-28T16:51:00Z'
---

## 背景

Runtime / Worker への移行後も、workspace には旧 Pod 実行系由来の `pod-registry` / `pod-store` crate と、それらに依存する TUI / worker / yoi CLI 経路が残っている。`pod-registry` は旧 Pod process / socket / delegated scope cleanup の registry であり、canonical Runtime Worker creation / fs-store / execution backend mapping の authority にすべきではない。

一方で `pod-store` には、名前は Pod だが現在の `worker` crate がまだ内部永続化として使っている `WorkerMetadataStore` / `FsWorkerStore` / `CombinedStore` などが含まれている。これをそのまま Runtime authority として持ち込むのは誤りだが、必要な Worker metadata/session persistence は Worker/Runtime 側の正しい crate に移してから削除する必要がある。

この Ticket では、旧 Pod 関連 crate と依存経路を棚卸しし、何を削除し、何を Worker/Runtime の正規 store として残すかを明確にしたうえで、`pod-registry` / `pod-store` を workspace から削除する。

## 目的

- 旧 Pod registry / metadata store を Runtime Worker authority から完全に外す。
- `crates/pod` は既に workspace から消えているため、再導入しない。残存調査の対象は `pod-registry` / `pod-store` と、それらに依存して残っている旧 Spawn/Stop/metadata/discovery 経路とする。
- `crates/pod-registry` を削除する。
- `crates/pod-store` を削除する。ただし必要な Worker metadata/session store 機能は正しい名前・責務の crate へ移す。
- Runtime Worker creation / persistence は `worker-runtime` fs-store と execution backend mapping を正とする。
- 旧 Pod socket / pod name / pod registry path / spawned pod relation を Worker identity にしない。

## 残すもの / 消すもの

### 残す

- `crates/protocol`
  - `protocol::Event` / `protocol::Method` など、Worker/TUI/Runtime の通信 protocol としてまだ現役のため残す。
  - ただし Pod 専用 naming / semantics が残る場合は別途 rename 対象として扱う。
- `crates/llm-engine`
  - LLM provider 呼び出し / streaming / completion 実行 engine として残す。
  - Pod registry / Pod store authority を持たせない。
- `crates/worker`
  - `llm-engine`、tools、memory、session、protocol event handling を束ねる agent/Worker orchestration layer として残す。
  - ただし `pod-registry` / `pod-store` 依存は除去または Worker/Runtime store 依存へ置換する。
- `crates/worker-runtime`
  - Runtime catalog / fs-store / Worker execution backend mapping の正規 authority として残す。
- `crates/session-store`
  - Worker history/session persistence として必要な範囲を残す。
- Worker metadata/session persistence のうち、実 Worker 実行に必要な generic 機能
  - `WorkerMetadataStore` 相当が必要なら、`pod-store` ではなく `worker` / `worker-runtime` / `session-store` の適切な場所へ移す。

### 既に消えている

- `crates/pod`
  - workspace 上には既に存在しない。
  - この Ticket では再導入を禁止し、残っている `pod-registry` / `pod-store` と旧 Spawn/Stop/discovery/metadata 経路の削除に集中する。

### 消す

- `crates/pod-registry`
  - 旧 Pod process registry / lock file / delegated scope allocation / cleanup authority。
  - Runtime Worker creation authority として使わない。
- `crates/pod-store`
  - crate としては削除する。
  - `FsWorkerStore` / `WorkerMetadata` など必要な型は、Pod naming を捨てて正しい Worker store 境界へ移すか、不要なら削除する。
- `manifest::paths::pod_registry_path` など pod registry path surface。
- `worker::runtime::pod_registry` re-export。
- SpawnPod / StopPod / delegated Pod cleanup を前提にした tool / registry path / tests。
- TUI / yoi CLI の旧 Pod metadata inventory / cleanup 表示。
  - 必要な UX は Runtime Worker API / RuntimeRegistry / worker-runtime fs-store へ移行する。
- Browser / Backend / Runtime API に旧 Pod metadata path / socket path / pod name を露出する経路。

## 要件

### Dependency audit

- workspace 内の `pod-registry` / `pod-store` dependency を全列挙する。
- 各依存を以下に分類する。
  - delete: 旧 Pod lifecycle 専用で削除する。
  - move/rename: generic Worker metadata/session store として必要なので移す。
  - replace: RuntimeRegistry / worker-runtime fs-store / session-store に置き換える。
  - historical/test-only: historical report や obsolete test fixture として扱う。
- audit 結果を Ticket thread または artifact に残す。

### Verify absent `pod` crate

- `crates/pod` は既に存在しないため、削除作業ではなく不在確認と再導入防止を行う。
- `Cargo.toml` workspace members / dependencies に `crates/pod` / `pod = ...` がないことを確認する。
- 旧 Pod feature adapter / Ticket tool adapter / HostAuthority adapter が必要機能として残っている場合は、既に他 crate へ移管済みか、別途正しい crate で実装されているかを確認する。

### Remove `pod-registry`

- `crates/pod-registry` を workspace members / dependencies から削除する。
- lockfile / package.nix / Nix source filtering を更新する。
- `pod_registry::default_registry_path` / lock / delegated scope APIs を呼ぶ経路を削除または Runtime/Worker authority へ置換する。
- StopPod / SpawnPod legacy cleanup は current develop の正規 path から削除し、before-worker hotfix が必要なら別 branch / 別 Ticket で扱う。

### Remove / replace `pod-store`

- `crates/pod-store` を workspace members / dependencies から削除する。
- `WorkerMetadata` / `WorkerMetadataStore` / `FsWorkerStore` / `CombinedStore` がまだ必要な場合は、Pod ではない名前と責務で移す。
- 移行先は実装時に選ぶが、Runtime Worker authority を歪めないこと。
  - Runtime catalog / Worker identity は `worker-runtime` fs-store。
  - execution session/history は `session-store` / `worker` internal store。
  - Runtime Worker id と execution session mapping は Runtime fs-store 側。
- 旧 `~/.yoi/pods` metadata を正規 store として読まない。

### UI / CLI cleanup

- TUI / yoi CLI / worker cleanup CLI から旧 Pod inventory / metadata cleanup 依存を削除する。
- 必要な list / attach / cleanup UX は Runtime Worker API / RuntimeRegistry / worker-runtime fs-store を正とする。
- Help text / diagnostics / tests に `pod` が active concept として残らないようにする。
- Historical docs / reports は対象外にしてよいが、active prompt / UI / CLI / protocol naming は分類する。

### Runtime authority invariant

- Runtime Worker creation は canonical `CreateWorker` / ConfigBundle / ExecutionBackend / fs-store 経路を通る。
- `pod-registry` / `pod-store` が Runtime Worker identity、scope authority、socket authority、Browser-facing projection の source にならない。
- Worker として公開されるものは transcript / observation / execution / persistence contract を満たす。

## Non-goals

- before-worker tag 系列の StopPod hotfix。
- Historical report / old migration note の全面 rewrite。
- Public product rename。
- Full multi-user auth / permission / redaction policy。
- `protocol` crate の全面 rename。
- `worker` crate 自体の削除。

## 受け入れ条件

- `crates/pod` が workspace に存在せず、再導入されていないことが確認されている。
- `crates/pod-registry` が workspace から削除されている。
- `crates/pod-store` が workspace から削除されている。
- `Cargo.toml` workspace members / dependencies / `Cargo.lock` / `package.nix` が更新されている。
- `pod` / `pod-registry` / `pod-store` dependency が active crates に残っていない。
- `manifest::paths::pod_registry_path` など active pod registry path API が削除されている。
- Runtime Worker identity / creation / persistence が `worker-runtime` fs-store と execution backend mapping を正にしている。
- `worker` crate 実行に必要な metadata/session persistence が Pod naming / Pod authority に依存しない形へ移行されている。
- TUI / yoi CLI / tests が旧 Pod metadata inventory に依存していない。
- Active UI / CLI / prompt / test output に旧 Pod concept が正規 concept として残っていない。残る場合は legacy/internal/historical として分類されている。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo test -p worker` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
