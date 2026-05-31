<!-- event: create author: tickets.sh at: 2026-05-31T07:42:58Z -->

## Created

Created by tickets.sh create.

---

<!-- event: close author: hare at: 2026-05-31T13:38:30Z status: closed -->

## Closed

This ticket is completed/superseded by `insomnia-crate-cli-owner`.

The original goal was to remove CLI parsing from `crates/tui/src/main.rs` so the TUI crate would stop owning top-level product CLI parsing. The later `insomnia-crate-cli-owner` implementation went further:

- the product binary moved from package `tui` to package `insomnia`;
- top-level CLI parsing and dispatch now live in `crates/insomnia/src/main.rs`;
- `tui` is now a library implementation crate with no product `main.rs`;
- `insomnia pod ...` and `insomnia memory lint ...` are routed by the `insomnia` crate;
- normal TUI launch is delegated to `tui::launch(...)`.

Because the TUI crate no longer owns the product entrypoint or CLI parser, there is no remaining `tui/src/main.rs` CLI parsing extraction to implement under this ticket. Further CLI parser cleanup, if desired, belongs to the `insomnia` crate, not this TUI cleanup ticket.

Validation evidence from `insomnia-crate-cli-owner` closure:
- `cargo test -p insomnia`
- CLI smoke for `--help`, `pod --help`, `memory lint --help`, invalid `--session`, and resume/Pod selection conflicts
- `cargo check -p client -p pod -p tui -p insomnia`
- `nix build .#insomnia`
- `./tickets.sh doctor`
- `git diff --check`


---
