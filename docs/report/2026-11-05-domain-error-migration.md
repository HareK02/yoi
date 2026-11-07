# T-815 domain公開境界の接続・検証記録

## 対象と判断の範囲

T-815要求digest `86c40fb57d3110da8886636da524d9bdc45f9211470188962b57f1fea63bb55c` に対する実装・検証記録。着手基点は `4de62650dad111dc6c96c450a8a31fe4c6b1c4ac`。T-852、T-837、公開されたT-845を通常合成し、直近のmergeは `f8a3f6524d7f504cb7d960795e1362e48634f14b`、treeは `f750e9af251cf87fe86183cf4561a14ecf3186af`（parents: checkpoint `166a4767b` とT-845 `fc67ce8be30a9f9a30df0aec505fb57874075324`）。後続のJournal試験fixture修正は実認証bindingを使う変更であり、製品認可を緩和しない。この記録だけから全要件合格・公開source一致・独立review承認を判断しない。

共通coreはT-810、HTTP/clientはT-812、Web receiverはT-813、Tool/Job/Service結果境界はT-814を使う。domainは既存の原因を `FailureCause` / `WireDetails` / `ProjectFor` へ接続し、共通formatter、codec、秘匿規則、設定writerを複製しない。Worker起動原因は採用済みT-840/T-843を利用する。

## 発生源からconsumerへの対応

この表は本候補の接続点と検証責務を示す。各行の存在はその全試験成功を意味せず、下記の固定source別証拠と区別する。

| 発生源・正本 | domain接続 | consumer・公開境界 |
| --- | --- | --- |
| Repository lookup、登録intent、source/access/trustと期待CAS | `repository_failure` と `repository_access_failure` が元lookup/決定点のtyped理由を保持する。汎用権限errorや競合文字列からRepository理由を推定しない | `server.rs` / `repository_access.rs` → `RepositoryApiError` / Repository access APIの共通Problem → Web `failure/domains.ts` とrepository-access画面。shared RepositoryApiErrorの起動・設定・Runtime等の利用者も生成型/共通receiverを使う |
| Workdir provider、session open・操作・close | `workdir::failure` は既存 `WorkdirTransportErrorCode` と任意のdenial/failure reasonを保つ。旧providerで理由が欠けた場合は欠けたまま保持する | Runtime署名HTTP、`RuntimeRegistryError::WorkdirSessionOpenFailed`、`WorkdirOperationApiError`、WorkerのHTTP/native WIP、登録Read/Write/Edit/Glob/Grep/Bash/ViewImage。外側502等の境界分類と内側provider原因を分離する |
| Workdirのcall-local監査receipt | `workdir::audit` のcollectorをServer/Workerで共用する。成功content、失敗、Cancelled/Interruptedと監査結果は独立 | 共通Tool出力を追記し元content/attachmentsを維持する。元request/receiptの同一性と非再送は元結果が所有し、監査失敗で元操作を失敗へ変えない |
| Workdir inventory/create/detail/cleanup、Backend removal operation | Runtime/Backend決定点の `WorkdirManagementFailure`。removalは既存request fingerprint、attempt、disposition、failure categoryとreconciliationを利用する | `RuntimeDiagnostic.workdir` → `WorkingDirectoryManagementDiagnostic` / `WorkdirManagementApiError` → Web管理画面と既存Tool/CLI。removal復旧のreadは観測のみで削除を再dispatchしない |
| attachment、外部grant fence/connection/cleanup | `attachment_failure`、`external_grant_failure`、`grant_cleanup_operation`。元結果、session close、Backend unlink、Runtime同期、補償、監査と保存失敗を関連する別観測として保持する | Attach/Detach・grant create/inspect/revoke/connect、Worker self-attachment、CLI workdir-share。RuntimeのMatching/Different/Unconfirmedは元operationのreceipt/世代/全requestとBackend CASなしに完了や再送許可にならない |
| T-852のretained/orphan session cleanup | 既存 `WorkdirCleanupFailure` / `WorkdirFailureDetails` に接続する。`RuntimeDiagnostic.session` はclosed SessionClose原因、Detachは `WorkdirSessionCloseFailed` を保持する | Browser/Toolで既存Session原因を再投影する。close未確認とWorkdir removal非dispatchを分離し、retained session identityで通常cleanupする。新しいattachmentや別Workerのsessionを古いcloseで解放しない |
| Drive storage/HTTP/maintenance | `workspace_drive::failure::DriveFailure` が元storage errorを保持し、`server_drive.rs` が共通HTTPへ投影する。maintenanceは既存結果を利用する | `server-api` Drive応答、Web共通receiver、Worker HTTP/native WIP/登録Tool。既存class/code、Host request ID、CAS、権限、OutcomeUnknownを原因と独立に維持する |
| config source/preview/採用/照合 | `ConfigurationFailure` がtree errorとcatalog/head/export/source/owner/request決定点を保持する。設定source/採用writer/同transaction receiptは既存T-838の正本 | owner APIのProblem、Web設定画面、既存同request照合。採用成功後のpublicationは採用receiptの成功を上書きしない |
| config採用後Runtime publication | `config_publication` が既存adoption receiptと全request fingerprint/sourceを結び、Runtimeごとの結果・原因を共有 `BackgroundFailure` / `StoredFailure` で保持する | owner照会から現在権限で再投影する。古いqueued sourceを現sourceへ採用せず、保存履歴を再dispatch許可にしない |
| Worker config tree/attachとTool履歴 | `ConfigFailure` が元WIP errorとoperation/stage/reasonを保持する。構造化invocationのoptional failure attachmentは既存Session receiptへ保存する | 実Feature/Worker/Tool結果、native WIP、append/reopen/prune後のモデル入力。未知・破損・現在認可不能のoptional証拠だけを非開示とし、元Unknown/同request/非再送は維持する |

