# 通常suiteの回帰テスト選択

## 判断の基準と対象source

**追加は公開境界の不足に絞り、既存の小さい故障検出と保存試験は維持する。** T-761の設計対象は `main@hare/develop` の `adfa42297352c5a64fe8100969aad6fda4df8a81`。着手時のHEAD・ローカルref・origin追跡refは同commit、treeは `4e85ca596e0f51f7eca4505c79c59518c87b6306`、基準との差分・未追跡変更はなかった。以下の採否・probe表はこのsourceでの設計時判断である。採用設計 `e75b027f39e9f307d1b527a92dfe79659fe8f921` を取り込んだ恒久実装・故障検証の結果は末尾へ分け、設計提出だけを完了とはしない。

[テスト設計規則](rust-testing-strategy.md)と[検証規則](validation.md)を適用する。[過去の時間調査](../report/test-suite-timing-investigation.md)の71.06秒・210 suitesは以前の1回測定であり、今回の実測ではない。60秒達成、追加速度最適化、全suiteの再分類は本設計の目的ではない。

## 優先順位と採否

| 優先       | 利用者の操作と困る故障                                                     | 判定・後続単位                                                                                                                                                        |
| ---------- | -------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P1         | Ticketを読むWorkerが、返された観測IDで同じ委任へ回答できない               | **A：薄い適合結合が必要、T-744所有**。通常Tools/WIPの実read→Answer。WIP probe成功・観測field脱落検出、通常Toolsは未試行。                                             |
| P1         | 結果返却Tool/WIPがHost retry identityを落とし実Backendへ同じ依頼を渡せない | **B：薄い適合だけ追加候補、T-744所有**。HTTP→実永続sessionは既存で充足。Tools/WIP適合は未試行。                                                                       |
| P1 維持    | 長いWorkspace操作を待つWorkerのHTTPが暗黙の30秒で切れる                    | **既存で充足**。timeoutの本番適用漏れを今回再現検出。31秒待ちを復活させない。                                                                                         |
| P1 維持    | 人間・Workerが保存済み履歴を読むと、budget中断後の行やoffsetが壊れる       | **既存で充足**。readerの中断状態喪失を今回検出。画像実storage接続は別の既存試験を維持。                                                                               |
| P2         | 親Runtime Workerの復元後、既知の子が一覧から消える／入力できない           | **恒久追加済み**。復元親のclientから公開list→input→canonical Sessionを接続する1ケース。実接続でsubjectの`kind`脱落を検出し修正。                                      |
| P2         | Ticketのlifecycle履歴を公開readとWebで正しく保持・解釈できない             | **C：分離して強化、T-744所有**。detail/history再read、局所履歴component、実Backend bytes→production parserを分担。waiting parser probe成功、全lifecycle/DOMは未試行。 |
| P2         | 真の旧schemaから移行・再open後にTicketを公開read/queryできない             | **D：公開read/queryだけ強化、T-744所有**。migration→配送対象は既存で充足。Dの追加部分は未試行。                                                                       |
| P2 維持    | 開発者が成功suiteを信じたが一部ケースを実行していない／取消後の子が残る    | **選んだ規則は既存で充足**。runnerのargv脱落を今回検出。結果件数の総合照合まで自動保証するとは主張しない。                                                            |
| P2 維持    | DB fixture共有により他ケースのseedが混入、再openで保存値や制約が消える     | **既存で充足**。fixture所有の隔離・実再読取り試験を維持し、全APIケースで同じ確認を繰り返さない。                                                                      |
| 保留       | RepositoryなしDrive資料の共有・更新を別入口から使えない                    | **T-725の既存証拠を再利用、不足は未確認**。ユーザー保留中であり本文改稿は再開指示ではない。本件で追加設計・起動しない。                                               |
| 仕様確認先 | 親を復元すればInternal SubWorkerも再出現すると期待する                     | **実装未対応**。現実装はInternal子を復元しない。成功テストの不足として扱わず、復元機能が必要か利用者・spawn所有者へ確認。                                             |

## 既存の検出範囲と追加不要の根拠

### timeout：helperではなく本番呼出しを守る

