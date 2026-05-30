<!-- event: create author: tickets.sh at: 2026-05-30T06:28:52Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T06:29:24Z -->

## Plan

Implementation plan:

1. Read the current target docs and authoritative code/type definitions.
2. Update high-level docs in-place, keeping edits scoped to stale claims rather than broad prose rewrites.
3. Mark superseded plan docs clearly when replacing them would be larger than useful.
4. Validate with `./tickets.sh doctor` and `git diff --check`.
5. Review the resulting diff against the audit findings before closing.


---

<!-- event: implementation_report author: hare at: 2026-05-30T06:39:11Z -->

## Implementation report

Implementation report:

- Refreshed `docs/architecture.md` for current protocol categories, session-store vs pod-store responsibilities, profile/manifest input, scope delegation, and current built-in tool categories.
- Updated `docs/compaction.md` to describe same-session segment rotation and `SegmentStart.compacted_from` instead of the old new-SessionId model.
- Rewrote `docs/tui-keybindings.md` around current command mode, queued input behavior, completion, task pane, and `Ctrl-R` rewind picker behavior.
- Marked `docs/plan/llm_presistence.md` as superseded and replaced old implementation details with current-session persistence pointers.
- Refreshed `docs/plan/ai-maintainer.md` and `docs/plan/maintainer-work-items.md` so current `tickets.sh` + `work-items/` authority replaces old `TODO.md` / legacy `tickets/` assumptions.
- Updated `docs/system-prompt-template.md` for current segment log naming (`ensure_segment_head`, `SegmentStart`) while fixing nearby stale session terminology.

Validation:

- `./tickets.sh doctor`
- `git diff --check`
- grep sweep for the high-priority stale phrases from the audit; remaining hits are historical reports/ref docs or explicit superseded notes.


---

<!-- event: close author: hare at: 2026-05-30T06:39:41Z status: closed -->

## Closed

docs stale audit の高優先指摘を反映し、architecture / compaction / TUI keybindings / system-prompt-template / plan docs を現行の protocol・session-store/pod-store・work-items 運用に合わせて更新した。旧 llm persistence / maintainer work item plan は superseded/historical と明示。検証は ./tickets.sh doctor と git diff --check。


---
