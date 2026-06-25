---
description: Ticket を読み、Orchestrator が planning return / spike / implementation / review / blocked / close へ明示的に routing する workflow
model_invokation: true
user_invocable: true
requires: [workflow-resource-boundary]
---

# Ticket orchestrator routing workflow

1. Ticket body/thread/artifacts/resolution 状態を読み、current state と blocking relation を確認する。queued Ticket は実装 side effect 前に `inprogress` を記録する。
2. 不足情報が具体的にある場合は planning return とし、何が足りないかを Ticket thread に残す。risk flag は context lookup・review focus・invariant・escalation として扱い、自動 stop gate にしない。
3. 実装に進む場合は Orchestrator workspace/orchestration HEAD を基準にし、launch input の explicit Git/worktree operation target だけを使う。role workspace/cwd は環境情報であり、original/root workspace を読み書き・検証・cleanup・git 操作対象にしない。
4. child implementation worktree は recorded implementation worktree root 配下に作成し、implementation branch は Orchestrator workspace current HEAD / orchestration branch HEAD から切る。Coder/Reviewer は sibling Pods として route し、Ticket intent・binding decisions・validation/report expectations を concrete handoff として渡す。
5. Reviewer が request-changes した場合は、scope 内で Coder に修正を戻すか、Orchestrator が blocked/escalation を記録する。approve evidence が揃うまで merge-ready とみなさない。
6. merge completion authority が無い場合は、Ticket/branch/commit identity、intent/invariant check、implementation summary、coder/reviewer evidence、blocker resolution、validation、risks、dirty state、decision needs を含む merge-ready dossier で停止する。
7. 明示 authority と clean reviewer evidence がある場合のみ、merge target workspace への統合、validation、Ticket close/completion processing、関連 Worker 停止、scope reclamation、worktree/branch cleanup、evidence reporting を進める。
