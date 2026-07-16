<!-- event: create author: "yoi ticket" at: 2026-07-16T17:17:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-16T17:19:41Z -->

## Decision

Overview-first extract implementation was split so this Ticket owns the staging payload / source anchor format first. The extract worker implementation will depend on this format through Ticket 00001KXMK7YMC.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-16T19:26:52Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-16T19:27:54Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Overview-first extract の前提となる staging payload / source anchor / evidence reference schema に限定されており、実装 scope と非目標が明確。
- `TicketRelationQuery(00001KXNYXNM6)` は incoming `depends_on` from `00001KXMK7YMC` と outgoing `related 00001KXMK846H` のみで、この Ticket 自身の blocker はない。
- `TicketOrchestrationPlanQuery(00001KXNYXNM6)` は事前 record なし。今回 accepted plan を記録した。
- `TicketList(inprogress)` は 0 件。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` は clean で、既存 implementation worktree/branch は見当たらない。
- Bounded code map で `crates/memory/src/extract/payload.rs`, `extract/staging.rs`, `schema/common.rs`, `consolidate/input.rs` 周辺に対象実装があることを確認した。

Evidence checked:
- Ticket body / thread / relations / artifacts。
- `TicketRelationQuery`, `TicketOrchestrationPlanQuery`, `TicketList(inprogress)`。
- Orchestrator worktree / branch / worktree state。
- `crates/memory/src` の staging / extract / SourceRef / consolidation input references。

IntentPacket:

Intent:
- Extract staging payload に entry-level source anchors / evidence references を追加し、各 decision/discussion/attempt/request entry がどの host-resolved evidence に基づくかを機械的に辿れるようにする。
- Overview-first extract worker / staging consolidation の前提 schema を固める。

Binding decisions / invariants:
- `StagingRecord.source` は維持し、record-level source は extract 対象 range 全体を示す。
- entry-level source refs は個々の claim/evidence を示す optional field とする。
- 旧形式 staging JSON は読めるままにする。新 field は default empty / skip serializing empty を基本とする。
- source/evidence refs は free-form rationale text ではなく、host が解決できる bounded anchor metadata にする。
- raw tool result content 全文を staging payload に埋め込まない。
- この Ticket では session-explore feature / evidence tools / extract worker 起動経路 / staging -> Memory resolution/disposition は実装しない。

Requirements / acceptance criteria:
- session id / segment id / entry range / evidence id / evidence kind / optional summary-label を表せる source anchor / evidence reference 型を追加する。
- evidence kind は message / tool_call / tool_result / file_ref / ticket_ref / objective_ref などを表せ、将来拡張可能な形にする。
- `DecisionEntry` / `DiscussionEntry` / `AttemptEntry` / `RequestEntry` に optional entry-level source refs を持たせる。
- new-format staging JSON が serialization / deserialization できる。
- old-format staging JSON が deserialization できる。
- consolidation input で entry-level source refs が human-readable に確認できるか、少なくとも lossless に通る。
- relevant `crates/memory` tests を追加/更新する。

Implementation latitude:
- 型名・module placement は既存 `SourceRef` / extract payload / staging style に合わせてよい。
- entry range の exact representation は serde 互換・bounded metadata である限り coder が選んでよい。
- consolidation input rendering は初期実装として readable summary でよい。
- 既存 `SourceRef` を拡張するか、新しい entry-level anchor type を追加するかは互換性と clarity を見て選んでよい。

Escalate if:
- 既存 `SourceRef` の breaking schema change が必要になる場合。
- session-explore tool/evidence resolver 実装なしでは anchor schema を決められないことが判明した場合。
- raw evidence content を staging に入れないと acceptance を満たせない場合。
- staging-to-Memory resolution/disposition まで同時実装しないと schema が成立しない場合。

Validation:
- `git diff --check`
- `cargo test -p memory`
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`
- 変更範囲が memory crate 以外へ広がる場合は affected crate tests も実行。

Current code map / likely touch points:
- `crates/memory/src/extract/payload.rs`
- `crates/memory/src/extract/staging.rs`
- `crates/memory/src/schema/common.rs`
- `crates/memory/src/consolidate/input.rs`
- `crates/memory/src/consolidate/staging.rs`
- relevant `crates/memory` tests。

Critical risks / reviewer focus:
- old staging JSON compatibility break。
- entry-level source refs silently serializing noisy empty data。
- source refs becoming free-form LLM-authored evidence text instead of bounded host-resolved anchors。
- raw tool result content leaking into staging payload。
- conflating record-level source range with per-entry evidence anchors。
- accidentally implementing session-explore worker or staging resolution out of scope。

Next action:
- Record `queued -> inprogress`, create dedicated implementation worktree, and route to sibling Coder Pod。

---

<!-- event: state_changed author: orchestrator at: 2026-07-16T19:28:00Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Dashboard queue authorization was inspected, no blockers were found, and an accepted plan was recorded. Moving queued Ticket to inprogress before creating worktree or spawning role Pods.

---
