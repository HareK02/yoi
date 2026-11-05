# T-815 domain公開境界の調査・接続記録

## 観測範囲

調査基点は `main / rebuild/d929-coordinated@4de62650dad111dc6c96c450a8a31fe4c6b1c4ac`（tree `0050b5fa231ed768907738d5110330a1e7ceab92`）。T-815の現要求digestは `86c40fb57d3110da8886636da524d9bdc45f9211470188962b57f1fea63bb55c`。これは着手時の調査と接続途中の記録であり、全ACの合格、配備、先行候補の承認を示さない。

T-810の純粋coreと採用済みT-840/T-843を利用する。共通HTTP/clientはT-812、Web受信・表示はT-813、Tool/Job/Service境界はT-814が所有する。ここで別formatter、codec、秘匿処理、設定writer、操作状態機械を作らない。

## 発生源からconsumerへの移行表

| domain / 発生源 | 現行のtyped情報と変換 | consumerと残る接続 |
| --- | --- | --- |
| Repository lookup | `RepositoryLookupError` の7variantを `repository_failure::RepositoryFailure` が原sourceごと保持する | `repository_lookup` と `repository_merge_evidence_error` のDebug変換を除き、`Error::RepositoryLookup` へ接続した。HTTP分類は操作とtyped reasonから決定し、既存公開diagnostic codeを保つ。共通HTTP応答とWebへのdetails接続は未完。 |
| Repository構成・SSH access | `scoped_create_repository` の同一intent replayと異intent競合、`require_workspace_owner`、`repository_access.rs` のcredential/trust/CAS検査 | 登録、SSH操作、Workdir materialization、Web repository-access。汎用 `RepositoryConflict(String)` / `WorkspacePermissionDenied(String)` の全発生元を一つのRepository理由と誤認せず、Repositoryの決定点でtyped causeを付ける。秘密の鍵やleaseをdetailsへ渡さない。 |
| Workdir provider | `WorkdirError::classification_source`、`denial_reason`、`failure_reason`、`WorkdirTransportError` | 本差分の `workdir::failure` が既存型からT-810へ接続する。元sourceはローカルに保持し、Browser/Tool/transfer/storage/telemetryにはclosed detailsだけを投影する。既存HTTP/statusや操作実行は変更しない。 |
| Workdir Runtime / Backend | `RuntimeHttpWorkdirError`、`RuntimeRegistryError::WorkdirSessionOpenFailed`、`InternalWorkdirOperationError::into_contract_error` | `WorkdirOperationApiError` は理由をserver-local metadataに保持するがwireではskipする。T-812の共通response/clientへの接続と生成型変更が残る。owner mismatchの404等、外側境界の秘匿分類を内側codeで上書きしない。 |
| Workdir Tool / 監査 | `WorkspaceAttachedWorkdirSession::operate`、`WorkdirSessionOperationResponse`、`WorkdirAuditFailure` | Workerは監査receiptをtracingへ通知し、操作resultだけを返す。T-814の既存Tool envelopeへの接続で成功＋監査失敗を別々に保持する必要がある。成功を実行失敗へ変更したり、監査失敗で再送したりしない。 |
| Workdir deferred open / cleanup | `DeferredWorkdirSession` はopener task失敗、open失敗、identity mismatchを区別。失敗したcloseの所有者も保持する | openerは `Result<WorkdirSessionHandle, String>` のため、より具体的な原因は現契約から証明できない。identity mismatch後のcleanup理由はログだけ。元原因をcleanupで置換せず、別の副次結果として接続する作業が残る。 |
| Workdir removal | `WorkdirRemovalOperation`、request fingerprint、attempt、disposition、failure category、既存reconciliation | `workdir_removal_response`、Web `workdir-removal.ts`、Worker lifecycle Tool。復旧は観測のみで削除を再実行しない。Backend/Worker/Webの説明表はdomain説明へ集約するが、結果・再実行条件はこの正本を利用する。 |
| Drive storage / HTTP | `workspace_drive::failure::DriveFailure` が9variantの元errorを保持し、closed operation/reasonを共通coreへ投影する。metadata/receiptは既存同一transactionのまま | 実storageでconflict/CAS/grant/replay/fence/SQLite障害からcodecまで検証した。`server_drive.rs::domain_error` はまだ既存公開codeへ縮退し、共通HTTP/Toolへの接続は未完。storage原因はremote更新の確定/Unknownを証明しない。 |
| Drive Worker / Tool | `drive/backend.rs` の `DriveError`、`backend_diagnostic::{Stage,Reason}`、同request status照会 | 内部理由は主にtracing、Toolは文字列へ変換される。理由を返値に保持するdomain mappingとT-814接続が残る。status GETは同IDであり2回目のmutationではない。 |
| config owner採用 | `ConfigurationFailure` が現 `ConfigTreeError` 15variantのsourceと、採用決定点のcatalog/head/export/source/owner/request理由を保持する | source helper、実SQLite採用、owner HTTPのsource/preview/adopt/reconcileへ接続した。非active/drainingもHTTPの決定点で付ける。T-838のwriter/transaction/receiptは変更せず、HTTP分類はtyped reasonから決定する。formatterのString診断、policy/contentの未型付け原因、共通応答/表示への接続は残る。 |
| config Worker論理tree | `server_workspace_config.rs`、`workspace_config/backend.rs`、post-commit observe | 旧writerはauthority sealとcanonical採用で拒否される。T-776回帰のUnknown/非再送を保つが、この旧経路だけの試験をowner採用の証拠にしない。 |
| 内部 / 背景 | Drive maintenance、config採用後のRuntime publication、既存Job/Service結果 | configのpublicationはcommit後best effortで失敗を返却/保存していない。採用receiptをrollback/Unknownへ変更せず、T-814の共有保存/照会方式へ副次失敗を接続する。別ledgerは作らない。 |

