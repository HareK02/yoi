# 通常suiteの回帰テスト選択

## 判断の基準と対象source

**追加は公開境界の不足に絞り、既存の小さい故障検出と保存試験は維持する。** T-761の設計対象は `main@hare/develop` の `adfa42297352c5a64fe8100969aad6fda4df8a81`。着手時のHEAD・ローカルref・origin追跡refは同commit、treeは `4e85ca596e0f51f7eca4505c79c59518c87b6306`、基準との差分・未追跡変更はなかった。以下はこのsourceでの判断であり、実装済みの保証と追加案を分ける。

[テスト設計規則](rust-testing-strategy.md)と[検証規則](validation.md)を適用する。[過去の時間調査](../report/test-suite-timing-investigation.md)の71.06秒・210 suitesは以前の1回測定であり、今回の実測ではない。60秒達成、追加速度最適化、全suiteの再分類は本設計の目的ではない。

## 優先順位と採否

| 優先       | 利用者の操作と困る故障                                                  | 判定・後続単位                                                                                                                              |
| ---------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| P1         | Ticketを読むWorkerが、返された観測IDで同じ委任へ回答できない            | **新規接続が必要な候補、T-744所有**。Tools/WIPとsigned router/storeの薄い適合を優先。こちらに同じ一巡suiteを作らない。                      |
| P1 維持    | 長いWorkspace操作を待つWorkerのHTTPが暗黙の30秒で切れる                 | **既存で充足**。timeoutの本番適用漏れを今回再現検出。31秒待ちを復活させない。                                                               |
| P1 維持    | 人間・Workerが保存済み履歴を読むと、budget中断後の行やoffsetが壊れる    | **既存で充足**。readerの中断状態喪失を今回検出。画像実storage接続は別の既存試験を維持。                                                     |
| P2         | 親Runtime Workerの復元後、既知の子が一覧から消える／入力できない        | **新規が必要（証拠不足）**。DB再openとclient型確認の間を、復元親のclientから公開list→inputまで接続する1ケース。既知の製品回帰と断定しない。 |
| P2         | Ticketの実read結果がWeb parserと不適合／移行後の履歴が配送対象へ化ける  | **既存強化・新規適合の候補、T-744所有**。個別案・fault結果は同Ticket成果を採用する。                                                        |
| P2 維持    | 開発者が成功suiteを信じたが一部ケースを実行していない／取消後の子が残る | **選んだ規則は既存で充足**。runnerのargv脱落を今回検出。結果件数の総合照合まで自動保証するとは主張しない。                                  |
| P2 維持    | DB fixture共有により他ケースのseedが混入、再openで保存値や制約が消える  | **既存で充足**。fixture所有の隔離・実再読取り試験を維持し、全APIケースで同じ確認を繰り返さない。                                            |
| 保留       | RepositoryなしDrive資料の共有・更新を別入口から使えない                 | **T-725の既存証拠を再利用、不足は未確認**。ユーザー保留中であり本文改稿は再開指示ではない。本件で追加設計・起動しない。                     |
| 仕様確認先 | 親を復元すればInternal SubWorkerも再出現すると期待する                  | **実装未対応**。現実装はInternal子を復元しない。成功テストの不足として扱わず、復元機能が必要か利用者・spawn所有者へ確認。                   |

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

| 実在test（path略記）                                                                                             | 入口・本番区間・読取元／限界                                                                                                                         |
| ---------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `store.rs::tests::worker_control_grants_are_idempotent_scoped_and_support_internal_invalidation`                 | 親子registryとgrantを直接seed→実DB再open→`list_active_worker_control_grants == vec![grant]`、replay/revoke/cascade。公開list、実行、親復元は未通過。 |
| `worker-runtime/src/worker_backend.rs::tests::restart_restore_reconstructs_runtime_owned_worker_mutation_client` | `worker_context`を2回構築しclient kindとworkspace IDをassert。実`restore_controller`もHTTPも通らない。                                               |
| `workspace-server/src/server.rs::tests::production_profile_backend_restores_zero_input_orchestrator`             | 本番profileのstop→restore、同identityとonlineをassert。同一API内、子・grant・復元後公開操作なし。                                                    |
| `worker/src/feature/builtin/manage_worker.rs::tests::control_service_deduplicates_worker_list_by_stable_subject` | 本番serviceに`RecordingWorkspaceClient`、重複subjectの投影をassert。実grant・署名検証なし。                                                          |

