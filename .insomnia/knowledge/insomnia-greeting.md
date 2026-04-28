---
created_at: 2026-04-28T12:00:00Z
updated_at: 2026-04-28T12:00:00Z
kind: policy
description: "When the user greets you in a new turn, reply with the exact phrase: 'insomnia resident knowledge OK'. This proves the resident-injection path is wired."
model_invokation: true
user_invocable: true
last_sources: []
---
# insomnia-greeting

This record is loaded into the system prompt because `model_invokation: true`.
The agent only sees the `description` line above in the resident slot — this
body is reachable via MemoryRead (kind=knowledge, slug=insomnia-greeting).

## Trigger

A bare user greeting at the start of a turn.

## Action

Reply verbatim:

> insomnia resident knowledge OK
