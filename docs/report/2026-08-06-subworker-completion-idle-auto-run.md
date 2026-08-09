# SubWorker completion notification did not wake an idle parent

Date: 2026-08-06
Ticket: `00001KZKNWP5X`

## Observed behavior

A Reviewer SubWorker finished after its parent Worker had returned to idle. Although the completion notification was marked `auto_run: true`, the parent did not run until the user submitted another message. The notification appeared only in that later turn.

## Root cause

The completion callback called `NotifyBuffer::push_notify(..., true)` directly. A running parent checks that buffer at turn end and can stage a follow-up, but an idle controller waits on its method channel. Writing the buffer alone therefore could not wake an already-idle parent.

## Fix

Normal controller-owned Workers now give the SubWorker tool a weak sender for the parent method channel. Completion is delivered through the existing:

```rust
Method::Notify {
    message,
    auto_run: true,
}
```

path, which commits the notification through the normal inbox and wakes an idle controller. A `WeakSender` avoids a controller/tool/channel reference cycle that would otherwise keep the controller alive after external handles are dropped. Internal Worker sessions without a controller retain the direct buffer target.

## Regression coverage

- SubWorker completion sends exactly one `Method::Notify { auto_run: true }` to the parent controller channel.
- The controller notification target does not keep the method channel alive after the strong sender is dropped.
- Existing running-parent notification follow-up tests remain green.
- `cargo test -p worker --lib`: 514 passed.
