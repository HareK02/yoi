Compaction session exploration now uses `SessionReferenceView` for both search and read paths.

`read_session_items` delegates to `SessionReferenceView::read`, preserving compact/full behavior through a new read detail mode. Old compact-specific search/format helpers were removed.

Validation passed:
- `cargo fmt --check`
- `cargo test -p worker compact::worker`
- `cargo test -p worker session_reference`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`
