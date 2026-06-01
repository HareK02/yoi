# secrets

## Role

`secrets` provides the local secret reference store used by provider and tool configuration.

## Boundaries

Owns:

- provider-independent secret id to value lookup
- modest plaintext-at-rest reduction and integrity checks
- secret store file format and validation

Does not own:

- provider-specific auth protocol (`provider`)
- Codex OAuth local integration shape (`provider`)
- prompting or model context
- work item or diagnostic redaction policy outside its API surface

## Design notes

The store is not a high-assurance keychain. It exists to avoid scattering plaintext credentials through config files and logs, not to provide strong local adversary protection.

Secret values must stay out of diagnostics, Debug output, CLI/TUI output, work items, docs, session logs, model context, and persisted plaintext files.

## See also

- [`../../docs/design/provider-model-boundary.md`](../../docs/design/provider-model-boundary.md)
