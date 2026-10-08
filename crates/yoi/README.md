# yoi

## Role

`yoi` is the product CLI facade. It owns the installed binary name, top-level argument parsing, normal TUI launch, and product subcommands such as `yoi worker` and `yoi ticket`.

## Boundaries

Owns:

- product command shape and user-facing CLI entry points
- profile/default selection for ordinary startup
- wiring library crates into the executable
- headless maintenance commands that are part of the product surface

Does not own:

- Worker runtime internals (`worker`)
- socket client mechanics (`client`)
- model turn orchestration (`agen`)
- TUI rendering/runtime implementation (`tui`)

## Design notes

Keeping product CLI ownership here prevents lower crates from depending on the binary name or parsing top-level arguments. Runtime launch should use typed command boundaries, not shell-command strings.

## Workspace creation and selection

`yoi init` remains Git-oriented and requires an explicit initial repository key:

```sh
yoi --backend https://backend.example init --display-name Development --repository-key platform
```

For a Workspace with no initial repository, use the same Backend creation API explicitly:

```sh
yoi --backend https://backend.example workspace create --display-name Research
```

Both commands record Workspace-to-Backend routing in the global client configuration,
not repository-local identity files. Repository-free creation prints the Workspace id;
select it explicitly from any directory, without Git discovery:

```sh
yoi --workspace-id <ID>
yoi --workspace-id <ID> workers
yoi --workspace-id <ID> resume
yoi --workspace-id <ID> panel
```

`--workspace-id` selects Backend even when `default_connection` is `local`, and conflicts
with `--local`. An explicit `--backend <URL>` overrides configured routing. Without an
explicit Workspace id, Backend commands retain Git-based Workspace discovery.

Normal Backend startup launches the existing workdir-free Worker form. Repository and
workdir attachment selection are not prerequisites imposed by this CLI; Runtime
availability and workdir requirements remain Backend/TUI responsibilities.

## See also

- [`../../docs/design/overview.md`](../../docs/design/overview.md)
- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
