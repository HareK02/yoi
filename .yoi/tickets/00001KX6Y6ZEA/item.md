---
title: 'Replace Worker workspace_root path with WorkspaceBackend'
state: 'planning'
created_at: '2026-07-10T21:16:22Z'
updated_at: '2026-07-10T21:16:50Z'
assignee: null
---

## 背景

Worker 実装は現在 `workspace_root: PathBuf` を持ち、project records / memory / workflow / Ticket config / Profile context / child spawn inheritance / default scope など複数の意味に使っている。しかし workspace identity と local filesystem authority が path で混ざっており、embedded no-workdir Worker のように「workspace 情報には API 経由でアクセスできるが local filesystem には触れない」状態を自然に表現できない。

workspace は local path ではなく Backend/API への handle として Worker に渡すべきである。local filesystem authority は別チケットの `WorkerFilesystemAuthority` に分離し、workspace 情報は `WorkspaceBackend` / `WorkspaceBackendRef` 経由で取得する。

## 要件

- Worker の workspace identity/context を `workspace_root: PathBuf` から `WorkspaceBackendRef` 相当へ移行する設計を導入する。
- 最小形として `WorkspaceBackendRef::None` と `WorkspaceBackendRef::Rest { base_url, workspace_id, ... }` を表現できる。
- `workspace` と `filesystem` を分離し、workspace backend が存在しても local filesystem authority があるとは扱わない。
- workspace-aware 機能（Ticket / memory / workflow / project records / workspace metadata）は Backend API 経由で取得する方向に寄せる。
- local path は Worker の workspace identity として保持せず、必要な場合も backend 実装の内側に閉じ込める。
- 移行中は既存 `workspace_root` 利用箇所を一括削除しなくてよいが、新規 filesystem authority の根拠として使わない境界を明確にする。

## capability の扱い

Capability は最小実装では必須ではない。まずは `None` / `Rest { base_url, workspace_id }` と backend 側の実 enforcement だけで成立する。

ただし capability は次の目的で有用である。

- Worker に登録する workspace-aware tools の表示・非表示を決める。
- read-only / read-write / unavailable の UX とエラーを fail-closed にする。
- Backend/API version 差分や remote backend の未対応機能を明示する。
- prompt/tool schema surface を不要に広げない。

一方で capability は client-declared authority にしてはいけない。最終的な権限判定は必ず backend が enforcement し、Worker 側 capability は advertised/discovered contract または UI/tool registration hint として扱う。

## 想定する型の方向性

```rust
struct WorkerContext {
    workspace: WorkspaceBackendRef,
    filesystem: WorkerFilesystemAuthority,
}

enum WorkspaceBackendRef {
    None,
    Rest {
        base_url: Url,
        workspace_id: WorkspaceId,
        // Optional after minimal implementation:
        // auth: Option<SecretRef>,
        // capabilities: WorkspaceCapabilities,
    },
}
```

## 受け入れ条件

- Worker context に workspace backend identity を path ではない形で保持できる。
- `workspace = None` と `filesystem = None` の会話専用 Worker を表現できる。
- `workspace = Rest(...)` かつ `filesystem = None` の、workspace API は使えるが local filesystem は使えない Worker を表現できる。
- workspace-aware tools は `WorkspaceBackendRef` を通して backend/API にアクセスし、local workspace path を authority として要求しない方向へ移行できる。
- capability を導入する場合は backend-enforced authority ではなく tool registration / UX / discovery hint として扱われる。
- `WorkerFilesystemAuthority` チケットとの境界が明確で、filesystem authority と workspace backend が混同されない。
- `cargo test` と `nix build .#yoi` が通る。
