## Resolution

`00001KVJKHAFE` を完了しました。

実装内容:
- `yoi mcp` CLI namespace を追加しました。
- Read-only inspection commands を追加しました。
  - `yoi mcp list`
  - `yoi mcp show <server>`
  - `yoi mcp tools [<server>]`
  - `yoi mcp resources [<server>]`
  - `yoi mcp prompts [<server>]`
- Human-readable output と `--json` output を追加しました。
- Inspection は static/resolved config のみを扱い、MCP server process を起動しません。
- `tools/call`, `resources/read`, `prompts/get` は実行しません。
- Live/provider-discovered state は `not_live` / `unavailable` と明示します。
- Env values, secret refs, env refs, args, resource content, prompt content は redacted/omitted します。
- Resource/prompt operation eligibility は content fetch なしで報告します。
- MCP namespace は Plugin CLI namespace と分離したままです。

主な commit:
- `c91f5fc9 mcp: add cli inspection`
- `5e0b023a merge: mcp cli inspection`

Review:
- r1 は `approve`。
- Reviewer は read-only boundary、no process start、no tools/resource/prompt content fetch、static-vs-live unavailable state、secret/content redaction、MCP namespace separation、help/tests を確認しました。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p yoi mcp`
- `cargo check -p yoi`
- `cargo run -q -p yoi -- --help` + MCP command grep
- `TicketDoctor`: 0 errors

Known unrelated note:
- `TicketDoctor` は既存 Ticket の warning 4 件を返しましたが、この Ticket の変更とは無関係です。

Nix validation:
- Not run because no dependency/package/source-filter files changed。

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-xrqves.log`