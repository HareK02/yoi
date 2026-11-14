# T-830: Decodal Unicode / retained toolchain validation

Base: `6b8132803da91977db3aec99d2fef6df9042f3cf` (`hare/develop`). Work branch: `work/t830-unicode`.

## Change

- Pin decodal, decodal-language-service and decodal-language-tools to 0.4.1; align `DECODAL_VERSION` and regenerate actual Web WASM. Registry checksum for decodal matches `aa240084ed31e869e0e8ca5b2a532b782c7db5cdb678e2ce9c6d4fdd7b488afc`.
- Read retained 0.4.0 source/evaluation metadata without silently reevaluating or replacing it. Keep old history/receipts. Normal evaluation and the direct legacy commit path require explicit owner adoption first.
- Reuse existing owner authorization, editing-head/catalog/source CAS, atomic adoption, rollback and exact-replay paths. A no-source-change owner preview/commit refreshes the toolchain and projection, rather than changing historical evidence.
- Shared Unicode corpus covers CJK/HELP `retained 雪`, accents, emoji, combining marks, escapes, source splitting, Unicode relative import, Markdown and multibyte errors. Regex remains feature-disabled and returns `unsupported_feature`; no new regex capability is claimed.

## Executed validation

- Native `config-source`: 56 passed; native WASM adapter: 3 passed.
- Real generated WASM suite: 26 passed (included in Web suite).
- Native example output compared to generated WASM: all 6 corpus cases match complete evaluation/projection, diagnostics including kind/path/span/provenance, Language Service and formatter output.
- Base commit's actual 0.4.0 WASM, using its own schema/toolchain fingerprints, reproduces corrupt Unicode string values and Unicode import failure. New WASM preserves the expected values and resolves the same split tree. This is a product artifact comparison, separate from the historical upstream standalone probe.
- Server: config adoption 31, config source 27, configuration HTTP/CLI integration 22, Profile 8, Prompt 7 = **95 passed** in the repository's normal child-only environment isolation. New storage tests model retained 0.4.0 bytes/contracts; they are not claimed as an independently executed old native engine.
- Additional final execution of the strengthened HTTP refresh test: **passed after cold API reconstruction** from retained 0.4.0 DB, with no old API/router/projection cache. Startup and GET retain old evidence; non-owner commit is denied; owner preview is read-only; commit preserves source, updates fingerprint/projection/head, and exact replay leaves the adopted state intact.
- Storage tests cover pre-adoption and published configurations, old raw history/receipts, failed evaluation, same-content ABA, stale head, changed operation input, six storage-failure boundaries, rollback and reopen.
- Web normal suite: **897 passed**. Web check: 0 errors / 0 warnings. Web build passed (existing large-chunk advisory).
- Root `cargo check --locked` passed. It reports existing unused-code warnings; the separate uncommitted warning cleanup is deliberately not included in this change.
- Changed Rust formatting, new corpus/script formatting, comparison script typecheck and `git diff --check` passed.
- Release WASM regenerated with Rust 1.95.0, wasm32 std/lld, wasm-bindgen CLI **0.2.117** matching lock. Repeated generation of JS, declarations and WASM compares byte-for-byte. Only WASM bytes change in the generated binding set.
- Generated WASM SHA-256: `9a0262aba0bad6ab5001ae6e3e143d7e421cdc8b1b86ea9b44e0cde453795b55`.

Reproduction commands and the explicit adoption path are in `crates/config-source/README.md`. Local detailed execution logs are under `target/t830-evidence/` (not durable repository artifacts).

## Review and limits

A read-only independent child reviewed native/source/storage/adoption paths and found no major defect in the inspected scope. Its requested cold-start test was added and passed. Its Web file access was incomplete; Web/artifact verification above was performed by the parent, not independently approved by that child. No MR review verdict is claimed.

No full Rust workspace test-suite success, live database refresh, deployed binary update, service restart or real-model behavior is claimed. Updating source dependencies does not itself refresh live saved configuration; owner adoption is intentionally separate. Existing warning cleanup work is preserved separately.
