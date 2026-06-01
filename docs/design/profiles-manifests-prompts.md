# Profiles, Manifests, and prompts

Profiles are reusable recipes. Resolved Manifests are runtime contracts. Prompt resources are managed assets. Keeping those layers separate prevents runtime state from leaking into reusable configuration.

## Profiles

A Profile describes how a Pod should normally be built: worker language, model/provider selectors, prompt choices, tool policy defaults, and other reusable preferences.

A Profile should not contain runtime-bound fields:

- `pod.name`
- concrete delegated `scope.allow`
- sockets or process identifiers
- session pointers
- restored spawned-child state
- raw secret values

Those fields depend on one run, one parent, or one machine. Putting them in a reusable Profile makes reuse unsafe.

Yoi is Lua Profile first. Lua gives project/user authors a controlled recipe layer through host-provided `require("yoi")` modules without treating Nix as runtime authoring. Nix remains useful for packaging and development records.

## Manifests

A resolved Manifest is the concrete contract used to create or restore a Pod. It carries defaults, resolved paths, permissions, scope, prompt references, provider/model decisions, and runtime identity.

Source/partial layers may omit fields. Resolved manifests should be explicit enough that Pod creation does not depend on ambient configuration later changing under it.

`--manifest <path>` exists as an explicit low-level escape hatch. Normal fresh startup should select a Profile through `profiles.toml` / builtin defaults rather than ambient manifest cascades.

## Spawned Pods

`SpawnPod.profile` is optional and resolves through defaults when omitted. The only concrete capability delegation in the tool call is `SpawnPod.scope`, and it must be a subset of the parent's effective scope.

`inherit` derives reusable settings from the parent's resolved Manifest while replacing child identity and delegated scope. It should not blindly reuse the parent's original Profile source or runtime state.

## Prompt resources

Prompts live under `resources/prompts` so builtins, project overrides, and user overrides have one asset boundary.

The prompt layer should explain policy and behavior, but it should not smuggle volatile state into model context. Runtime facts that affect later turns must still go through history.

Builtin resources should be embedded at compile time. User/project profiles, explicit profile paths, prompt overlays, provider/model overrides, and explicit manifests remain filesystem-based.

## Why this separation matters

Without this split, configuration becomes unreproducible: a Profile might accidentally depend on a parent Pod's socket, a prompt override might act like hidden state, or a restored Pod might observe different defaults than the run that created it.

The boundaries make it clear which information is reusable authoring, which is resolved runtime contract, and which is durable run history.
