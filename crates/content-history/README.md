# Content history

This crate owns immutable full-text content, typed identities and pure structural
operations. It does not own persistence, authorization, retention decisions,
transaction adoption, head compare-and-set, application events or text rendering.
[`Object`] and [`Tree`] define canonical content; [`History`] separates editing
identity from content identity. [`restore`], [`diff`] and [`replace_path`] operate
through a caller-scoped [`ObjectReader`].

## Canonical identity and compatibility

Version 1 hashes the entire canonical encoding using SHA-256, including object
kind and format version. Every object starts with `YCH\0`, a big-endian `u16`
version (`1`), and a kind byte (`1` blob, `2` tree, `3` history). Integer lengths
are unsigned big-endian `u64`; strings are UTF-8 prefixed by their byte length.

- A blob contains a length and the exact full bytes.
- A tree contains a mode byte (`0` map, `1` sequence), an entry count, then each
  length-prefixed key, child kind byte (`1` blob, `2` tree), and 32 hash bytes.
- A history contains 32 root hash bytes, a parent flag (`0` absent, `1` present),
  optional 32 parent hash bytes, a length-prefixed actor, and a signed big-endian
  `i64` timestamp in microseconds.

Map keys are unique and sorted by exact UTF-8 byte order. Sequences also have
unique keys but preserve input order. Decoding never sorts a noncanonical map.
There is no Unicode, newline, whitespace, path or source normalization. Empty
keys and separator characters are opaque strings; adapters own path validation.
No extension fields or trailing bytes are accepted. Unsupported versions fail
explicitly: a future migration must retain old bytes and identities, not silently
reserialize old objects under a new format. [`verify`] checks kind and hash after
strict decoding. Typed IDs are identities, not evidence that an object exists.

## Store and adapter boundary

Readers must be bound to an authorized business target and retained root/history.
Knowing a hash must not grant access. They must refuse erased or revoked content
for that target even when shared physical bytes remain for another target. The
model cannot enforce this policy or safely implement global garbage collection.
Readers also own I/O buffer bounds; [`TraversalLimits`] bounds work after reading.
Full restoration validates every reachable object and returns no partial success.
A structural diff intentionally skips equal IDs and does not prove closure integrity.

Path replacement returns new ancestor objects without writing anything. The
caller must validate and save the complete closure, append a history with the
current head as its parent, and CAS-adopt the new history in the same caller-owned
transaction as business projections. A rollback or stale head must leave the old
head and content unchanged. Comparing only content roots would accept stale
A-to-B-to-A edits. Restoring an old root reuses content but creates a new history;
it never rewinds business events or adopts the old history identity.

Configuration adapters can map validated virtual path components to map entries
and exact source files to blobs. Ticket adapters can map stable document fields
to blobs and stable post IDs to a sequence; reordering posts changes content.
Ticket IDs, post IDs, existing content digests, content roots and history heads
remain distinct contracts. Neither adapter belongs in this pure model. Contract
fixtures exercise these shapes without claiming database or authorization coverage.

## Cross-target fixture

`tests/fixtures/v1.tsv` freezes bytes and hashes independently derived from the
format. The `wasm_parity` raw `cdylib` example exports `parity_len` (digest words)
and `parity_word(index)` (four digest bytes interpreted big-endian). It has no JS
or host-import dependency. A runner can compare its words with the fixture hashes
on native and `wasm32-unknown-unknown`; an out-of-range index returns `u32::MAX`.
Building a WASM module alone is not evidence of executed native/WASM parity.
The optional execution adapter uses only Deno's built-in WebAssembly runtime;
run from the workspace root after registering this crate and installing the target:

```text
cargo test -p content-history
cargo build -p content-history --example wasm_parity --target wasm32-unknown-unknown
deno run --allow-read crates/content-history/tests/wasm_parity.ts target/wasm32-unknown-unknown/debug/examples/wasm_parity.wasm
```
