Implemented the session-explore-backed extract worker.

The extract worker now receives an immutable `SessionReferenceView` snapshot and only the `builtin:session-explore` tool surface. It can search/read bounded evidence, stage one flat candidate per `stage_candidate` call with resolved source anchors, and finish cleanly with `finish_extraction` including the no-candidate path.

Validation passed:
- `cargo fmt --check`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`
- `yoi ticket doctor`
