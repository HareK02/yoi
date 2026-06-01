Completed the clean Yoi rename implementation.

The public identity is now `Yoi` / `Yoi agent` for prose, `yoi` for package/CLI names, `.yoi` for local project/data paths, and `YOI_*` / `YOI-READY` only for environment variables and protocol literals.

No old `insomnia` command aliases, env aliases, prompt/profile aliases, old search roots, or old-path handling are implemented. Existing dogfood state was moved manually outside the codebase.

Validation recorded in `artifacts/implementation-report.md`; final focused checks after correction passed:

- `cargo fmt --check`
- `cargo test -p manifest paths`
- `cargo test -p pod spawn_profile_selector_accepts_default_inherit_and_registry`
- `git diff --check`
