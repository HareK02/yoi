# manifest

## Role

`manifest` resolves reusable profile/configuration inputs into the concrete runtime Manifest used to create or restore Workers.

## Boundaries

Owns:

- Profile and Manifest data structures
- source/partial/resolved configuration layering
- path and prompt-resource resolution
- tool permission and filesystem scope configuration types
- model/provider references as configuration records

Does not own:

- provider HTTP clients or secret lookup implementation (`provider`, `secrets`)
- Worker lifecycle (`worker`)
- product CLI parsing (`yoi`)
- generated memory records (`memory`)

## Design notes

Profiles are reusable recipes; resolved Manifests are runtime contracts. Keep runtime-bound fields such as Worker name, concrete delegated scope, sockets, session pointers, and raw secrets out of reusable Profiles.

`feature.workdir_catalog.enabled` defaults to `true` for read-only Repository/Workdir/attachment reference discovery in WIP mode. It is independent of `feature.manage_workdir` (which defaults disabled), registers no normal Tools, and does not grant filesystem, attachment, or Workspace authority. Permissions remain independent. Standalone Tools-mode profiles may retain this default; only WIP installation consumes the flag. Set it to `false` to opt out.

## See also

- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
