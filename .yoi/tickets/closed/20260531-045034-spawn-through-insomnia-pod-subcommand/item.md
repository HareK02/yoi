---
id: 20260531-045034-spawn-through-insomnia-pod-subcommand
slug: spawn-through-insomnia-pod-subcommand
title: CLI: spawn Pods through insomnia pod runtime
status: closed
kind: task
priority: P2
labels: [cli, pod, client, nix]
created_at: 2026-05-31T04:50:34Z
updated_at: 2026-05-31T05:27:04Z
assignee: null
legacy_ticket: null
---

## Background

Parent/umbrella ticket: `single-binary-insomnia-cli`.

`insomnia-pod-subcommand-runtime` added `insomnia pod ...` as a Pod runtime entrypoint while keeping the old `insomnia-pod` binary as a temporary thin wrapper. Internal Pod spawning and restore paths still default to invoking `insomnia-pod` as an executable-only command.

The next single-binary migration step is to make internal callers able to spawn Pod runtime processes via `program + prefix_args`, and then switch the default runtime command to the current `insomnia` executable with `pod` as the prefix argument.

## Requirements

- Introduce a typed Pod runtime command representation instead of shell-string parsing.
  - Example shape: `{ program: PathBuf, prefix_args: Vec<OsString> }`.
  - The default command from the `insomnia` binary should be `current_exe()` + `pod` prefix args.
  - Preserve an explicit override mechanism for development/debugging, but do not parse arbitrary shell command strings.
- Switch internal spawn/restore paths that currently call `insomnia-pod` to use the typed runtime command:
  - TUI/create/restore paths in `client`/`tui` if applicable;
  - `SpawnPod` child creation;
  - `RestorePod` / discovery restore flows.
- Preserve detached process behavior and the `INSOMNIA-READY` stderr handshake.
- Preserve devshell/Nix behavior enough that local development still works.
  - If `insomnia-pod` is still needed for devshell wrapping in this step, document exactly why and leave removal to the next ticket.
- Keep `insomnia-pod` installed output/package removal out of this ticket unless the migration is already complete and validation is straightforward.
- Do not rename the `tui` package/crate.
- Do not merge Pod controller into the TUI process.

## Non-goals

- Removing `insomnia-pod` from Nix/package outputs as the primary goal. That is the next cleanup once runtime spawning no longer depends on it.
- Changing Pod runtime flags or profile/manifest semantics.
- Changing Pod protocol.
- Large CLI UX redesign.

## Acceptance criteria

- Internal default Pod runtime spawn/restore commands use `insomnia pod ...` through a typed `program + prefix_args` representation.
- `INSOMNIA_POD_COMMAND` or any equivalent override remains safe and documented, or is replaced by a typed override with clear behavior.
- Existing Pod spawning/restore tests pass, with focused tests proving prefix args are included in the spawned command.
- `insomnia pod --help` and existing `insomnia-pod --help` continue to work while the temporary wrapper remains.
- `SpawnPod` and `RestorePod` behavior is unchanged from the user/tool perspective.
- Follow-up for removing `insomnia-pod` packaging is recorded if not completed here.
- `cargo fmt --check`, focused client/pod/tui tests, `cargo check -p client -p pod -p tui`, `./tickets.sh doctor`, and `git diff --check` pass.
