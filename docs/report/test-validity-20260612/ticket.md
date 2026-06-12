# テスト妥当性レビュー: ticket
- 評価: 概ね良い

## 確認範囲

- 対象: `crates/ticket` / package `ticket`
- 主要責務:
  - typed Ticket domain API と `.yoi/tickets/<ticket-id>/` local file backend
  - Ticket lifecycle state, thread event, relation, orchestration-plan artifact の読み書き
  - `.yoi/ticket.config.toml` の role/backend 設定解決
  - LLM-facing Ticket tools の schema / bounded output / backend adapter
- 既存テスト:
  - `src/lib.rs` unit tests
  - `src/config.rs` unit tests
  - `src/tool.rs` async unit tests
- 変更: なし。読解とテスト実行のみ。

## 現在のテストがよくカバーしている点

- `cargo test -p ticket` は成功しており、66 tests が通過している。
- backend の主要 happy path はかなり広く押さえられている:
  - create / show / list 相当の local layout
  - opaque canonical id
  - `item.md`, `thread.md`, `artifacts/.gitkeep`, `resolution.md`
  - obsolete frontmatter field を出さないこと
  - Japanese record language の generated defaults
  - numeric-looking YAML scalar の quoted round-trip
- lifecycle invariant のカバーは良い:
  - `planning -> ready -> queued -> inprogress -> done`
  - `ready/queued -> planning`
  - disallowed transition rejection
  - stale transition rejection
  - generic `set_state_field` で `state` / `workflow_state` / `status` を変更できないこと
  - `queue_ready` が non-ready Ticket を mutate しないこと
- thread event / doctor の基本検証は妥当:
  - review status
  - state_changed / intake_summary attributes
  - invalid thread event attributes が append されないこと
  - legacy bucket / obsolete fields / invalid state / malformed typed event の doctor diagnostics
- relation / orchestration-plan の重要点も押さえられている:
  - forward relation storage
  - inverse derived view
  - dependency / incoming blocker が queue を止めること
  - relation cycles / dangling target / self-target artifact を doctor が拾うこと
  - orchestration plan JSONL append / query / invalid artifact detection
- config tests は fixed role config の方針に合っている:
  - missing config defaults
  - scaffold の全 role coverage
  - backend-only config が role launch ready ではないこと
  - unknown role / stale investigator role / unknown field rejection
  - backend provider と legacy kind の扱い
  - relative root resolution
  - path-like profile selector rejection
- tool tests は LLM-facing surface として重要な bounded output を一部確認できている:
  - tool name partitions
  - generated schemas
  - create/list/show/doctor flow
  - list limit cap / title and hint truncation
  - list が body/thread/artifact/resolution content を漏らさないこと
  - workflow and relation tools の basic flow

## 不足 / 疑問のあるテスト

- `TicketCreate` tool の optional `state` 入力に対する output 整合性が未検証。実装上、backend には `params.state` を渡している一方、tool output は常に `"state": "planning"` を返しているため、`state: ready|queued|done|closed` で作成した場合の不一致をテストが捕まえられない。
- `TicketShow` の bounded output が十分に直接検証されていない。`event_limit`, `artifact_limit`, `body_max_bytes` の truncation と UTF-8 boundary safety は tool の安全性に直結するが、現状は list 側の bounded behavior の方が厚い。
- compound mutation の atomicity が一部未検証。特に `mark_intake_ready` は intake summary append と state transition が組み合わさるため、後段の state change validation 失敗時に partial event が残らないかをテストした方がよい。
- Markdown thread parser の body delimiter edge case が弱い。thread entry は `---` 行を区切りとしているため、event body に Markdown horizontal rule として `---` が含まれる場合の仕様がテストで固定されていない。
- relation API の negative cases は doctor artifact 手書き検査に寄っており、public backend API 経由の拒否が薄い:
  - duplicate relation
  - self-target relation
  - missing target
  - empty / multiline / oversized target, note, author
  - dependency target が `done` / `closed` になった後は blocker ではなくなること
- `queued -> inprogress` 側の blocker enforcement は実装されているが、直接テストは薄い。`queue_ready` の blocker rejection はあるため大枠は安心だが、acceptance step としての `set_workflow_state(queued, inprogress)` でも同じ relation blocker が効くことを明示するとよい。
- config parser は代表的な rejection を持つが、top-level unknown field、unsupported profile source、`builtin:` のような empty registry name、absolute backend root の扱いは未固定。
- tool input validation の negative tests が少ない。invalid JSON、invalid enum、empty required text、same-state transition、invalid author/reason の error shape と no-mutation を追加すると tool boundary の信頼性が上がる。
- tests は private helpers / exact diagnostic substring にかなり依存している。これは file-backend crate では許容範囲だが、diagnostic wording 変更で壊れる brittle tests が一部ある。仕様として固定したい文言だけ exact assertion にするのが望ましい。

## 追加候補

- `TicketCreate` tool:
  - `{"title": "...", "state": "ready"}` で作成し、tool output / `TicketShow` / `TicketList state=ready` が同じ state を返すこと。
- `TicketShow`:
  - large body / many events / many artifacts を作り、`body_max_bytes`, `event_limit`, `artifact_limit` が count / returned / truncated を正しく返すこと。
  - multibyte UTF-8 の truncation が invalid UTF-8 や panic にならないこと。
- Lifecycle atomicity:
  - invalid `TicketStateChange` を渡した `mark_intake_ready` が intake_summary だけを残さないこと。
  - `TicketWorkflowState` tool の same-state transition rejection と no-mutation。
- Thread parser:
  - event body に quote, backslash, whitespace を含む attributes/body の round-trip。
  - body 中の `---` 行を許すのか禁止するのかを仕様化し、期待動作をテスト。
- Relation:
  - duplicate/self/missing-target を `add_ticket_relation` 経由で拒否するテスト。
  - dependency target を `done` または `closed` にした後、queue / queued->inprogress が通ること。
  - `supersedes` / `duplicate_of` notices の outgoing/incoming view。
- Orchestration plan:
  - `accepted_plan` missing / accepted_plan supplied to non-accepted kind / `waiting_capacity_note` without note / oversized or multiline single-line fields。
- Config:
  - unknown top-level field rejection。
  - unsupported profile source and empty source-qualified profile name。
  - absolute backend root が意図通り保持されること。
- Tool boundary:
  - malformed JSON and invalid enum values return `ToolError::InvalidArgument`。
  - mutating tools の invalid input が backend files を変更しないこと。

## 実行したコマンド

```sh
cargo test -p ticket
```

結果:

```text
66 passed; 0 failed; 0 ignored
doc-tests: 0 passed

