# Manifest profiles

Manifest profiles are the human-authored Nix entrypoint for generating an Insomnia runtime manifest. The Rust side evaluates a selected profile with `nix eval --json --file <path>`, deserializes the resulting JSON artifact, and validates it through the existing `PodManifest` pipeline.

This keeps composition/import/common logic in Nix. Insomnia does not add an implicit profile cascade or merge TOML profile layers at runtime.

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

Run it with:

```sh
insomnia-pod --profile ./coder.nix
# or through the TUI fresh-spawn dialog
insomnia --profile ./coder.nix
```

`--profile` conflicts with `insomnia-pod --manifest` and with restore/session/adopt modes. Use `--profile-pod-name <name>` when a launcher needs a creation-time Pod name override without invoking `--pod` restore semantics. Profile evaluation is a creation-time path; Pod resume should restore saved Pod state/resolved snapshots rather than re-evaluating the Nix source.

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
