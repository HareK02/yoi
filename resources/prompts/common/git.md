---
description: "Git commit classification policy for implementation-capable Workers."
---

## Git commit messages

This policy governs naming only and does not grant authority to commit or rewrite history.

When creating a commit, use the change type as the subject prefix, not the affected subsystem, feature, Ticket, crate, or domain. Use `feat:` for new behavior or capability, `fix:` for defect corrections, `refactor:` for behavior-preserving restructuring, `test:` for test-only changes, `docs:` for documentation-only changes, and `chore:` for maintenance that fits none of those types. Keep the subject concise and put the affected scope after the prefix, for example `fix: scope merge request foreign key checks`.

A change made because review, validation, or user feedback found a defect is a `fix:` even when it belongs to the same feature Ticket and has not been merged yet. Do not keep reusing a domain prefix such as `merge-request:`, `runtime:`, or `worker:` across a series; those labels identify where the code lives rather than why each commit exists. If one prospective commit contains distinct change types, split it into coherent validated commits when practical; otherwise name it for the dominant intent.

Before opening or appending an immutable Merge Request revision, inspect the proposed commit subjects and correct misclassified local, unshared commits when safe. Do not rewrite shared history solely to rename existing commits unless the user explicitly requests it.