本番接続根拠は `ProfileRuntimeWorkerFactory::restore_controller` の `worker_context` 再構築→`Worker::restore_from_worker_metadata_with_context`→返す `RuntimeWorkerController.workspace_client`、Serverの `list_known_workers`（認証親→grant→Runtime summary）と `send_known_worker_input`（grant付きcontrol handle→子入力）である。既存 `worker_control_handle_rechecks_revoked_grants_for_each_operation` はhandle直呼びの失効拒否、`worker_control_input_is_idle_only_and_returns_rejection_as_http_error` はfixture Runtimeへのinput種別を守るが、この復元接続は守らない。

- **配置・最小fixture**：`crates/workspace-server/src/server/tests/worker_operations_tests.rs` の通常lib結合に、仮名 `restored_runtime_parent_lists_and_inputs_existing_child` を追加する。既存production restore/永続session支援を再利用し、隔離temp DB・Runtime FS store・loopback router・親P/子Cだけ。Ticket、Git操作、Workdir、Subjektiv、実モデル課金を持ち込まない。P/Cのregistryとcontrol grantは復旧前だけ作成し、再open後は再seedしない。子作成の冪等性は既存spawn試験に任せる。
- **操作入力・入口**：親Pを停止してshutdown/join完了後に旧store/APIを解放、同DB/FSを再openし本番factoryでPをrestore。復元controller自身の`workspace_client`（公開field）を使う本番 `WorkspaceWorkerControlService` からlistし、その応答の完全subjectをinputの対象へ渡す。外からPを名乗る署名requestを作るだけでは代替しない。HTTP構成を選び `build_router` の外側署名認証も通す。embedded dispatcherだけの成功でHTTP署名を主張しない。
- **観測・落ちるassertion**：listにCのruntime/worker IDと権限が一致、grant ID/件数不変。IdleとなったCへ一意な本文markerを`UserIfIdle`として1回送信し、acceptedだけでなくCのcanonical sessionを読みmarkerが1件であることをassertする。CのIdleは観測イベントで同期し初期IdleをSubmit完了と誤認しない。LLM判断は範囲外。
- **fault位置**：復元時の`worker_context`からsigner/Workspace接続を落とす一時mutationではlist成功/subject一致が失敗すること。grantを復旧後にrevokeする境界faultでは次のinputが`UnknownWorker`相当の公開拒否となり、Cのmarker件数不変であること。前者は接続の回帰、後者は権限の再判定を確認する。全API操作の失効行列やC自身のrestoreは既存の別規則へ任せる。
- **同期・回収**：`confirm_shutdown_and_join`相当の既存完了通知とtask joinを使い、loopback serverもfinally/RAIIで停止・join。bounded watchdogを設け、固定sleep、`drop(api)`だけ、稼働Runtimeの借用をしない。
- **target/filter・区分**：`cargo test -j 2 -p yoi-workspace-server --lib restored_runtime_parent_lists_and_inputs_existing_child -- --test-threads=1`（提案名、まだ存在しない）。通常runner対象。DB/socketだけを理由にopt-inにしない。suiteを増設せず既存libに収める。
- **コストと未確認**：fixture接続を含め1–2人日、追加本体は秒〜十数秒規模を予算とする**推定**。保守費はfactory/認証/回収の結合fixture追従が中心で、純粋unitより高い。このためP1の薄いadapter適合より後に置く。追加不要判定の各項目は実行時間・fixture保守とも増分0である。未実装・未試行・未実測であり、再open後のproduction factoryとHTTP構成の組立て、課金なしの子入力完了支援の再利用可能性を最初に試す。spawn用LLM hookがrestoreにも効くとは仮定しない。支援拡張が大きければ一旦採否を戻し、製品全体E2Eや公開API拡張へ膨らませない。client署名だけの別の新規unitを並行追加して同じ保証を二重計上しない。

Internal子は `worker/src/spawn/registry.rs::load_from_worker_state_with_reclaim` が空registryを生成する。`crates/worker/tests/internal_subworker_restore_test.rs::restore_reclaims_and_clears_legacy_process_children` は親scope回収、`spawned_children`空・reclaimed記録・projection `[]`をassertする。これは旧子の復活成功ではなく、その逆の保証である。

## Ticket / Driveとの重複・コストの扱い

