# Review finding severity の入力契約

T-710 の独立レビュー中、Reviewer が `ReviewMergeRequest` に finding severity `medium` を渡し、HTTP 422 で拒否された。Backend の許可値は `blocker` / `major` / `minor` / `note` で、最初の呼び出しでは verdict が記録されなかった。

親は committed session と MR thread を確認して未記録であることを確かめ、Reviewer の新しい turn で同じ capability・同じ MR/source に対して `major` を使った訂正を依頼した。訂正は成功し、MR sequence 3 に `request_changes` が記録された。別の保存経路や Ticket comment で verdict を代替しなかった。

## 改善案

- finding severity は公開 tool schema でも Backend と同じ enum として示し、受理可能な語彙を入力前に確認できるようにする。
- capability を使う操作の検証エラーでは、イベントが未記録であることと、訂正した入力を再送できるかを明確に返す。Reviewer が「一回の呼び出し」と「一回の有効な verdict 記録」を混同し、未記録のまま止まる障壁を減らせる。

これは observed rejection と訂正成功に基づくフィードバックであり、Backend の review authority を迂回する提案ではない。
