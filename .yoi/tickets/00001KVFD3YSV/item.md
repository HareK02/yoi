---
title: 'Plugin: add read-only CLI inspection list/show'
state: 'closed'
created_at: '2026-06-19T07:39:23Z'
updated_at: '2026-06-19T14:22:41Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'cli', 'diagnostics', 'read-only', 'json-output', 'no-execution']
queued_by: 'workspace-panel'
queued_at: '2026-06-19T10:19:28Z'
---

## Background

Plugin package discovery / explicit enablement / Tool registration / WASM Tool runtime / permission grants まで実装されたため、次に必要なのは「なぜ Plugin が見えない / 有効化されない / 実行できないのか」を headless に確認できる read-only inspection surface である。

Panel や TUI diagnostic に出す前に、CLI で deterministic に確認できる `yoi plugin list` / `yoi plugin show <ref>` を追加する。この CLI は Plugin code を実行せず、package discovery、manifest parse、enablement resolution、grant validation、static diagnostics を表示するだけにする。

目的は、Plugin の多段 failure point を human / JSON の両方で確認できるようにすること。

```text
package discovered?
manifest valid?
api version compatible?
explicitly enabled?
digest/version/source match?
requested permission granted?
tool schema valid?
runtime config present?
```

## Requirements

- Top-level product CLI に read-only Plugin inspection command を追加する。
  - `yoi plugin list`
  - `yoi plugin show <ref>`
- `--json` output を最初から提供する。
  - `yoi plugin list --json`
  - `yoi plugin show <ref> --json`
- Human-readable output は JSON 用 typed report の thin formatting にする。
- Workspace / Profile resolution は通常起動に近い意味にする。
  - default は current workspace。
  - 既存 CLI 方針に合わせて `--workspace <path>` を扱う。
  - Profile 指定が必要なら既存 Profile selector と整合する option を使う。
- Plugin code を実行しない。
  - WASM module を実行しない。
  - Tool call を発生させない。
  - Hook / Service / Ingress を起動しない。
- Read-only とする。
  - install / update / enable / disable / trust / sign / run は non-goal。
  - Plugin package / config / Ticket / memory / Pod state を変更しない。
- Inspection report は typed data として実装する。
  - future Panel diagnostic / tests / agent-readable output で再利用できる形にする。
- `list` は package/ref 単位の overview を出す。
  - ref
  - source
  - package path (human output では必要に応じて短縮)
  - version
  - api version
  - digest
  - status
  - enabled surfaces
  - diagnostic count / summary
- `show <ref>` は詳細を出す。
  - manifest metadata
  - source-qualified identity
  - package path
  - digest
  - version / api version
  - runtime kind/config summary
  - enabled surfaces
  - Tool definitions and registration eligibility
  - requested permissions
  - granted permissions
  - effective grants / denied grants
  - diagnostics
- Status vocabulary を明確にする。
  - `active`: enabled and statically valid for at least one surface/tool.
  - `disabled`: discovered but not explicitly enabled.
  - `missing`: enablement refers to a package that is not discovered.
  - `rejected`: invalid manifest / incompatible api / digest mismatch / grant mismatch / invalid schema etc.
  - `partial`: package is usable but some surfaces/tools are rejected.
- Diagnostics は bounded / safe にする。
  - secret-like values / auth / file contents を出さない。
  - path は必要最小限。JSON では absolute path が必要なら workspace/user store source と一緒に出す。
  - denial / parse / digest / grant mismatch reasons を区別できる。
- Ambiguous unqualified ref は fail closed し、`show` で diagnostic を返す。
- JSON schema は stable typed structure として test で固定する。

## Example human output

`yoi plugin list`:

```text
REF                    SOURCE   VERSION  STATUS    SURFACES  DIGEST
project:example.echo   project  0.1.0    active    tool      sha256:...
project:broken         project  -        rejected  -         -
user:fetch             user     0.2.1    disabled  tool      sha256:...
```

`yoi plugin show project:example.echo`:

```text
Plugin: project:example.echo
Source: project
Package: .yoi/plugins/example.echo.yoi-plugin
Version: 0.1.0
API: yoi-plugin-1
Digest: sha256:...
Status: active

Enabled surfaces:
- tool

Tools:
- example_echo
  status: registered
  schema: valid
  external_write: false

Permissions:
Requested:
- surfaces.tool
- tool:example_echo

Granted:
- surfaces.tool
- tool:example_echo

Diagnostics:
- none
```

## Acceptance criteria

- `yoi plugin list` prints a bounded human-readable overview without executing Plugin code.
- `yoi plugin show <ref>` prints detailed static inspection for a Plugin ref without executing Plugin code.
- `--json` output is available for both commands and uses a stable typed structure.
- Valid enabled Plugin appears as `active`.
- Discovered but not enabled Plugin appears as `disabled`.
- Enabled but missing package appears as `missing`.
- Invalid manifest / incompatible api version appears as `rejected` with diagnostic.
- Digest / version / source mismatch appears as diagnostic.
- Grant denial / missing requested permission appears as diagnostic.
- Partial tool/surface rejection can be represented without marking the whole package as fully active.
- Ambiguous unqualified id fails closed with diagnostic.
- Plugin code / WASM / Tool execution is not triggered by list/show.
- Tests cover:
  - list human output for active / disabled / rejected / missing packages
  - show human output for active package with Tool surface and grants
  - JSON list structure
  - JSON show structure
  - invalid manifest diagnostic
  - digest mismatch diagnostic
  - missing grant diagnostic
  - ambiguous ref diagnostic
  - no runtime execution from inspection path
- Validation: focused CLI/plugin inspection tests, relevant `cargo check` / `cargo test`, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` because product CLI / packaging surface changes.

## Non-goals

- Plugin install / update / remove.
- Enable / disable mutation.
- Trust / signature / registry implementation.
- Plugin code execution.
- WASM validation beyond static runtime config/manifest inspection.
- `https` host API implementation.
- `fs` host API implementation.
- Service / Ingress startup.
- Panel/TUI Plugin diagnostics UI.

## Implementation notes

- Product CLI ownership stays in the `yoi` crate.
- Avoid embedding resolver logic directly in display formatting; build a typed inspection report first.
- Reuse existing Plugin resolver / diagnostics where possible.
- Keep CLI output deterministic and suitable for tests.
- Do not introduce user-facing terminology `contribution category`; use Plugin runtime / surface / host API / grants.

## Related work

- `00001KV5R5V2S` — Plugin package discovery and explicit enablement resolver.
- `00001KV5W3PHA` — Plugin Tool surface registration.
- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KSXRQ4G8` — Plugin runtime / surface / minimal host API model design.
