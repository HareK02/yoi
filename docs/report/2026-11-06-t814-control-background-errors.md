# T-814 原因保持の移行・運用記録

## 対象と保証の境界

本記録は `4de62650dad111dc6c96c450a8a31fe4c6b1c4ac` を起点にしたT-814候補の移行点と調査手順を示す。T-812の公開共有HTTP/client・生成器とT-813の共通Web受信・表示を再利用した。起動domain本体・他branchのDB migrationを一括採用したものではなく、親の統合時にはcatalog migrationの処理内容・ledger identityを別途照合する。

失敗原因は既存操作の確定状態と別情報である。要求受理、配送、Controller受付、実処理、停止、cleanup、通知、結果保存を同一視しない。再試行hintや過去の原因を、新しいoperation、Job試行、Worker作成の許可として使わない。

## 発生源からconsumerまでの移行点

| 発生源 | 共通境界・記録点 | consumerと残存範囲 |
| --- | --- | --- |
| domainの認可済みTool投影 | `tools::failure::execution_failure` → `ToolError::ExecutionFailure` → Agenの二つの実行経路 → 通常Tool結果commit | `ToolResult`の失敗disposition・説明・構造化contentを保存し、次のモデル要求へ渡す。dispatch引数不備とは別。既存domain結果を同じenvelopeに保持する。 |
| WIP compatibility operation | `WipOperationError::ExecutionFailure` → operation response / Tool passthrough | protocol拒否とは別の監査結果。encode不能でも成功や新しい再送権限へ変えない。domain固有producerはT-815と分担。 |
| MCP Tool | `is_error` → 既存MCP renderer → 失敗Tool結果 | 受信した実行失敗を正常decodeだけで成功扱いしない。MCP固有のtransport/protocol原因の追加構造化は残存する。 |
| Worker lookup | Embedded型付きerror / Remote client型付きfailure → `WorkerLookupOutcome` → registry | 不存在と観測失敗を別variantへ。severity・診断順・説明から選ばない。Remoteの未対応HTTP bodyをdomain拒否と推測しない。 |
| Worker制御Tool | 既存入力disposition、command acknowledgement、Restore/Stop domain結果 → 共通Tool投影 | 受付と処理・終了を分離。旧入力応答でdisposition不明ならUnknown。Runtime/ServerのProblem投影とtyped receiver、Toolの主原因保持を接続。T-815公開認可ownerのactual status型を再利用し、拒否文面から分類しない。 |
| Runtime/Server HTTP・Web | ownerの`RuntimeError::control_cause` → `ControlBoundary` → T-812 HTTP hooks → T-813 receiver | raw非2xxは実status/media/byte上限で検証し、受信説明を再公開せず原因から再投影。2xx制御結果は主`control`とBrowser投影`control_explanation`を保持し、Webは診断順・severity・文面を選ばない。state/dispositionは説明の有無で変更しない。Restoreは既存startup domain primaryをBrowserへ再投影し、粗いcontrol拒否・cleanup・保存不明と別に運ぶ。実Serde応答を共有fixtureへ固定し、strict Web parserとbrowser表示へ接続する。 |
| Backend Job dispatch/input/recovery/deadline | `backend_job`のproducer変換 → 既存store transactionの`*_with_cause` | 元のattempt/Job状態と原因を同じtransactionで保存。型付きstartup primaryを保持し、conflicting/投影不能を汎用原因で上書きしない。保存失敗は`Error::BackendJobFailure`のprimaryとrecordingに分離。 |
| Backend Job cleanup | 既存admission resource・owner/run/history fence → cleanup専用原因列 | 実行結果や成功したJobをcleanup失敗で再分類しない。通知deliveryは従来の別ledgerに残る。 |
| Standalone Job | `JobExecutionError` variant → 既存settlement・attempt保存 | primary、結果受領不明、結果後Worker失敗、cleanupを別slotで記録。元のowned reportとcallback/finalizer所有を保持する。 |
| Plugin Service invocation/lifecycle | 実`ExecutionError` / effect observation → completion request → consumer outcome transaction。Stop・Complete/Release応答不明は独立`ObserveLifecycle` | 同じconsumer/event/invocation/attemptへ原因を記録。indeterminate effectはretryを許可しない。既存outcome列のversioned envelopeにinvocation bytesと独立lifecycle slotsを保存し、同Worker/正確な現行または最新released proofでfenceする。応答喪失はcommit失敗証明ではない。 |
| 保存結果reader | `BackgroundFailure` / `StoredFailure`とdomain projector | 旧データの原因欠落、未知version、破損を別扱い。保存説明を直接再公開しない。Standalone queryとBackend native `get_backend_job[_attempt]`結果の`failure_for_browser/tool`は現在visibilityを明示し、primary/cleanupを分離。Service認可済Diagnosticsはownerに残ったlifecycleも再構成する。Backendに存在しない汎用HTTP query/queueを新設するものではない。 |

Tool終了の正本はcommit済みSession Tool結果であり、ログ通知は代替ではない。Job試行・終端の正本は各ownerのDB行、Service障害の正本はconsumerに保存されたlast outcomeである。driverのCompletion/Release観測はDB commitの証明にならない。

## 保存schemaの結合接点

