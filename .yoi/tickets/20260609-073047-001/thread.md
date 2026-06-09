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
