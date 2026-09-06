# Schema baseline cleanup was generalized into migration deletion policy

## Summary

A one-time Workspace SQLite baseline cleanup in `89856eb7` was documented as a permanent policy
that retained historical migrations were not part of the contract. Later schema changes followed
that text by deleting the immediately preceding migration whenever a new version was added.

This left valid dogfooding databases stranded. A schema-50 database could run the historical
50-to-51 migration, but the schema-52 binary required a fresh schema-51 baseline and rejected the
resulting two-row history. The current schema-53 binary retained only 52-to-53. The CLI also listed
`migrate` as an expected command after its implementation had been removed.

## Why this was harmful

A request or decision to retire sufficiently old generations does not imply that every migration
should be discarded at the next version bump. Treating a baseline reset as a standing policy
removed the only executable data-preservation path without an explicit retention decision or
operational replacement.

The narrow tests reinforced the mistake: each version tested only a freshly constructed previous
baseline, not a database migrated from the oldest retained version through the full chain.

## Corrective rules

- Keep an ordered, composable migration chain from an explicit oldest supported version.
- Do not infer migration retirement from a schema-version increment.
- Require a separately reviewed baseline-retirement change and operational plan.
- Test the oldest retained fixture through every migration to the latest schema.
- Make startup migration and explicit migration CLI use the same planner and runner.
- Make dry-run execute the real path against an in-memory SQLite backup rather than maintaining a
  second approximation.
