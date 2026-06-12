---
description: worktree と sibling の coder / reviewer Pod を使い、下位 orchestrator が concrete Ticket 群の実装・外部レビュー・修正・完了準備を管理する orchestration フロー
model_invokation: true
user_invocable: true
requires: [workflow-resource-boundary]
---

# Multi-agent workflow

1. Orchestrator は concrete Ticket intent、binding decisions/invariants、implementation latitude、validation expectations、report expectations、worktree path、branch を Coder に渡す。Coder は delegated child worktree/branch のみを編集し、main/root workspace、merge、push、Ticket close、worktree cleanup、generated `.yoi` runtime/local/memory/log/lock/cache/socket/secret-like state を触らない。
2. Coder は local tactics を選んで実装し、変更ファイル・summary・validation・commit/status・unresolved notes・review readiness を報告する。design/permission/history/prompt-context/dependency/Ticket-boundary の未決事項は Orchestrator に escalate する。
3. Reviewer は Ticket/intent packet、binding decisions/invariants、acceptance criteria、diff/commits、validation evidence を読んでから判断する。実装を recorded requirements と project design boundary に照らして review し、blocker / non-blocking follow-up / parent-decision item を分類する。
4. Reviewer report は approve/request-changes evidence、file/line evidence where useful、validation performed、risks、unresolved concerns を含める。Reviewer は merge、close、push、cleanup、root/original workspace operation、Coder への直接指示を行わない。
5. Orchestrator は blocker fix loop、review approval、merge-ready dossier、integration/cleanup/close authority を管理する。下位 Pod の completion notification は transient hint とし、durable output/session/worktree state で確認する。
