# SubWorker実装中の契約接点の伝達

## 観測

T-741のdomain/store担当とserver認可/router担当を並行委任した。Worker管理の説明では実行中のWorkerへadvisoryを送る`WorkerNotify`が案内されていたが、親に提供されたtool inventoryは`WorkerList` / `WorkerSendInput` / `WorkerStop` / session observationで、`WorkerNotify`はなかった。

実装中に、認証済Runtime bindingをwrite transaction内でも照合する必要と、Assign/解除receiptにも同じsource-bound比較が必要という接点が判明した。Running childへの`WorkerSendInput`は使わず、session observationで子の実際の契約・diffを読み、子がIdleへ戻った自然な境界で追加依頼を送った。未完了の子を止めたり、入力の拒否を回避したりはしていない。

## 障壁と改善案

- 説明が存在しても実際のcapabilityがないため、同じ依存契約の追記をRunning中の担当へ早く伝えられない。双方がコンパイルエラーや作業木から接点を再発見する時間が増える。
- capabilityがある場合だけ実行中advisoryの説明を提示するか、明示的な親→直接子のadvisory toolを公開する。通常SubmitやIdle-onlyの入力と混同しない。
- 付与しない場合は、短い契約確定チェックポイントを分担の最初に設けることが現状の回避策になる。これは一般的な自動停止・polling・再起動の許可ではない。

本記録はこのSessionのtool提供面の観測であり、Backendの全環境に同機能がないと断定するものではない。

## Trusted Reviewer起動時の前提

同じSessionで、`builtin:reviewer`と明示MRのreview bindingを指定し、codeをread-only委任して起動を試みたところ、Backendから「writable delegated scopeが必要」、続いて「explicit command grantが必要」と引数検証で拒否された。Tool説明のread-only Reviewerという記述だけでは、この起動前提を予測できなかった。

最終的に、code全体のread scopeに加えて専用の空scratch `.review-t741` だけをwrite委任し、commandを明示grantして起動できた。source・Git metadataへのwriteを委任せず、Reviewerの調査・一時出力をscratchに限定した。拒否されたreview authorityを別経路で代替したものではなく、trusted review bindingはBackendが生成してMRに記録した。

必要なwrite/commandとcode read-onlyの両立例を、起動前のTool説明に示すと不要な引数試行を避けられる。また、Reviewerの最初の判定提出はseverity `medium`がHTTP 422で拒否され、許容値が`blocker/major/minor/note`と判明した。未記録をMR正本で確認し、新しいreview turnで適切なseverityによる独立判定を提出した。Role側の提出回数制約と引数validation failure後の返却・再開方法も、許容値とともに明示すると扱いやすい。
