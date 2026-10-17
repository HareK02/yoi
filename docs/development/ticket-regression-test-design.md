# Ticketの接続回帰テスト設計

T-744の設計判断。公開入口で観測した値を次の操作へ渡せることを優先し、保存・配送の既存保証を一巡E2Eで再実装しない。一般則は[テスト設計](rust-testing-strategy.md)・[検証](validation.md)に従う。

## 対象sourceと判断の前提

- 調査・実行source: `main@develop` の `10d200d9ba8bc313bb7f72eabafc279c9def3cf5`、tree `546e6b517b1a82dd90f2e48f758b1364147dd6e3`。cleanな専用checkoutから `work/T-744-ticket-test-design` を作成した。
- 改稿基準: `hare/develop@adfa42297352c5a64fe8100969aad6fda4df8a81`。`git merge-base --is-ancestor 10d200d9 adfa4229` は成功。`git diff 10d200d9 adfa4229` は15 files、260追加/206削除。追加履歴は `b1808b7c` とmerge `adfa4229`。
- 差分は `builtin-source/src/lib.rs`、`manifest/src/profile.rs`、`merge-request/src/lib.rs`、Workerの `feature/builtin/merge_request.rs`・`prompt/{catalog,system}.rs`、Serverの `authority.rs`・`server.rs`・`server/tests/ticket_evidence_tests.rs`、resourcesの `flows/coder-review.dcdl`・`profiles/orchestrator.dcdl`・`prompts/common/merge-request.md`・`prompts/role/{coder,orchestrator,reviewer}.md`（Rustパスは `crates/` 起点）。主にclose後のMR follow-upでretained assignmentを使う変更、認可、Prompt簡素化。develop側のactive assignment条件と同一視しない。
- 以下の主証拠であるassignment_public、assignment tracking/delivery、implementation result、migration、Webのファイルは両sourceで同一。ただし共通 `server.rs` とauthorityの差分があるため、実行結果は上記developだけのもの。他branchの変更・最適化を取り込んでいない。
- 2026-10-17にShowTicketでT-744現行本文（digest `7c943b0d2e1ee7decffaa3f4414c519d929466e6dba8ba9dfaef8149e639982f`）、T-743/T-752〜755の現行証拠、T-761/T-725本文を確認。T-743の公開面、T-752の共通routing、T-753/755のMR・返却接続と診断は既存成果として再利用する。T-754のJob全体は本設計で再設計しない。T-761は本設計の優先順位・重複・コストを横断整理する。T-725はユーザー保留のまま、Drive共有を含めない。

以下 `WS/` = `crates/workspace-server/src/`、`WT/` = `crates/worker/src/feature/builtin/ticket/`、`WEB/` = `web/workspace/src/lib/workspace/`。テスト名は対象sourceの証拠を特定するためのもので、一般仕様一覧ではない。

## 採否の要約

| 優先 | 利用結果／具体的故障 | 判定と後続単位 |
| --- | --- | --- |
| P1 A | Repositoryなしで担当を読み、その観測IDで同じ委任へAnswer。readのID欠落・route差・投影差で次操作が拒否される | **新規が必要**。既存Server targetに通常Tools/WIPと実Backendの薄い適合結合。局所CAS/入力表は再作成しない |
| P1 B | 実装assignment/MRなしで結果返却。Hostのretry keyが実Backendで別操作になる／結果と通知sourceが分離する | **HTTP→保存→実sessionは既存で充足**。**Tools/WIP→同HTTPの適合だけ新規が必要**。Aとtransportを共有し、実Worker二重起動を避ける |
| P2 C | doneの結果、置換・case履歴を読み直してWebに表示。currentとpastの取り違え、parser拒否、旧結果の表示欠落 | **既存を強化＋薄いadapter結合が必要**。Server公開readの観測追加、Web履歴component assertion、実Backend bytes→production parserを分ける |
| P2 D | 旧schema移行・再open後もunknownを依頼にせず検索・読取りできる | **既存を強化**。migration→配送対象は既存充足、同DB→authority公開read/queryへの接続だけ補う |
| 維持 E | 保存→配送→永続session、ACK喪失・復旧・並行scanで重複しない | **既存で充足**。既存実Workerケースを維持し、recording backendを実session証拠へ格上げしない |
| 対象外 | 実LLMの判断品質、任意OS crash、Drive共有、close後のMR処理を両branchで統一 | **仕様確認／別scopeが先**。機構テストで自然言語判断・電源断耐性を主張しない。MR差分はT-761/Orchestratorへsource差として返す |

