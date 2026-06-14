<!-- event: create author: tickets.sh at: 2026-06-01T06:49:53Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-01T06:50:33Z -->

## Decision

Distribution direction from user discussion:

- Initial plugin packages should be single-file archives placed in user or workspace plugin stores, such as `~/.config/insomnia/plugins/` and `./.insomnia/plugins/`.
- Package presence is discovery only. Tool/Hook registration, WASM initialization, or MCP process startup requires explicit manifest/profile enablement and capability grant resolution.
- The package should contain a root `plugin.toml` and optional runtime assets such as `module.wasm`, schemas, README, and license files.
- User and workspace stores map to source/trust categories. `user:<id>` and `project:<id>` are distinct; ambiguous selectors should fail closed.
- Workspace packages are repository-provided and must be treated conservatively, especially for executable runtimes and MCP/process-spawn behavior.


---

<!-- event: decision author: hare at: 2026-06-13T15:29:21Z -->

## Decision

決定:
- `pod::feature` は API / contribution substrate として扱い、Plugin や MCP の権限管理を担わせない。
- Plugin は `pod::feature` をユーザー向け package/config/runtime 形式で使わせる層であり、Plugin permission / trust policy は Plugin layer で定義する。
- MCP は `pod::feature` 上に protocol-backed integration layer を構築するが、MCP server enablement / command-env-secret policy / trust boundary / MCP-specific permission は MCP layer が独自に持つ。
- MCP local stdio server の OS-level side effects は Yoi feature authority では制御できないため、feature-layer authority / grant を MCP や Plugin の permission model に流用しない。

反映:
- `00001KTR81P9X` は authority ではなく provider lifecycle / dynamic contribution / normal ToolRegistry path / untrusted normalization に絞る。
- `00001KTR82RB7` は MCP 固有の explicit config と trust model を持つ。
- `00001KSXRQ4G8` と `00001KT0Z4BK8` は Plugin permission を Plugin layer として扱い、MCP を初期 Plugin packaging/runtime から分離する。


---

<!-- event: decision author: hare at: 2026-06-14T14:34:33Z -->

## Decision

決定:
- Plugin discovery stores, source-qualified identity, discovery/enablement separation, Pod-startup initialization timing, restore behavior, and MCP boundaryをこの `00001KT0Z4BK8` の安定 contract として統合した。
- User Plugin package store は `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/` とし、`~/.config/yoi/plugins/` は使わない。
- Package presence は discovery only であり、実行・登録・runtime initialization は explicit enablement 後の Pod startup に限定する。
- Restore は metadata snapshot の enabled Plugin plan を正本とし、fresh discovery で silent upgrade しない。


---
