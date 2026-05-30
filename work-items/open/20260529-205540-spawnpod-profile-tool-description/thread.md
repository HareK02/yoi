<!-- event: create author: tickets.sh at: 2026-05-29T20:55:40Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-30T02:53:19Z -->

## Decision

Clarified selector semantics:

- `default` / omitted means resolve the effective child default profile.
- `<slug>` / source-qualified selectors mean resolve a discovered role profile.
- `inherit` means derive reusable child config from the spawner's resolved Manifest.

`inherit` is explicitly not the same as reusing the Profile source that created the parent. It extracts reusable fields from the parent resolved Manifest and still replaces runtime-bound/authority fields such as `pod.name` and concrete `scope.allow` with the SpawnPod inputs. Reusing the parent's original Profile source can be considered later as a separate feature if needed.


---
