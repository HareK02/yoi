Implemented Workspace Backend config file schema and resolver.

Delivered:
- `WorkspaceBackendConfigFile` TOML schema with `server`, `data`, `limits`, and `runtimes.remote` sections.
- Fixed workspace-local config path `.yoi/workspace-backend.local.toml`.
- Defaults when config file is absent.
- Unknown-field rejection.
- Workspace-root relative path resolution and absolute path preservation.
- Resolved config boundary for `ServerConfig`, listen address, and database path.
- Data root derivation for `workspace.db` and `embedded-runtime`.
- Secret value exclusion from schema; `token_ref` fails closed until secret ref resolution is implemented.
- `yoi-workspace-server serve` loads config and applies only existing legacy dev CLI overrides.
- Focused tests for defaults, unknown fields, path handling, data overrides, and secret field behavior.

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi --no-link`
