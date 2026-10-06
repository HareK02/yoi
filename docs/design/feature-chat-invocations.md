# Declarative Feature chat invocations

Feature chat invocations are explicit structured user input. They let an enabled Feature contribute discoverable chat actions without teaching each client Feature-specific syntax or asking the LLM to infer an operation from text.

## Namespaces and identity

Composer sigils have separate meanings:

- `:` is a client control command for the whole draft.
- `@` is a Worker-readable file reference.
- `/` selects a Feature invocation and may occur among ordinary prose.
- `#` has no invocation meaning.

A public name such as `attach` is presentation, not authority. The payload carries the installed source-qualified `FeatureInvocationIdentity` (for example `builtin:attachments/attach`). Installation rejects duplicate public names and aliases, so clients never choose an arbitrary winner. Disabled, failed-to-install, or unavailable Features are absent from completion and cannot be resolved at execution.

## Declarative grammar

The common spelling is:

```text
/name(positional, named=value) following natural language
```

The closing `)` is the explicit argument/prose boundary. String values use JSON double quoting and escaping (including Unicode escapes and escaped control characters). Bare values end at whitespace, comma, or `)`; comma separates arguments and a trailing comma is accepted. Integers are decimal signed 32-bit values (`-?[0-9]+`); booleans are `true` or `false`. Positional indices are contiguous; every argument can also be represented by its stable name in restored chip text. Mixed named/positional input advances the positional cursor only for positional values, and assigning the same argument twice is an error.

A descriptor is declarative data, for example:

```json
{
  "identity": "builtin:prepare/prepare",
  "name": "prepare",
  "aliases": [],
  "display_name": "Prepare context",
  "description": "Prepare scoped context before processing the request",
  "syntax": "parenthesized",
  "arguments": [
    {"name": "path", "position": 0, "required": true, "value_type": {"kind": "worker_file"}, "completion": {"kind": "worker_file"}},
    {"name": "mode", "required": false, "value_type": {"kind": "enum", "values": ["safe", "fast"]}, "completion": {"kind": "static", "values": ["safe", "fast"]}}
  ]
}
```

`/prepare("資料/a b", mode=safe) explain the result` produces a typed invocation and a separate following Text segment only when explicitly confirmed. Missing required arguments, unknown or duplicate arguments, invalid enum/primitive values, and incomplete quotations remain validation errors, not partial business execution.

Descriptors declare requiredness, value type, positional index, completion strategy, and optional description. Completion strategies are static values, Worker files, client files, or a host-owned provider. Metadata and completion are side-effect free. Arbitrary parser or provider code never crosses into a client.

Clients may parse this spelling only for input assistance. A call becomes executable only after the user selects a descriptor and the client emits `Segment::FeatureInvoke`. Free text, URLs, paths, pasted content, attachments, assistant output, and tool output are not scanned or promoted by the Host.

## Composer and completion lifecycle

TUI and Web composers retain selected invocations as atomic typed chips containing identity and structured arguments. Backspace/Delete and range deletion remove an unsent chip; removal is not an undo for an already executed operation. Draft history, transport retry, queueing, Session history, snapshot/live projections, and rewind restoration preserve the typed segment rather than flattening and reparsing it.

Completion requests carry a kind, fresh `request_id`, and source-qualified argument context. Host and Runtime echo the request ID unchanged. Clients correlate replies with that nonce plus their current draft revision, cursor, target Worker, authority snapshot, and connection generation; even identical-prefix/context ABA replies are discarded. Old uncorrelated replies cannot establish a selected invocation. Client-file arguments use a declared `InvocationClientAdapter`; they do not ask the Worker to resolve a client-local path.

## Attachment adapter

`/attach` is the built-in real use of the generic contract. Its descriptor declares a required client-file argument and the attachment client adapter.

- TUI may select a path visible to the TUI process, stage an upload, and replace the invocation intent with an `UploadedFile` chip.
- Web opens the existing browser picker; it does not enumerate the browser host filesystem.
- The local path is never sent to or resolved by the Worker.
- Uploading is cancellable draft staging, not invocation execution. Removing an in-progress upload chip by Backspace/Delete, range replacement, or draft abandonment cancels its reservation and adapter upload; the removed upload must no longer block a text-only Submit. Late callbacks and Undo cannot restore that cancelled reservation as sendable input. A completed unsent uploaded-file chip may retain its staged resource while editor Undo can restore it, releasing abandoned resources at the draft boundary; a late completion must never turn a deleted pending reservation into such an Undo-owned resource.
- Authoritative `SubmissionAccepted` (not socket-send success) transfers uploaded references to the durable input lifecycle. Draft removal cannot delete an in-flight reference whose admission may already have succeeded, or an accepted attachment. Rejection restores the exact typed intent for retry; retry retains request/invocation IDs. Upload completion alone is not submission acceptance.
- TUI standalone currently has no client-local upload staging transport. It reports the unsupported adapter and preserves the draft; it does not reinterpret a local path as a Worker host path.

