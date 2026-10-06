# SubWorker follow-up後のcommitted sessionを確認できなかった

T-700の実装中、既存のIdleなdirect child `backend`へ`WorkerSendInput`で追加修正を依頼した。toolは成功を返し、その後Idle通知と`WorkerList`のIdle表示を確認した。

一方、`ViewSessionOverview`と`SearchSessionEntries`が返した最後のentryは、follow-up前の保存Manifest fixture修正報告（entry range 744）だった。今回送った追加依頼の文字列の検索は0件で、offset 744からもその旧報告1件しか見えなかった。したがって、通知だけから追加作業の完了は認定できなかった。これは最新turnの実行失敗、captureの遅延、観測面のfreshnessのいずれかを切り分ける証拠ではない。

親は現在のdiffを直接確認し、子をStopしてdelegated scopeを解放した後、残りのShutdown admission修正を新しいdirect childへ限定して委譲した。新しい子では最初の依頼と修正報告をsession observationから確認できた。

改善案:

- Idle通知に、対象turnとcommitted captureの末尾entryの対応が分かる情報を含める。
- follow-upの開始またはcaptureに失敗した場合、成功した旧turnだけを返す代わりにbounded diagnosticを表示する。
- observationで最新のcommitted captureが未取得の場合、古いsnapshotであることを明示する。

credential、raw prompt、Backend保存領域の直接読取で観測制限を迂回することは行っていない。
