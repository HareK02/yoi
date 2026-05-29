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

[alias]
work = "project:coder"
```

Table entries can carry descriptions:

```toml
[profile.coder]
path = "profiles/coder.nix"
description = "Project coding assistant"
```

Relative registry paths are resolved against the `profiles.toml` file that declares them. Discovery checks bundled builtin profiles, then the user registry at `<config_dir>/profiles.toml`, then the nearest project registry at `.insomnia/profiles.toml`. Later defaults override earlier defaults, so a project default wins over a user default. Unqualified alias/default targets resolve within the declaring source by default. Unqualified ambiguous names fail closed:

```sh
insomnia --profile coder          # fails if both user:coder and project:coder exist
insomnia --profile project:coder  # source-qualified selection
insomnia --profile default        # selected registry default
```

The fresh-spawn TUI also uses discovery. If a default profile is configured, the new Pod dialog shows a selectable source-qualified profile row such as `profile: project:coder (default)`. `Tab`/`Down` cycles forward through discovered profiles, `Shift-Tab`/`Up` cycles backward, and the `manifest cascade` choice opts out of a default profile for that spawn. Passing `insomnia --profile <selector>` opens the same new Pod dialog with that selector selected and leaves Pod-name editing unchanged.

## Artifact contract

A profile should evaluate to one of:

- `{ profile = { format = "insomnia.nix-profile.v1"; ... }; manifest = { ... }; }`
- `{ profile = { format = "insomnia.nix-profile.v1"; ... }; config = { ... }; }`
- a raw manifest/config object for debug/test paths.

The `manifest`/`config` object uses the same field names as the existing manifest config. Relative paths are resolved against the directory containing the profile file, then builtin defaults are merged and validation produces the runtime `PodManifest`.

Secret values must stay as typed references. `resources/nix/profile-lib.nix` emits secret references as JSON like:

```json
{ "kind": "secret_ref", "ref": "llm.anthropic.default" }
```

The encrypted secret store is intentionally not implemented by this profile foundation; attempting to use a `secret_ref` as a live provider credential currently fails with a clear diagnostic at provider construction time.
