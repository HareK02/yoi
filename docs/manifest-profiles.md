# Manifest profiles

Profiles are reusable Lua-authored recipes for generating an Insomnia runtime manifest. The Rust resolver evaluates a selected `.lua` profile in-process, validates that it is Profile-shaped rather than a complete Manifest, then binds runtime values such as Pod name and concrete scope to produce the persisted `PodManifest` snapshot.

Profiles are intentionally not authority-bearing manifests. `pod.name`, concrete `scope.allow` / `scope.deny`, runtime directories, sockets, active session state, and raw secret material do not belong in reusable profiles. Use `insomnia keys` to store provider/WebSearch credentials, then reference explicit secret ids such as `auth = { kind = "secret_ref", ref = "providers/anthropic/default" }` or `web.search.api_key_secret = "web/brave/default"`. Use `--manifest` when you need the explicit low-level complete Manifest escape hatch.

## Minimal profile

```lua
local profile = require("insomnia.profile")
local models = require("insomnia.models")
local scope = require("insomnia.scope")

return profile {
  slug = "coder",
  description = "Example coding Pod",

  model = models.catalog("codex-oauth/gpt-5.5"),
  worker = {
    reasoning = "high",
  },
  scope = scope.workspace_write(),
}
```

Run an explicit path with:

```sh
insomnia pod --profile ./coder.lua
# or through the TUI fresh-spawn dialog
insomnia --profile ./coder.lua
```

`--profile` accepts an explicit path, `path:<path>`, a discovered profile name, `default`, or a source-qualified name such as `project:coder`, `user:coder`, or `builtin:coder`. Path-like values containing `/`, starting with `.`, or ending in `.lua` are explicit paths. ``.nix` paths are no longer supported as profiles and fail with a diagnostic that points users at Lua profiles or `--manifest`.

`--profile` conflicts with `insomnia pod --manifest` and with restore/session/adopt modes. Use `--profile-pod-name <name>` when a launcher needs a creation-time Pod name override without invoking `--pod` restore semantics. Profile evaluation is a creation-time path; Pod resume restores saved Pod state/resolved snapshots rather than re-evaluating the profile source.

## Controlled Lua environment

Profiles run in a restricted Lua VM. Host virtual modules are available through controlled `require`:

- `require("insomnia")`
- `require("insomnia.profile")`
- `require("insomnia.models")`
- `require("insomnia.compact")`
- `require("insomnia.scope")`

Profile-local modules may be required by dotted names such as `require("shared")` or `require("shared.models")`; those resolve only under the selected profile file's directory. Unsafe/unrestricted Lua facilities such as `os`, `io`, `debug`, `package`, `dofile`, and `loadfile` are unavailable by default.

## Profile discovery

Profile discovery is separate from runtime manifest merging. User/project `profiles.toml` files may declare profile registry metadata, but those files are application/project UX configuration and are not merged into the selected profile artifact.

Example project config at `.insomnia/profiles.toml`:

```toml
default = "coder"

[profile]
coder = "profiles/coder.lua"
reviewer = "profiles/reviewer.lua"
```

Table entries can carry descriptions:

```toml
[profile.coder]
path = "profiles/coder.lua"
description = "Project coding assistant"
```

Relative registry paths are resolved against the `profiles.toml` file that declares them. Discovery checks bundled builtin profiles, then the user registry at `<config_dir>/profiles.toml`, then the nearest project registry at `.insomnia/profiles.toml`. The bundled `builtin:default` profile is the fallback default when no user/project registry declares another default. Later defaults override earlier defaults, so a project default wins over a user default, and either wins over the builtin default. Unqualified defaults resolve within the declaring source by default. Unqualified ambiguous names fail closed:

```sh
insomnia --profile coder          # fails if both user:coder and project:coder exist
insomnia --profile project:coder  # source-qualified selection
insomnia --profile default        # selected registry default
```

The fresh-spawn TUI also uses discovery. The new Pod dialog defaults to the selected registry default, normally `builtin:default` unless a user/project registry overrides it. `Tab`/`Down` cycles forward through discovered profiles and `Shift-Tab`/`Up` cycles backward; there is no ambient manifest-cascade opt-out. Passing `insomnia --profile <selector>` opens the same new Pod dialog with that selector selected and leaves Pod-name editing unchanged.

## One-file manifests

`insomnia pod --manifest <PATH>` remains as an explicit compatibility/debug path. It reads exactly that TOML file, resolves relative paths against the file's parent directory, merges builtin defaults, and validates through the same `PodManifestConfig -> PodManifest` boundary as profile artifacts. It does not load user or project `manifest.toml` files and conflicts with `--profile`.

Ambient user/project `manifest.toml` cascade startup has been removed. Normal fresh spawns use profile discovery/default selection, with `profiles.toml` acting only as a profile registry/default selector.

## Resolver contract

A Lua profile should return either `profile { ... }` or a plain table containing Profile fields. The resolver converts reusable fields such as `model`, `worker`, `compaction`, `memory`, `web`, `permissions`, `session`, and scope intent into a concrete Manifest. Runtime Pod name and concrete scope authority are supplied by launch context, then the resolved Manifest snapshot is persisted for restore.

Profile and one-file manifest CLI paths currently use builtin prompt assets only. `$insomnia/...` instruction refs work; `$user/...` and `$workspace/...` prompt refs need a future explicit prompt-loader source design instead of reviving ambient manifest discovery.
