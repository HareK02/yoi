<!-- event: create author: tickets.sh at: 2026-06-01T11:00:26Z -->

## Created

Created by tickets.sh create.

---

<!-- event: implementation_report author: hare at: 2026-06-01T11:09:35Z -->

## Implementation report

# Implementation report

## Summary

Reorganized maintained repository documentation around current Yoi design intent and development workflow.

## Changes

- Replaced the root `README.md` with a concise developer entry point.
- Added `docs/README.md` as the documentation map and contribution boundary.
- Added maintained design docs under `docs/design/`:
  - `overview.md`
  - `pod-session-state.md`
  - `context-history.md`
  - `profiles-manifests-prompts.md`
  - `tool-permissions-scope.md`
  - `memory-knowledge.md`
  - `compaction.md`
  - `provider-model-boundary.md`
- Added maintained development docs under `docs/development/`:
  - `work-items.md`
  - `workflows.md`
  - `validation.md`
  - `dogfooding.md`
  - `environment.md`
- Moved old top-level docs, `docs/plan/`, `docs/ref/`, and `docs/research/` material into ignored `docs/.local/old-docs/` so the repository documentation surface no longer presents it as current authority.

## Validation

- `./tickets.sh doctor` => `doctor: ok`
- `git diff --check` => passed
- stale-surface grep over `README.md`, maintained `docs/`, and crate READMEs excluding `docs/report/` and `docs/.local/` => no output


---
