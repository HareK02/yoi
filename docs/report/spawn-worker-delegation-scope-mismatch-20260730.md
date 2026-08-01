# SpawnWorker rejects a workspace subdirectory inside the parent write boundary

Date: 2026-07-30

## Symptom

While implementing the multiplexer protocol foundation, the parent Worker could write the checkout but could not delegate `crates/protocol` to a spawned Coder:

```text
Invalid argument: requested child scope .../checkout/crates/protocol Write is outside this Worker's delegation scope grant
```

The requested child scope was a recursive subdirectory of the checkout advertised as writable in the parent instructions. No child was created.

## Impact

The parent had to implement and review the protocol slice serially. This removed useful parallel review and made the long Runtime/Server protocol change more interruption-prone.

## Suggested improvement

- Make the effective delegation grant visible separately from the Worker's direct filesystem write scope.
- Validate and explain which ancestor rule prevents delegation.
- When a requested child scope is a strict subset of a writable/delegable checkout, accept it consistently.
- Distinguish “parent may write but may not delegate” from a malformed/out-of-bound scope error.