### 生成と混在version

生成元は既存Server/Runtime/config API trait・OpenAPI・`repository_openapi_typescript` とdomain schema generator。`web/workspace/src/lib/generated` に別の手書きschemaを置かない。`WorkdirManagementCause` はmanagement、session、Runtime controlという既存の閉鎖原因の受信carrierであり、owner objectはunknown fieldを拒否し、OpenAPIは排他的な `oneOf` を使う。混合objectを一つの原因へ都合よくdecodeしない。

HTTP受信ではstatus、media、identity、byte上限を共有receiverで検査する。未知reason/version、破損、過大応答、transport失敗を既知の拒否に捏造しない。起動・Runtime control・Workdir managementの原因は互いに置換しない。decoderが観測したUnsupported/Invalid/ResponseTooLarge等を後段の汎用ExecutionRejectedで上書きしない。正常long-pollと待機timeoutを操作失敗へ変換しない。

## 公開field対応と非露出

| 原情報 | 公開する閉鎖field | ローカル限定・独立した正本 |
| --- | --- | --- |
| Repository lookup/access source | operation、既存variant/決定点由来のreason | Repository/provider診断、selector、path、鍵、lease本文。登録intent、access/trustと期待CASは別の正本 |
| Workdir session source | operation、classification、任意のdenial_reason/failure_reason | 引数、path、command、内容、生HTTP/provider本文。受信の任意説明/context/correlationは再公開しない |
| Workdir management source | operation、reason。session/Runtime原因は既存unionの別枝 | inventoryやscopeの現在権限。管理HTTP境界とRuntime内部のstatusは独立 |
| 監査receipt | closed stage/reason/operation_outcome | 元operation ID、正本receipt、効果回数、Unknown/Cancelled/Interrupted。operation ID hash等の既存運用相関を原因のretry authorityにしない |
| Drive source/client | closed operation/reason、既存class/code | SQL/blob/path/name/content。remote Unknownをstorage原因から確定させない |
| config owner/client | closed operation/reason、必要なstage | source/schema本文、import/path、catalog/head/receipt/policy。元WIP UnknownとBackend同request照合は別契約 |

