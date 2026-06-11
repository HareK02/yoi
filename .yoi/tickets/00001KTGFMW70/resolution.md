完了しました。

実施内容:
- embedded builtin Workflow resources を `resources/workflows` に追加しました。
  - `ticket-intake-workflow`
  - `ticket-orchestrator-routing`
  - `multi-agent-workflow`
- embedded builtin Knowledge resource を `resources/knowledge/workflow-resource-boundary.md` に追加しました。
- `WorkflowSource::Builtin` を追加し、builtin Workflow registry loading を実装しました。
- Workspace `.yoi/workflow/<slug>.md` が同 slug の builtin workflow を override します。
- 既存 skill shadow behavior は維持しています。
- Workflow invocation system item に workflow source provenance を表示します。
  - `from builtin workflow`
  - `from workspace workflow`
  - etc.
- Workflow-required Knowledge resolution は workspace `.yoi/knowledge/<slug>.md` を優先し、missing の場合だけ builtin Knowledge に fallback します。
- required Knowledge system item に source provenance を表示します。
  - `from workspace`
  - `from builtin`
- workspace Knowledge は同 slug の builtin Knowledge を override します。
- tests を追加・更新しました。
  - builtin registry fallback
  - workspace workflow override precedence
  - builtin workflow provenance
  - builtin required Knowledge fallback
  - workspace Knowledge override
  - existing workflow invocation behavior

Merge:
- Branch: `builtin-workflow-knowledge-resources`
- Implementation commit: `2418ad33 feat: add builtin workflow resources`
- Merge commit: `ef2099c1 merge: builtin workflow knowledge resources`

確認:
- Branch-local reviewer `reviewer-builtin-workflow-knowledge-resources` が approve。
- `cargo fmt --check` passed。
- `cargo test -p workflow --lib` passed（34 passed）。
- `cargo test -p pod workflow --lib` passed（8 passed）。
- `cargo check -p workflow -p pod` passed。
- `git diff --check` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- Broader builtin KnowledgeQuery / user-level Workflow-Knowledge resource directories は follow-up 境界です。