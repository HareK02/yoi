<!-- event: create author: "yoi ticket" at: 2026-07-16T04:34:01Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-16T04:35:41Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-16T04:35:41Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: decision author: hare at: 2026-07-16T04:38:21Z -->

## Decision

ユーザー指示により、設計で止める Ticket ではなく実装 Ticket として再スコープした。Overview-first extract / evidence tools は実装要件に変更し、staging 審査・剪定も resolution/disposition の実装まで含める。


---

<!-- event: decision author: hare at: 2026-07-16T14:06:41Z -->

## Decision

実装者が対象領域と意図を追いやすいように、実装意図、対象 module、実装方針、想定順序を item.md に追記した。特に `extract/input.rs`、`extract/tool.rs`、`extract/payload.rs`、`extract/staging.rs`、`worker.rs`、`memory_extract_system.md` を主な対象として明示し、初期 evidence tools は extract slice 内の bounded read-only search/read/anchor 解決に閉じる方針を示した。


---
