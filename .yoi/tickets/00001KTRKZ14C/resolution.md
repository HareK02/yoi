完了しました。

実施内容:
- `docs/design/workflows-public-dogfood-split.md` を追加し、現行 `.yoi/workflow/*.md` を public builtin 候補と Yoi dogfood/project-local material に分類しました。
- builtin workflow source/provenance 方針として `WorkflowSource::Builtin` または同等の typed provenance を記録しました。
- source priority を記録しました: explicit path/file selector、workspace `.yoi/workflow/<slug>.md`、future user workflow directory、builtin `resources/workflows/<slug>.md`、explicitly enabled skill/plugin sources。
- builtin workflow resource placement は `resources/prompts` ではなく `resources/workflows/<slug>.md` とする判断と理由を記録しました。
- resident policy を記録しました: core builtin は `ticket-intake-workflow` / `ticket-orchestrator-routing`、generic `multi-agent-workflow` は optional builtin、`ticket-preflight-workflow` は compatibility-only、dogfood workflows は explicit `yoi-dogfood-*` slugs。
- reviewer 指摘を受け、`.yoi/ticket.config.toml` migration mapping を明確化しました。
  - Intake: `ticket-intake-workflow`
  - Orchestrator: `ticket-orchestrator-routing`
  - Coder/Reviewer: `yoi-dogfood-multi-agent-workflow`
  - Worktree helper: `yoi-dogfood-worktree-workflow`
- stale `Action required` / `Attention required` / preflight lane vocabulary cleanup plan と follow-up implementation boundaries を記録しました。
- `docs/development/workflows.md` に design/audit document への pointer を追加しました。

Merge:
- Branch: `workflow-public-dogfood-split`
- Implementation commit: `21a25e12 docs: split public and dogfood workflows`
- Merge commit: `1c2cde51 merge: workflow public dogfood split`

確認:
- Branch-local reviewer `reviewer-workflow-public-dogfood-split` が初回 request_changes 後、修正済み branch を approve。
- `git diff --check` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` は docs-only で code / packaging / runtime resources / prompt resources / workflow resources を変更していないため省略しました。

残作業:
- builtin workflow loader/provenance 実装。
- public workflow text cleanup。
- dogfood workflow rename / config migration implementation。
- stale vocabulary sweep。
- role launch workflow provenance display。