`crates/worker-runtime/src/workspace_request.rs::tests::blocking_workspace_client_clears_initial_request_timeout` は短い初期deadlineを持つbuilderから本番 `execute_blocking_inner` → `prepare` → `.timeout(request.timeout)` → 実loopback HTTP → body読取りを通す。遠隔Workspaceだけがfixtureで、Backend認証・保存は範囲外。要求headersの到着を同期点とし20msのdeadlineを跨いで応答、5秒watchdog内でstatus OKとbody bytes一致をassertする。時間経過は故障刺激であり完了推測ではない。

`worker_source.rs` の `runtime_owned_workspace_client_returns_successful_response_without_request_timeout` は本番wrapperからstatus/bodyと既定None、`runtime_owned_workspace_client_honors_per_request_timeouts` / `runtime_owned_workspace_client_reports_request_timeouts` は公開エラーのpathと`timed out`をassertしserver threadをjoinする。これらとdeadline適用試験は重複ではなく、wrapperと有効な待機方針の接続を分担する。新規fixture・追加コストは不要。

### reader：小さい境界行列と実画像接続を分担する

`crates/session-store/src/worker_session_store.rs::tests` の実tempfileから本番 `RetainedSegmentReader::previous_record` を呼ぶ4ケースが主たる境界試験で、DB・Runtime・モデルはない。

- `retained_reader_preserves_records_and_offsets_across_chunk_boundaries`：chunk±1等、逆順recordのstart/end、serialized entryと元bytes一致、総read量一致。
- `retained_reader_resumes_after_budget_exhaustion_and_serves_buffered_records`：budget/0で拒否後に再開、元entry bytes・offset一致、buffer内recordは追加read 0。
- `retained_reader_requires_the_prior_boundary_byte_at_exact_chunk_edges`：直前newline不足で拒否、1 byte追加で成功。
- `retained_reader_rejects_empty_malformed_and_unterminated_records`：破損入力拒否。

`public_snapshot.rs::tests::large_legacy_inline_images_project_as_bounded_refs_and_fetch_from_retained_storage` は実canonical保存→`read_retained_session_snapshot`→同reader→attachment再読取りを通す。snapshotが16KiB未満・base64を含まない、live/retained参照一致、元画像ごとの長さ・全bytes一致、不正session/ID拒否をassertする。HTTP/UIは通らない。小さいreader試験で画像の保存接続を代替せず、巨大画像に境界全行列も複製しない。今回の試行は内容の正しさの証拠であり、二次量処理が再導入されない性能保証ではない。

### DB準備：保存の保証をfixtureの速さと混同しない

`crates/workspace-server/src/store_test_support.rs` は実SQLite backup・独立connection・本番SQLite設定を使い、外部providerを使わない。

- `api_fixture_databases_isolate_writes_and_reopen_persisted_rows`：並列DBのdisplay_nameを実再open後に再読取り、FK/WAL、第三のpristine DBのaccount数0をassert、threadをjoin。
- `prepared_fixture_connections_observe_only_committed_writes_and_keep_foreign_keys`：別connectionからcommit前0・後1、WALと不正FK拒否。
- `api_fixture_reopen_rejects_schema_drift_instead_of_replacing_the_database`：triggerを落として再openが名前付きエラー、実sqlite_schemaでもtrigger不在。既存DBを空templateで上書きする故障を検出するassertionである（今回mutation未試行）。

`store.rs::tests::retained_fixture_copies_isolate_seed_rows_and_keep_connection_constraints` は旧schemaの空構造だけの共有を確認する。これらは本番migration全体・公開操作の証拠ではない。Ticket移行→公開readの不足はT-744へ集約する。通常API全体への制約assertion再追加は不要。

### runner：選択の帳簿と実行引数を照合する

`tools/test.test.ts` の `round-robin shards retain every listed name exactly once at thresholds and the 32-shard cap` は本番parser/sharderへ実形式のlistingを入力し、全名の一意分配と `task.args.slice(3) === selectedNames`、exact/thread/ignore flagsをassertする。`runtime listing persists complete name inventory for large and small binaries` はDeno子でlibtest listingを代替し、本番discovery/executeの保存inventoryを再読取りする。実Rust libtest全体の実行結果照合ではない。

