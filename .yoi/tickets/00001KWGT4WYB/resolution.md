Implemented explicit Workspace initialization command and removed implicit initialization from serve.

Delivered:
- `yoi workspace init [--workspace <PATH>]`.
- `yoi-workspace-server init [--workspace <PATH>]`.
- `init` creates `.yoi/workspace.toml` and `.yoi/workspace-backend.default.toml` only.
- `serve` requires `.yoi/workspace.toml` to already exist and fails with an init diagnostic otherwise.
- `serve` no longer creates workspace identity or default config template.
- Help text and focused tests updated.

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo test -p yoi`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
