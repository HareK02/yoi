<!-- event: create author: LocalTicketBackend at: 2026-06-08T10:38:42Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-08T10:57:37Z -->

## Plan

## Intake refinement

readiness は `implementation_ready`。既存 Ticket の目的・受け入れ条件・除外範囲は十分に具体的で、Orchestrator が実装 routing できる。

### Binding decisions / invariants

- `legacy_ticket` と `needs_preflight` は current Ticket schema / tool input / tool output / CLI output / frontmatter writer / doctor requirements から削除する。通常の新規 Ticket 作成経路で再び出力してはいけない。
- `workflow_state: intake` は互換 alias として残さず、parser / doctor / tests で invalid として扱う。
- `workflow_state` 未指定時の bounded migration fallback を残す場合でも、`intake` 語彙を復活させてはいけない。
- `.yoi/tickets/**/item.md` の current frontmatter は stricter schema に合わせて移行する。`thread.md` / `resolution.md` の過去本文に残る語は audit history として扱い、通常は書き換えない。
- `legacy_ticket` の代替として generic external issue link field を追加しない。関係メタデータは別 Ticket `typed-ticket-relation-metadata` の範囲とする。

### Implementation latitude

- 実装者は parser / writer / metadata structs / tool schemas / panel view model / tests / prompts / docs のどこから整理するかを選んでよい。
- 移行専用の一時的な読み取り経路が必要な場合は、current API surface に露出しない bounded migration path に限定してよい。
- fixture や local Ticket record の更新方法は手動・補助スクリプトのどちらでもよいが、差分は schema 変更に必要な範囲へ限定する。

### Escalation conditions

- 非 null の `legacy_ticket` 値を削除することで保持すべき current relation 情報が失われると判断した場合は、代替フィールドを silently 追加せず Orchestrator / human に戻す。
- `needs_preflight` または `workflow_state: intake` をまだ実行時互換として保持すべき外部利用者が見つかった場合は、後方互換要件として戻す。
- 変更が Ticket schema 以外の authority boundary / role workflow semantics に広がる場合は、別 Ticket 化または routing 判断を求める。

### Validation focus

- `yoi ticket doctor` が stricter schema の local records で通ること。
- `TicketCreate` / `TicketShow` / `TicketList` / panel 表示に `legacy_ticket` と `needs_preflight` が current field として出ないこと。
- `workflow_state: intake` を invalid とする focused test を追加または更新すること。
- 既存指示どおり、実装完了時は focused tests、`cargo fmt --check`、`git diff --check`、必要に応じて `nix build .#yoi` で確認すること。

Open questions: なし。
Risk flags: `ticket-schema`, `migration`, `tool-api`, `panel-output`, `docs-prompts`.

---

<!-- event: intake_summary author: intake at: 2026-06-08T10:57:43Z -->

## Intake summary

既存 Ticket を精査し、obsolete な `legacy_ticket` / `needs_preflight` と `workflow_state: intake` 互換の削除範囲を実装可能な契約として整理した。binding decisions は current schema/tool/API/panel/docs/prompts から legacy fields を削除し、`intake` alias を拒否すること。historical thread/resolution prose は audit history として通常は保持する。Open questions はなく、risk flags は ticket-schema / migration / tool-api / panel-output / docs-prompts。

---

<!-- event: state_changed author: intake at: 2026-06-08T10:57:43Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement が完了し、Orchestrator が routing できる状態になったため `ready` に変更する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T11:21:33Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
