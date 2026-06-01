<!-- event: create author: tickets.sh at: 2026-06-01T12:36:41Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-01T12:37:30Z -->

## Plan

# Delegation intent: dependency/license audit

Intent:
- Audit Yoi's external dependencies and license posture before public MIT publication.

Requirements:
- Inventory Rust dependencies from `Cargo.lock` / `cargo metadata`, separating direct workspace dependencies from transitive dependencies where practical.
- Identify direct dependencies that look heavy, weakly justified, redundant, or replaceable with simpler local code or already-present dependencies.
- Check license metadata for direct and transitive Rust dependencies; flag unknown, missing, copyleft, non-standard, or notice-relevant licenses.
- Inspect Nix/system dependencies from `flake.nix`, `package.nix`, and `devshell.nix` at a high level.
- Produce a report at `work-items/open/20260601-123641-dependency-license-audit/artifacts/audit-report.md`.

Invariants:
- Do not modify dependency manifests, source code, lockfiles, docs, or work item files other than the audit report artifact.
- Do not read ignored secret-like file contents.
- Treat Cargo/Nix files and command output as current-state evidence; do not rely on resident memory for exact dependency/license facts.
- Distinguish release blockers from advisory cleanup opportunities.

Non-goals:
- Do not remove dependencies.
- Do not change licenses.
- Do not implement replacements.
- Do not perform a public-release history sanitation audit beyond dependency/license implications.

Escalate if:
- A dependency appears incompatible with MIT publication.
- License metadata is missing for a significant dependency and cannot be resolved from local registry metadata without web access.
- A dependency replacement would require design judgment rather than obvious cleanup.
- Commands require network access or mutation beyond the artifact report.

Validation/evidence:
- Record every command used for inventory/license evidence.
- Prefer read-only commands such as `cargo metadata --locked`, `cargo tree --locked`, `nix flake metadata`, and direct file reads.
- If a tool is unavailable, record the fallback method.


---
