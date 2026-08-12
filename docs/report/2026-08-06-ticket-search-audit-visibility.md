# Ticket検索・close監査に必要な履歴visibilityが不足している

Tracking Ticket: `00001KZRNHB35`

## 発生日

2026-08-06

## 概要

active Ticketから「Implementation reportやapprove、commit evidenceはあるがcloseされていないTicket」を監査した際、model-facing Ticket toolだけでは候補抽出とscope履歴の照合が難しかった。

この不足により、Workerが複数Ticketの履歴を目視で突き合わせ、`00001KZ46DP6K`のImplementation reportを別のWorkdir delete要件と誤対応させた。最終的にはfull thread、current source、commit `e6a2da54`を再確認して訂正したが、Ticket bodyの誤編集と誤った監査commentを発生させた。

誤判定自体はWorkerの確認不足である。一方、現在のtool surfaceはこの種の事故を防ぐための検索・version history・typed evidence projectionを提供していない。

## 観測した障壁

### `TicketList`から候補を抽出できない

今回のmodel-facing結果では、`TicketList`が次のようなcount summaryだけを返す場合があった。

```text
Listed 23 ticket(s) for state active
```

個々のTicket id、title、state、updated_atを取得できないため、既知のIDを使って`TicketShow`を全件呼ぶ必要がある。pagination offsetやevent/evidence filterもない。

必要だった検索条件は以下。

- activeかつImplementation reportあり
- activeかつapproveあり
- activeかつcommit evidenceあり
- `done`だが未close
- Implementation report後にitem body/titleが変更された
- unresolved `request_changes`あり／なし
- commit hashまたは本文文字列を含むTicket

### `TicketShow`のprojectionが安定しない

同じ`TicketShow`でも、state一行だけになる場合と、body/thread/relationsを含むfull JSON projectionになる場合があった。

```text
Ticket 00001KZ46DP6K state planning
```

`body_max_bytes`と`event_limit`を大きくしても常にfull projectionになるとは限らず、Workerは「eventがない」のか「表示が省略された」のかを区別できない。

### Implementation reportがtyped eventではない

Implementation reportとcommit evidenceは通常のMarkdown commentとして保存されている。

```markdown
## Implementation report
...
commit `e6a2da54`
```

そのため、backendは以下を構造化して検索できない。

- implementation reportの存在
- repository id
- base/head commit
- test evidence
- dirty state
- assignment id
- report対象revision

Markdown headingや文言に依存した監査になる。

### item editのversion historyを復元できない

threadの`item_edit` eventは、変更fieldとreplacement countだけを返す。

```text
Ticket item updated: title, body. Body replacement applied to 10 occurrence(s).
```

編集前title/body、編集後snapshot、item revision idを取得できない。Implementation reportがどのitem revisionを対象にしたかも記録されない。

このため、「実装完了後にTicketが別scopeへrescopeされた」のか、「Implementation reportの読み違い」なのかをthreadだけから確実に判定できない。

### lifecycle監査queryがない

`TicketDoctor`はschema/consistency diagnosticsには有効だが、次のsemantic lifecycle driftを検出しない。

- Implementation report + approve + reachable commitがあるのにplanning/inprogress
- state `done`だが未close
- close後にunresolved request_changesが追加された
- Implementation report後のmaterial rescope
- report commitがcurrent repositoryで解決不能
- MR current revisionとreview対象revisionが不一致

### relation削除operationがない

`TicketRelationRecord`と`TicketRelationQuery`はあるが、誤登録やstale relationを削除するtyped operationがない。storage直接編集はauthority bypassになるため実施できない。

## 影響

- active Ticket全件へのN+1 `TicketShow`が必要になる。
- known IDを持たないWorkerは候補自体を列挙できない。
- Markdown commentの目視照合で別Ticketのevidenceを混同しやすい。
- rescope後のcurrent bodyだけを見て過去Implementation reportを誤評価する。
- close漏れ監査が高コストで、Ticket state driftが蓄積する。
- 今回は誤ったTicket body editと監査commentを追加し、後から訂正eventを積むことになった。

## 提案

### read toolを`Query` / `Show`へ整理する

既存のselection-only `TicketList`へ検索責務を積み増すのではなく、read surfaceを次の責務へ整理する。

- `QueryTicket`: Ticket workflowのselection、全文検索、structured filter、attention候補
- `ShowTicket`: 一件のcurrent item、version refs、bounded thread、resolution、artifacts
- `QueryObjective`: Objectiveのselection、全文検索、state/linked-Ticket filter
- `ShowObjective`: 一件のcurrent bodyとlink projection

