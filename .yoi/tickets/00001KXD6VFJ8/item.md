---
title: 'Remove Runtime-owned runtime_id'
state: 'inprogress'
created_at: '2026-07-13T07:42:49Z'
updated_at: '2026-07-13T07:43:36Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T07:43:36Z'
---

## 背景

Remote Runtime は backend から URL で到達される実体であり、backend 側の runtime id / alias は workspace-server が endpoint を識別するための名前である。Runtime process / Worker は、その backend alias を感知する必要がない。

現在は worker-runtime crate の `RuntimeId` / `WorkerRef { runtime_id, worker_id }` が Runtime 内部、fs-store、REST server、Worker 名生成に入り込んでおり、`--runtime-id` を指定しないと restart crossing restore が成立しない。これは backend の識別子を Runtime 実体に渡している設計で、remote alias と Runtime 内部永続化 namespace が混ざっている。

## 要件

- `worker-runtime` の Runtime 実装から Runtime-owned `runtime_id` を削除する。
- `worker-runtime` の Worker identity は Runtime-local `WorkerId` / `WorkerRef` にする。
- Worker / Worker controller / restore 用 deterministic Worker name は backend alias や Runtime id に依存しない。
- REST runtime server の `--runtime-id` 指定を削除する。
- fs-store は runtime id ごとのディレクトリ分割をやめ、指定された root 自体を単一 Runtime store として扱う。
- Workspace backend の runtime id / alias は workspace-server 側の endpoint identifier として残し、Runtime 内部へ渡さない。
- 既存の frontend / workspace-server API で必要な runtime id は workspace-server projection 層で付与する。

## 受け入れ条件

- `cargo run -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --store fs --fs-root <dir>` が `--runtime-id` なしで durable Runtime として起動する。
- 同じ fs-root で再起動した時、persisted Worker record が Runtime-local id で読める。
- `Runtime` 内部 API は Worker lookup に backend alias を要求しない。
- workspace-server remote runtime id は引き続き URL endpoint alias として機能する。
