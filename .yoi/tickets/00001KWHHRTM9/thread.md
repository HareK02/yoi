<!-- event: create author: "yoi ticket" at: 2026-07-02T13:54:52Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T13:56:05Z -->

## Plan

Workspace Backend Runtime connection management と永続 config の Ticket として詳細化した。

方針:
- Runtime が Backend に接続するのではなく、Backend が Runtime source を RuntimeRegistry に登録する現行構造を前提にする。
- embedded Runtime は built-in connection として表示し、config 管理対象や削除対象にしない。
- remote Runtime connection は `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` に保存する。
- 管理 API は list/add/delete/test を提供する。
- v0 は config persistence を優先し、config 更新後は `restart_required = true` とする。live register/unregister は対象外。
- raw token 値、socket/session/store path、Runtime event cursor、live handle は UI/API/config に出さない。


---

<!-- event: decision author: hare at: 2026-07-02T14:00:49Z -->

## Decision

Runtime connection management Ticket は Settings shell 先行 Ticket に依存する形へ整理した。

Decision:
- `00001KWHJ0XH6` が Workspace-local Settings の entry point / route / shell / section navigation を用意する。
- `00001KWHHRTM9` はその Settings shell 上に Runtime Connections section と Backend API / config persistence を追加する。
- Runtime connection 管理 Ticket 内で Settings shell 設計を同時に進めない。


---

<!-- event: decision author: hare at: 2026-07-02T14:55:21Z -->

## Decision

Runtime connection management に negotiation / compatibility check を含める方針へ更新した。

Decision:
- 現状は明示的な negotiation / handshake は無く、`GET /v1/runtime` が事実上の疎通確認になっているだけ。
- Runtime connection `test` は単なる ping ではなく lightweight negotiation / compatibility check として扱う。
- v0 check は `GET /v1/runtime` の parse、observed runtime id/display/status/capabilities、worker list/detail、event websocket construction、spawn/input/config-bundle support などを確認する。
- 現行 API に protocol version が無い場合は fake version を作らず、`v1 runtime endpoint responded` のような compatibility basis として表現する。
- negotiation result / observed capabilities / health result / checked_at / diagnostics は persisted config に保存しない。必要になった場合は Backend DB の observation/cache として扱う。
- persisted config、RuntimeRegistry live state、negotiation result を混同しない。


---

<!-- event: intake_summary author: hare at: 2026-07-02T16:25:43Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T16:25:43Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-02T16:45:19Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
