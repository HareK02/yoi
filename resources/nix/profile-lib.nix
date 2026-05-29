# Insomnia Nix profile helpers.
#
# A profile file can use:
#
#   let insomnia = import ./path/to/profile-lib.nix {};
#   in insomnia.mkProfile {
#     name = "coder";
#     manifest = insomnia.mkManifest { ... };
#   }
#
# The output is consumed by `insomnia-pod --profile <path>` via
# `nix eval --json --file <path>`.

{ }:

let
  profileFormat = "insomnia.nix-profile.v1";

  optional = name: value:
    if value == null then {} else { ${name} = value; };

  secretRef = ref: {
    kind = "secret_ref";
    inherit ref;
  };

  mkManifest = manifest: manifest;

  mkProfile =
    { name ? null
    , description ? null
    , manifest ? null
    , config ? null
    , ...
    }@args:
    let
      resolvedManifest =
        if manifest != null then manifest
        else if config != null then config
        else removeAttrs args [ "name" "description" "manifest" "config" ];
    in
    {
      profile = ({ format = profileFormat; }
        // optional "name" name
        // optional "description" description);
      manifest = resolvedManifest;
    };

  semanticPresets = {
    # Skeleton for users to extend in their own Nix. Rust does not attach any
    # hidden semantic meaning to these helpers; they only generate manifest JSON.
    codingAssistant = { modelId ? "claude-sonnet-4-20250514", authRef ? null }:
      {
        model = {
          scheme = "anthropic";
          model_id = modelId;
        } // (if authRef == null then {} else { auth = secretRef authRef; });
      };
  };

in
{
  inherit profileFormat mkProfile mkManifest semanticPresets;
  secrets = {
    ref = secretRef;
  };
}
