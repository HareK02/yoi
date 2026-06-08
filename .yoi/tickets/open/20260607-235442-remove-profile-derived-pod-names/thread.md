<!-- event: create author: LocalTicketBackend at: 2026-06-07T23:54:42Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T00:05:23Z -->

## Decision

## Decision update: remove `resume_by_pod_name` / `--profile-pod-name` split

While removing Profile-derived Pod names, also remove the confusing startup split that led to this boundary problem.

Current issue:

- `SpawnConfig::resume_by_pod_name` does not mean "inherit only the name while resuming"; it means "pass `--pod <pod_name>` to the child process so name-keyed restore/create happens".
- When `resume_by_pod_name = false`, `spawn_pod` may omit `--pod <pod_name>`, allowing profile resolution to derive Pod identity from profile slug/source.
- `--profile-pod-name` exists as a workaround for profile startup needing a Pod name without using `--pod`, but this preserves the false separation between profile selection and runtime identity.

Desired direction:

- `--pod <name>` is the runtime Pod identity for both restore and fresh create.
- `--profile <selector>` is only the Profile recipe selection.
- `--workspace <path>` / runtime workspace context supplies the workspace root.
- Profile slug/source must never supply `pod.name`.

Implementation requirements:

- Remove `resume_by_pod_name` from `SpawnConfig` or replace it with a clearer startup-mode enum only if a distinct mode is truly needed.
- Make spawn helpers pass explicit Pod identity for all fresh/restore paths instead of conditionally omitting `--pod`.
- Remove `--profile-pod-name` from the public/runtime argument surface if possible; otherwise mark it internal/deprecated with a follow-up to remove it.
- Support `--pod <name> --profile <selector>` as the normal way to start a named Pod with a selected Profile.
- If restore/create semantics need to be controlled, model that explicitly as startup policy rather than a boolean named `resume_by_pod_name`.
- Add regression coverage for the previous failure mode: project profile `project:companion` must not create Pod `companion` when the runtime requested workspace Pod name is `yoi` or another workspace basename.

---