### 共通consumerと生成物

`RepositoryApiError` はRepository専用ではない。Subject body、Worker起動、Runtime trust/management、configuration adoption、permissions、client CLI、Worker Toolにも共有される。`failure` は基点で起動専用attachmentを受けるため、非起動domainをその型に偽装しない。

Web `api/workspace-model.ts::parseRepositoryApiError` は `diagnostics/error/message` のみをexact-keyで受ける。`api/runtime-management.ts::repositoryApiError` にも独立parserがある。型の追加だけではruntime parserの拒否を防げない。T-813の共有decoderへ移行する単位で、既存readerを照合する。

生成元は `server-api` のAPI trait/OpenAPIと既存 `repository_openapi_typescript`。Repository、Workdir、Drive、Runtime、Worker launch、configuration adoption等の各生成器を、それぞれの変更元に合わせて検証する。共通型の手書きTS複製は作らない。T-813のsource `work/t813-error-web@838b6cd3c3dbf45fe555958ef9c017f1bc09bc6b` は取得・API確認済みだが、本記録時点では未採用で正式review済みでもない。

## Workdir公開field対応と制約

`workdir::failure` は新しい共通coreではなく、既存Workdir型をT-810の `FailureCause` / `WireDetails` / `ProjectFor` に対応させるdomain adapterである。

| 情報 | 内部の保持 | 投影する情報 / 条件 |
| --- | --- | --- |
| 原原因 | `WorkdirFailure` が元 `WorkdirError` とsource chainを所有 | 原errorのSerializeやDisplayをencoderへ渡さない。 |
| 対象操作 | 既存 `WorkdirSessionOperation` のvariantからclosed kindを取る | 操作kindと、その固定の人間向け表現。引数、path、command、内容は取らない。 |
| 既存分類 | `WorkdirTransportErrorCode` | `classification` をそのまま保持。Categoryは集計用でありHTTP/retry/完了の判定には使わない。 |
| 観測された理由 | 既存 `WorkdirDenialReason` / `WorkdirFailureReason` | Workdirとそのfailure diagnosticsの現在の閲覧許可を呼出し境界が確認した場合にclosed valueと固定説明を公開する。visibilityのdefaultは拒否。enumであるだけで一般利用者へ無条件公開しない。 |
| 旧providerの理由欠落 | 受信transportのOptionをlocal error再構成前に保持 | `from_transport_error` を利用し欠落は欠落のまま。理由らしいmessageから推定しない。 |
| 補足context / 相関 | 元facts/受信envelopeは信頼しない | 操作/段階はclosed factsから再構築。受信説明、operation_ref、任意correlation、任意contextはコピーしない。安全な相関の責任境界接続はT-812成果との統合で行う。 |
| Unknown、監査、cleanup、同request照合 | 既存domain結果が正本 | この原因型へretryable、mutation identity、汎用rollback状態を追加しない。成功時監査receiptをこの実行失敗型へ変換しない。 |

