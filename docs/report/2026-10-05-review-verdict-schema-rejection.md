# Reviewer verdict のvalidation rejectionとone-call制約

## 確認した事実

T-703のMR `01a10b3b-87ec-7093-b0c4-a00a78095695`、source
`3174142bd1314fe34c5de7b6a66654ff1c2a8af2` の独立ReviewerはRestoreとno-op
attention checkの競合を見つけ、`ReviewMergeRequest`でrequest_changesを提出しようとした。
severityに`medium`を指定したためHTTP 422となった。Backendの許容値は
`blocker` / `major` / `minor` / `note` だった。

親の`ShowMergeRequest`再読でthreadはReviewRequested seq2までであり、verdictが
永続化されていないことを確認した。Reviewerは「ReviewMergeRequestを正確に一度だけ
呼ぶ」という指示に従い、validation rejection後も再提出を行わなかった。親のfollow-upでも
同じ境界を報告したため、再提出を強制せずReviewerを停止した。

## 障壁

正しいBackend拒否を迂回すべきではない一方、mutationが行われていない入力validation失敗が
一回限りの提出枠を消費する扱いだと、レビュー済みの指摘がMR authorityに残らずpendingになる。
セッションでの報告をverdictや承認の代わりにはできない。

## 改善案（このTicketの実装対象ではない）

- Reviewer-facing tool schemaにseverityのBackend enumを明示し、不正値を入力時に防ぐ。
- one-call指示について「一度のdurable verdict」と「validationで拒否された呼出」を区別できるか
  設計上判断する。無制限再提出や記録済みverdictの変更を認める提案ではない。
- mutationなし／retry可否をtyped errorとして返せると、親とReviewerの双方が推測なしに扱える。

T-703では元のMR/source selectorを保持し、指摘を修正したcommitを通常pushした後、fresh
exact-source reviewで修正内容とその証拠をMRに記録する。未記録のverdictを承認扱いしない。
