# Work Items backend

`work-items/` is the canonical file backend for repository work after the migration from `TODO.md` and `tickets/*.md`.

## Layout

```text
work-items/
  README.md
  open/
    <id>/
      item.md
      thread.md
      artifacts/
  pending/
    <id>/
      item.md
      thread.md
      artifacts/
  closed/
    <id>/
      item.md
      thread.md
      resolution.md
      artifacts/
```

`<id>` is timestamp based: `YYYYMMDD-HHMMSS-<slug>`. There is no central sequence file.

## `item.md` schema

`item.md` is Markdown with YAML-like frontmatter. The MVP parser intentionally supports only simple `key: value` lines.

Required fields:

```yaml
---
id: 20260527-000001-example
slug: example
title: Human-readable title
status: open
kind: feature
priority: P2
labels: [maintainer, workflow]
created_at: 2026-05-27T00:00:01Z
updated_at: 2026-05-27T00:00:01Z
assignee: null
legacy_ticket: tickets/example.md
---
```

`status` must match the containing directory (`open`, `pending`, or `closed`). `legacy_ticket` records the migrated source path when one existed; use `null` for items created from a TODO-only entry or new work.

## `thread.md` schema

`thread.md` is an append-only Markdown event log. `tickets.sh comment`, `tickets.sh review`, and `tickets.sh close` append an HTML-comment event header, a Markdown heading, the event body, and a `---` separator.

Example:

```md
<!-- event: review author: orchestrator at: 2026-05-27T12:00:00Z status: request_changes -->

## Review: request changes

Review notes.

---
```

Review state belongs in `thread.md`; new `tickets/*.review.md` files should not be created.

## Commands

Use `../tickets.sh --help` from this repository root for the command reference. The script performs file operations only; it does not run `git add` or `git commit`.

Common commands:

```sh
./tickets.sh list --status all
./tickets.sh show <id-or-slug>
./tickets.sh create --title "Title" --slug title
./tickets.sh comment <id-or-slug> --role plan --file notes.md
./tickets.sh review <id-or-slug> --approve --file review.md
./tickets.sh close <id-or-slug> --resolution "Done"
./tickets.sh doctor
```

## Migration policy

The migration moved unfinished `TODO.md` entries and `tickets/*.md` bodies into `work-items/open/*/item.md`. Existing review files, when present, must be represented as `review` events in `thread.md`. After migration:

- `work-items/` is the source of truth.
- `TODO.md` is only a legacy/generated-view notice and must not carry open ticket state.
- `tickets/*.md` and `tickets/*.review.md` must not remain as canonical unfinished items.
- Closed items are moved to `work-items/closed/` instead of being deleted.
- `./tickets.sh doctor` checks schema, status placement, duplicate IDs/slugs, and leftover legacy ticket files.
