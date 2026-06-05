<!-- event: create author: tickets.sh at: 2026-06-01T02:11:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T23:01:38Z -->

## Decision

Updated based on user direction:

- keep this as the existing `tui-composer-history-persistence` ticket rather than creating a duplicate;
- default user-data shape should be like `~/.yoi/<path-to-pwd>/composer-history.json` using a path-safe/stable workspace key;
- do not create composer history under workspace `./.yoi/`;
- bound persisted recall history to about 30 entries per workspace instead of the older 100-entry note;
- keep typed `Segment` storage, non-destructive recall semantics, and no Pod/session transcript mutation.


---
