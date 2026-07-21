Backend/runtime authoritative worker list is now surfaced in the TUI.

Completed scope:
- Added backend runtime worker list DTO/API to the client crate.
- Added `yoi workers` and Backend runtime picker launch routing.
- Added client-side Backend URL resolution from explicit `--backend` or `$XDG_CONFIG_HOME/yoi/client.toml`, with workspace id read from explicit `--workspace-id` or `<workspace>/.yoi/workspace.toml`.
- Added TUI Backend worker picker using Backend `WorkerSummary` as authority and displaying runtime id, worker id, label/profile, state, and working-directory summary.
- Fixed missing GET routes for runtime worker list endpoints.
- Aligned the Backend worker picker styling/controls with the existing inline resume picker.
- Fixed the `nix develop` shell hook syntax error in `devshell.nix`.

Validation performed under `nix develop` included targeted cargo check/test/fmt and `git diff --check`. Manual TUI verification was also reported successful by the user.