`execute preserves live stdout/stderr and times out a group containing a grandchild`、`execute escalates SIGTERM to SIGKILL and reaps a stubborn direct child`、`parent SIGINT/SIGTERM cleans up active groups, reaps children and does not spawn queued commands` は実隔離子プロセスを起動し、PID消滅・保存result・非起動のqueued PIDを観測する。fixtureに独立watchdogがあり、finallyでも回収する。同process group外へ自ら離脱した子孫は対象外。局所入力表を重いrunner全体起動へ移さない。

## 新規の最小変更単位：復元Runtime親からの公開子操作

**追加するのはRuntime Workerの接続試験1つであり、Internal子の復元ではない。** 主な不足は次の既存証拠の間にある。

| 実在test（path略記）                                                                                                        | 入口・本番区間・読取元／限界                                                                                                                                                                                                          |
| --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `store.rs::tests::worker_control_grants_are_idempotent_scoped_and_support_internal_invalidation`                            | 親子registryとgrantを直接seed→実DB再open→`list_active_worker_control_grants == vec![grant]`、replay/revoke/cascade。公開list、実行、親復元は未通過。                                                                                  |
| `worker-runtime/src/worker_backend.rs::tests::restart_restore_reconstructs_runtime_owned_worker_mutation_client`            | `worker_context`を2回構築しclient kindとworkspace IDをassert。実`restore_controller`もHTTPも通らない。                                                                                                                                |
| `workspace-server/src/server.rs::tests::production_profile_backend_restores_zero_input_orchestrator`                        | 本番profileのstop→restore、同identityとonlineをassert。同一API内、子・grant・復元後公開操作なし。                                                                                                                                     |
| `worker/src/feature/builtin/manage_worker.rs::tests::control_service_projects_runtime_subject_and_deduplicates_worker_list` | 本番serviceに`RecordingWorkspaceClient`、重複subjectの投影をassert。設計時は`control_service_deduplicates_worker_list_by_stable_subject`で、fixtureの応答に既に`kind`があった。恒久実装では実Backend形へ補正。実grant・署名検証なし。 |

本番接続根拠は `ProfileRuntimeWorkerFactory::restore_controller` の `worker_context` 再構築→`Worker::restore_from_worker_metadata_with_context`→返す `RuntimeWorkerController.workspace_client`、Serverの `list_known_workers`（認証親→grant→Runtime summary）と `send_known_worker_input`（grant付きcontrol handle→子入力）である。既存 `worker_control_handle_rechecks_revoked_grants_for_each_operation` はhandle直呼びの失効拒否、`worker_control_input_is_idle_only_and_returns_rejection_as_http_error` はfixture Runtimeへのinput種別を守るが、この復元接続は守らない。

