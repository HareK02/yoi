# Plugin package authoring

Dynamic Worker Plugins are not currently installable or executable. This guide covers only the retained offline `.yoi-plugin` authoring format.

## Commands

Every retained command uses an explicit local input or destination:

```sh
yoi plugin new rust-component-tool ./example-plugin
yoi plugin new rust-component-service ./example-service
yoi plugin check ./example-plugin
yoi plugin pack ./example-plugin --output ./example-plugin.yoi-plugin
yoi plugin check ./example-plugin.yoi-plugin
```

- `new` writes an embedded starter template to the named destination and refuses unsafe or non-empty destinations.
- `check` parses and validates the named directory or package without executing Plugin code.
- `pack` validates the named directory and writes a deterministic constrained archive.

`list`, `show`, `--workspace`, and `--profile` are intentionally unavailable. They previously implied ambient Workspace/user catalog discovery.

## Safety and authority

Offline package commands do not:

- inspect repository or ancestor `.yoi/plugins` directories;
- inspect a user-data Plugin store;
- enable or install a package;
- mutate Profile or Manifest configuration;
- register Worker Tools, Services, or Ingress handlers;
- instantiate or execute a WASM component; or
- grant filesystem, network, secret, Ticket, or Workspace authority.

`plugins` and `feature.plugins` are rejected by current Worker Manifest/Profile resolution. Statically compiled built-in Features are the only current Worker capability source.

## Package format

A package directory contains `plugin.toml` plus the files named by that manifest. A packed `.yoi-plugin` uses the constrained deterministic archive format documented in [`../design/plugin-packages.md`](../design/plugin-packages.md). Validation rejects malformed metadata, unsafe paths, links, unsupported entries, bounds violations, digest inconsistencies, and legacy raw core-Wasm runtime declarations.

Generated templates include a placeholder `plugin.component.wasm`. Replace it with a real built component before `check` can report the package as verified. A verified package is still only an offline artifact; verification does not install or authorize it.

## Future Server Plugin platform

Do not copy packages into repository or user-data catalogs. Future installation must go through Server-owned package/artifact authority, immutable identity and digest selection, Backend-authored Worker execution plans, Runtime verification, and sandboxed execution. That platform is separate work and must not reintroduce filesystem catalog fallback.