visibilityはdefault拒否。Browser/Toolだけでなくtransfer/storage/telemetryも該当診断への現在の権限を境界が確認する。保存時の公開ラベルを将来の閲覧権限にしない。現在可視性が同じBrowser/Toolには同じ意味を投影し、原sourceのDisplayをSerializeしてから原因へ戻さない。公開可能な具体原因を「private details omitted」の一文へ消す方式にも戻さない。

## 正本・相関・復旧の調査経路

| 事象 | 確認する正本と必要権限 | 行ってはいけない推論 |
| --- | --- | --- |
| Repository拒否/競合 | 認証されたWorkspace操作者が現source、登録intent、access/trustと期待CASを再読する。秘密管理はownerの独立判断 | 同名存在、説明文、categoryから元intent成功や権限追加を推定しない。鍵・生provider本文を問合せへ貼らない |
| Workdir監査/操作Unknown | 当該呼出しの元Host request/receiptと同request照合。現在のattachment/scope/connection、操作の既存公開結果を確認する | 監査失敗やUnavailableを未実行とせず、別IDの更新を作らない |
| Stop受理後cleanup失敗 | Stopの受理結果と独立したcleanup cause、retained session/使用guard、元終了receiptを確認する | cleanupを再試行するためStopを再送しない。受理receiptだけで実scope終了を断定しない |
| retained/orphan session未解放 | 元Runtime connection、Worker/attachment connection/session IDを既存診断で相関する。正常Remove/Detach/cleanupはその所有者が保持するclose identityを使う | 件数をゼロにする、古いcloseを新sessionへ当てる、readをclose/削除の権限にすることを避ける |
| Workdir removal未確定 | 既存operation、request fingerprint、attempt/owner、materialization fingerprint、dispositionを再読する | recovery readで再削除しない。現在NotFoundや同じ表示名だけで別materializationへ元操作を採用しない |
| 外部grant/attachment cleanup | fence intent、frozen admission/full request、session/unlink checkpoint、Runtimeの元operation receipt/current generation、Backend fullsetとCASを照合する | Differentを未実行/再送許可にしない。Matchingだけで元の同試行完了にしない。同setの別試行・同key/Worker ABAを遡及採用しない |
| Drive OutcomeUnknown | 元Host request IDと保存されたreceipt/結果を照会し、現在のgrantと観測entryのCASを確認する | directory内容の一致、分類、説明変更から元更新の完了や新規再送権限を作らない |
| config採用/publication | ownerが既存adoption receiptと全request fingerprintを照合し、publication結果は独立に照会する | publication失敗で採用をUnknown/失敗に変更しない。古いsourceのqueued結果や照会をwriter/再送許可にしない |

Runtime failed-create publicationのAfterRenameも別の接続点である。create ownerのoperation lock内で固定した元candidate全aggregateを既存canonical readとdurability barrierで照合し、一致する場合にだけそのwrite fenceを解除する。元Unknown errorは保持する。read/sync失敗やowner/Stop/attachment世代不一致ではfenceを残す。rollbackはread単独の副作用ではなく、別途確認した元cleanup operation-bound scope terminationを必要とする。汎用read helperに削除/停止権限を追加しない。

## 保存schemaとの合成

canonical/retained startup・Job・Serviceの公開済み履歴とJournal bridge/receiptを維持し、T-815の独立publication/connection観測は、先採用T-845のPlan廃止128/Ticket schema 17の後に単一appendのWorkspace migration 129として追加した。Runtimeの16→17は別storeの変更である。既存receiptやmigration名を整理目的で書き換えず、空migrationやshape recognizer迂回で合格させない。最古対応schema、実DDL、値・制約・履歴、rollback/retry/reopenの検証は登録されたmigration suiteが所有する。

## 固定source別の証拠と未確認事項

