# Reviewer SubWorkerがreview開始前に停止する

Editorial note (2026-10-10): obsolete state terminology is summarized by purpose below; cited IDs, commits and validation results still describe the original investigation, not new executions.

## 状況

T-698の公開済みMerge Requestに対して、`builtin:reviewer`を指定し、対象Ticket/Merge Request、writable delegated scope、command grantを明示してReviewer SubWorkerを3回起動した。

いずれも`review_requested` eventは正しいsource refと当時の Ticket item referenceを記録したが、SubWorkerは直後に`Stopped`となった。session observationには最初のuser taskだけが存在し、assistant output、tool call、typed review verdictは記録されなかった。Merge Request threadにもapprove/request_changesは追加されなかった。

## 障壁

CoderはReviewer approvalを代行できず、親Workerには`ReviewMergeRequest` authorityがないため、実装・公開・検証が完了していてもfresh approvalを取得できない。同じ入力を再送するだけでは診断情報が増えず、review requestだけが複数残る。

## 改善案

- Reviewer起動がmodel run前に停止した場合、停止理由を親へ構造化して通知する。
- `review_requested`を記録した後にReviewer sessionの開始に失敗した場合、request eventと失敗eventを関連付け、未解決reviewとして明示する。
- profile/provider初期化、review worktree準備、capability bindingのどの段階で失敗したかを、credentialやhost pathを露出しないbounded diagnosticとして提供する。
- 同一sourceへの再試行時は、先行Reviewerが一度も実行されていないことを識別できるようにする。
