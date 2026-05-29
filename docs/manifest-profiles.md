# Manifest profiles

Manifest profiles are the human-authored Nix entrypoint for generating an Insomnia runtime manifest. The Rust side evaluates a selected profile with `nix eval --json --file <path>`, deserializes the resulting JSON artifact, and validates it through the existing `PodManifest` pipeline.

This keeps composition/import/common logic in Nix. Insomnia does not add an implicit profile cascade or merge TOML profile layers into the selected runtime manifest.

## Minimal profile

```nix
let
  insomnia = import ./resources/nix/profile-lib.nix {};
in
insomnia.mkProfile {
  name = "coder";
  description = "Example coding Pod";
  manifest = insomnia.mkManifest {
    pod.name = "coder";
    model = {
      scheme = "anthropic";
      model_id = "claude-sonnet-4-20250514";
      auth = insomnia.secrets.ref "llm.anthropic.default";
    };
    scope.allow = [
      { target = "."; permission = "write"; }
    ];
  };
}
```

Run an explicit path with:

```sh
insomnia-pod --profile ./coder.nix
# or through the TUI fresh-spawn dialog
insomnia --profile ./coder.nix
```

`--profile` accepts an explicit path, `path:<path>`, a discovered profile name, `default`, or a source-qualified name such as `project:coder`, `user:coder`, or `builtin:coder`. Path-like values containing `/`, starting with `.`, or ending in `.nix` preserve the original explicit-path behavior.

`--profile` conflicts with `insomnia-pod --manifest` and with restore/session/adopt modes. Use `--profile-pod-name <name>` when a launcher needs a creation-time Pod name override without invoking `--pod` restore semantics. Profile evaluation is a creation-time path; Pod resume restores saved Pod state/resolved snapshots rather than re-evaluating the Nix source.

## Profile discovery

Profile discovery is separate from runtime manifest merging. User/project `profiles.toml` files may declare profile registry metadata, but those files are application/project UX configuration and are not merged into the Nix profile artifact.

Example project config at `.insomnia/profiles.toml`:

```toml
default = "coder"

[profile]
coder = "profiles/coder.nix"
reviewer = "profiles/reviewer.nix"
```

Table entries can carry descriptions:

```toml
[profile.coder]
path = "profiles/coder.nix"
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

`insomnia-pod --manifest <PATH>` remains as an explicit compatibility/debug path. It reads exactly that TOML file, resolves relative paths against the file's parent directory, merges builtin defaults, and validates through the same `PodManifestConfig -> PodManifest` boundary as profile artifacts. It does not load user or project `manifest.toml` files and conflicts with `--profile`.

Ambient user/project `manifest.toml` cascade startup has been removed. Normal fresh spawns use profile discovery/default selection, with `profiles.toml` acting only as a profile registry/default selector.

## Artifact contract

A profile should evaluate to one of:

- `{ profile = { format = "insomnia.nix-profile.v1"; ... }; manifest = { ... }; }`
- `{ profile = { format = "insomnia.nix-profile.v1"; ... }; config = { ... }; }`
- a raw manifest/config object for debug/test paths.

The resolved artifact is deserialized into the same `PodManifestConfig -> PodManifest` boundary used by direct one-file manifests, so builtin defaults and required-field validation stay shared. Explicit profile paths and user/project registry profile artifacts resolve relative manifest paths against the profile file's directory. Builtin profile artifacts resolve manifest-relative paths against the launch workspace/current directory so the bundled default can grant `scope.allow target = "."` for the workspace rather than for `resources/nix/profiles`.

Profile and one-file manifest CLI paths currently use builtin prompt assets only. `$insomnia/...` instruction refs work; `$user/...` and `$workspace/...` prompt refs need a future explicit prompt-loader source design instead of reviving ambient manifest discovery.

Secret values must stay as typed references. `resources/nix/profile-lib.nix` emits secret references as JSON like:

```json
{ "kind": "secret_ref", "ref": "llm.anthropic.default" }
```

The encrypted secret store is intentionally not implemented by this profile foundation; attempting to use a `secret_ref` as a live provider credential currently fails with a clear diagnostic at provider construction time.