There is no `/clear-attachments` command. Normal composer editing is the removal mechanism.

## Installation and execution

A Feature declares invocation descriptors using `FeatureDescriptor::with_chat_invocation` and registers matching handlers with `context.chat_invocations().register` (or `register_with_completion`) during `FeatureModule::install`. Registration must match the declared descriptor and the installing Feature's source-qualified identity. The invocation registry is checkpointed and rolled back with other Feature contributions. Only successfully installed, currently available descriptors are exposed through completion.

Authority-sensitive handlers override the side-effect-free `is_available()` discovery hook and `validate()` semantic hook. They must still revalidate authorization and use scoped idempotency at their business commit boundary inside `invoke()`; defaults grant no extra domain authority. Availability is checked again after asynchronous provider/handler work. Reusing a successful preparation result or resuming its dependent model run also rechecks the side-effect-free semantic/argument-specific authority validation; general Feature availability alone is insufficient. Completion strategies come from installed metadata, never from a client-selected provider identifier. Enum and boolean candidates can derive directly from value types when no explicit completion strategy is supplied.

Completion output is bounded to 64 entries / 64 KiB encoded data, with per-field byte limits. Oversized candidate values are discarded instead of altered. Handler results allow 4 KiB messages and 16 KiB context; oversized results/errors after execution become `OutcomeUnknown`, withholding dependent model context rather than assuming the operation was unexecuted.

The Worker validates every structured payload again against the installed descriptor. It executes invocations in segment order only when the durable submission is activated, after committing the user input and before the dependent LLM request. A completion choice, draft edit, queued-but-cancelled submission, reconnect, history view, or restored snapshot never executes a handler.

A submission may contain at most eight invocations with distinct invocation IDs. The first failed or outcome-unknown result fences later invocations **and prevents the dependent LLM run**; no cross-invocation rollback is promised. Every result is committed as typed Session history and rendered as bounded model-visible context. `OutcomeUnknown` is distinct from failure and is never automatically retried.

Before calling a handler, the Worker appends a start receipt through the existing `worker.pending_activations.v1` extension checkpoint. The result and updated receipt are appended together with the annotated system item. An unresolved start (including handler completion followed by a failed result append) remains a fence across same-process retry, restart, Pause/Resume, compaction, and rewind. Reusing an ID with changed arguments is rejected; exact replay can reuse its recorded result but does not invoke again. Receipt capacity is bounded and refuses untracked execution rather than evicting recovery evidence. Restoring or displaying history never reexecutes a handler.

Rewind preserves invocation-only receipts and the recovery-blocked fence independently of submission/notification queues. The retained history prefix and recovery checkpoint replace the segment together through the existing atomic Store replacement contract. There is no truncate-then-checkpoint repair window in which a crash can erase a completed or ambiguous business operation's recovery evidence.

The recovery receipt retains the invocation ID, qualified identity, SHA256 typed-payload digest, and bounded result—not another copy of every argument. Invocation IDs are nonempty and at most 128 UTF-8 bytes; clients generate fresh UUIDs on explicit selection and preserve them through transport retry. The session retains at most 1,024 recovery receipts without eviction. Corrupt, duplicate, inconsistent, or oversized recovery evidence fails closed.

`submission_request_id` protects submission admission and retry, while `invocation_id` is the stable idempotency key passed to a side-effecting handler. A handler must scope that identity at its domain boundary. This design does not claim a new generic exactly-once substrate.

## Authority boundaries

Possession of syntax, metadata, identity, or a typed payload grants no permission. Runtime target, enabled Feature, argument schema, and captured domain authority are checked at execution. Completion providers cannot perform business mutations or expose credentials. Handlers may share an underlying domain operation with Worker tools, but chat discovery does not expose every tool automatically.

Text-only clients submit text. The Backend and Host never reinterpret text as an invocation. Unknown typed input is visibly unresolved in restoration and **rejected on submission**, not silently flattened or processed by the LLM. Notify remains text-only: extra typed wire fields or typed Runtime input cannot be discarded into an advisory message or executed as a side-effecting call.