## 既存証拠と限界

| 証拠（file / test名） | 実際のassertion・本番区間と代替境界 |
| --- | --- |
| `WT/assignment_public/tests.rs` / `continuing_answer_forwards_same_assignment_event_cas_and_saved_receipt`、`public_assignment_rejections_and_unknown_outcomes_never_auto_retry_or_replace_identity`、`assignment_wip_binds_ticket_and_preserves_normal_permission_and_cas_inputs` | 同assignment/event/key/bodyの転送、保存receipt、403/409/unknownで自動retryなし、WIP route束縛と入力偽装拒否。**固定応答Transport**であり実router・storeのJSONとの適合ではない。`assignment_tracking.rs`のreadも同じ限界 |
| `WS/server_assignment_tracking_tests.rs` / `public_assignment_answer_authenticates_source_replays_saved_result_and_rejects_read_reassign_race` | 公開HTTPの認証、Answerイベント1件・認証actor・本文、tracking不変、exact replay、異なるpayload/stale/actor偽装の拒否。SQLite/routerは本番。readの観測値はhelperから取得し、通常Tool/WIPの出力を次操作に渡してはいない |
| 同file / `same_principal_reassignment_retains_old_result_and_projects_replacement_identity` | signed HTTPの旧assignment readで `current=false`、done/result保持、`release.action=replaced`・後継ID、次assignmentはwaiting・resultなし。fresh DB起点。履歴ページは `tracking_history_is_assignment_scoped_and_paged_and_rejects_unknown_cursors` が担当 |
| `WS/server_assignment_delivery_tests.rs` / `assignment_done_historical_inputs_do_not_restart_work_but_reissue_is_new_delegation` | typed close/reopen→scanで入力なし、done不変、明示reissueは新IDで入力1件、旧結果保持、旧Assign replayで再実行なし。**recording backend**の入力観測。公開detail/historyの再照合やDOMまでの証拠ではない |
| 同file / `assignment_real_worker_response_loss_restore_and_concurrent_scans_commit_once` | 実Worker受付後に応答だけを落とす。DB再open・同Worker復元・title/self-report変更後も同request ID/本文、同一通知commit1回、8並行scanの新Answerは1配送、tracking不変。`delivery_receipts` は永続sessionのactivation receipt、`delivery_commits` はcanonical notification。LLM・応答喪失だけ代替。Answer準備はstore直呼びなのでAを代替しない |
| `WS/server_implementation_result_tests.rs` / `implementation_result_signed_http_reaches_persistent_orchestrator_without_assignment_or_mr` | 実装assignment/MRなしのsigned HTTP→結果照会receipt一致→実Orchestrator session commit、exact replay、異payload/別actor衝突、偽MR拒否、公開診断runtime_accepted・処理確認false、再scanでreceipt増加なし、case/handling不変。Orchestrator委任はあり、LLMは固定。`mr_result_recovery_and_handoff_are_distinct_persistent_inputs` はMRイベントとhandoffを別入力として確認。通知未接続を改めて追加する必要なし |
| `WS/implementation_result_tests.rs` / `implementation_result_notification_failure_rolls_back_and_concurrent_retries_share_receipt`、`implementation_result_v97_to_v98_preserves_legacy_bytes_receipts_and_notification_evidence` | 実DBのsource insert失敗で結果0・業務状態不変、並行retry receipt共有。旧JSON bytes・ACK・failure・制約・rollbackを保持、再open照会/replayで通知再captureなし。Workerを使わない保存保証。移行fixtureは旧schemaを再構成したものなので、真の古いDB全体からの更新とは区別 |
| `WS/assignment_tracking_tests.rs` / `tracking_store_real_schema91_to92_retains_queue_cases_principals_audit_receipts_and_claims` | 実旧schema91 fixture→migration、pending/progressだけwaiting/inprogress、invalid-fence/closed-reopen/replaced/historical-doneはtrackingなし。原assignment・監査・receipt・claim保持、DB再open後の履歴一致 |
| `WS/assignment_delivery_tests.rs` / `assignment_delivery_schema92_upgrade_uses_reconciled_migration_evidence_not_legacy_history` | 同旧fixtureから最新storeへopen。配送対象はpending/progressのみ、input IDはmigration tracking由来、保存行同値・FK違反0。**migration→配送対象の不足という旧想定は採用しない** |
| `WS/store.rs` / `schema_v89_runs_auth_then_cases_preserving_legacy_states_evidence_and_responsibility`・`schema_v90_cases_preserve_auth_archive_legacy_states_evidence_and_responsibility`、`crates/ticket/tests/migration.rs` / `standalone_schema7_migration_preserves_six_state_snapshot_and_evidence_without_success_inference` | 旧done/closedをClosed+Unknownへ保存、旧snapshot/evidence・case履歴を保持、reopenで成功やactive assignment復活を推論しない。Workspace helperは再openも確認。公開query/Webとの接続とは別 |
| `WS/server/tests/ticket_evidence_tests.rs` / `case_route_preserves_corrected_history_replay_and_reopen_aba_guard_without_mr` | 公開detailのcurrent_closure/history/keyが保存値と一致、訂正で旧判断不変、reopen・replay・古いclosure keyのABA拒否。`WS/ticket_browse_tests.rs` / `ticket_browse_all_filters_are_applied_before_limit_and_batch_fields_match` はbrowse/detailのtracking・closure一致、`ticket_browse_assignment_current_missing_unknown_and_cleared_semantics` は過去doneを現在検索へ混ぜない。移行DB起点ではない |
| `WEB/api/ticket-operations.test.ts` / `waiting answers carry the same assignment and observed event without status or Assign mutation`、`retained assignment page preserves result and release rather than rewriting handling to done` | Answer inputの同一ID/event、done/null観測を拒否、releaseとhandlingの分離。`api/ticket-browser.test.ts` の `Ticket Browser keeps self-reported delegation independent of case and Worker state` と `tags named ready and closed coexist with case judgment and retained reopen history without inferring state` はparserの値保持。手書きAPI fixtureでありBackend適合ではない |
| `WEB/tickets/ticket-operations.browser.spec.ts` / `an answer response loss keeps exact assignment/event/key/body for retry even when observed responsibility changes`、`a done reissue persists before a failed refresh and does not pretend delivery or execution succeeded` | happy-dom上でfetchをfake、再render後もexact bytes維持、保存済み＋refresh失敗を配送成功と表示しない。`TicketHandling.browser.spec.ts` の `answer save keeps handling unchanged and refreshes history without diagnostics reads` はwaiting維持。**実HTTP/認証/Backend適合・実browserの証拠ではない** |