T-845通常合成時の `t845-composition-build-1` / `t845-composition-migrations-1` は通常11crate test buildと、failure-format・combined・host Plan retirement・Job原因移行・config原因23件の成功（exit 0/sourceSame）を保存する。T-845の凍結Host113/Ticket10 SQL、Ticket schema所有者、Host113/128 migration本体は公開targetとbyte一致を確認した。`t845-composition-generated-refresh-1` と `t845-composition-generated-check-1` はroot check、canonical OpenAPI・6派生TS・domain/protocol/config生成と実cmp、fmt/diff/命名検査の成功を保存する。

`composition-baseline-build-1` / `composition-baseline-build-confirm-1` / `composition-baseline-comparison-1` は独立にcheckoutしたclean target `71c350dd2f4db25a31c3e4b33037970d2bc3e3fa` のbinary SHA `74f98f675d7b926bfa62abb2df6802f95d71c4a29ffc84cfac0ba5286c1a3590` を固定する。`composition-baseline-failures-1/group-{0,1}.stdout` とT-845合成後 `t845-residual-1/focus.stdout` では、次の7件の失敗箇所と観測された原因が一致する。名前だけの一致から共通原因を推定したものではなく、これらを成功件数に含めない。

| 試験 | baselineと合成後で一致する残余 |
| --- | --- |
| `chat_delegation_connected::reassignment_during_private_chat_result_evaluation_refuses_old_report_as_execution_authority` / `restricted_chat_uses_current_ticket_assignee_target_and_saved_result_before_publication` | target Subjectの文言を要求するassertに、403の型付き `runtime_control/result_unavailable` が返る |
| `ticket_worker_messaging_connected::public_two_subject_ticket_spawn_notify_edit_result_retry_and_fences` | 同じwrong-assignee probeの文言assertと403/`result_unavailable` |
| `rejected_orchestrator_spawn_stays_offline_and_can_be_retried` | 旧provider断片文言を要求するdiagnostic assert |
| `restored_parent_control_tests::restored_runtime_parent_lists_and_inputs_existing_child` | 取消後の旧 `unknown worker` 文言assertと具体的なWorker不存在説明 |
| `unset_subject_mcp_activation_denies_before_discovery_process_spawn` | 旧 `mcp` 文言assertと `FeatureLifecycle` の403認可拒否 |
| `failed_subject_spawn_releases_occupancy_only_after_runtime_proof` | string-only fixtureのspawn結果Unknownに対して旧Rejectedを要求するassert |

上記は共通Runtime/起動・認可系の既存残余として保持し、UnknownをRejectedに変えたり公開説明を旧provider文字列へ戻して合格させない。Journal capacity試験の別のbinding不一致はT-837の現bindingを使うfixtureへ修正し、`t845-composition-journal-binding-2` で元のcapacity・同reservation所有の保証が成功した。Service migrationの旧fresh期待差はT-845合成後に成功した。

`t845-normal-consumers-1`の登録3386件は1件失敗だった。生成前にcompileしたOpenAPIの `include_str!` が原因で、生成後の通常再build `t845-composition-build-2` と `t845-composition-openapi-recovery-2` で実一致を確認した。この通常runのsourceSame=falseはConfigTreeSnapshotの既存TS export-testによる整形差のみであり、Deno formatter適用で元tracked bytesへ戻ったことを確認した。旧binary結果とfresh artifact20の結果は別証拠として保持する。

正確なcommand、feature、toolchain/environment、source payload SHA、binary hash、exit/timeout、資源sampleは `target/t815-evidence/` の各preflight/resultとraw logに保存する。下記は同じ候補の重複しない総件数ではなく、段階別の証拠索引である。

