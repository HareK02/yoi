# Internal SubWorker follow-up observation gap

## Observed barrier

During T748's read-only caller inventory, the parent could read the first Runtime
inventory assistant result by stable entry ref
`E01a12e76-3893-70a0-bb73-28ee0c52f87b`. It then sent a follow-up to the same
Idle direct child, `t748-runtime-caller-inventory`, to close four remaining
execution/ownership boundaries.

After a turn-ended/Idle notification, `WorkerList` confirmed Idle, but bounded
`SearchSessionEntries` for assistant entries after follow-up user ref
`E01a12e76-d696-73d3-80d7-b8cde9935b37` returned no entries. Tool observations
were visible. The overview showed only the initial and first follow-up user
anchors, not a final result. A single additional Submit asking for the existing
findings (not re-investigation) returned success. After its turn-ended/Idle
notification, the same bounded assistant query still returned no entries, and the
overview still ended at the first follow-up user anchor.

This is an observation-surface symptom, **not proof** that an assistant result
was generated, lost, or that the underlying investigation completed. No raw
session storage or hidden worker state was read to bypass the projection.

The same symptom also occurred for `t748-domain-caller-inventory`: its first
result `E01a12e73-7ae4-7f02-9a7e-203140971635` was readable, but after the
follow-up turn-ended/Idle notification the overview ended at the follow-up's
starting commentary `E01a12e75-9c19-7bd0-9284-299116a05761` and intervening tool
records. No final follow-up result was visible. The parent released that
read-only delegation without another Submit and took over the remaining generated
adapter/forwarder and launch lower-edge checks. Neither `WorkerStop` tool outcome
was treated as evidence that the investigation completed.

## Handling

The parent did not mark the delegated work or Ticket complete, publish source,
or replace independent review with the notification. It stopped only the two
parent-owned read-only children to release delegation and took over the open
Runtime and domain boundaries. No live product process, parent Worker, target
branch, or attachment was stopped or cleaned.

## Improvement proposal

Expose enough committed turn metadata through the authorized observation surface
to distinguish a finalized assistant result, a turn ending without a result, and
an observation capture not yet including an accepted Submit. An explicit
completion marker tied to the visible Submit entry would prevent an Idle
notification from being confused with inspectable evidence. Preserve append-only
session entries and the current authorization projection; do not expose raw
prompts/reasoning or require polling/retry loops as a workaround.
