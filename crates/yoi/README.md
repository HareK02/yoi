# yoi

## Role

`yoi` is the product CLI facade. It owns the installed binary name, top-level argument parsing, normal TUI launch, and product subcommands such as `yoi pod` and `yoi memory lint`.

## Boundaries

Owns:

- product command shape and user-facing CLI entry points
- profile/default selection for ordinary startup
- wiring library crates into the executable
- headless maintenance commands that are part of the product surface

Does not own:

- Pod runtime internals (`pod`)
- socket client mechanics (`client`)
- model turn orchestration (`llm-worker`)
- TUI rendering/runtime implementation (`tui`)

## Design notes

Keeping product CLI ownership here prevents lower crates from depending on the binary name or parsing top-level arguments. Runtime launch should use typed command boundaries, not shell-command strings.

## See also

- [`../../docs/design/overview.md`](../../docs/design/overview.md)
- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