`QueryTicket`は次のoptional queryを受け取る。

```text
QueryTicket {
  query?: string,
  states?: [...],
  event_kinds?: [...],
  evidence?: implementation_report | commit | approved,
  review_status?: none | approved | request_changes | unresolved_changes,
  attention?: done_not_closed
            | implementation_report_not_closed
            | report_after_rescope
            | unresolved_review
            | missing_commit
            | blocked
            | unblocked,
  related_ticket_id?: TicketId,
  relation_kind?: RelationKind,
  linked_objective_id?: ObjectiveId,
  updated_before?: timestamp,
  updated_after?: timestamp,
  limit,
  cursor?,
}
```

`attention`は自動mutationを行わず、候補と根拠だけを返す。Worker/Userが`ShowTicket`、review、repository authorityを再読してclose判断する。

resultは常にbounded summaryを返す。

- Ticket id/title/state/readiness
- matched field/event kind
- bounded snippet
- event id/sequence
- updated_at
- unresolved blocker/review count
- `next_cursor` / `truncated`

`QueryObjective`も同じ共通query infrastructureを使い、`query`、`states`、`linked_ticket_id`、`updated_*`、cursor paginationを提供する。domain固有filterとresult型は混ぜない。

### 横断検索toolは追加しない

同じqueryを`QueryTicket`と`QueryObjective`へそれぞれ実行すれば十分である。異なるauthorityとresult型を`WorkspaceSearch`へ混ぜるとsurfaceとprojectionが増えるため、横断toolは追加しない。

### typed Implementation report

Markdown bodyに加えて、最低限次をtyped attributesとして保存する。

- `assignment_id`
- `repository_id`
- `base_commit`
- `head_commit`
- `merge_request_id` / `revision_id`
- validation evidence refs
- dirty/untracked state
- source Runtime/Worker identity

reportは作成時のTicket item revisionを参照する。`QueryTicket.evidence`と`attention`はMarkdown headingをparseせず、このtyped authorityをqueryする。

### retrievable item revisions

`ShowTicket`のoptional revision selectorまたはbounded version projectionでitem historyを取得できるようにする。専用tool追加は、同じprojectionではsize/authorityを分離できない場合だけ検討する。

```text
TicketItemRevision {
  revision_id,
  title,
  body_digest,
  body or bounded diff,
  edited_at,
  source,
}
```

Implementation report/review/close resolutionから対象item revisionを参照する。

### stable tool projection

`QueryTicket`、`ShowTicket`、`QueryObjective`、`ShowObjective`は、同じparameterなら常に同じshapeのbounded JSONを返す。省略時は明示的な`truncated`、`returned`、`next_cursor`を返し、「entryなし」と「projection省略」を区別する。

### write commandは副作用単位で明示する

relationのadd/remove、Queue、review、Close、item editなどはauthorization、precondition、idempotency、notification、compensation、audit/result型が異なるため、generic `MutateTicket`や`MutateObjective`へ統合しない。明示commandを維持し、model-facing tool数はprofile・role・Flow別catalog projectionで抑える。

Implementation reportはtyped Ticket thread eventとして扱い、item revision参照とbounded query結果から監査可能にする。

## 推奨順序

1. `QueryTicket`を追加し、全文検索・state・updated time・thread kind・resolution・artifact・relation filterとcursorを入れる
2. `ShowTicket`へitem/threadの安定したversion referenceを加える
3. `QueryObjective`を同じquery/cursor infrastructureで追加する
4. `ShowObjective`へlink projectionを加える
5. typed Implementation reportとprofile/role/Flow別catalog projectionを追加する
6. WebUIに検索・filter・pagination・implementation-report表示を追加する

## 期待する監査手順

1. `QueryTicket(states=active, attention=implementation_report_not_closed)`で候補抽出。
2. candidateごとに`ShowTicket`でcurrent item revision、report対象revision、latest reviewを取得。
3. typed commit/MR evidenceをrepository authorityで検証。
4. unresolved `request_changes`とdependencyを確認。
5. User/Orchestratorが明示的にcloseする。

この順序なら、全TicketのMarkdown threadを目視で横断せず、scopeの異なるevidenceを誤対応させずにclose漏れを監査できる。横断検索toolやgeneric mutation toolを増やさず、roleごとのcatalog projectionでsurfaceを限定できる。
