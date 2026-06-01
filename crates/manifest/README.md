# manifest

## Role

`manifest` resolves reusable profile/configuration inputs into the concrete runtime Manifest used to create or restore Pods.

## Boundaries

Owns:

- Profile and Manifest data structures
- source/partial/resolved configuration layering
- path and prompt-resource resolution
- tool permission and filesystem scope configuration types
- model/provider references as configuration records

Does not own:

- provider HTTP clients or secret lookup implementation (`provider`, `secrets`)
- Pod lifecycle (`pod`)
- product CLI parsing (`yoi`)
- generated memory records (`memory`)

## Design notes

Profiles are reusable recipes; resolved Manifests are runtime contracts. Keep runtime-bound fields such as Pod name, concrete delegated scope, sockets, session pointers, and raw secrets out of reusable Profiles.

## See also

- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
