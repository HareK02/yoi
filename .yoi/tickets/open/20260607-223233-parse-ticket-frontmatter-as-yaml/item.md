---
id: 20260607-223233-parse-ticket-frontmatter-as-yaml
slug: parse-ticket-frontmatter-as-yaml
title: Parse Ticket frontmatter as YAML
status: open
kind: task
priority: P1
labels: [ticket, yaml, parser, bug, panel]
workflow_state: done
created_at: 2026-06-07T22:32:33Z
updated_at: 2026-06-07T23:38:32Z
assignee: null
legacy_ticket: null
queued_by: workspace-panel
queued_at: 2026-06-07T22:43:03Z
---

## Background

Ticket `item.md` frontmatter is written in YAML-like syntax and uses YAML null values such as:

```yaml
assignee: null
legacy_ticket: null
attention_required: null
queued_by: null
queued_at: null
```

However the current Ticket frontmatter parsing path treats many scalar values as raw strings. Some fields have ad-hoc `"null"` filtering, but newer nullable fields such as `attention_required` can be read as `Some("null")` instead of `None`.

Observed bug:

- `workspace-panel-nonblocking-transitions` has:

```yaml
workflow_state: planning
attention_required: null
```

- Panel should derive `Clarify` from `workflow_state: planning`.
- Instead, `attention_required` is interpreted as set and Panel derives `Edit` with an `attention_required is set` blocker.

YAML `null` is valid YAML; the bug is that Ticket frontmatter is not consistently parsed as YAML/null-aware data.

## Goal

Parse Ticket frontmatter as YAML data, or otherwise provide a YAML-compatible typed parsing layer, so nullable fields, lists, booleans, and strings are interpreted consistently with the file format.

## Requirements

- Replace or wrap the current raw string frontmatter parsing for Ticket `item.md` with a YAML-aware parser.
- YAML null forms should parse as absent/`None` for nullable fields:
  - `null`
  - `Null`
  - `NULL`
  - `~`
  - empty value where YAML treats it as null
- Preserve existing Ticket records and doctor compatibility.
- Preserve current emitted frontmatter format unless intentionally changed and documented.
- Parse list fields such as `labels` / `risk_flags` as YAML sequences when present.
- Preserve tolerant behavior for existing simple inline list syntax, quoted strings, and old optional fields.
- Ensure optional string fields do not treat the literal YAML null as meaningful text.
- Avoid broad Ticket schema redesign in this ticket; focus on parsing correctness.
- Update tests to cover YAML null/list/bool behavior and the panel `attention_required: null` regression.

## Candidate affected fields

At minimum verify:

- `assignee`
- `legacy_ticket`
- `readiness`
- `risk_flags`
- `action_required`
- `workflow_state`
- `attention_required`
- `queued_by`
- `queued_at`
- `labels`

## Acceptance criteria

- A Ticket with `attention_required: null` is parsed with `attention_required == None`.
- A Ticket with `workflow_state: planning` and `attention_required: null` derives Panel action `Clarify`, not `Edit`.
- Existing nullable fields no longer rely on per-field ad-hoc string `"null"` filtering.
- YAML sequence labels/risk flags parse correctly.
- Existing Ticket fixture tests and `target/debug/yoi ticket doctor` pass.
- Focused tests cover null variants (`null`, `~`, empty), quoted literal string values where appropriate, and regression behavior.
- `cargo test -p ticket ... --lib`, `cargo test -p tui workspace_panel --lib`, `cargo fmt --check`, and `git diff --check` pass.
