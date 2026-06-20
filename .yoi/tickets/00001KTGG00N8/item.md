---
title: "Improve Pod notification injection guidance"
state: 'closed'
created_at: "2026-06-07T07:33:13Z"
updated_at: '2026-06-20T16:23:37Z'
---

## Background

During long orchestration sessions, spawned Pod completion notifications can arrive as system-injected messages. Although the notification says it is not a blocking request, the assistant may still respond with conversational phrasing that appears to answer the notification or stale user context, rather than clearly reporting the notification-handling result.

This is especially noticeable in long contexts where multiple similar ticket/worktree/Pod reports are present. The issue appears more like notification framing/turn-target ambiguity than a direct task failure.

## Goal

Improve the system-injected Pod notification wording so the assistant is guided to handle notifications as background events and report concrete handling results, not respond as if the notification were a user message.

## Requirements

- Review current Pod notification injection text for spawned Pod finished/shutdown events.
- Adjust wording to more strongly distinguish notifications from user requests.
- Add guidance for the assistant's next response after handling a notification, e.g.:
  - do not answer the notification as if it were a user message;
  - if you choose to handle it now, read the Pod output and report the action taken;
  - summarize which Pod was handled, what changed, and what remains pending;
  - otherwise continue the current user task and handle the notification later.
- Keep notification text concise enough not to bloat context excessively.
- Preserve the existing policy that notifications are not blocking and should not force polling/wait loops.
- Ensure wording does not authorize extra workflow actions, merges, ticket closes, or cleanup solely because a notification arrived.
- Add/update tests or prompt snapshot tests for notification text if such tests exist.

## Non-goals

- Redesigning the Pod event protocol.
- Implementing typed hidden event channels.
- Solving all long-context degradation issues.
- Changing tool behavior or automatic notification handling policy.

## Acceptance criteria

- Pod notification injection text clearly frames notifications as background events, not user turns.
- The text gives concrete response-shaping guidance for notification-handling reports.
- Existing orchestration policy remains: verify output/evidence before treating child work complete, and do not start/merge/close solely from a notification.
- Tests/snapshots are updated where applicable.
