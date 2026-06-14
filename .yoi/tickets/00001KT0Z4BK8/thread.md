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

<!-- event: intake_summary author: hare at: 2026-06-14T14:53:35Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-14T14:53:35Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T14:54:52Z from: ready to: planning reason: panel_return_to_planning field: state -->

## State changed

Panel returned ready Ticket 00001KT0Z4BK8 to planning for requirements sync. This is not Queue routing and must not start implementation.

## User refinement instruction

前何回かに渡って実装されたlatencyとかE2Eとかの話なんだけど


---

<!-- event: comment author: intake at: 2026-06-14T14:56:35Z -->

## Comment

Intake 確認:
- Ticket `00001KT0Z4BK8` の body / thread / artifacts を確認した。この Ticket の現在の主題は Plugin distribution package format / discovery / enablement boundary であり、直近の user refinement「前何回かに渡って実装されたlatencyとかE2Eとかの話なんだけど」は、この Ticket の plugin package 要件とは直接対応しない。
- 関連しそうな既存 Ticket として `00001KV0723PC`（Panel Quit latency）、`00001KSKBP9YG`（E2E harness）、`00001KV0TJVN5`（E2E binary/env isolation）、`00001KV0YK5S0`（E2E tmp/runtime isolation）、`00001KV10SN02`（E2E critical-path coverage）を確認した。いずれも closed で、latency/E2E の実装・検証・残差整理はそちらの系譜に記録されている。
- 現時点では、Plugin package Ticket の readiness / requirements を変更する根拠は不足している。ユーザーに、(a) `00001KT0Z4BK8` に latency/E2E 由来の検証要件を追加したいのか、(b) latency/E2E 系の既存 Ticket に追記したいのか、(c) 新しい concrete follow-up Ticket が必要なのか、を確認する。

状態:
- このコメントは要件同期の保留記録であり、queue / implementation routing ではない。
- duplicate Ticket は作成していない。

---
