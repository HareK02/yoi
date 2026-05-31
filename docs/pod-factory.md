# Pod Factory: Profile resolution and prompt assets

`PodFactory` は、選択された Profile または明示的な one-file Manifest から、検証済みの `PodManifest` と `PromptLoader` を生成する境界である。通常の fresh spawn は profile discovery/default selection を使い、user/project `manifest.toml` の ambient cascade は使わない。

`PodManifest` は Pod 起動に必要な完全な runtime recipe で、Pod 名、具体的な scope、解決済みパス、model/provider 設定、worker 設定などを含む。Profile はその前段の再利用可能な recipe template であり、Pod 名や concrete scope authority など runtime-bound な値は resolver が起動入力から埋める。

---

## 起動モード

| モード | 入力 | 用途 | ambient manifest cascade |
|---|---|---|---|
| Profile | `--profile <selector>` または省略時 default | 通常の fresh spawn。Lua profile を解決して runtime Manifest を作る | 使わない |
| One-file Manifest | `--manifest <path>` | デバッグ/互換用の完全 Manifest escape hatch | 使わない |
| Restore/attach | `--pod` / `--session` 等 | 保存済み Pod state / session snapshot から再開 | profile を再評価しない |
| SpawnPod internal | hidden `--spawn-config-json` | 親 Pod が子用 config を解決して渡す | 使わない |

`profiles.toml` は profile registry/default selection のための UX 設定であり、Pod Manifest 層ではない。Profile の中身へ merge されない。

## Profile resolution

Profile は Lua で書かれる。Rust resolver は selected profile を restricted Lua VM 内で評価し、返り値が Profile-shaped であることを検証してから `PodManifest` に変換する。

```lua
local profile = require("insomnia.profile")
local models = require("insomnia.models")
local scope = require("insomnia.scope")
local compact = require("insomnia.compact")

local model = models.catalog("codex-oauth/gpt-5.5")

return profile {
  slug = "coder",
  description = "Project coding profile",

  model = model,
  worker = {
    reasoning = "high",
  },
  compaction = compact.ratio {
    threshold = 0.8,
    request = 0.9,
  },
  scope = scope.workspace_write(),
}
```

Profile が表現してよいのは reusable な recipe fields である。

- model selection / provider policy
- worker settings / reasoning
- compaction policy
- memory / web policy
- permissions / tool policy
- session diagnostics policy
- scope intent such as `scope.workspace_write()`

Profile に入れてはいけないもの:

- `pod.name`
- concrete `scope.allow` / `scope.deny`
- runtime directories, sockets, callback addresses
- active/pending/session/pod-store runtime state
- resolved absolute paths that bind the profile to one machine/workspace
- raw resolved secret material

これらが必要な場合は、resolver の runtime input または `--manifest` の explicit Manifest path で扱う。

## Profile selector semantics

`--profile` は以下を受け付ける。

| 形 | 意味 |
|---|---|
| 省略 / `default` | registry default を使う。通常は bundled `builtin:default` |
| `<name>` | unqualified profile name。ambiguous なら fail closed |
| `builtin:<name>` / `user:<name>` / `project:<name>` | source-qualified selector |
| `path:<path>` / `./profile.lua` / `/abs/profile.lua` | 明示 path profile |

`.nix` profile files are no longer supported. Reusable profiles are Lua; complete low-level recipes belong behind `--manifest`.

Discovery は bundled builtin profiles、user registry (`<config_dir>/profiles.toml`)、project registry (`<project>/.insomnia/profiles.toml`) を読む。後段の default が前段の default を上書きするため、project default は user/default builtin より優先される。unqualified ambiguous names は source-qualified suggestion を出して失敗する。

Example `.insomnia/profiles.toml`:

```toml
default = "coder"

[profile]
coder = "profiles/coder.lua"
reviewer = "profiles/reviewer.lua"

[profile.orchestrator]
path = "profiles/orchestrator.lua"
description = "Project orchestrator"
```

Relative registry paths are resolved against the `profiles.toml` file that declares them.

## Controlled Lua environment

Profile evaluation runs with controlled host-provided `require`.

Host virtual modules:

- `require("insomnia")`
- `require("insomnia.profile")`
- `require("insomnia.models")`
- `require("insomnia.compact")`
- `require("insomnia.scope")`

Profile-local modules can be reused with dotted names such as `require("shared")` or `require("shared.models")`; they resolve only under the selected profile file's directory. Unsafe/unrestricted Lua facilities such as `os`, `io`, `debug`, unrestricted `package`, `dofile`, `loadfile`, `load`, and `collectgarbage` are unavailable by default.