- **配置・最小fixture**：`crates/workspace-server/src/server/tests/restored_parent_control_tests.rs` の通常lib結合に `restored_runtime_parent_lists_and_inputs_existing_child` を追加した。既存production restore/永続session支援を再利用し、隔離temp DB・Runtime FS store・loopback router・親P/子Cだけ。Ticket、Git操作、Workdir、Subjektiv、実モデル課金を持ち込まない。P/Cのregistryとcontrol grantは復旧前だけ作成し、再open後は再seedしない。子作成の冪等性は既存spawn試験に任せる。
- **操作入力・入口**：親Pを停止してshutdown/join完了後に旧store/APIを解放、同DB/FSを再openし本番factoryでPをrestore。復元controller自身の`workspace_client`（公開field）を既存の公開 `manage_worker_feature(...)` へ渡し、公開Feature installを通して登録したWorkerList／WorkerSendInput Toolで操作する。内部の `WorkspaceWorkerControlService` はprivateのまま使い、Server testから直接構築したり公開範囲を拡大したりしない。list応答の完全subjectをinputの対象へ渡す。外からPを名乗る署名requestを作るだけでは代替しない。HTTP構成を選び `build_router` の外側署名認証も通す。embedded dispatcherだけの成功でHTTP署名を主張しない。
- **観測・落ちるassertion**：listにCのruntime/worker IDと権限が一致、grant ID/件数不変。IdleとなったCへ一意な本文markerを`UserIfIdle`として1回送信し、acceptedだけでなくCのcanonical sessionを読みmarkerが1件であることをassertする。CのIdleは観測イベントで同期し初期IdleをSubmit完了と誤認しない。LLM判断は範囲外。
- **fault位置**：復元時の`worker_context`からsigner/Workspace接続を落とす一時mutationではlist成功/subject一致が失敗すること。grantを復旧後にrevokeする境界faultでは次のinputが`UnknownWorker`相当の公開拒否となり、Cのmarker件数不変であること。前者は接続の回帰、後者は権限の再判定を確認する。全API操作の失効行列やC自身のrestoreは既存の別規則へ任せる。
- **同期・回収**：`confirm_shutdown_and_join`相当の既存完了通知とtask joinを使い、loopback serverもfinally/RAIIで停止・join。bounded watchdogを設け、固定sleep、`drop(api)`だけ、稼働Runtimeの借用をしない。
- **target/filter・区分**：`cargo test -j 2 -p yoi-workspace-server --lib restored_runtime_parent_lists_and_inputs_existing_child -- --test-threads=1`（恒久実装の実行filter）。通常runner対象。DB/socketだけを理由にopt-inにしない。suiteを増設せず既存libに収める。
- **設計時のコスト見積り**：fixture接続を含め1–2人日、追加本体は秒〜十数秒規模を予算とした。保守費はfactory/認証/回収の結合fixture追従が中心で、純粋unitより高い。追加不要判定の各項目は実行時間・fixture保守とも増分0である。組立てと課金なしの子入力完了支援は当時未試行だった。実装ではspawn用LLM hookがrestoreには効かないことを確認したうえで、公開APIやtest支援を拡張せず実現した。最終費用・制限は末尾を参照する。client署名だけの別unitを追加して同じ保証を二重計上しない。

Internal子は `worker/src/spawn/registry.rs::load_from_worker_state_with_reclaim` が空registryを生成する。`crates/worker/tests/internal_subworker_restore_test.rs::restore_reclaims_and_clears_legacy_process_children` は親scope回収、`spawned_children`空・reclaimed記録・projection `[]`をassertする。これは旧子の復活成功ではなく、その逆の保証である。

## Ticket / Driveとの重複・コストの扱い

T-744最終handoff（result `01a147f8-cde5-7cf2-8900-d6e2a44240bf`）の全文をtyped queryで確認し、上表のA〜Dを採否・コスト判断に用いる。handoff時の設計sourceは `eeca864b23d7b476db829da60fff067830d33667`。Orchestratorがfixture factoryの代替境界などを補正した採用版は `2e9e8bc0729c595e48f5c6c7c74aff1eeb607f46`、branch `work/T-744-ticket-test-design` の `docs/development/ticket-regression-test-design.md`、baseは `develop@10d200d9ba8bc313bb7f72eabafc279c9def3cf5`。T-761基準との差15 filesはMR follow-up/Prompt中心で主要Ticket testは同一との報告。T-744文書はこのtreeへ未採用なので相対リンクを作らず、source/pathで参照する。以下は他Workerの最終報告による実績であり、こちらでの再測定・再設計ではない。