## 必要な変更の具体形

### A: 公開readの観測をそのままAnswerへ渡す（P1）

- **配置・最小fixture:** `WS/server/tests/ticket_public_roundtrip_tests.rs` を既存Server lib targetの子moduleにする。`delivery_recording_fixture`相当の一時SQLite・登録Runtime/Worker・署名identity、Repository/WorkdirなしTicket1件、Orchestrator waiting1件。共有支援は既存test module内へ置き、private helperのために製品APIを広げない。
- **入口:** 通常側は `Worker::new`＋NoLlm・空scope・一時FsStoreへ `TicketFeature` を `install_features` し、`engine().tool_server_handle().flush_pending()/get_tool` で実登録Toolを呼ぶ（構築は `server_workspace_config_wip_tests.rs`、Tool取得は `server/tests/worker_drive_tests.rs::native_tools` の既存形を再利用）。WIP側は公開 `mount_workspace_http_ticket_wip`＋`WipRuntime::from_mounts/invoke`。Queryの実出力からassignment_id/last_event_idを取り、Answerへ渡す。`/tickets/<ref>` のrouteはWIPが束縛し、別Ticket入力を注入しない。
- **本番区間/fake:** Tool/WIP投影→署名WorkspaceClient adapter→production router/middleware→実store。adapterは `worker_drive_tests.rs::RouterClient` と同様、毎要求を署名してrouterへ送り、固定JSONを返さない。Runtime実行側はrecording、LLMは未呼出し。sync clientは別threadでrouterを駆動し、Tokio runtime内の入れ子block_onを避ける。
- **faultと観測:** 実read応答の `last_event_id` 脱落、Answerのassignment ID改変、commit後の成功応答喪失を各境界で独立に注入。Toolの公開出力がDB projectionのID/eventに等しいこと、receiptのassignment ID、保存Ticketのassignment_answerイベント本文・認証actor・1件、tracking/委任行不変を再readでassertする。staleは409相当、unknownはエラーのまま、自動別key送信なし。同一操作を明示retryして同receipt/イベント1件。
- **自己通知と外部入力を分離:** 現在Orchestratorが自分でAnswerした場合、保存は成功しても自己通知抑止で新入力0が正しい。外部ユーザーAnswerの配送はEの既存実sessionケースへ任せ、ここに新Worker/sessionを足さない。UIからの外部AnswerはCの公開API適合対象。
- **期待検出:** IDが欠ければ観測一致assertionまたはAnswer入力検証が失敗する。DB helperの値で補完して成功させない。正常操作後のstale表や認証行列は既存APIテストへ委ねる。
- **実行・コスト:** 通常 `cargo test -p yoi-workspace-server --lib ticket_public_roundtrip`（提案filter）。WIP最小probeは実測2.37秒、通常Tools追加分は未測定で概ね既存DB fixture＋小Worker構築1回分。保持資源はtempdir/DB、課金・外部サービスなし。A/Bで同じtransport/署名/cleanupを所有し、既存巨大targetを増やさない。

