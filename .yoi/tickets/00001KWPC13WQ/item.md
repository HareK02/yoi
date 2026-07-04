---
title: 'Config-driven Repository registry for Workspace Backend'
state: 'planning'
created_at: '2026-07-04T10:50:45Z'
updated_at: '2026-07-04T10:50:45Z'
assignee: null
---

## 背景

現在の Workspace Backend の Repository view は、`--workspace .` / cwd / workspace root を暗黙に local Git repository として inspect して表示する薄い read-only view になっている。これは短期 UI には便利だったが、Objective の方向性とは合わない。

今後は Workspace を Git repository root と同一視せず、Backend は明示設定された Repository registry を持つ必要がある。`./.yoi` は当面 backend fs-store / local descriptor として残すが、将来的には `~/.yoi/` 側に Backend store を置き、1 Backend が複数 Workspace を扱える設計へ進む。そのため `--workspace .` は workspace config root の指定に縮退させ、cwd を自動的に Repository として扱う経路は廃止する。

`./` を Repository として扱いたい場合も、`.yoi/workspace-backend.local.toml` に明示的な Repository entry として指定する。

## 要件

- `WorkspaceBackendConfigFile` / `.yoi/workspace-backend.local.toml` に明示的な Repository registry を追加する。
- `[[repositories]]` entry は少なくとも `id`, `provider`, `uri`, optional `display_name`, optional `default_selector` を持てる。
- `uri = "."` は backend process cwd ではなく、workspace config root からの相対 URI/path として解釈する。
- `--workspace` は Repository ではなく workspace config root / local descriptor root として扱う。
- cwd / workspace root を暗黙 Repository として Browser/API に返す fallback を廃止する。
- `/api/repositories` は configured repositories だけを返す。
- Repository 未設定時は empty list と typed diagnostic を返し、暗黙の cwd Repository を作らない。
- v0 provider は `git` を主対象にし、必要なら `local_fs` は placeholder/diagnostic 程度に留める。
- `RepositoryId` / `RepositorySelector` / `RepositoryPoint` の用語と将来拡張を壊さない型境界にする。Selector は provider-specific な未解決 locator、Point は解決済み evidence であり、Git branch/tag/hash 固定の抽象化にしない。
- Browser-facing response は token/auth ref/secret、不要な absolute host path、internal config/store/runtime paths を漏らさない。

## 受け入れ条件

- `.yoi/workspace-backend.local.toml` に `[[repositories]]` を書ける。
- dogfood workspace config に `id = "main"`, `provider = "git"`, `uri = "."` 相当の Repository が明示されている。
- `/api/repositories` は configured Repository のみを返し、`--workspace` / cwd を自動 Repository として返さない。
- Repository 未設定 workspace では `/api/repositories` が empty list + `repository_config_empty` 相当の warning diagnostic を返す。
- configured Git Repository については branch/head/status/log など既存 UI が必要とする summary を取得できる。
- `/api/repositories/{id}/log` は configured Git Repository id のみ受け付け、未知 id / unsupported provider には typed sanitized error を返す。
- 既存 web UI は Repository 未設定時にも壊れず、明示 Repository がある場合だけ repository view を表示できる。
- `cargo test -p yoi-workspace-server` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。

## 非目標

- Runtime materialization / Execution Workspace 作成をこのチケットで実装すること。
- multi-workspace Backend store を `~/.yoi/` に完全移行すること。
- Repository credential / clone / fetch / write operation を実装すること。
- Ticket target から RepositoryPoint を解決して Worker launch に渡すこと。
