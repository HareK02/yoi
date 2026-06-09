<!-- event: create author: LocalTicketBackend at: 2026-06-09T07:30:47Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: intake at: 2026-06-09T10:26:34Z -->

## Decision

Intake refinement / readiness decision。

この Ticket は `implementation_ready` として扱える。目的、対象範囲、受け入れ条件、collision handling、migration 対象、非目標がすでに具体化されており、Orchestrator が実装 routing を判断できる。

Binding decisions / invariants:

- Ticket と Objective の canonical ID は共通の fixed-width base32 encoded Unix epoch milliseconds text に統一する。
- ID は title / slug / content words を含めない opaque path component とする。
- fixed width により lexicographic sort と chronological sort が一致することを維持する。
- 同一 millisecond collision は suffix / random tail ではなく `+1ms` retry で解決する。
- `created_at` / `updated_at` は frontmatter の人間可読 timestamp field として維持する。
- 既存 Ticket / Objective record の migration、lookup、doctor、関連 metadata / linked-ticket 参照の整合性を対象に含める。

Implementation latitude:

- exact width と alphabet は要件を満たす範囲で実装時に確定してよい。現行 epoch milliseconds を十分な期間表現でき、紛らわしい文字を避け、path-safe で、固定長 ordering property を満たすこと。Crockford base32 系と 9 chars は推奨例として扱える。
- 共通 helper の crate / module 配置は、Ticket create path と Objective create path から共有でき、将来の project records に自然に拡張できる範囲で実装判断してよい。
- migration は既存 durable project records を壊さず、canonical ID/path と内部参照を一貫して更新できる方式を選ぶ。

Risk flags / reviewer focus:

- `storage-migration`
- `canonical-id-schema`
- `ticket-objective-cross-reference`
- `backward-compatibility-boundary`
- `cli-doctor-validation`

Escalation conditions:

- fixed-width base32 epoch milliseconds では要件の ordering / collision / path-safety を同時に満たせないと判明した場合。
- 既存 Ticket / Objective migration が reference integrity を安全に保てない場合。
- exact width を 9 chars から大きく外す、Crockford 系ではない alphabet を採る、または random / suffix fallback を追加したくなった場合。
- ID timestamp と `created_at` の関係を推奨と異なる semantics にしたい場合。

Validation expectation:

- Ticket body の既存 validation に加え、コード・パッケージングに関わるため完了前に `nix build .#yoi` も通す。

---

<!-- event: intake_summary author: intake at: 2026-06-09T10:26:42Z -->

## Intake summary

Ticket と Objective の canonical ID を共通の fixed-width base32 epoch-milliseconds 形式へ統一する concrete work item として整理済み。Open questions はなく、binding decisions は opaque/path-safe ID、title/slug/content words 非包含、lexicographic ordering、`+1ms` collision retry、`created_at` / `updated_at` 維持、既存 Ticket / Objective migration と参照整合性。Risk flags は storage-migration / canonical-id-schema / ticket-objective-cross-reference / backward-compatibility-boundary / cli-doctor-validation。関連する先行 Ticket `20260608-110940-001` は closed の非ブロッキング前提、`20260609-085212-001` は非ブロッキング関連。

---

<!-- event: state_changed author: intake at: 2026-06-09T10:26:42Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、目的・範囲・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions・validation focus が揃ったため、Orchestrator が routing できる ready に移行する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:31:17Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T10:35:08Z -->

## Decision

Routing decision: waiting_for_active_record_churn_to_settle

This Ticket remains queued for now, despite the updated parallelism preference.

Reason:
- This Ticket is a broad Ticket/Objective ID migration touching current project-record paths and internal references.
- There are multiple active worktrees currently writing Ticket records and code.
- Running a storage/ID migration in parallel with active Ticket record churn would create high merge/migration risk, not a small conflict.

State decision:
- Leave this Ticket `queued`.
- Re-read when active worktrees are merged/closed or the workspace is otherwise quiescent enough for a record migration.

---

<!-- event: decision author: orchestrator at: 2026-06-09T11:30:25Z -->

## Decision

Routing decision update: still_waiting_record_migration_window

This Ticket remains queued for now despite the increased parallelism preference.

Reason:
- This is a broad Ticket/Objective ID migration that will touch project record paths and many references.
- I am starting two smaller independent workstreams now (`20260609-082209-001` and `20260609-095518-001`).
- Running another record identity migration in parallel with active Ticket schema/profile work would create high migration/merge risk, not a small mechanical conflict.

State decision:
- Leave this Ticket queued.
- Re-read and route after current active schema/profile work lands or if the workspace is otherwise quiescent enough for a record migration.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T12:46:44Z from: queued to: inprogress reason: implementation_acceptance field: state -->

## State changed

Accepting queued implementation now that the parallel Profile feature-flag and Ticket schema cleanup branches are landed, validated, closed, and cleaned up. This Ticket is broad record/path migration work, so it is intentionally started only after workspace quiescence.

---

<!-- event: decision author: orchestrator at: 2026-06-09T12:46:44Z -->

## Decision

Routing decision:

Start implementation now. The earlier high-conflict schema/profile work is complete, leaving this broad Ticket/Object ID and path migration as the only queued work.

Implementation boundaries:
- Use a dedicated worktree/branch.
- Keep migration coherent across Ticket and Objective records, lookup paths, relation metadata, CLI/tool outputs, doctor, and tests.
- Preserve `created_at` / `updated_at` as human-readable frontmatter.
- Keep IDs title/slug-free, fixed-width, lexicographically chronological, path-safe, and collision-handled by `+1ms` retry rather than suffixes.
- Do not introduce old-name compatibility aliases unless needed for a bounded migration/lookup bridge and test-covered.
- Treat current `.yoi/tickets` and `.yoi/objectives` records as project data to migrate in the branch.

Review focus should include migration correctness, relation/reference updates, doctor behavior, and whether base32 timestamp IDs remain stable/sortable under collision handling.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T12:47:58Z -->

## Implementation report

Implementation routing started.

Worktree/branch:
- Worktree: `.worktree/base32-record-ids`
- Branch: `base32-record-ids`
- Base/routing commit: `0803bc3 ticket: accept base32 id migration`

Spawned sibling implementation Pod:
- `coder-base32-record-ids`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Runtime note:
- The current orchestrator session is still running the pre-Profile-feature-flags binary, so `project:coder` cannot decode the newly merged `feature` profile field until the process is refreshed.
- The Pod was therefore launched with `builtin:default` as a runtime workaround, with explicit Coder-role instructions in the handoff. This is a session-runtime workaround only, not a project configuration change.

---