Browserとモデル向けToolは、現在の可視性が同じなら同じ原因意味・説明を使う。保存/transferからの再公開にも再投影を要求し、保存時の公開ラベルを新しい閲覧者の権限にしない。全opaque sourceを公開するのでも、具体原因を一律omittedへ消すのでもない。

## Repository・Drive・configの公開field対応

これらの追加でも共通coreのdomain match、formatter、秘匿処理、codecを増やしていない。domainが `FailureCause` / `WireDetails` / `ProjectFor` を実装し、既存codecへ同じ方法で接続する。

| domain | sourceから採る公開field | ローカル限定・独立する正本 |
| --- | --- | --- |
| Repository | `RepositoryFailureDetails { operation, reason }`。reasonは元lookup variantのみ。provider一般失敗から細かい原因を推測しない | Repository ID/key、provider名、selector、commit、path、provider診断は元source内に限る。access/grant/trust/登録intentの競合はこのlookup原因とは別の決定点。 |
| Drive storage | `DriveFailureDetails { operation, reason }`。reasonは元storage error variantのみ | `Invalid` は複数の不正条件、`Conflict` はCAS/name/folder/request競合を含み、それ以上の細分化を捏造しない。SQL/blob/storage source、名前/内容/物理identityは非公開。remote Unknownは別の結果契約。 |
| config | `ConfigurationFailureDetails { operation, reason }`。tree variantまたは実際の採用/owner分岐で決まった理由 | path/import/source/schema本文/診断を公開しない。tree sourceは保持するが既知の分岐に架空のsource errorを作らない。joint migration baseline検査はsource/export不一致のまま保持する。head/catalog/receipt/policyはT-838の正本。 |

各domainのvisibilityはdefault拒否で、Browser/Toolだけでなくtransfer/storage/telemetryも現在の診断可視性を要求する。許可後の説明はclosed factsと操作の固定表現から再構成し、受信説明や任意context/correlationを転送しない。安全な原因を公開することと、原sourceを全公開することは別である。

実Gitのancestry拒否と実SQLite採用のhead/catalog/source/entry拒否からBrowser/Tool/codecまでの接続を検証した。採用失敗後のsource/policy/head/receiptと同request replayも検証したが、これは実Tool結果の保存・照会、HTTP→Web、相関検索基盤の完成証拠ではない。保存バイトの再読試験も、新しい背景結果storeの実装や検証とは区別する。

## 相関検索から正本・復旧へのrunbook

以下は既存の結果正本と必要権限を示す調査時点の手順である。まだ公開されていないstatus APIや新しい操作権限の存在を仮定しない。

| 事象 | 参照・相関と必要権限 | 正本と安全な対処 |
| --- | --- | --- |
| Repository拒否/競合 | 認証されたWorkspace操作者。secret管理はWorkspace owner。既存の操作と安全なRepository keyを確認し、秘密そのものや生provider本文を問い合わせへ貼らない | 登録intent、現Repository source/access/trust、期待CASを再読する。競合は同名成功の証明ではない。権限追加は失敗表示ではなくownerの独立判断で行う。 |
| Workdir認可/session-open | 現Workdir/Workerへの閲覧・操作権限。運用ログ閲覧は別の権限。既存ログの固定operation/stageとID fingerprintで範囲を絞る | Runtime/Backendの現在のowner binding、session/attachment、grant期限を照合する。404等の秘匿境界を迂回して他ownerの存在を公開しない。session-open一般失敗からprovider内部原因を捏造しない。 |
| Workdir操作成功＋監査失敗 | 公開audit receiptのoperation identityを保全。ログ側のoperation ID hashとstageで検索。監査store閲覧は認可された運用担当のみ | 元operation resultと監査receiptを別々に読む。監査失敗で成功を取り消さず、mutationを再送しない。保存された監査状態の修復は既存監査責任境界で判断する。 |
| Workdir cleanup/削除Unknown | 現Workdirへの管理権限。現公開結果と、認可されたServer側の既存removal operation/store観測を照合する | disposition/request fingerprint/attemptを保持し、既存reconciliationで存在・所有・cleanup結果を読む。公開operation-status APIの追加を仮定しない。Unknownやtimeoutで別requestの削除を開始しない。元原因とcleanup失敗を分け、未解放ownerを保持する。 |
| Drive更新後Unknown | Workspace-bound Drive grantと対象scope。元request IDと入力を保持し、既存request-statusを同IDで照会。内部ログでは既存hashed correlationを利用 | `Committed` のidentity/metadata検証でのみ確定する。未commit snapshotや照会失敗は未実行の証明ではない。新IDでcreate/write/uploadを繰り返さず、CAS/grantを緩めない。 |
| config owner採用の応答不明 | 現在のWorkspace owner。完全な元request（operation ID、期待head/catalog/export、source変更）を保持し、owner reconcile APIを使用 | `Adopted` は既存receiptを参照する。`PendingTransfer` / `NotObserved` は不明のまま。旧source-tree writerやWorker writerへ迂回しない。明示的に同一requestを再開できる既存契約と、新規採用を区別する。古いreceiptを現在policyへ再適用しない。 |
| config採用成功＋Runtime publication失敗 | ownerが採用receiptを確認し、認可された運用担当がRuntime側の現観測を確認 | source保存、policy commit、次のcurrent-policy checkとRuntimeへのbest-effort publicationは別結果。publication失敗で成功採用を未実行としない。共有背景adapterへの原因保存接続は未完である。 |

