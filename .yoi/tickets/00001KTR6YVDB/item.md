---
title: 'LLM向けプロンプト直書きを廃止してresources/promptsへ集約する'
state: 'inprogress'
created_at: '2026-06-10T07:29:13Z'
updated_at: '2026-06-10T08:11:58Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-10T07:49:23Z'
---

## 背景

この repository の設計要旨では、プロンプトはすべて `resources/prompts` に集約することになっている。管理効率・override 可能性・レビュー容易性のため、Rust code や workflow 起動処理に LLM-facing prompt 文言を直書きしてはいけない。

現在、少なくとも Ticket role launch / Intake handoff 周辺では、LLM に渡る generated launch prompt が `crates/client/src/ticket_role.rs` に直書きされている。例として `build_launch_prompt`、`TicketIntakeHandoff::append_prompt_lines`、role-specific execution guidance などが該当する。これらは UI label / notice ではなく model context に入る prompt なので、`resources/prompts` 管理へ移す必要がある。

この Ticket は Intake 文言だけでなく、LLM に渡る hardcoded prompt prose 全体を audit し、`resources/prompts` 経由に移すためのものとする。

## 要件

- LLM-facing prompt 文言を Rust code に直書きしない。
- Ticket role launch prompt の固定文言を `resources/prompts` 配下へ移す。
  - `# Ticket role launch` から始まる launch context guidance。
  - Ticket id / language / user instruction / handoff / intent packet / worktree / validation / report expectation の説明文。
  - Intake handoff guidance。
  - Orchestrator / coder / reviewer 向け role execution guidance。
- Intake workflow の本体文言は引き続き `.yoi/workflow/ticket-intake-workflow.md` を正とし、launch prompt 側には必要最小限の runtime context と参照関係だけを載せる。
- `resources/prompts` に追加する prompt resource は、用途・入力変数・出力境界が分かる単位に分割する。
- Rust 側は prompt resource を読み込み、必要な runtime 値を安全に埋め込むだけにする。
- prompt resource の読み込みは既存の prompt/resource 管理方針に合わせる。
  - bundled resource として扱うべきものは compile-time embedded resource に含める。
  - user override 可能性を壊さない。
- repository 内の hardcoded LLM-facing prompt prose を audit し、少なくとも既知の Ticket role launch 系はこの Ticket で解消する。
- 残す文言がある場合は、UI 表示文言・diagnostic・protocol literal など、LLM prompt ではない理由を明確にする。
- 将来の再発を防ぐため、可能なら grep しやすい境界・テスト・lint 的な確認を追加する。

## 受け入れ条件

- `crates/client/src/ticket_role.rs` から LLM-facing prompt prose の大きな直書きがなくなっている。
- Ticket role launch で LLM に渡る固定 guidance は `resources/prompts` の prompt resource から組み立てられる。
- Intake handoff の LLM-facing guidance が Rust 直書きではなく prompt resource 管理になっている。
- Orchestrator / coder / reviewer role execution guidance が Rust 直書きではなく prompt resource 管理になっている。
- runtime 値の埋め込みは prompt injection を広げない形で bounded / escaped / sectioned に維持される。
- 既存の Ticket role launch behavior は semantic に維持される。
- `resources/prompts` 側の resource 名・責務が明確で、後続の文言修正を code edit なしで行える。
- targeted tests が追加・更新され、launch prompt が resource 経由で生成されることを確認している。
- repository audit の結果が Ticket thread または implementation report に記録され、残存する hardcoded LLM-facing prose がない、または非対象理由が明示されている。
- `cargo test -p client ticket_role` または該当する targeted test が通る。
- `target/debug/yoi ticket doctor` が通る。

## 非目標

- TUI の表示ラベル、status line、notice、error diagnostic など、LLM に渡らない UI 文言を `resources/prompts` に移すこと。
- workflow markdown 本体を `resources/prompts` に移すこと。project-authored workflow は `.yoi/workflow` が正である。
- protocol literal、tool name、field name、file path、command name を翻訳・抽象化すること。
- prompt resource system 全体の大規模再設計。
