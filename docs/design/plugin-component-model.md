# Plugin Component Model boundary

Dynamic Worker Plugin execution is not part of the current product. The `.yoi-plugin` component metadata retained in `manifest` is an offline package-format contract only.

## Current behavior

- Worker creation and restore install no dynamic Plugin modules.
- Manifest/Profile input rejects `plugins` and `feature.plugins`.
- Runtime and Server startup perform no repository, ancestor, cwd, or user-data Plugin discovery.
- No persisted local `package_path` is execution authority.
- Only statically compiled trusted built-in Features contribute Worker capabilities.
- `yoi plugin check` parses an explicitly named directory or package without instantiating a component.

The package validator may reject legacy core-Wasm artifacts and require Component Model metadata, but passing validation does not make an artifact installable or executable.

## Future platform constraints

A future Server Plugin platform may execute Wasmtime Component Model packages only after the architecture is implemented as a coherent authority boundary:

1. an operator installs an immutable package into Server-owned artifact authority;
2. a Workspace owner selects an installed package through an immutable Addon revision;
3. Backend authors a per-Worker execution plan containing exact identities, digests, configuration, and bounded grants;
4. Runtime fetches only Server-authorized digests and verifies package bytes and execution-plan identity;
5. Runtime instantiates a fresh bounded Wasmtime Store with no ambient WASI authority; and
6. restore uses the persisted execution plan and exact artifact rather than current Workspace settings or a filesystem path.

Default components receive no filesystem, sockets, environment, clocks, randomness, subprocess, Workdir, broad Workspace client, credential, or network authority. Any host import must be narrow, typed, explicitly granted, live-revalidated where necessary, bounded, and audited.

This future platform must not restore `.yoi/plugins`, user-data catalogs, cwd/ancestor discovery, native dynamic libraries, downloaded Cargo manifests, or local paths as compatibility authority.
