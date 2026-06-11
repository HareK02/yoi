<!-- event: create author: LocalTicketBackend at: 2026-06-11T16:03:28Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T16:03:33Z -->

## Intake summary

Ticket role launch の初回 user message を短縮し、情報所管を Instruction / Workflow / Submit / control-plane に分離する方針で合意済み。language と workspace_root/cwd は Instruction 側の環境情報、Git/worktree 操作対象として必要な path は Submit に重複記載可。workflow slug/profile selector は初回 user message に含めない。

---

<!-- event: state_changed author: intake at: 2026-06-11T16:03:33Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・境界判断が揃ったため、Orchestrator routing 可能。実装はユーザーが panel で queue した後に開始する。

---
