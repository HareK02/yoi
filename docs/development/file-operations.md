# Shared text file operations

`fs-operation::text` owns the provider-neutral read/edit/write argument contracts,
JSON decoding, argument validation, pure text outcomes and byte-bound calculations.
The argument schemas also supply WIP parameter declarations. Ordinary Tools flatten
these arguments alongside their own Workdir routing fields; checkout WIP uses the
same Tool execution functions, not a third editing implementation.

Edit requires a nonempty `old_string`, a different `new_string`, and a unique
non-overlapping match unless `replace_all` is true. Limits count UTF-8 bytes, are
supplied by each provider, and are checked (including arithmetic/capacity overflow)
before allocating a replacement result. Outcomes carry replacement count and byte
size independently of persistence metadata.

## Provider-owned differences

- Workdir read exposes optional line offset/limit and numbered, bounded output.
  Workspace settings read remains a whole-text read with no arguments, preserving
  content type and digest. Line range defaults and numbered rendering live in the
  core but are only selected by the Workdir adapter.
- Workdir routing, attachment generation, Scope and permission identity remain in
  Worker/Tools/Workdir. Read history and content hashes stay in the existing
  attachment-scoped tracker. Descriptor-protected reads and checked edits/writes
  retain their existing validator, identity and save boundary in the provider.
- Workspace settings grants, access mode and connection lifetime remain in Backend.
  The settings adapter binds the read to captured entry metadata and generates a
  canonical commit using the same tree revision/digest. Decodal validation,
  formatting and atomic multi-file persistence remain in the canonical Backend
  commit. Content type, create/delete/rename/apply_changes and attach are settings
  operations, not ordinary OS file operations.

The core does not perform I/O, authorize anything, or own revision/existence
mirrors. Providers invoke pure processing inside their existing consistency
boundary; it must not be used to turn a checked edit into an unchecked
Read–Write sequence.

## External contracts

Operation names, argument shapes and successful responses remain unchanged.
Workspace settings Edit now rejects identical old/new strings before reading or
committing (the intended behavior change). Pure text failures are explicitly
mapped to Tool argument errors or WIP `InvalidArguments`/`ResourceLimitExceeded`
by the adapters. A mutation's post-dispatch transport or response failure is still
`OutcomeUnknown`; the core does not retry it or reinterpret it as a validation
rejection.

Pure policy matrices belong in `fs-operation::text` tests. Provider tests cover
entrypoint wiring and the existing authorization, observation, checked-save,
configuration CAS and unknown-outcome boundaries rather than copying that matrix.
