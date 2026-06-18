---
title: 'Plugin: package discovery and explicit enablement resolver'
state: 'closed'
created_at: '2026-06-15T13:40:15Z'
updated_at: '2026-06-18T12:22:04Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'package-loading', 'discovery', 'enablement', 'capability-boundary', 'startup-restore']
queued_by: 'workspace-panel'
queued_at: '2026-06-15T13:59:47Z'
---

## Background

Plugin を利用可能にする最初の実装として、Plugin package を安全に発見・検査し、明示設定に基づいて enablement target として resolve できるようにする。

この Ticket では Plugin code の実行、Tool / Hook / Service / Ingress の登録、WASM runtime、`https` / `fs` host API 実装は行わない。package discovery と explicit enablement resolver を先に固め、後続の Plugin runtime / surface 実装が同じ解決結果を使えるようにする。

既存設計では以下を分離する。

```text
package discovery != enablement != runtime initialization != contribution registration
```

package が存在するだけで実行・登録されてはならない。Yoi は package を read-only に発見し、profile / config 側の明示 enablement entry と照合できるところまでをこの Ticket の範囲とする。

## Scope

- Plugin package discovery
- Plugin package archive / directory safety validation
- `plugin.toml` manifest parsing
- Plugin package identity resolution
- Explicit enablement entry resolution
- Resolved Plugin metadata / diagnostics surface
- Startup / snapshot restore で再現可能な resolved input の保持または再構築方針

## Package locations

最初の discovery source は以下を対象にする。

- User plugin store:
  - `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/*.yoi-plugin`
- Workspace plugin store:
  - `<workspace>/.yoi/plugins/*.yoi-plugin`

実装上、`.yoi-plugin` を archive として扱うか directory として扱うかは、既存設計 `00001KT0Z4BK8` を確認して合わせる。未実装部分がある場合は、最小実装を明示し、後続 Ticket で拡張できる構造にする。

## Requirements

- Plugin package discovery は read-only とする。
  - 発見した package を実行しない。
  - Tool / Hook / Service / Ingress を登録しない。
  - package の存在だけで Plugin が有効化されない。
- `plugin.toml` を package root から読み込む。
  - 必須 metadata を parse する。
  - unsupported / incompatible api version は fail closed にする。
- Package safety checks を行う。
  - path traversal を reject する。
  - package root 外への symlink / escape を reject する。
  - bounded file count / total size / manifest size を設ける。
  - deterministic digest を計算する。
  - malformed package は diagnostic に残すが、Pod startup 全体を不要に壊さない。
- Plugin identity を source-qualified に扱う。
  - `user:<id>`
  - `project:<id>`
  - `builtin:<id>`
- Unqualified id が複数 source に一致する場合は ambiguous として fail closed にする。
- Profile / config 側の explicit enablement entry を読む。
  - exact config location / shape は既存 Profile / plugin design に合わせ、必要なら最小の typed structure を追加する。
  - enablement entry は package ref、version / version constraint、digest pin、enabled surfaces、grant request / grant reference を表現できるようにする。
- Enablement resolver は discovered package と enablement entry を照合する。
  - missing package
  - duplicate package id
  - ambiguous ref
  - version mismatch
  - digest mismatch
  - incompatible api version
  - requested surface unsupported
  - grant missing / unsupported
  を diagnostic として区別できるようにする。
- Resolved Plugin は後続の runtime initialization / contribution registration に渡せる typed data として表す。
  - package identity
  - source
  - package path
  - digest
  - manifest metadata
  - enabled surfaces
  - effective grants / unresolved grants
  - diagnostics
- Startup / snapshot restore で同じ resolved Plugin set を再現できること。
  - runtime-only mutable state に依存しない。
  - 必要な場合は Pod metadata / snapshot に resolved digest 等を含めるか、startup 時に deterministic に再解決する。
- Diagnostics は model-visible context に勝手に差し込まない。
  - user-visible / logs / snapshot など既存の診断面に合わせる。
  - secret-like path / auth / file content を出さない。

## Non-goals

- Plugin code execution。
- WASM runtime 実装。
- Tool / Hook / Service / Ingress の実登録。
- `https` host API 実装。
- `fs` host API 実装。
- external network / filesystem side effect の実行。
- plugin package manager / install / update / registry 実装。
- package signature / trust chain 実装。
- MCP bridge との統合。

## Acceptance criteria

- User / workspace plugin store から `.yoi-plugin` package を発見できる。
- Valid package の `plugin.toml` を parse し、typed manifest と deterministic digest を得られる。
- Invalid package は fail closed し、bounded diagnostic として報告される。
- Package が存在するだけでは Tool / Hook / Service / Ingress は登録されない。
- Profile / config の explicit enablement entry なしでは Plugin は有効化されない。
- Enablement entry と discovered package を照合し、resolved Plugin metadata を生成できる。
- `user:<id>` / `project:<id>` / `builtin:<id>` の source-qualified identity を扱える。
- ambiguous unqualified id は fail closed する。
- version mismatch / digest mismatch / incompatible api version / missing package が区別可能な diagnostic になる。
- Startup / restore 時に resolved Plugin set が deterministic に再現できる。
- Tests cover:
  - valid user package discovery
  - valid workspace package discovery
  - package without enablement is not active
  - explicit enablement resolves package
  - duplicate / ambiguous id fail closed
  - digest mismatch fail closed
  - path traversal / root escape rejected
  - unsupported api version rejected
  - malformed manifest diagnostic
  - no contribution registration from discovery-only path
- Validation: focused tests for plugin discovery/resolver, `cargo fmt --check`, relevant `cargo check` / `cargo test`, `git diff --check`, and `nix build .#yoi` if runtime resources / packaging / dependency graph change.

## Implementation notes

- Prefer adding a small typed Plugin package/resolver module rather than embedding resolver logic inside runtime launch code.
- Keep terminology aligned with the design record:
  - Plugin runtime
  - Plugin surface
  - Plugin host API
- Do not reintroduce `contribution category` as user-facing terminology.
- Host APIs remain intentionally narrow. If this Ticket needs to mention APIs, use `https` and `fs`, not `web`.
- Discovery and enablement should integrate with the existing Profile / feature contribution direction without giving plugins ambient workspace filesystem authority.

## Related work

- `00001KSXRQ4G8` — Plugin runtime / surface / minimal host API model design.
- `00001KT0Z4BK8` — Plugin package / discovery format design.
- `00001KTR81P9X` — `pod::feature` API / plugin host substrate follow-up.
