Merged and completed the Ticket record language split.

Summary:
- Added optional `[ticket].language` support to Ticket config, independent from `worker.language` and `memory.language`.
- Set this repository's `.yoi/ticket.config.toml` to `language = "Japanese"` under `[ticket]`.
- Added scaffold support and validation for the new setting.
- Propagated configured Ticket record language through Ticket CLI/backend construction, built-in Ticket feature backend construction, Panel backend construction, and Ticket role launch context/prompt guidance.
- Added minimal generated-text support for configured Japanese on prominent generated Ticket record paths while preserving English/default behavior when absent or unsupported.
- Existing Tickets were not bulk-translated or rewritten.

Merged branch/worktree:
- Branch: `separate-ticket-record-language-from-worker-language`
- Commit: `fb261bb feat: add ticket record language config`
- Merge commit on `develop`: `a74e315 merge: separate ticket record language`

Validation passed after merge:
- `cargo test -p ticket config::tests --lib`
- `cargo test -p ticket create_uses_configured_japanese_record_language_for_generated_defaults --lib`
- `cargo test -p client configured_ticket_record_language_is_included_in_role_prompt --lib`
- `cargo test -p yoi ticket_cli_init_writes_explicit_ticket_config_scaffold`
- `cargo test -p client ticket_role --lib`
- `cargo test -p ticket --lib`
- `cargo check -p pod -p tui`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/separate-ticket-record-language-from-worker-language`.
- Deleted branch `separate-ticket-record-language-from-worker-language`.

Residual notes:
- This is not full localization infrastructure. Some generated Ticket-related messages may remain English unless separately localized later.
- Japanese generation coverage is intentionally scoped to prominent configured record paths and role guidance.