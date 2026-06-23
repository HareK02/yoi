---
title: 'Persist local Workspace identity in .yoi/workspace.toml'
state: 'inprogress'
created_at: '2026-06-23T06:43:28Z'
updated_at: '2026-06-23T07:25:27Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-23T06:47:18Z'
---

## 背景

Workspace server の local dev bootstrap は現在、`workspace_id` を `local:{display}` のように workspace root の basename から導出している。これは暫定 namespace としては分かりやすいが、DB の primary key / Workspace identity としては不適切である。

問題:

- `workspace_id` が表示名 / path basename から予測可能に作られている。
- 同じ basename の workspace が複数あると衝突しうる。
- workspace root の rename / move で identity が変わる。
- `local:` namespace と display label が DB PK に混ざっている。
- `--workspace .` のような相対 path では display fallback に落ちるなど、初期化責務が `ServerConfig::local_dev` に寄りすぎている。

Workspace identity は `.yoi/workspace.toml` を local Workspace record として永続化し、未作成なら Workspace backend の初期化処理で払い出す。これにより、DB を再作成しても Workspace identity は維持され、将来の DB authority / hosted Workspace / migration でも扱いやすくなる。

## 方針

- `.yoi/workspace.toml` を local Workspace identity の authority とする。
- `workspace_id` は path / display name から導出しない。
- 新規 local Workspace 初期化時に UUIDv7 を発行して `workspace_id` として保存する。
- `display_name` は identity ではなく UI label として保存する。
  - 初期値は canonicalized workspace root の basename でよい。
  - 将来的に rename 可能な field として扱う。
- `ServerConfig::local_dev` は Workspace ID を生成しない。
  - CLI/bootstrap 層で workspace root canonicalization と `.yoi/workspace.toml` load-or-init を行い、結果を `ServerConfig` に渡す。
- `.yoi/workspace.toml` が壊れている場合は勝手に上書きせず、明示的な error で止める。
- 既存 `.yoi/workspace.db` 内の `local:*` workspace row については、初期実装では destructive migration を必須にしない。
  - 新しい UUIDv7 workspace row を upsert し、古い row は残してよい。
  - 必要なら後続で cleanup/migration policy を設計する。

## `.yoi/workspace.toml` v0

最小 schema:

```toml
workspace_id = "0197..." # UUIDv7 canonical string
created_at = "2026-06-23T...Z"
display_name = "yoi"
```

Validation:

- `workspace_id` は UUID として parse できること。
- 可能なら UUID version 7 であることを検証する。
- `created_at` は RFC3339 UTC timestamp として parse できること。
- `display_name` は空文字列でないこと。
- unknown fields の扱いは将来拡張を考え、明示的に決める。
  - v0 では deny でも preserve でもよいが、選択理由を実装に残す。

## 初期化フロー

`yoi-workspace-server serve --workspace <path>` の local bootstrap は将来的に以下の責務分離にする。

1. `--workspace` を canonicalize する。
2. workspace root 配下の `.yoi/` を確認し、なければ作成する。
3. `.yoi/workspace.toml` を読む。
4. 存在しない場合:
   - UUIDv7 `workspace_id` を発行する。
   - `created_at` を現在 UTC にする。
   - `display_name` を canonicalized workspace root basename から作る。
   - `.yoi/workspace.toml` を atomic-ish に書き込む。
5. 存在する場合:
   - parse / validate する。
   - 壊れていたら停止し、上書きしない。
6. `ServerConfig` には canonicalized workspace root と Workspace identity を渡す。
7. SQLite `workspaces` row は `.yoi/workspace.toml` の `workspace_id` / `display_name` で upsert する。
8. API `/api/workspace` は DB row / identity record を元に stable id と display name を返す。

## 実装候補

- `crates/workspace-server` に Workspace identity loader を追加する。
  - 例: `workspace_identity.rs` または `identity.rs`。
- 型例:

```rust
struct WorkspaceIdentity {
    workspace_id: Uuid,
    created_at: DateTime<Utc>,
    display_name: String,
}

impl WorkspaceIdentity {
    fn load_or_init(root: &Path) -> Result<Self, WorkspaceIdentityError>;
}
```

- `uuid` crate の UUIDv7 support を確認し、必要なら依存 feature を追加する。
- TOML parse/write には既存 dependency を優先し、なければ最小 dependency を検討する。
- `ServerConfig::local_dev` は `workspace_id` / `display_name` を引数または identity object で受け取る形に寄せる。

## Non-goals

- Hosted / multi-user Workspace identity の完全設計。
- Workspace rename UI。
- 既存 `.yoi/workspace.db` の destructive migration。
- Ticket / Objective storage の DB authority migration。
- `.yoi/workspace.toml` を全 Workspace settings store に拡張すること。

## 受け入れ条件

- local Workspace identity が `.yoi/workspace.toml` に永続化される。
- `.yoi/workspace.toml` 未作成時、Workspace backend 初期化時に UUIDv7 `workspace_id` が払い出される。
- `workspace_id` が path basename / display name から導出されなくなる。
- `display_name` は `.yoi/workspace.toml` の field として扱われ、identity と分離されている。
- `--workspace .` でも canonicalized workspace root をもとに初期化される。
- `.yoi/workspace.toml` が壊れている場合、勝手に再生成/上書きせず error で停止する。
- `ServerConfig::local_dev` または同等の bootstrap helper が workspace id を自前生成しない。
- SQLite `workspaces` row は `.yoi/workspace.toml` の identity を使って upsert される。
- 既存 `local:{display}` 形式は新規 workspace id として生成されない。
- `cargo test -p yoi-workspace-server`、`git diff --check`、`nix build .#yoi --no-link` が通る。