Workspace側は `SqliteWorkspaceStore` が管理するWorkspace catalog DBで、構成上のDB identityは `WorkspaceApi.config.database_path`。JobとPlugin Serviceは同じcatalogの順序付きmigration ledgerを使用する。候補は既存schema123の後にJob原因のnullable・byte上限付き列、続いてService outcome列を追加し、fresh compositionとretained schema verifierを揃える。旧結果、intent、attempt、resource、cleanup/notification identityを変換目的で再生成しない。

Standalone側は `StandaloneJobs::open` に渡される専用Job DB（製品では `jobs.sqlite3`）。Workspace catalogとは別owner/ledgerで、既存version8の後にattempt-localな原因保存列を追加する。古いfailure文章を解析して構造化原因へ昇格しない。

T-812のreservation結果、T-842のWorker removal進捗もWorkspace catalogの後続migrationを持つため、branch-local番号の一致だけで同じmigrationとみなさない。統合候補は親が処理内容・ledger名・依存順を選び、既存receiptを失わないchain、最古対応baseline、rollback、reopen、freshとの差分を再検証する。未公開の別branch DDLを推測してこの候補へ複製しない。

## 調査runbook

### 確定した開始拒否

- 必要権限：対象Workspace/Worker/Job、またはStandalone ownerの既存読取り権限。Serviceは現行execution/installation/Service認可も必要。
- 正本：同じJob/attemptとpre-dispatch admission/allocationの既存receipt。Toolではdispatch結果と実行結果のboundaryを確認する。
- 相関：既存call ID、Session entry、Job ID、attempt ID、input digest、Worker/run。秘密、生prompt、provider bodyを相関情報へ複製しない。
- 処置：拒否のdomain条件を解消した後、既存ownerの明示retry条件・attempt予算・cleanup・resource占有を再確認する。原因の一時的分類だけでは試行を増やさない。

### 受理後の実行失敗

- 正本：実行attemptの保存状態とprimary。元原因と結果受領・cleanup・通知の別観測を読む。
- 同identity：同じinput digest/attempt/runに限定して再照会する。新Workerへ結果を付け替えない。
- 処置：確定した失敗、残り予算、先行ownerの終了、domain grant/現行認可を既存規則で確認する。保存成功した結果後のWorker失敗は、成功結果を取り消す理由にならない。

### timeout・応答喪失・結果不明

- 正本：既存allocation/input/command/result receiptとownerの最後の確定観測。HTTP timeout、Idle、プロセス不在だけで失敗・停止完了としない。
- 同identity：同じrequest ID、tracked input/run/result IDで照合する。新IDの再送やallocationを行わない。
- Serviceは同じconsumer/event/invocation/attemptを照合し、effect indeterminateでは既存の明示recoveryに従う。Job finalizerのack喪失時も封印済みpayloadを照合し、副作用を再実行しない。
- 必要権限：照会権限に加え、回復操作固有の現在の権限・owner/fence。保存された公開ラベルは新しい閲覧者の権限ではない。

### cleanup・補償・保存失敗

- 正本：元操作の結果と、独立したcleanup child/owner/run/history/admission resource、保存またはdeliveryの結果を分けて読む。
- cleanupが未確認なら占有とownerを保持する。missing admission resourceや空のin-memory mapを終了証明として再作成しない。
- 保存errorは永続成功ではない。typed primaryをownerに保持し、既存storage probe/同attempt reconciliationへ渡す。二回目のJob状態機械や別queueを作らない。
- trace sampling/sink障害・通知失敗を原因保存の代替や操作状態変更の契機にしない。調査者は必要なDB/Session読取り権限と、変更時には対象回復操作の権限を別々に満たす。

## 検証証拠の扱い

実行記録は本Worker sessionのtool entriesと `target/t814/` のbounded logsに保持する。ログ参照だけで成功とはせず、source、実command、環境、exit/timeout、到達範囲をhandoffに添える。Runtime停止で中断されたcompile、時間切れの試験、未実行caseを成功件数に含めない。

Worker通常suiteは既存restore fixtureが要求するcheckout内の専用config/data/runtime namespaceを起動環境で指定する。指定なしのcontroller fixture拒否は環境不備として区別し、隔離環境で再実行する。製品のlive設定を借りない。

HTTP/clientは実受信status・mediaを保持する既存の共通hooksを使用する。Runtimeの旧JSON middlewareがtyped Problemを上書きする回帰と、create fingerprint conflictを文面で409へ分類する旧方式は、媒体の区別とownerのtyped conflictへ修正した。旧Runtime HTTPテストも実Problemのdecodeを検証する。Web表示試験のBackend認可・副作用はfixture境界の外であり、実Runtime/Service ownerの試験と区別する。

目視では1440/768/390pxのRuntime接続失敗を変更前後で確認し、Restoreの主表示が汎用UnknownからSession登録拒否へ変わることを確認した。操作ボタンと既存stateを保ち、予期した403以外のpage/requestエラー・ページ横overflowはなかった。画面・console・画像hashは `web/workspace/.svelte-kit/runtime-control/` に保存する。共通wire fixtureは公開投影の検証資料であり、認可成功の証明ではない。

一連の試験で出た失敗・timeoutを後続成功へ書き換えない。Serviceの旧認可fixture不備・Release proof保持前shutdownは既存ownerの正しい境界を使って修正し、実embedded/remote・reopenを再検証した。Runtime並列実行での期限超過は隔離および逐次package全体で再確認したが、並列deadline感受性そのものの解消は主張しない。統合後catalog chainと正式source承認は親の採用判断・MRの最新記録を正本とする。