## One-file Manifest mode

`--manifest <path>` reads exactly one TOML file, resolves relative paths against that file's parent directory, applies builtin defaults, and validates through `PodManifestConfig -> PodManifest`. It does not load user/project `manifest.toml` files and conflicts with `--profile`.

Use `--manifest` only when you need the complete low-level Manifest escape hatch or a focused debug fixture.

## Builtin defaults

Base defaults that are independent of profile choice live in Rust constants under `crates/manifest/src/defaults.rs` and in `PodManifestConfig::builtin_defaults()`. The bundled default role profile lives at `resources/profiles/default.lua` and is discovered as `builtin:default`.

デフォルト値を変更するときは、次のどちらを変更するのかを明確にする。

- all manifests/profiles の baseline default: Rust defaults
- ordinary dogfooding/default role: `resources/profiles/default.lua`

## Path resolution

Profile-local Lua module paths are resolved relative to the selected profile file's directory and are constrained to that directory tree.

Manifest paths in one-file Manifest mode are resolved relative to the manifest file's parent directory before validation. `scope.*.target` and auth file paths must be absolute by the time `PodManifest` is constructed; a remaining relative path indicates a resolver bug and returns `ResolveError::RelativePath`.

Pod cwd is not part of the Profile. It is the process `current_dir()` for manual startup, or the parent-selected `Command::current_dir` for spawned Pods.

## Unknown fields and type errors

Profile validation is stricter than old ambient manifest cascade semantics at the top-level boundary. Complete-Manifest/runtime-authority fields such as `manifest`, `config`, `pod`, and concrete `scope.allow`/`scope.deny` are diagnosed rather than silently treated as reusable profile data.

One-file Manifest deserialization keeps the Manifest compatibility behavior: unknown fields may warn/ignore where the Manifest schema allows it, while type mismatches are hard errors with file/path context.

## Prompt assets

`worker.instruction` refers to a prompt asset used as the main system prompt body. Import-map prefixes:

| Prefix | Resolution |
|---|---|
| `$insomnia` | bundled `resources/prompts/` (`include_dir!`) |
| `$user` | `<config_dir>/prompts/` |
| `$workspace` | `<project>/.insomnia/prompts/` |

`.md` extension can be omitted, e.g. `$insomnia/default` resolves to `resources/prompts/default.md`. Missing files are hard errors; prefixes do not fall through.

Profile and one-file Manifest CLI paths currently use builtin prompt assets only for initial loader construction. `$insomnia/...` works; `$user/...` and `$workspace/...` prompt refs need a future explicit prompt-loader source design instead of reviving ambient manifest discovery.

The rendered instruction body is followed by fixed Rust-provided sections for working boundaries and, when present, `AGENTS.md`. User templates cannot remove the scope section.

## `insomnia pod` CLI

Normal fresh startup uses profile discovery/default selection:

```text
insomnia pod [--profile <selector>] [--profile-pod-name <name>] [-s/--store <path>]
```

| Flag | Description |
|---|---|
| `--profile <selector>` | Select a Lua profile. Omitted means registry default, normally `builtin:default` |
| `--profile-pod-name <name>` | Fresh-spawn runtime `pod.name` override for profile resolution |
| `-s, --store <path>` | Session persistence directory, default `<data_dir>/sessions/` |

Restore/attach uses Pod/session state and does not re-evaluate profile sources.

```text
insomnia pod --pod <name>
insomnia pod --session <uuid>
```

Spawn children use hidden `--spawn-config-json`, `--adopt`, and `--callback <path>` flags. These are internal handoff details used by `SpawnPod` after the parent has allocated scope and prepared the child config.

`SpawnPod.profile` accepts registry/default selectors and the special `inherit` selector. Typical orchestration calls are `SpawnPod(profile = "project:coder")` for implementation work, `SpawnPod(profile = "project:reviewer")` for independent review, optionally `SpawnPod(profile = "project:orchestrator")` for lower-level coordination, or `SpawnPod(profile = "inherit")` when the child should reuse the parent role configuration. The explicit `SpawnPod.scope` remains the only delegated filesystem capability; selected or inherited profile scope is replaced by the tool argument.

## Programmatic boundary

New code should resolve profiles through the profile resolver and then construct Pods from the resulting `PodManifest` and `PromptLoader`. One-file Manifest helpers remain for tests/debugging. Avoid reintroducing user/project manifest cascade APIs as normal startup behavior.

```rust
let (manifest, loader) = resolve_profile_or_manifest(cli_inputs)?;
let pod = Pod::from_manifest(manifest, store, loader).await?;
```