相関IDは検索用であり、Drive request ID、config operation ID、Workdirのfingerprint/attemptを置き換えない。HTTP status、severity、reason prefix、retry hint、文面は再実行の許可ではない。通常long-poll空応答は故障記録へ変換しない。

## 既存回帰と追加検証の対応

既存試験の所在を調査したもので、ここに名前があること自体は実行成功を示さない。

- Repository: `repository_log_rejects_unknown_or_unsupported_configured_repository`、`repository_secret_management_is_owner_only`、`configured_repository_import_rejects_changed_source_intent`、`ssh_rotation_http_fences_key_identity_and_replays_exact_operations`、`repositories::tests::sanitizes_remote_credentials_and_local_paths`。
- Workdir理由/秘密: `workdir/tests/{denial_reasons,fs_denial_origins,remote_denial_reasons,failure_diagnostics}.rs`。API/logは `server/tests/api_logging_tests.rs` と `api_logging_tests/workdir_open.rs`、署名Runtime openは `http_server/tests/runtime_workdir_failure_tests.rs`。
- 正常long-poll: `real_http_waiting_command_survives_long_poll_expiry`。監査/効果回数は `workspace_admission_tests/audit_tests.rs` の成功、拒否、応答喪失、timeout、dispatch前監査失敗の各試験。session close/recoveryは `workdir_session_close_tests.rs` とdeferred Workdir試験。
- Drive: `production_mutation_loss_reconciles_only_original_identity_and_never_resends`、`production_deadline_does_not_cancel_effect_or_infer_uncommitted_from_snapshot`、`http_reply_loss_keeps_original_receipt_and_retained_effect_without_reexecution`、Server `drive_api_tests.rs`、`workspace-drive/tests/storage.rs`。更新回数、同ID照合、CAS、再openを維持する。
- config: `editing_head_rejects_aba_and_parallel_editors_but_receipts_reconcile_lost_responses`、`failure_at_each_storage_boundary_rolls_back_and_reopens_the_previous_adoption`、`server_failure_after_seal_rolls_back_and_reopen_resumes_only_the_exact_full_request`、`server_configuration_adoption_tests.rs` のowner/origin/異内容同ID/旧writer拒否/receipt非再適用、Web adoption API/editor試験。
- 本差分のWorkdir domain試験: 実Local provider＋ReadOnly wrapper＋本番dispatcherから投影/共通codecへ接続し、拒否と非更新を確認する。Browser/Tool意味一致、受信文面変更不変、保存バイト再読時の現在可視性、秘密非露出、旧理由欠落、Unknownと原因の独立、unsupported/corrupt/小さいboundary budgetを検査する。

最後のdomain試験は、まだ実HTTP→Web、実Tool envelope、既存背景storeの保証ではない。T-812/813/814の公開artifactを利用した接続試験、生成物、Web check/build/browserと実画像目視、Server/Runtime build、全要件への正式reviewが残る。

## 並行変更との結合点

T-837は現行Feature認可、主体/委譲、管理UIを変更中である。可視性決定を古い保存receiptや起動時grantへ固定せず、採用source上の現在の境界で確認する。T-842は削除、Job記録、writer解放、Workdir detachと共有DTO/保存結果を変更中である。削除待ちを完了へ投影せず、元結果・secondary failureと未解放資源を保持する。共有file名だけで依存とせず、target移動時に実際の型・feature・保存/初期化・回復経路の結合影響を再評価する。
