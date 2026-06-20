---
title: 'MCP: add yoi CLI inspection commands'
state: 'queued'
created_at: '2026-06-20T13:29:16Z'
updated_at: '2026-06-20T13:31:00Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-20T13:31:00Z'
---

## 背景

MCP local stdio integration は server config / lifecycle client / ToolRegistry registration / tools-call / resources-prompts explicit operations / list_changed diagnostics まで進んだ。一方で、ユーザーが `yoi` CLI から MCP の設定・解決結果・接続状態・discovered capabilities を確認する read-only inspection surface がまだ整理されていない。

Plugin には `yoi plugin list/show` があるが、MCP は plugin model そのものではなく protocol-bound bridge/runtime kind として扱う決定になっている。そのため MCP には専用の CLI namespace を用意し、設定済み server と provider-discovered tools/resources/prompts、起動/初期化/refresh diagnostics を確認できるようにする。

## 要件

- `yoi mcp ...` もしくは同等の top-level CLI namespace を追加する。
- 初期 scope は read-only inspection とし、MCP server process の起動・Tool 実行・resources/prompts の content fetch は通常 runtime/tool path を迂回して行わない。
- 最低限のコマンドを提供する。
  - `yoi mcp list` または `yoi mcp server list`: resolved MCP server の一覧。
  - `yoi mcp show <server>`: server config/ref、transport、trust policy、resolved status、diagnostics、capability summary。
  - `yoi mcp tools [<server>]`: provider-discovered MCP tools と Yoi 側 stable tool name / registration eligibility / diagnostics。
  - `yoi mcp resources [<server>]`: discovered resources/resource templates の summary と explicit operation eligibility。
  - `yoi mcp prompts [<server>]`: discovered prompts の summary と explicit operation eligibility。
- 各コマンドは human-readable output と `--json` を持つ。
- `--workspace <PATH>` と `--profile <REF>` を Plugin CLI と同じ感覚で扱い、resolved Profile/config から MCP server configuration を読む。
- 出力は bounded / content-safe にする。
  - resource contents、prompt full text、secret values、environment secret refs の解決結果は表示しない。
  - external server 由来の descriptions/schema/annotations は untrusted data として扱い、必要なら truncate する。
- 接続済み live Pod state が必要な情報と static config inspection だけで得られる情報を区別する。
  - static mode では config/resolution/package-less diagnostics を表示する。
  - live state が未実装または unavailable の場合は、silent stale state ではなく `not live / unavailable` を明示する。
- `notifications/*/list_changed` 由来の restart/reinitialize-required diagnostics がある場合は CLI で見えるようにする。
- CLI help に MCP namespace を追加する。

## Non-goals

- MCP tool を CLI から直接実行すること。
- MCP resources/prompts の content を CLI から直接取得して表示すること。
- MCP server の install/update/distribution。
- Streamable HTTP / remote auth / OAuth の実装。
- sampling / elicitation の実行または approval flow。
- Plugin CLI に MCP を混ぜること。

## 受け入れ条件

- `yoi --help` に MCP CLI namespace が表示される。
- `yoi mcp list --json` が resolved MCP server の bounded structured report を返す。
- `yoi mcp show <server> --json` が server identity、transport kind、trust policy summary、capabilities summary、diagnostics を返す。
- `yoi mcp tools [<server>] --json` が Yoi stable tool name と MCP server/tool identity、schema availability、registration status/diagnostics を返す。
- `yoi mcp resources [<server>] --json` と `yoi mcp prompts [<server>] --json` が content を取得せず summary/eligibility を返す。
- Human-readable output は empty/missing/invalid/unavailable を区別して表示する。
- Secrets and resource/prompt contents are not printed in normal or JSON output.
- Focused CLI tests cover list/show/tools/resources/prompts, missing server, invalid config, and JSON output.
- Validation before completion includes `cargo fmt --check`, focused MCP/CLI tests, `cargo check`, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`.
