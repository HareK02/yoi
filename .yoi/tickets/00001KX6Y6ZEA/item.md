---
title: 'Replace Worker workspace_root path with WorkspaceBackend'
state: 'planning'
priority: 'P1'
created_at: '2026-07-10T21:16:22Z'
updated_at: '2026-07-10T21:32:47Z'
assignee: null
---

## 背景

Worker 実装は現在 `workspace_root: PathBuf` を持ち、project records / memory / workflow / Ticket config / Profile context / child spawn inheritance / default scope など複数の意味に使っている。しかし workspace identity と local filesystem authority が path で混ざっており、embedded no-workdir Worker のように「workspace 情報には API 経由でアクセスできるが local filesystem には触れない」状態を自然に表現できない。

workspace は local path ではなく Backend/API 側の identity として扱うべきである。Worker が durable/context identity として持つのは `Option<WorkspaceId>` に留め、`WorkspaceBackendRef` や backend adapter/client の materialization は Runtime/host 側が持つ。Worker は Runtime から注入された narrow `WorkspaceClient` / handle を通じて workspace-aware API に request する。local filesystem authority は別チケットの `WorkerFilesystemAuthority` に分離する。

## 要件

- Worker の workspace identity/context を `workspace_root: PathBuf` から `workspace_id: Option<WorkspaceId>` へ移行する設計を導入する。
- `WorkspaceBackendRef` は Worker ではなく Runtime/host 側が保持し、backend endpoint / auth / adapter selection / capability discovery を解決する。
- Runtime は `WorkspaceBackendRef` から scoped `WorkspaceClient` / handle を materialize し、Worker に注入する。
- Worker は `WorkspaceBackendRef` や secret を自分で解決せず、注入された narrow client/handle を通じて workspace-aware API に request する。
- `workspace_id` と `filesystem` を分離し、workspace id/client が存在しても local filesystem authority があるとは扱わない。
- workspace-aware 機能（Ticket / memory / workflow / project records / workspace metadata）は Backend API 経由で取得する方向に寄せる。
- local path は Worker の workspace identity として保持せず、必要な場合も backend 実装の内側に閉じ込める。
- 移行中は既存 `workspace_root` 利用箇所を一括削除しなくてよいが、新規 filesystem authority の根拠として使わない境界を明確にする。

## capability の扱い

Capability は最小実装では必須ではない。まずは Runtime/host 側が `workspace_id` から backend binding を解決し、backend 側の実 enforcement で成立する。

ただし capability は次の目的で有用である。

- Worker に登録する workspace-aware tools の表示・非表示を決める。
- read-only / read-write / unavailable の UX とエラーを fail-closed にする。
- Backend/API version 差分や remote backend の未対応機能を明示する。
- prompt/tool schema surface を不要に広げない。

一方で capability は client-declared authority にしてはいけない。最終的な権限判定は必ず backend が enforcement し、Worker 側 capability は advertised/discovered contract または UI/tool registration hint として扱う。

## 想定する型の方向性

```rust
// Worker が durable/context identity として持つもの。
struct WorkerContext {
    workspace_id: Option<WorkspaceId>,
    workspace_client: Option<WorkspaceClientHandle>,
    filesystem: WorkerFilesystemAuthority,
}

// Runtime/host 側が保持・解決する backend binding source。
enum WorkspaceBackendRef {
    None,
    Rest {
        base_url: Url,
        workspace_id: WorkspaceId,
        // Optional after minimal implementation:
        // auth: Option<SecretRef>,
    },
}

// Runtime が Worker 起動時に materialize して注入する narrow client。
trait WorkspaceClient {
    // ticket / memory / workflow / project-record API を必要に応じて提供する。
}
```

`workspace_id` は Worker の durable identity であり、接続能力そのものではない。`WorkspaceBackendRef` / endpoint / secret / adapter は Runtime/host 側の責務とする。

## 受け入れ条件

- Worker context に workspace identity を path ではない `Option<WorkspaceId>` として保持できる。
- Runtime/host が `WorkspaceBackendRef` を保持・解決し、Worker に scoped `WorkspaceClient` / handle を注入できる。
- `workspace_id = None` と `filesystem = None` の会話専用 Worker を表現できる。
- `workspace_id = Some(...)` かつ `filesystem = None` の、workspace API は使えるが local filesystem は使えない Worker を表現できる。
- workspace-aware tools は注入された `WorkspaceClient` / handle を通して backend/API にアクセスし、local workspace path を authority として要求しない方向へ移行できる。
- Worker が `WorkspaceBackendRef` / REST endpoint / SecretRef を直接解決しない境界が明確である。
- capability を導入する場合は backend-enforced authority ではなく tool registration / UX / discovery hint として扱われる。
- `WorkerFilesystemAuthority` チケットとの境界が明確で、filesystem authority と workspace identity/client が混同されない。
- `cargo test` と `nix build .#yoi` が通る。
