# yoi

`yoi` is the product CLI entry point. It handles argument parsing, connection and Profile selection, TUI launch, and assembly of administrative subcommands. Worker execution, transport, and terminal rendering are delegated to libraries.

## Implementation entry points

Command dispatch, connection selection, and Workspace bootstrap are binary-internal. The library exposes:

- [`workdir_share`] — sharing existing Workdirs.
