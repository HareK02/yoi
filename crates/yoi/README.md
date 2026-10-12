# yoi

`yoi` is the product CLI entry point. It handles argument parsing, connection and Profile selection, TUI launch, and assembly of administrative subcommands. Worker execution, transport, and terminal rendering are delegated to libraries.

## Implementation entry points

- `src/main.rs`: product command parsing and dispatch to launch targets.
- `src/cli_connection.rs`: per-command connection requirements and target resolution.
- `src/workspace_bootstrap.rs`: Workspace creation and Backend routing records.
- [`workdir_share`] — sharing existing Workdirs.
