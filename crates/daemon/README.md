# daemon

## Role

`daemon` is reserved for future long-lived Pod lifecycle management.

## Boundaries

Owns:

- daemon-specific lifecycle coordination when that design is implemented

Does not own today:

- current Pod socket serving (`pod`)
- normal CLI/TUI startup (`yoi`, `tui`)
- live registry mechanics already handled elsewhere (`pod-registry`)

## Design notes

This crate exists as a placeholder. Do not route current runtime authority through it until there is a concrete daemon design and work item.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
