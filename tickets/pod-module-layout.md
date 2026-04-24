# pod クレートのモジュール分割

## 背景

`crates/pod/src/` は 25 ファイル・約 10k 行がフラットに並んでおり、責務の軸が立っていない。命名も `pod_events` / `pod_interceptor` / `pod_comm_tools` のように、所属クレート名である `pod_` prefix を冗長に付けているものと、`spawn_pod` / `spawned_pod_registry` のように prefix なしのものが混在しており、どの単位で読むべきかが構造から読み取れない。

同規模の `llm-worker` は `llm_client/scheme/{anthropic,gemini,...}/` と `timeline/` でサブモジュール化されており、`provider` も `codex_oauth/` を切っている。`pod` だけが規模に対して階層を持っていない。

## 要件

### 目標レイアウト

```
crates/pod/src/
├── lib.rs
├── main.rs
├── pod.rs                   ← Pod 本体
├── controller.rs            ← PodController / PodHandle
├── factory.rs               ← manifest cascade からの組み立て
├── shared_state.rs          ← 横断的な共有状態
│
├── ipc/                     ← Pod ↔ クライアント（自 Pod の対外 socket）
│   ├── mod.rs
│   ├── server.rs            ← socket_server.rs
│   ├── event.rs             ← pod_events.rs
│   ├── interceptor.rs       ← pod_interceptor.rs
│   ├── notifier.rs
│   └── notification_buffer.rs
│
├── spawn/                   ← Pod ↔ Pod（親子関係とツール群）
│   ├── mod.rs
│   ├── tool.rs              ← spawn_pod.rs
│   ├── registry.rs          ← spawned_pod_registry.rs
│   └── comm_tools.rs        ← pod_comm_tools.rs
│
├── prompt/
│   ├── mod.rs
│   ├── catalog.rs           ← prompts.rs
│   ├── loader.rs            ← prompt_loader.rs
│   ├── system.rs            ← system_prompt.rs
│   └── agents_md.rs
│
├── compact/
│   ├── mod.rs
│   ├── state.rs             ← compact_state.rs
│   ├── worker.rs            ← compact_worker.rs
│   ├── token_counter.rs
│   ├── usage_tracker.rs
│   └── prune.rs
│
├── hook.rs                  ← フック基盤（将来拡張予定だが現在 1 ファイルなのでフラットに残す）
├── interrupt_and_run.rs     ← `impl Pod` の付加メソッド（pod.rs の延長）
│
└── runtime/
    ├── mod.rs
    ├── dir.rs               ← runtime_dir.rs
    └── scope_lock.rs
```

### 軸の置き方

- **`ipc/` と `spawn/` を分離**する: 前者は「自 Pod 外界（クライアント）との通信」、後者は「他 Pod（子）との通信」。対向が違うので混ぜない。
- **`compact/` に会計系を集約**する: `token_counter` / `usage_tracker` / `prune` は compaction の判断材料として利用側から見ると一塊。
- **トップレベルに残すのは核 4 個**: `pod` / `controller` / `factory` / `shared_state`。どのサブに入れても不自然になる「束ねる側」。
- **`pod_` prefix を全廃**する: `pod` crate 内で `pod::pod_events` は冗長。`pod::ipc::event` にする。

### 公開 API パスの変化

`lib.rs` の `pub mod` 経由で露出している型は外部からパスが変わる。例:

| 旧 | 新 |
|---|---|
| `pod::socket_server::SocketServer` | `pod::ipc::server::SocketServer` |
| `pod::pod_events::{...}` | `pod::ipc::event::{...}` |
| `pod::pod_comm_tools::{...}` | `pod::spawn::comm_tools::{...}` |
| `pod::spawn_pod::spawn_pod_tool` | `pod::spawn::tool::spawn_pod_tool` |
| `pod::spawned_pod_registry::SpawnedPodRegistry` | `pod::spawn::registry::SpawnedPodRegistry` |
| `pod::runtime_dir::{...}` | `pod::runtime::dir::{...}` |
| `pod::scope_lock::{...}` | `pod::runtime::scope_lock::{...}` |

現時点で `pod::XXX` パスを参照しているのは `crates/pod/tests/` のみで、他クレートからの `use pod::<submodule>` 参照は無い（`manifest` / `llm-worker` 内の該当箇所はコメントのみ）。

### スコープ外

- `pod.rs`（1496 行）・`scope_lock.rs`（1103 行）自体の内部分割は別チケット。本チケットは **ファイルの配置と命名の変更のみ** を扱い、1 ファイルの中身は移動だけに留める。
- `lib.rs` の `pub use` re-export で旧パスを残す互換レイヤは作らない（外部参照が tests のみで、一斉更新できるため）。

### 完了条件

- 新構造でビルドが通る (`cargo check`)
- `crates/pod/tests/` が全部緑 (`cargo test -p pod`)
- 既存の振る舞いに変更なし（純粋な再配置）

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./pod-module-layout.review.md](./pod-module-layout.review.md)
- 日付: 2026-04-24