### B: assignment/MR不要の返却ToolのBackend適合だけ足す（P1）

- **配置・準備/入口:** Aと同suiteに別test `implementation_result_tool_and_wip_keep_host_owned_retry_identity`（提案）。同じ実DB/署名adapter、実装assignmentもMRも持たない登録Worker1件。通知先のOrchestrator委任も不要な状態から開始する。通常ToolとWIP Invokeから `TicketImplementationResult` → Query。kind/bodyだけをモデル入力にし、operation keyは同一 `ToolExecutionContext` のsession/tool_call_idで固定する。
- **fault/読取り/assertion:** commit後responseを一度落として同じcontext＋全入力で明示retry。公開エラーがunknownを成功へ変換せず、実storeの結果・notification sourceが各1件、同result_id/receipt、Query本文一致、case/handling/assignments不変、sourceはno_recipientの抑止で後のAssignへ遡及配送されない。後者の局所保存規則は既存 `implementation_result_without_any_assignment_is_authenticated_and_never_retargeted` が実DB・再open・no_recipient/pendingなしを確認済みで、その表を再実装せず公開入口の代表だけ追加する。異contextは別報告、同contextの異bodyは衝突。Host key生成と実Backendのoperation key/actor bindingのずれを検出する。source insert失敗rollbackは既存store試験を再実装しない。
- **本番/fake・残る不足:** 通常Tools/WIP・Host key生成・署名router・実storeは本番、Runtimeはrecordingでよい。session到達は既存signed HTTP試験が担当。Tool試験だけから実session到達と称さず、両証拠を組にする。この追加は未試行。既存 `implementation_result_transport_key_uses_only_trusted_invocation_identity`・`implementation_result_wip_and_native_dispatch_preserve_trusted_call_context` は固定Transportの局所証拠として維持する。
- **実行/コスト:** 通常、Aと同target/filter。1 DB fixtureの数秒程度を推定、実Worker startup9秒級を再度増やさない。保守点はHost contextと本番認証bindingの更新に限定する。

### C: lifecycleの公開履歴とWeb表示をつなぐ（P2）

