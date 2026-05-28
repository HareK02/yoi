<!-- event: create author: tickets.sh at: 2026-05-28T16:32:38Z -->

## Created

Created by tickets.sh create.

---

<!-- event: close author: hare at: 2026-05-28T16:49:25Z status: closed -->

## Closed

---
id: 20260528-163238-multi-pod-view-section-layout
slug: multi-pod-view-section-layout
title: Polish multi-Pod view section layout
status: closed
kind: task
priority: P2
labels: [tui, pod, ux]
created_at: 2026-05-28T16:32:38Z
updated_at: 2026-05-28T16:49:25Z
assignee: null
legacy_ticket: null
---

## Background

`20260527-000023-multi-pod-view-ui` implemented the initial `tui --multi` dashboard. The current layout should be polished before building more interaction on top of it.

The desired list shape is sectioned by Pod state rather than a flat row list. The list area should visually emphasize live work first and keep closed/stopped history compact.

Target shape:

```text
--pending---
a
b
--working---
c
d



--closed--
# only a few rows
```

The blank area between `working` and `closed` is intentional: the live sections should occupy the available vertical space, while the closed section stays compact at the bottom.

There is also a visual defect where the input-area separator and the list-area separator produce two adjacent separators. The multi-Pod view should have a single clean boundary between the Pod list/dashboard and the composer/input area.

## Requirements

- Change `tui --multi` list layout to explicit sections:
  - `pending`: live Pods that are idle/waiting and ready for input.
  - `working`: live Pods that are running/processing, plus paused Pods if a separate paused section is not introduced.
  - `closed`: stopped/restorable history entries.
- Render section headers even when a section is empty only if that makes the view easier to understand; otherwise empty sections may be hidden. The choice should be consistent and tested/snapshotted where practical.
- Allocate vertical space so that:
  - live sections (`pending` + `working`) take the main flexible area.
  - `closed` is pinned near the bottom of the list/dashboard area.
  - `closed` shows only a small fixed number of rows initially, with 3 visible rows as the target.
  - excess height appears as blank space above the `closed` section rather than expanding closed history.
- Keep selection/navigation sane across sections.
  - Selection should move through visible rows in display order.
  - Direct send eligibility remains based on the selected `PodListEntry` action state.
  - Hidden closed rows must not accidentally become selected unless scrolling/paging for closed entries is explicitly implemented.
- Fix the double-separator defect between the Pod list/dashboard and the composer/input area.
  - There should be one visual boundary, not two adjacent horizontal rules/borders.
  - Do not introduce the same double-border issue between section headers and rows.
- Preserve existing `tui --multi` behavior outside layout.
  - `tui --multi` CLI entrypoint and conflicts remain unchanged.
  - Composer contents are preserved across selection changes.
  - Direct send to selected idle live Pod remains supported.
  - running/paused/stopped targets remain safely disabled unless separately implemented.

## Acceptance criteria

- `tui --multi` renders Pod rows grouped into `pending`, `working`, and compact `closed` sections.
- The closed section is limited to about 3 visible rows and is visually anchored below the flexible live area.
- The blank/flexible space is placed above `closed`, not below it and not by expanding closed history.
- The boundary between list/dashboard and composer has a single separator/border.
- Selection and direct-send target mapping still use the underlying `PodListEntry` and remain correct after sectioning.
- Focused tests cover section classification, closed-row limiting, selection over visible section rows, and composer separator layout state where practical.
- `cargo fmt --check`
- `cargo test -p tui multi --no-default-features` or equivalent focused tests.
- `cargo check -p tui -p client -p pod`

## Out of scope

- Reopening the completed `multi-pod-view-ui` ticket.
- Adding per-section scrolling unless needed for a minimal correct implementation.
- Changing `PodList` discovery/visibility semantics.
- Changing direct-send delivery semantics.
- Adding new CLI flags.


---