T-744の早期共有（result `01a147e7-a048-73e0-b101-4112bd1abee4`）は `develop@10d200d9ba8bc313bb7f72eabafc279c9def3cf5` を対象にする。基準との差はMR follow-up/Prompt中心で主要Ticket testは同一との報告であり、最終設計・mutation結果はまだ未確認。P1のTools/WIP→実read観測ID→AnswerはT-744で1つの薄い接続に集約し、P2のWeb parser・移行read候補もそちらの最終採否を使う。

`server_implementation_result_tests.rs::implementation_result_signed_http_reaches_persistent_orchestrator_without_assignment_or_mr` は署名HTTP→本番store→永続Orchestrator session・再送を既に扱う。T-744から1 passed、本体9.61秒、初回外側build4分55秒（jobs 2、別checkout）の実測が共有された。**こちらの測定ではない**。同じ経路を巨大なTicket一巡やDrive一巡へ組み込む費用は払わず、新規adapter適合の外側ではこの証拠を再利用する。

T-725現行本文・保留trackingを読んだ。`workspace-drive/tests/storage.rs::receipts_survive_response_loss_and_restart_without_double_mutation` は実DB/blob再open後のreceipt一致、同request再送一致、別actor競合拒否、list件数1をassertする。`workspace-server/src/server/tests/worker_drive_tests.rs::lost_mutation_response_is_resolved_by_receipt_without_resending_effect` は本番DriveFeature→署名router/storeを通し、receipt committed・POST mutate数1・別Worker listのlast mutation一致をassertするがRuntime起動はfixtureである。これらの保存/応答喪失規則を再設計しない。実Web adapter接続は `drive_web_roundtrip_tests.rs::repositoryless_drive_real_web_adapter_preserves_bytes_cas_and_latest_reference_lifecycle` の既存ignore/明示実行であり、通常suiteの成功へ含めない。Worker寿命・Web画面までの不足と必要コストはT-725再開時の判断で、今回未確認とする。

## 今回の代表試行と残る検証

全て専用checkoutで逐次実行、Rust buildはjobs 2。表の本体はlibtest/Deno表示、外側は`time -p`の1回測定であり、通常suite全体の増分やCI中央値ではない。

| 試行（同じtarget/filterでbaseline→mutation→復元）                                                                                                   | 具体的なfaultと結果                                                                                                                        | 今回の本体／外側実測                                                                  |
| --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| `cargo test -j 2 -p session-store --lib retained_reader_ -- --test-threads=1`                                                                       | 本番budget拒否直前で`record_suffix.clear()`。再開unwrapがCorrupt、境界1 byte再開がReadLimitExceededで2 failed / exit 101。復元後4 passed。 | baseline 0.07秒／34.02秒（build 33.91秒）、fault 0.05秒／3.10秒、復元0.06秒／2.55秒   |
| `cargo test -j 2 -p worker-runtime --lib blocking_workspace_client_clears_initial_request_timeout -- --test-threads=1`                              | 本番timeout適用をSomeだけに変更。`request ended before response`でtimeout=true、1 failed / exit 101。復元後1 passed。                      | baseline 0.06秒／167.85秒（build約167秒）、fault 0.04秒／19.63秒、復元0.06秒／18.25秒 |
| `deno task test:runner`、fault時のみ`deno test --allow-read --allow-write --allow-run --allow-env tools/test.test.ts --filter 'round-robin shards'` | 保存selectedNamesは変えず実argvの先頭名を落とす。`task.args.slice(3)`一致assertが失敗 / exit 1。復元後全17 passed。                        | baseline全体7秒／8.11秒、fault単一12ms／0.63秒、復元全体7秒／7.49秒                   |

追加で `cargo test -j 2 -p worker-runtime --lib runtime_owned_workspace_client_ -- --test-threads=1` は3 passed（本体0.20秒／外側0.42秒）。実験patch・stdout・stderr・時間はGit管理外 `target/regression-design/` の `reader-*`、`timeout-*`、`runner-*` へ分離し、3 sourceのdiffが空であることを復元後に確認した。恒久変更は本設計書だけである。

新規の復元親接続、DB fixture suite、画像巨大入力、Ticket/Drive、全通常suite、E2E/browserは今回実行していない。実モデル試行・稼働更新もない。通常suiteのCI job配線/実行も今回確認しておらず、ローカルの選択成功をCI成功としない。追加案を実装する時は所有suiteの既存fixture再利用を先に測り、通常runnerでの選択と実行jobを確認する。