- **Server強化:** `assignment_done_historical_inputs_do_not_restart_work_but_reissue_is_new_delegation` に、typed close/reopen直後とreissue後のHTTP detail・retained assignment・historyページの再readを追加。元結果/更新ID・close判断履歴が保持され、currentは新ID/waiting、旧IDはdone/replacedであることをassert。既存scan入力なし/明示reissue1件のassertionも保持。fakeはrecording Runtimeだけ。case reopenでAssign/Worker起動は増えないことをstoreのassignment数と記録入力で照合する。新しい一巡testは不要。
- **Web局所強化:** `TicketHandling.browser.spec.ts` にcurrent waitingとpast done/replaced＋resultの最小2行、history2ページを追加。`Past assignment and release history`から旧assignmentを選び、Previous result・release actor/action・後継IDを表示、Moreでevent順を保持、current AnswerのIDを旧IDに替えないことをassert。faultは `TicketHandling.svelte` のretained result/release表示を外す、historyのcursor/assignment bindingを外す位置。`ticket-browser.test.ts`のclosure履歴値保持は再利用し、巨大snapshotを増やさない。このcomponentはAPI fakeの表示保証に限る。
- **実Backend適合:** 同Server suiteのresponse bytes（detail、assignments page、retained row、history、Answer receipt、case後のdetail）を改変せずTSへ渡し、`parseTicketDetail/parseAssignmentsPage/parseTrackingHistory/parseAnswerReceipt` とproduction `answerOperation`・`ticket-panel.ts::handlingSummary`を使う。両側で期待JSONを手書きしない。old/new result・release・closure historyがparser後も同じ意味、doneはAnswer不可、waitingは観測CASを生成するとassert。faultはBackend公開projectionの旧結果脱落/current誤り、parser側でresultを捨てる位置。公開HTTPの認可は既存signed/browser route testsへ委ねる。
- **配置/実行:** Server観測追加は通常lib、提案filter `assignment_done_historical_inputs`。componentは通常 `cd web/workspace && deno task test:component -- src/lib/workspace/tickets/TicketHandling.browser.spec.ts`。TS bridgeは `web/workspace/test/ticket-real-api/` とServer子module（提案 `ticket_web_roundtrip_tests`）へ、既存Driveのsubprocess管理方式のみ再利用しDrive資源を作らない。外部Denoが必要なので `cargo test -p yoi-workspace-server --lib ticket_web_roundtrip -- --ignored` の明示target/jobとする。恒久化時に実行選択を配線する。現時点でCI実行済みの主張はしない。
- **コスト/未確認:** parser bridgeの一時probeはwaiting1状態で0.137秒、Backend準備はAに相乗り。lifecycle全snapshot、historyページ、componentの新assertionは未試行。追加見込みはDB観測の数query＋Deno起動（概ね1秒以内を推定）、componentはrunner起動費用が別。実Backend→DOM全接続や実browserを確認した証拠ではない。まずparser適合と局所描画を分け、残るDOM接続を理由なく巨大E2Eで埋めない。

### D: 旧DBから公開read/queryへ出る代表を補う（P2）

- **配置/fixture/入口:** `assignment_delivery_schema92_upgrade_uses_reconciled_migration_evidence_not_legacy_history` の既存真のschema91 fixtureを再利用。最新へmigration/open後、storeをdropして再openし、同DBの `SqliteWorkspaceAuthority` から公開 `ticket/show_ticket`・`query_tickets` を読む。constructor/contextは `ticket_browse_tests.rs::Fixture` の形を共有する。実Workerや全旧状態×全画面行列は作らない。
- **最小状態/期待値:** pending=waiting、progress=inprogress、closed-reopenまたはhistorical-done=trackingなしを代表にする。公開rowのassignment_id/last_event_id/migration_diagnostic、case/legacy履歴がstoreと一致。queryのprincipal＋handling filterでwaitingとinprogressを区別し、unknownはどちらにもdoneにも該当せず、limit=1でも漏れない。現行の配送対象pending/progressだけというassertionを保持し、public read/query後も候補集合不変。旧case移行のClosed+Unknownはstore既存で充足、必要なら同helper末尾へ公開detailの一観測だけ追加する。
- **fault/読取り:** authorityのprojectionでtrackingなしをwaitingへ補う／queryが履歴doneを拾う／配送候補が旧履歴を再採用する位置。公開API型の値と同DBのstore read、pending候補を読取り元にする。fakeは旧保存入力だけ、migration/store/authority/queryは本番。queryと配送対象を互いの期待値生成元にせず、既存fixtureの意味に対して各々assertする。
- **実行/コスト:** 通常 `cargo test -p yoi-workspace-server --lib assignment_delivery_schema92_upgrade`。既存本体1.91秒、tracking migration2.04秒を実測。追加は同DB再openと少数query（秒未満〜数秒を推定）、旧DBをケースごとに再構築しない。強化部分の実行・mutationは未試行。typed close/reopen＋reissueの意味はC/Eが既に担い、このmigration fixtureで全一巡を追加しない。

