---
title: 'embedded worker-runtimeをworker crate実行に接続する'
state: 'queued'
created_at: '2026-06-27T18:26:46Z'
updated_at: '2026-06-27T19:08:35Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-27T19:06:30Z'
---

## 背景

`worker-runtime` の embedded Runtime は Backend RuntimeRegistry / Worker Console から見える Worker を作れるが、実際の LLM agent 実行は既存 `worker` crate 側にある。現在はこの execution-plane が接続されていないため、embedded Worker は transcript / observation の箱としては見えるが、user input を実行できない。

この Ticket では、前段の execution backend 境界に対して、既存 `worker` crate を使う embedded execution adapter を実装する。Runtime が Worker を spawn したときに実 `worker::Worker` / `WorkerController` 相当を生成し、input を既存 Worker の run lifecycle に渡し、`protocol::Event` を Runtime observation bus に bridge する。

## 目的

- embedded Runtime Worker が既存 `worker` crate の実行 engine を使って動く。
- user input が fake response ではなく、実 Worker run / LLM execution に渡る。
- 既存 `protocol::Event` が Runtime observation stream に流れる。
- Runtime / Backend / Web Console が Worker 実行の正規経路を共有する。

## 要件

### Adapter placement / dependency boundary

- `worker-runtime` と `worker` crate の依存関係を確認し、循環依存を作らない場所に adapter を置く。
  - `worker-runtime` の optional feature、別 adapter crate、または Backend 側 adapter のいずれかを選んで Ticket thread に理由を記録する。
- Adapter は `worker-runtime` の execution backend 境界に接続する。
- Runtime public API / Browser-facing API に `worker::Worker` handle、socket path、session path、secret、raw manifest path を露出しない。

### Profile / config / authority resolution

- Runtime Worker spawn request の Profile selector / config bundle ref から、既存 Worker 実行に必要な config を解決する。
- prompt resources、model/provider config、SecretRef、ToolRegistry、HostAuthority、memory/session/history scope の扱いを明示する。
- Backend internal Companion 用であっても、暗黙の broad workspace authority を付けない。
- config / secret / provider が不足している場合は typed diagnostic として spawn/input を拒否し、fake response を返さない。

### Input / run lifecycle

- Backend Worker input API からの user input を、既存 Worker の `Method::Run` 相当または同等の run API に渡す。
- Worker が busy の場合、二重 run を typed rejection / queue policy で扱う。v0 で queue しないなら明示する。
- run started / completed / errored / cancelled を Runtime Worker status と transcript projection に反映する。
- stop / cancel が実装できる場合は既存 Worker control に接続し、できない場合は typed unsupported とする。

### Protocol event bridge

- 既存 Worker が出す `protocol::Event` を Runtime observation bus へ bridge する。
- Snapshot / transcript reconstruction / text delta / text done / thinking / tool call / tool result / usage / status / error を、既存 protocol semantics のまま流す。
- Runtime が event variant subset や別 output model を作らない。
- Worker history / session persistence と Runtime transcript projection が矛盾しないようにする。

## Non-goals

- Workspace Companion 専用 UX の完成。
- Web Console の visual redesign。
- remote Runtime process の execution adapter 実装。
- Full multi-user auth / permission / redaction policy。
- Task scheduling / background queue。

## 受け入れ条件

- embedded Runtime に `worker` crate 実行 adapter が接続されている。
- embedded Worker への user input が実 Worker run に渡る。
- provider / config が不足している場合、fake response ではなく typed diagnostic になる。
- 実行時の `protocol::Event` が Runtime observation bus と Backend client-facing WS に流れる。
- transcript projection が実 Worker run の user / assistant / tool / error output と整合する。
- raw execution handle / path / credential / session path が Browser-facing API に漏れない。
- Focused tests が fake provider または deterministic provider で user input -> assistant output -> protocol events -> transcript projection を確認する。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
