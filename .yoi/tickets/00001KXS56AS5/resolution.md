Implemented `SessionReferenceView` common substrate.

The Worker crate now has an immutable session reference view that can build User/Assistant overview items, index user/assistant/system/tool references, search with kind and tool input/output filters, perform bounded reads with truncation metadata, and resolve source refs for future extract staging anchors. Compaction search now uses this common view; read-session behavior remains unchanged for compatibility.

Validation passed:
- `cargo fmt --check`
- `cargo test -p worker session_reference`
- `cargo test -p worker compact::worker`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`