## 代表試行・実測と残る未確認

2026-10-17、このcheckoutで `-j 2`、稼働Server/Runtime・外部モデルを使わず実行した。以下は単発のホスト測定で、CI・中央値・T-761のhare/develop測定ではない。

| 試行 | 結果／本体時間（外側buildは別） |
| --- | --- |
| 既存 `implementation_result_signed_http_reaches_persistent_orchestrator_without_assignment_or_mr` | 1 passed、9.61秒。初回外側build4分55秒、全コマンド5分05秒 |
| 既存 `assignment_real_worker_response_loss_restore_and_concurrent_scans_commit_once` | 実験復元後1 passed、14.58秒。再build1分27秒は実行時間に含めない |
| 既存 `assignment_done_historical_inputs_do_not_restart_work_but_reissue_is_new_delegation` | 1 passed、2.41秒 |
| 既存 `assignment_delivery_schema92_upgrade_uses_reconciled_migration_evidence_not_legacy_history` / `tracking_store_real_schema91_to92_retains_queue_cases_principals_audit_receipts_and_claims` | 各1 passed、1.91 / 2.04秒 |
| 既存 `same_principal_reassignment_retains_old_result_and_projects_replacement_identity` / `case_route_preserves_corrected_history_replay_and_reopen_aba_guard_without_mr` / `public_assignment_answer_authenticates_source_replays_saved_result_and_rejects_read_reassign_race` | 各1 passed、0.54 / 0.66 / 0.72秒 |
| `api/ticket-{browser,case,operations}.test.ts` | 32 passed、test合計147ms、Deno外側0.867秒 |
| Aの一時WIP probe | 本番mount→invoke→毎要求署名router→実SQLite。実readからID/eventをAnswerへ渡し、同receipt replay・保存イベント1件・waiting不変・自己入力0を確認。単独1 passed、2.37秒（外側build48.94秒） |
| Aのfault probe | 上記と同じ実router応答からlast_event_idだけをtransport境界で削除。正常1 passed＋fault側1 failed、3.49秒。`read[0].last_event_id == store.last_event_id` がNull対Stringで意図どおり失敗（コマンドexit101）。Backend自体のprojectionを書き換えたmutationではなく、公開応答脱落の境界faultである |
| Cの一時parser probe | 上記DBの実HTTP detail/pageとWIP Answer receiptをファイル経由でproduction Web parserへ渡し、case=open/waiting・ID一致、次Answerの観測CAS、event欠落時の操作拒否を確認。成功、Deno全体0.137秒。DOM/HTTP fetch adapter全体は未試行 |

一時probeは小fixtureで接続とassertionを試すためだけに作成した。初版の非公開install API呼出し・入れ子runtimeはfixture構築失敗、IDがTicket文字列を含むという誤期待と自己Answerの配送1件という誤期待はprobeの誤りとして修正した。いずれも製品不具合／回帰検出成功とは数えない。通常Tools側は公開Worker install/Tool取得を使う上記設計までで、未試行である。

試行用source/includeは復元し、最終tracked treeは文書だけ。ローカルの `target/t744-experiments/` にprobe source・Backendの合成fixture応答・ログ・一時include diffを保持するが、共有成果や維持対象fixtureとして参照しない。恒久テスト・製品改修・migration変更は含めない。再現時はAの手順で小probeを作り、実router応答の単一field脱落だけで失敗するかを確認する。

**未実行:** Aの通常Tools/response-loss/stale強化、BのTool/WIP実Backend結合、Cのlifecycle全履歴bridge/新DOM assertion、Dの公開query強化、Web component/browser新規実行、全通常suite、実モデル判断試行。既存ケースの成功をこれらの成功へ拡張しない。仕様確認が必要になった場合の確認先は、Answer/self-suppressionはT-742/T-752、返却の非依存・Host identityはT-753現行Tools契約、case/retained表示はT-739/T-741/T-743、移行unknownはT-739/T-741の保存契約。変更要求はT-744 Orchestratorへ返し、古いTicket本文のassignment必須記述を現行返却APIへ逆輸入しない。