| 所有設計 | 最終採否と検出実績                                                                                                                                                                                                                                                                           | 横断の費用判断・未確認                                                                                                                                                                                                                                                                   |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A / P1   | 通常Tools/WIPの実read観測ID→同一Answerの薄い適合結合が必要。一時WIP mount→署名router/実SQLiteのprobeで保存イベント1件・同receipt replay・waiting不変を確認。実read応答の`last_event_id`だけの脱落でNull対String assertionが失敗。                                                            | 単独probe2.37秒、field脱落の正常1 pass/fault1 failは3.49秒の共有値。通常Tools側・障害表は未試行で、この値を恒久追加コストとは断定しない。自己Answerの通知抑止を尊重し、外部Answer配送は既存証拠へ分離。                                                                                  |
| B / P1   | Host retry identityを含む返却Tool/WIP→実Backendの薄い適合だけ追加候補。assignment/MR不要のHTTP→本番store→実永続Orchestrator session・再送は `server_implementation_result_tests.rs::implementation_result_signed_http_reaches_persistent_orchestrator_without_assignment_or_mr` で既存充足。 | B適合は未試行。既存signed HTTP→sessionは9.61秒。重い接続の再起動を追加せず、adapter境界だけに費用を限定する。                                                                                                                                                                            |
| C / P2   | 既存lifecycleへの公開detail/history再read、Web局所履歴componentのassertion、実Backend bytes→production parser適合を分離して強化。                                                                                                                                                            | 実Backend detail/page/receipt→production parser/answerOperationのwaiting probe0.137秒。既存lifecycle2.41秒、Web parser/operation/case 32 testsは本体147ms/外側0.867秒。全lifecycle bridge・新component/DOMは未試行。parser成功を画面保証へ広げず、既存fixtureと局所componentを分担する。 |
| D / P2   | 真の旧schema→移行・再open→公開read/queryだけを強化。migration→配送対象、ACK喪失/復旧/並行scan→実sessionは既存で充足。                                                                                                                                                                        | Dの公開query追加は未試行。既存migration2件は1.91/2.04秒、real Worker response loss/restore/concurrent scansは14.58秒。移行行列ごとに実Worker配送を繰り返すE2Eは追加不要。                                                                                                                |

T-744では既存Rust代表8件が成功（上表以外の公開reissue/case/Answerは0.54/0.66/0.72秒）。初回jobs 2の外側build4分55秒は実行時間と分離されている。これらは追加テストの増分見積りではなく、既存fixture再利用の費用判断に使う。具体的配置・fault・assertion・同期回収・通常/明示実行・保守費は上記source/pathの最終設計を参照し、T-761で同じケースを作り直さない。初版probeの非公開API/runtime/ID形式/自己配送の誤期待はfixture誤りの修正であり、製品バグ検出とは数えない。Aのfield脱落検出とCのparser成功も、未試行のB適合・C全履歴・D公開queryを充足した証拠にはしない。

T-725現行本文・保留trackingを読んだ。`workspace-drive/tests/storage.rs::receipts_survive_response_loss_and_restart_without_double_mutation` は実DB/blob再open後のreceipt一致、同request再送一致、別actor競合拒否、list件数1をassertする。`workspace-server/src/server/tests/worker_drive_tests.rs::lost_mutation_response_is_resolved_by_receipt_without_resending_effect` は本番DriveFeature→署名router/storeを通し、receipt committed・POST mutate数1・別Worker listのlast mutation一致をassertするがRuntime起動はfixtureである。これらの保存/応答喪失規則を再設計しない。実Web adapter接続は `drive_web_roundtrip_tests.rs::repositoryless_drive_real_web_adapter_preserves_bytes_cas_and_latest_reference_lifecycle` の既存ignore/明示実行であり、通常suiteの成功へ含めない。Worker寿命・Web画面までの不足と必要コストはT-725再開時の判断で、今回未確認とする。

**2026-10-19、T-725再開後の候補実装**は[Drive接続の調査・検証記録](../report/t725-drive-connections.md)へ分離した。通常Server libにuser upload→WorkdirなしTools/WIP→owner/別Workerの同一node・mutation・bytes・receipt、実Runtime Worker停止/削除→明示grant付き交換Worker、実Runtime HTTP経由のsource Workdir削除→Drive資料保持の3件を追加した。保存/応答喪失/GC/backupは既存を維持し、Ticket一巡も複製しない。3件の単独libtest実測合計5.54秒（Cargo wall合計6.324秒、build温存時）であり、通常runner全体の増分・並列負荷は未測定。代表的な本文投影欠落とcleanupによる誤ったDrive blob削除のmutantを検出し、復元後Server全crateを成功確認した。既存ignored Web adapterも別途1件成功したがbrowser/実モデル/全Workspace runner/CIの成功には含めない。上の保留記述はT-761設計時の観測であり、この候補を対象developへ採用・統合したという記録ではない。

## 設計時の代表試行と残った検証

全て専用checkoutで逐次実行、Rust buildはjobs 2。表の本体はlibtest/Deno表示、外側は`time -p`の1回測定であり、通常suite全体の増分やCI中央値ではない。

