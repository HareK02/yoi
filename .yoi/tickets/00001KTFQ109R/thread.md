<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: comment author: hare at: 2026-06-07T01:21:43Z -->

## Comment

## Status context boundary

When local role session / Ticket claim overlay support is added, it can become one source of read-only Companion status context. The Companion should treat it as local runtime status, distinct from authoritative git-tracked Ticket project records.

Default Companion policy should still prohibit direct mutation of Ticket records and direct role Pod spawning/claiming unless a later explicit design grants that authority.

---

<!-- event: decision author: hare at: 2026-06-07T02:45:32Z -->

## Decision

## Companion Bash policy decision

Default Companion policy should not include Bash.

Rationale:
- Companion and Orchestrator both operate around the workspace root, but only Orchestrator should hold workspace operation authority.
- Companion is a human-facing status/understanding assistant, not an actor that creates orchestration side effects.
- Bash is too broad to treat as safely read-only by prompt alone. Even seemingly read-only commands can touch git locks/index state, build caches, `target/`, package caches, or long-running CPU/IO resources.
- Adding reliable read-only constraints to Bash would become a sandbox/policy redesign, not a small Companion-policy detail.

Policy:
- Default Companion: no Bash, no direct file writes, no Ticket mutation, no SpawnPod/worktree/merge authority.
- Prefer typed read/status tools and derived panel/registry/Ticket/Pod context for situational awareness.
- If future dogfooding shows Companion needs shell diagnostics, create a separate explicit design/ticket for an opt-in diagnostic Bash/read-only shell capability rather than adding Bash to the default Companion profile.

Operational trigger for revisiting:
- Users repeatedly want Companion to perform clear shell-based diagnostics; or
- Prompt-level "read-only" instructions prove insufficient and Companion attempts or performs unsafe Bash actions.

---

<!-- event: state_changed author: hare at: 2026-06-10T10:03:36Z from: planning to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-10T10:03:36Z status: closed -->

## 完了

Companion の lifecycle / panel integration は 00001KTFQ109T で実装済み。Companion を含む project role profile の tool surface / feature policy は 00001KTNQK1V8 で実装済み。残る prompt 文言整理は LLM-facing prompt の resources/prompts 集約チケット側で扱うため、この古い計画チケットは完了扱いで閉じる。


---
