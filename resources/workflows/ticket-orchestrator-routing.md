---
description: Public Ticket orchestrator routing workflow
model_invokation: true
user_invocable: true
requires: [workflow-resource-boundary]
---
# Ticket Orchestrator Routing Workflow

Read the Ticket, relation metadata, orchestration-plan records, and relevant workspace state before deciding the next action. Treat `queued -> inprogress` as the implementation acceptance marker and record it before worktree creation, role Pod spawn, or other implementation side effects.

Classify the Ticket as planning return, blocked, spike, implementation-ready, review-needed, close-ready, or noop. If implementation-ready, record an IntentPacket with binding decisions, implementation latitude, acceptance criteria, escalation conditions, validation, and reviewer focus.