- `grant-production-validation-20261106.md` は合成前のRuntime operation/persistence、Server cleanup/attachmentのproduction caller接続と失敗反復の記録。新target全体の合格に転用しない。
- `target-composition-validation-20261106.md` と `composition-migration-tests-2` / `generated-check-1` は最初のtarget合成・単一128・生成の固定source条件を記録する。
- `composition-normal-build-6` / `composition-normal-runtime-4` は71c合成前のfailed-create限定修正と通常Runtime/yoiの成功。新combined条件の証拠ではない。旧runtime1/2/3の失敗は別に残す。
- `composition-normal-build-8` / `composition-session-focus-1` / `composition-normal-runtime-5` は71c合成後のWorkdir/session/Runtime/tools/yoiを記録する。build7の初期化漏れcompile失敗は0 testであり、成功に読み替えない。後続Detach原因接続とmigration fixture更新はまだこの成功sourceに含まれない。
- `composition-server-shards-1` は通常Serverの一部。外側Bash600秒timeoutと内側shardのexit/timeoutは別事実で、全Server成功ではない。未完ケース、migrationの古いtail期待、既存termination/Embedded/Feature storage fixture等の失敗は個別にtriageし、baseline未確認をbaseline扱いしない。
- `composition-visual-review.json` は実Web＋合成HTTP/WS/WASMの4scenario限定目視。変更後38画像と変更前代表画像を実装者が開き、1440/768/390、light/dark、owner/non-owner、原因の即時可視性と既存狭幅overflow等を確認した。実Backend永続効果や新retained-close失敗画面の証明とは区別する。

### T-845合成後の通常suiteと再利用条件

`target/t815-evidence/t845-composition-server-unique-ledger-2.json` は通常Server登録2,208件・62 shardの各結果を原ログ/binary hashへ結ぶ。初回は2,153成功・14失敗・41 ignored、未回収0、内側test-command timeout 0。外側Bash group 2/8の600秒timeoutは別に保持する。内側の有限runnerが保存した全終端結果を後から回収したことで、外側timeoutを成功へ読み替えたものではない。

通常再build `t845-composition-build-3` のartifact21（Server lib SHA `f70aa2b9dc1a295ce5335c71f808022cf208e0dd3a8da1264e737cdbf4ddd1e6`）では、6件の旧期待を適応した。migration tailの2件は129を明記、Journalは現行versionを参照、future generationは既存preflight拒否と実versionを要求、Journal保存比較はT-845の空Plan table廃止とTicket 17の一段append以外の全旧列・値・receipt・applied_atを保持、assignment通知はPlan文言廃止後の現在Ticket/assignment/event再読指示を確認する。製品writer・権限・元結果・履歴は変更しない。`t845-migration-notice-recovery-1` はこの6件とServiceを含む7成功、60.48秒、exit 0。

Serviceのfactory leaseが初回に存在しなかった失敗は、隔離same-sourceで成功しただけでは原因確定としない。さらにartifact21の同じ36ケース・同一順のshardを `t845-service-identical-shard-recovery-1` で再実行し、36成功・104.65秒・exit 0を確認したが、初回失敗は原因未確定のflaky残余として残す。最新の非重複通常Server台帳は2,160成功・7失敗・41 ignoredであり、全greenではない。

artifact20から21までの差は上記6 test bodyのみで、production、共有harness/初期化、migration実装、Cargo.lock、feature、依存、登録case集合は不変である。変更試験とServiceの再実行以外はartifact20の結果を再利用し、artifact21で全62 shardを再実行したとは主張しない。`t845-normal-consumers-2` はartifact21の通常Workdir/Worker/Runtime/tools/CLI/config-source/API/client/Ticket全3,386件、`t845-drive-protocol-normal-1` はartifact22の通常Drive/protocol全162件がそれぞれ成功、ignored/失敗/timeout 0。`t845-drive-real-web-2` は別途opt-inの実SQLite/FeatureStorage/loopback Server→production Web Drive adapterで1成功（5.36秒）。通常suiteでignoredだったこの1件を通常集計から除去せず、追加選択として扱う。Runtime spawnはfixtureであり、画面E2Eではない。

### 7件の残余と受入条件の到達範囲

baseline一致だけを理由に免除せず、失敗点から確認できる範囲を分ける。