| 試行（同じtarget/filterでbaseline→mutation→復元）                                                                                                   | 具体的なfaultと結果                                                                                                                        | 今回の本体／外側実測                                                                  |
| --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| `cargo test -j 2 -p session-store --lib retained_reader_ -- --test-threads=1`                                                                       | 本番budget拒否直前で`record_suffix.clear()`。再開unwrapがCorrupt、境界1 byte再開がReadLimitExceededで2 failed / exit 101。復元後4 passed。 | baseline 0.07秒／34.02秒（build 33.91秒）、fault 0.05秒／3.10秒、復元0.06秒／2.55秒   |
| `cargo test -j 2 -p worker-runtime --lib blocking_workspace_client_clears_initial_request_timeout -- --test-threads=1`                              | 本番timeout適用をSomeだけに変更。`request ended before response`でtimeout=true、1 failed / exit 101。復元後1 passed。                      | baseline 0.06秒／167.85秒（build約167秒）、fault 0.04秒／19.63秒、復元0.06秒／18.25秒 |
| `deno task test:runner`、fault時のみ`deno test --allow-read --allow-write --allow-run --allow-env tools/test.test.ts --filter 'round-robin shards'` | 保存selectedNamesは変えず実argvの先頭名を落とす。`task.args.slice(3)`一致assertが失敗 / exit 1。復元後全17 passed。                        | baseline全体7秒／8.11秒、fault単一12ms／0.63秒、復元全体7秒／7.49秒                   |

追加で `cargo test -j 2 -p worker-runtime --lib runtime_owned_workspace_client_ -- --test-threads=1` は3 passed（本体0.20秒／外側0.42秒）。実験patch・stdout・stderr・時間は当時のGit管理外 `target/regression-design/` の `reader-*`、`timeout-*`、`runner-*` へ分離し、3 sourceのdiffが空であることを復元後に確認した。当時の恒久変更は本設計書だけだった。

設計時には新規の復元親接続、DB fixture suite、画像巨大入力、Ticket/Drive、全通常suite、E2E/browserを実行していなかった。以下は再開後の実装結果であり、これら過去のprobeや71.06秒を再測定したものではない。

## 恒久実装の結果と費用

**本番restore接続を1ケース追加し、実接続で見つかったsubject変換の不具合を修正した。** 同じ `adfa4229` 基準の専用checkoutへ採用設計の3コミットを順に取り込み、設計sourceとの差分が文書1ファイルのみであることを確認した。製品修正は `2f0e0b28`、恒久テストは `46df60cb` に分離している。

### 実現した境界

- 実SQLite・Runtime FS・literal loopbackのouter `build_router` を使う。実Runtime backend/factoryでP/Cを作成し、registryとcontrol grantは復旧前のみ作る。Pの最初のSubmitでSessionを保存し、`stop_worker` のshutdown完了・execution task join後、server taskをabort/join、Feature storageをshutdown、旧storeのWeakがupgrade不能であることをassertしてから同DB/FSを再openする。
- Runtimeの明示restore admissionから本番 `ProfileRuntimeWorkerFactory::restore_controller` でPを復元する。Cのexecution backendは生存させ、factoryで復元したcontrollerはPだけであることをassertする。C自身やInternal子の復元は保証しない。
- 復元controllerの公開fieldから得たclientを無変更で `manage_worker_feature` に渡す。公開 `Worker::install_features` を使う非controllerのTool hostで登録Toolを取り出して実行する。このhostはRuntime registryの第三のWorkerではなく、署名主体も作り直さない。private serviceの直構築や公開APIの拡張はない。
- outer署名認証→実grant→Runtime summary/inputを通し、完全なtagged subject、CのID、権限とgrantの全record一致をassertする。listで得たsubjectを補正せずTool inputへ渡す。入力前にCのイベントを購読し、`SubmissionAccepted` のID→`RunEnd::Finished`→同IDのfinished fenceを持つIdleで同期する。canonical Sessionのtyped `UserInput` entryにmarkerが正確に1件あることを実保存readerで確認する。初期Idle、acceptedのみ、JSON内の文字列出現回数では代替しない。
- 外部モデルだけを固定する既存spawn hookを使う。restoreにこのhookは効かない。Pは保存済みの認証不要モデル設定で復元し、復元後にモデルturnは発生しない。Cの1回のturnはspawn時の固定clientを使う。factory/restore/Workspace signerのfixture置換も、restore用の試験支援追加もない。45秒watchdogとpanic/timeoutを捕捉するfinally経路でcontroller stop/join・server task回収・storage shutdownを行う。

