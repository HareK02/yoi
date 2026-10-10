# Ticket案件状態・終了判断

Ticketの現行案件状態は `open` / `closed` のみ。`open` は引き続き扱う案件であり、着手可能・queue済みではない。`closed` は現時点で案件としての対応を終えた判断であり、要求実現済みとは限らない。Orchestratorの追跡、実作業拘束、Workerの稼働、MR証跡とは別の正本である。

## 判断の公開境界

通常Tool `TicketCaseUpdate` とWIPの `tickets/T-*/case` は、API `POST /workspaces/{workspace_id}/tickets/{ticket_id}/case` と同じ判断契約を使用する。旧complete/state/close専用判断APIは現行契約ではない。

- `Close { reason }`: openをclosedにする。
- `Reopen`: closedをopenにし、現在の終了判断を外す。
- `CorrectClosure { reason }`: closedの現在判断を訂正する。過去の判断は変更しない。
- 理由は `completed` / `not_planned` / `duplicate` / `unknown` / `{ other: string }`。`completed` だけが要求実現済みを表す。すべての操作に説明を必要とし、任意のtyped成果参照を保持する。

`ShowTicket`/detailは現在の終了判断、最後の終了判断のoperation key、close/reopen/訂正のimmutable履歴とlegacy handoffを返す。query/listにも現在の判断と観測用key、handoffを公開する。再開後も過去の理由、説明、成果参照、判断主体、時刻を失わない。判断主体は認証済みBackend actorから設定し、公開API/LLM入力からの偽装を許可しない。案件のcloseにRepository/MR/第三者承認や固定段階の通過を一律要求しない。

## 更新競合と再送

更新前に次の三つを同じ判断transaction内で照合する。

1. `expected_content_digest`: 実際のeditable要件（identity/title/body/順序付きtargets）のSHA-256。threadや案件判断履歴はこのdigestを動かさない。
2. `expected_state`: 観測した案件状態。
3. `expected_closure_operation_key`: 観測した最後の終了判断のkey。未closeなら明示的なnullを送る。省略はunknownとして拒否する。

最後の終了判断のkeyはreopen後も残る。これにより、本文とopen状態が同じでもopen→closed→openを見落とさず、closedの訂正同士の競合も検知する。時刻や到着順だけではこの一致を証明できない。

operation keyのexact replayは、全入力と認証済みsource identityのfingerprintに一致する場合だけ受理する。古い操作を再適用せず、現在の案件を返す。異入力の同key再利用は拒否する。判断履歴とreceipt、案件状態、Workspaceの作業releaseは同じtransactionで扱い、hook失敗時も途中の判断を残さない。

## 担当・作業・資産との分離

closeは案件の実作業拘束を終える一方、T-715の終端担当責任を表示用に保持する。reopenから古い実行identity、claim、grantを復活させない。新しい実作業には明示的なrouting/claimが必要であり、open案件を自動queueと解釈しない。close/reopen/訂正そのものはWorker起動・停止・復元・削除、担当変更、資産grant、MR承認・統合を行わない。停止・削除のhold/pin/attachment等の独立guardは維持する。

## 旧保存データの移行

T-739/T-740合成sourceのTicket componentはschema10（タグと案件判断）とする。旧schema7（六状態）・8（六状態とタグ）を明示的な移行入力として扱い、番号だけで旧保存形式を推測しない。Workspaceはdevelopのauth/runtime connection移行89→90をそのまま保持し、案件移行90→91を追加する。旧89からはauth→案件の順に進み、現90からは案件移行だけを行う。component6からの既存移行も維持する。案件のみのcomponent9は、表・列・FKとimmutable triggerの保存形式を検証したうえでタグだけを追加し、既存の終了判断・receipt・migration actor/key・handoffを再生成しない。偽のversion markerや欠けたschema objectは拒否する。

| 旧workflow state | 現行案件状態 | 終了理由 |
| --- | --- | --- |
| planning / ready / queued / inprogress | open | なし |
| done / closed | closed | unknown |

旧doneを成功の証拠に昇格しない。移行による終了判断はmigration actorを明示し、旧resolutionの説明を保持する。旧thread、completion/close receipt、担当履歴、MR証跡を改ざんしない。旧phase、explicit flag、status、queued_by/queued_at、resolutionをimmutable `typed_ticket_legacy_workflow_snapshot` に保存し、`legacy_handoff` として後続追跡/routing移行へ渡す。これは現行案件状態の第二正本ではない。移行だけで過去のWorkerや終了済み作業を起動しない。

T-740のタグschemaを案件schemaへ合成する際、タグのID・名前・tombstone・membership、operation receiptの入力・認証済みsource identity・応答、audit/threadの観測時snapshotを保持する。タグ変化からclose/reopen/訂正やeditable content digestの更新を起こさない。旧7およびタグあり8からの移行、再実行、タグ/監査/終了履歴、FKとauth archiveの保持を合成fixtureで検証する。番号を分けただけでは合成完了と扱わない。

## 検証境界

- `crates/ticket/tests/case.rs`: Repository/MRなしの理由、再開/訂正、actor/ref/history、CAS/ABA、exact replay/異入力拒否、並行close、Workspace境界、transaction rollback。
- `crates/ticket/tests/migration.rs`: 六状態、旧thread/receipt/担当/MR参照の保持、理由不明、再open、タグあり8との合成と再実行。
- Workspace Store: 六状態×MRあり/なし、終端担当、旧queue/handoff、migration resumption、Workerを起動しない移行。
- Workspace API/authority/lifecycle: 偽装actor・無権限・他Workspace・旧route拒否、作業releaseと表示責任、reopen時の新routing identity、独立削除guardとWorkspace purge rollback。
- Worker通常Tool/WIP、Client、CLI、Web parser: 同じcase入力、モデルに見えるnullable観測keyと履歴、Backend以外のfallback禁止、stale失敗の伝達。

実経路統合・Orchestrator追跡の置換は後続単位の責務であり、以上の合成source検証を実環境のmigration・更新や全経路統合検証と混同しない。
