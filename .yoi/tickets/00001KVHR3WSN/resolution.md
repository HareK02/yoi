## Resolution

`00001KVHR3WSN` を完了しました。

実装内容:
- MCP `resources/list`, `resources/read`, `prompts/list`, `prompts/get` の typed protocol structs / helpers を追加しました。
- Server capabilities に応じて explicit namespaced Yoi tools を登録します。
  - `Mcp_<server>_resources_list`
  - `Mcp_<server>_resources_read`
  - `Mcp_<server>_prompts_list`
  - `Mcp_<server>_prompts_get`
- Resources/prompts operations は ordinary Tool path / `ToolOutput` を通って実行されます。
- Returned resources / prompt templates / prompt messages は untrusted Tool result data として serialization され、hidden context injection はありません。
- Result serialization は list items、resource contents、prompt messages、text fields、`_meta`、structured JSON depth/node count、rich blobs/images/audio、final output bytes を bounded に扱います。
- Capability が advertise されていない operation は model-visible tool として expose されません。
- `list_changed` refresh、sampling、elicitation は実装していません。

主な commit:
- `3a22360a mcp: expose resources prompts tools`
- `4a4590f8 merge: mcp resources prompts tools`

Review:
- r1 は `approve`。
- Reviewer は explicit Tool operations、ordinary `ToolOutput` path、no hidden context injection、untrusted/bounded serialization、capability-gated registration、no sampling/elicitation/list_changed scope creep を確認しました。

最終 validation:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p pod mcp::tests`
- `cargo test -p mcp`
- `cargo check -p pod -p mcp`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113403880`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-4oVSE2.log`