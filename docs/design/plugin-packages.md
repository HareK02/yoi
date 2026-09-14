# Plugin packages

Yoi retains `.yoi-plugin` as an offline authoring and inspection format. The format is not a Worker capability source.

## Current product boundary

Normal Worker creation, restore, Profile/Manifest resolution, Server/Runtime startup, and CLI execution do not discover Plugin catalogs from:

- repository or ancestor `.yoi/plugins` directories;
- user-data Plugin directories;
- the current working directory; or
- persisted local `package_path` values.

`plugins` and `feature.plugins` are rejected in Worker Manifest/Profile input. Dynamic Plugin Tools, Services, Ingress handlers, and WASM components are not installed or executed. Worker capabilities come only from statically compiled trusted built-in Features.

## Offline format operations

The CLI keeps only operations whose input or destination is explicit:

```sh
yoi plugin new rust-component-tool ./example-plugin
yoi plugin check ./example-plugin
yoi plugin pack ./example-plugin --output ./example-plugin.yoi-plugin
yoi plugin check ./example-plugin.yoi-plugin
```

These commands parse, validate, or write the named local artifact. They do not scan a Workspace, mutate Profile/Manifest configuration, install a package, register Worker Tools, or execute Plugin code. `plugin list` and `plugin show` were removed because their catalog semantics depended on ambient repository and user-data stores.

## Future installation authority

Server-installed Plugin packages and Addons are future work. That platform must provide explicit immutable package identity and digest, Server-owned artifact delivery and Workspace selection, a Backend-authored per-Worker execution plan, Runtime verification, and sandboxed execution. It must not restore repository-local or user-data discovery as a compatibility fallback.