| 残余 | 到達した境界と本件との関係 | 未確認のまま残すもの |
| --- | --- | --- |
| private chat 2件・public messaging 1件 | wrong-assigneeのRuntime control probeが403となり、共通control観測のUnknownを保持した。Repository/Workdir/Drive/configのdomain callerではない | 文言assertがadapter内でpanicし、その後のchat/assignmentの実往復はtimeout。本件のdomain非再送テストで、この3件の後段を代替したとはしない |
| rejected orchestrator spawn | 起動拒否後diagnosticの旧provider断片文言assert。T-840/T-843/T-814起動/control境界であり、新domain理由の発生点ではない | この試験後半のretry/offline確認。本件を通すためprovider本文の再公開を戻さない |
| restored parent control | 列挙・input後の取消済みWorker照会が現在の具体的不存在説明を返し、旧文言assertが失敗 | 当該assert以降。Repository/Workdir生成型のconsumer確認を親子control全体の成功とはしない |
| unset Subject MCP activation | discovery開始前にFeatureLifecycleの403認可拒否 | 意図したMCP理由への到達。この認可拒否を回避してdomain試験へ進めない |
| failed Subject spawn occupancy | string-only startup fixtureがUnknownで、旧Rejected期待に不一致 | このfixtureでの後続occupancy/proof検証。UnknownをRejectedへ変えることは本件の受入条件に反する |

本件のHTTP/consumer/保存と復旧条件は、別の成功した登録試験で確認する。例として `configuration_http_problem_preserves_catalog_conflict_owner_visibility_and_durable_state`、`publication_http_connects_adoption_dispatch_projection_runtime_and_read_only_lookup`、`publication_registry_failure_preserves_committed_receipt_and_never_resends_on_lookup_or_replay`、`workdir_operation_errors_preserve_typed_public_status_and_code`、`accepted_stop_http_keeps_cleanup_failure_separate_and_does_not_retry_stop_or_close`、`abandoned_close_failure_preserves_session_cause_and_never_dispatches_directory_removal`、`external_grant_owned_unknown_survives_reopen_without_get_or_restart_redispatch`、`detach_compensation_preserves_recreated_same_alias_connection_and_original_session_cause` が全Server台帳で成功している。Web adapter/strict parser・表示は下記のWeb証拠と組み合わせ、上表の7件自体を成功とは扱わない。

`t845-composition-web-validation-1` は現合成sourceでWeb check 0 errors/warnings、864 unit・823 component成功、build成功、28.224秒、exit 0/sourceSame。目視は `t837-composition-visual-review.json` の38画像・6 sheetを実装者自身が再閲覧した。以後render/fixture/CSS/依存は不変、generated差分は6 TSの型宣言のみであり、この範囲で再利用する。実Backend永続効果、7件の後段、全domainのBrowser操作E2Eの証拠とはしない。`t845-composition-doc-doctest-1` は関連13 packageのdoc生成と、libraryを持つ12 packageのdoctest選択がexit 0/sourceSame。登録doctestは各0件であり、実行例の追加成功件数には含めない。

### 容量整理と残る判断

利用者の容量整理指示に対し、自Workdirの旧incrementalだけを実削除した。`cache-cleanup-20261107T0833Z/{plan,retained-before,result}.json` に全1,132対象（旧unit 1,043・旧session 89）と保持inventory/証拠hash照合を保存。targetは238,319,665,152→81,070,227,456 B、回収157,249,437,696 B、filesystem空き46,600,257,536→203,800,150,016 B。最新123 unitと全binary/依存/fingerprint/ログ/画像、source/未commit差分は保持・不変。全clean・他Workdir・共有環境・稼働processは操作していない。

`t845-composition-final-root-static-1` は最終6 test bodyと本記録を含むsnapshotでroot `cargo check --offline --locked -j2`、`cargo fmt --all -- --check`、`git diff --check HEAD`、状態契約命名検査が13.610秒・exit 0/sourceSame（596 matching、0 unapproved、0 stale）。既存dead-code/ts-rs warningは残る。公開provider exact、同MRの直属fresh全要件review/verdictは別の証拠として確認する。この記録は残余の免除、全要件合格、採用・配備・Ticket完了を主張しない。
