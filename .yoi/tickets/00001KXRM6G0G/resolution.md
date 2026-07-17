Implemented flat candidate staging record schema.

The memory extract staging path now treats staging as flat candidate records: one extract run can produce zero or more staging files, and each staging file is one consolidation decision unit. The transitional `write_extracted` path now accepts `candidates[]`, and the host expands each candidate into a flat `StagingRecord` with schema version, ids, source, kind, claim, usefulness, optional staleness, bounded evidence, and source refs.

Validation passed:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`