### 検出した不具合と代表fault

| 状態                    | 具体的観測                                                                                                                                                                                             | 本体／build（各1回、秒） |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------ |
| 修正前の実接続          | listのsubjectが`runtime_id/worker_id`だけで、期待する`kind: runtime_worker`付きsubjectと不一致。次Toolのschemaへ渡せない。既存unit fixtureが最初からkindを含めていたため未検出だった                   | 10.12／81                |
| 製品修正後              | Backend subject DTOをTool用tagged subjectへ投影し、既存unit fixtureも実Backend形に補正。Backend schemaは不変。復元親→list→input→canonical marker 1件→revoke後の拒否・Session不変が成功                 | 7.78／56.58              |
| restore接続脱落mutation | restore側`worker_context`だけ、`workspace_request_client`引数を`None`へ変更。outer signerがないfallbackから実loopbackへ要求し、`WorkerList from restored parent failed`、HTTP 401で1 failed / exit 101 | 6.14／64                 |
| mutation復元後          | 同filterで1 passed。`worker_backend.rs`のdiffが空であることを確認                                                                                                                                      | 7.78／56.58              |

除去する位置の選定時に、remove専用`worker_mutation_identity`だけを落とした試行は成功（6.53秒）した。この経路のList/Inputはouter Workspace request署名を使い、remove専用forwarderを使わないためであり、その試行を故障検出の証拠に数えない。最初のWorkspace接続脱落では不正base URLで止まったため、fixtureの保存API URL・署名audienceを実loopbackへ揃えてHTTP 401まで確認し直した。patch/各ログはGit管理外 `target/restored-parent/` に保持する。grant revokeは恒久ケース内の境界faultで、次入力の`unknown worker`公開拒否とCのSession snapshot全体不変を毎回assertする。

### 通常選択・検証・未確認

- `cargo test -j 2 -p worker -p yoi-workspace-server`：**2,370 passed / 2既存ignored / 0 failed**、doctestも成功（0件）。外側290秒のうちbuild約103秒、Worker lib21.94秒・Server lib155.38秒。これは通常runner全体の増分や過去の全suiteとの比較ではない。
- root `cargo check -j 2` 成功（96秒）、`yoi-runtime` と `yoi-server` の製品entry binary build成功（137秒）。fmt/diff・状態契約命名検査も成功。実モデル・稼働更新はない。
- 通常runnerが使うCargo test artifacts→`rustTestCommands`→実libtest `--list`→`discoverRustCommands`/sharderを実行し、新ケースが実argvに正確に1回含まれることを確認した。選ばれたServer libの通常shard（38件）を本番`execute`で実行し成功、20.30秒。新しいtarget・ignore・test用featureは追加していない。
- 新ケース単独の最終本体は7.78秒、推定予算内。単発・温まった依存artifactの値でありCI中央値ではない。保守費は実factory/署名/Session/回収のfixture追従で、保存schema・製品APIの追加費用はない。初回test buildは164秒、故障試行などのrebuildは51–136秒だった。追加速度最適化は行っていない。
- 通常CI workflow/jobはこのtreeで確認できず、Nixの`doCheck = false`も不変。ローカルの通常選択成功をCI実行済みとはしない。全Workspace通常runner、browser/E2E、実モデルは今回未実行。merge/closeは本報告の証明範囲外である。
- T-744のA〜Dは別Workerが実装中であり、上記表は設計時の最終handoffに基づく値を保持している。今回のtyped再読でも新しい恒久実装の最終費用・結果はまだ返っていなかったため、実装成功や最終コストへ読み替えない。採用統合時にはT-744の公開source・最終結果を同表と照合する。同じ接続ケースの再設計・重複実装、T-725保留解除、T-737変更は行わない。
