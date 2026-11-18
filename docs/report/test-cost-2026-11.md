# T-871: 全体時間分布とテスト責務の見直し

通常Rustの初回8,039名を計測し、保存形式の履歴組合せを純粋な判定と実DB接続へ分離した。追加で実SQLite Busyの待機、未使用のWasmコンパイル、移行dry-runの反復、Job cleanupの過大fixtureを削減した。第二弾時点の8,045名の結果は全inventoryと照合済み。第三弾でcleanupを約91%短縮し、E2Eのcompile障害を修正して実製品processの計測も進めた。ただしJournal再openの不安定な失敗、browserの契約不整合、外部供給fixtureを必要とする未実行部分が残るため、全体成功・全体最適化完了とはしない。

## 計測条件と範囲

基点は `515af263bf32027ffae7142be002ae20b587a7c7`。Linux、24論理CPU、Rust 1.95.0、Deno 2.7.14。専用の無負荷ホストではない。既存 `tools/test.ts` のCargo artifact解決、全件列挙、分割、隔離、process group回収を再利用した。通常Rustをworkspace全体でbuildし、E2Eは別選択とした。外側buildは424.16秒。

通常Rustは131バイナリ・8,039選択名・312区分。4 top-level並列、Worker/Runtime/Serverはprocess内1 thread、他の大きい区分は既存の2 threadsを使用した。ケース時間はlibtestのJSON report-timeで取得した。計測用の `RUSTC_BOOTSTRAP=1` はtest processにだけ渡し、Cargo buildには設定していない。

診断の各区分上限は540秒。通常runnerの180秒は変更していない。Toolの600秒上限でバッチが中断されたため、未完区分を再実行した。完了結果は8,039名に一致し、欠落・余剰・重複0を機械照合した。再実行を含む個々の時間の和は、一回の通常runnerのwall timeでもCPU時間でもない。

初回は7,960成功・33失敗・既存46 ignoredだった。26失敗はWasm linker `lld` がPATHにないことによる。既定dev shellのlldだけを一時PATHに加えた再実行で解消した。Runtimeの7件のassert/同期失敗は残り、全Workspace成功は主張しない。Server66区分は全て成功した。

## 確認した重い箇所

| 成功ケース | 初回ケース時間 |
| --- | ---: |
| 保存形式の81経路移行/reopen | 404.57秒 |
| combined failureのpublished history matrix | 253.33秒 |
| Job cleanupの先頭page保留中の後続page回収 | 180.23秒 |
| Service delegationのqueue中置換 | 153.70秒 |
| Journal multisession/multisegmentの保持と処理 | 147.13秒 |
| 256MiB超のarchive保持・receipt replay | 137.76秒 |
| backend_job upgradeのretained baseline行列 | 110.20秒 |
| StandaloneのSQLite Busyをfast budget超過まで保持する6ケース | 各約50–56秒 |

これは混在した分割実行中の時間であり、単独の再測定値とは区別する。全ケースの時間表と所属binary/区分、状態はローカル `target/t871-parent/baseline/all-case-times.json` に保存した。

### 81経路の内部内訳

既存の入力・assertion・移行経路を維持して一時計測した単独実行は452.55秒で成功した。全81入力の不足・重複も照合した。

| 区間 | 合計 |
| --- | ---: |
| 初回prepare/migration | 371.89秒 |
| 再open prepare | 34.35秒 |
| 明示的な履歴検査、初回＋再open | 31.78秒 |
| fixture構築 | 9.71秒 |
| disk backup | 2.83秒 |

行比較・snapshot取得はそれぞれのphase合計で0.31秒以下。fixture共有や不要な全行読取りの削減だけでは主因を解消できない。時間の大部分は、履歴の組合せごとに同じ物理移行・検証を反復することにあった。

## 最初の実装

### 履歴組合せと物理schemaを分離

本番のreceipt識別、122/123のprefix照合、tail照合、残りのplan構築をprivateな純粋関数へ分離した。通常のDB受入れは引き続き物理schema・dataを検査し、その後にplanを作る。保存schema、migration SQL、履歴名は変更していない。

81通りの履歴はDBなしで識別・途中のtail・残りのplanを検証する。欠落、重複、改名、混在、順序不正の負例を追加した。実SQLiteの移行・保存値保持・close/reopenは14種類全ての物理schemaで維持する。元receiptのversion/nameだけでなく `applied_at` も保持を確認する。

独立した読み取りレビューで、全代表をbaseline113にすると曖昧なbaseline114から非先頭のshapeを選んだ後の移行が未検証になると指摘された。Removal/1とJobs/1はbaseline114から移行する代表に変更した。名前付き履歴の保持は残る代表が担当する。全81経路の結合を復活させずに、この接続固有の故障を検出する。

物理shapeの誤認、SQL制約/trigger欠損、transaction rollback、保存prefixの改変、再open時の再適用は純粋なreceipt判定だけでは検出できない。このため実DB検証は削除していない。

### test profileの依存最適化

既存調査でCPUコストが確認されていた `sha2:0.11.0`、`decodal`、`serde_json` にtest profileだけopt-level 2を設定した。dev/releaseの最適化設定、debug assertions、overflow checksは変更しない。成功結果をcacheしたり、入力・assertionを省略したりする変更ではない。

初回の依存再buildは540秒の診断上限に達し、残ったbuildは次の実行129.95秒で成功した。この合計を正常なcold build時間とは扱わない。実行時間の改善とは別の初期費用である。

## 候補の検証と効果

一時計測付きの候補で `store::storage_formats::` 全22件が311.26秒で成功、選択・結果の欠落/重複0。14schemaの実移行/reopen単独は95.67秒。旧81経路の404.57秒と比べ約76%短いが、単体化とtest profile変更を合わせた候補の値であり、両者の寄与率は分離していない。新しい純粋matrixの時間も最終比較で別途加算する必要がある。

テスト内容を変更していないケースの単独再測定:

| ケース | 旧profile | 新profile |
| --- | --- | --- |
| Chat Busy通知から次のmodel入力 | 30.30 / 28.87秒 | 27.30 / 30.25秒 |
| Journal terminal3条件の実Session purge | 26.54 / 29.07秒 | 20.77 / 22.48秒 |
| 256MiB超archive保持 | 132.91秒 | 58.50 / 50.81秒 |

Chatでは有意な短縮を確認できない。Journalとarchiveには改善が見える。これを全体の短縮率には外挿しない。

一時計測を除去した最終sourceでも22件を再実行し、全成功・欠落/重複0、合計263.42秒だった。実移行/reopenは75.72秒、81履歴の識別/計画は0.0027秒、破損tailの負例は0.0152秒。入力識別と計画の故障検出を単体へ寄せ、必要なSQLite接続を残したまま、重い接続部分は初回404.57秒から約81%短縮した。初回と最終は実行条件・ホスト負荷差を含み、この率を専用環境の反復中央値とは扱わない。

第一弾のsourceではworkspace test build、root `cargo check`、`cargo fmt --all -- --check`、`git diff --check`、状態契約命名検査が成功した。以下は追加変更と、その後の全体照合である。

## 第二弾の変更と故障検出

### 実Busyと待機時間を分離

Standaloneの長いBusy復旧6件を、同じ選択・process内1 threadで各2回測定した。

| 比較対象 | 変更前（2回） | 変更後（2回） |
| --- | ---: | ---: |
| 長い6件のケース時間合計 | 314.28 / 310.66秒 | 1.893 / 1.866秒 |
| before-budget対照を含む7件のprocess時間 | 320.96 / 316.57秒 | 2.267 / 2.226秒 |

6件の平均削減率は99.40%。初回の変更後focused buildは17.43秒で実行時間に含めない。fixture所有の接続だけ`busy_timeout(Duration::ZERO)`とし、実競合writer→typed DatabaseBusy→実復旧を残す。本番のSQLite 5秒待機と100ms指数backoff、FAST_PASSES=8、retained retryは変更せず、テスト内のTokio仮想時計だけを進める。admissionのJob実行/cleanupは実時計に戻す。

コード上の内訳は8×5秒のSQLite待機＋fast backoff合計9.5秒＋retained側1秒。各phaseの独立stopwatch値ではなく、設定と実測全体値の照合である。

故障注入は別コピーで行った。8passでretryを停止すると6/6件が失敗、writer lockを除くとfailed-recovery 5/5件が失敗、9pass以降のdomain再確認を飛ばすとstale/retired 2/2件が失敗した。cleanup、元grant、保存snapshot/receipt/reopenのassertionを維持している。型付きBusyの発生源、ロック解放のみからの永続復旧、Job/domainの権限整合は単体fakeでは検出できないため実DBを残す。multi-thread負荷やOSクラッシュの証拠とはしない。

親の通常workspace Cargo build後にもlib187件・外部integration29件が成功し、既存ignored 1件を別計上した。詳細は`target/t871-busy/REPORT.md`と`comparison.json`。

### Pluginの未使用guestコンパイル

Host-onlyのService認可テストは、同じfixtureでupload/install・現在のSubject権限・plan・実DB・署名経路を構築した後、guestを実行しない。そのfixtureまで毎回Wasmtime/Componentをコンパイルしていた。

fixture内の`OnceLock<(Executor, CompiledTool)>`で、guest/contextが必要なケースだけコンパイルする。mutable runtimeや判定結果をテスト間で共有しない。実guest→Hostの接続テストは従来どおり実compile/invokeを行う。代表の「queue中にdelegation source置換」は14条件を減らさず、変更前85.33秒→変更後59.90/57.87秒で成功。全体の分割再実行中は74.64秒だった。初回153.70秒とは負荷が異なるため、その差を本変更の効果としない。

### 28経路の余分なdry-runとrunner単位

combined failureは、明示的`migration_plan`がコピー上の実移行を行い、続く`migrate_database`でも同じdry-runを行い、それから実openと2回の再openをしていた。明示的dry-run＋通常openでの実apply＋再openに変更した。dry-runの非破壊、計画どおりのreceipt追記、全保存値・制約・現在schema・FK整合の検査を残す。CLI wrapperの戻り値契約は`migration_entrypoint_tests`等が担当する。

28通りのfresh/ordered/prefix・物理境界は全て残し、canonical/publishedを独立した2テストへ分けた。単体で代替できない実DDL/保存値保持の接続であり、件数だけを減らす変更ではない。

旧単独354.06秒→重複除去後218.88秒。分割後のmodule全9件は319.22秒で成功し、うち2経路群は119.41/84.29秒。個別再測定は91.63/86.80秒。各群は通常runnerの180秒上限内だったが、これだけで全shardが上限内とは判断しない。

### Chatと全体回帰で見つけたfixture不整合

ConnectedChatはPOSTごとのClient構築をfixture内1個へ変更した。timeout・requestごとのBearer認証・実HTTPを維持する。分割再実行の代表ケースは23.97秒だが、Client再利用だけの効果は分離していない。

Webの12失敗は、専用Subject policy APIを読むUIへ旧configuration応答を返すfixtureが原因だった。専用response、encoding、unset/tombstone/explicit-empty、401/403、origin、refresh拒否を検証し、製品parser/UIを緩めず修正した。Runtimeの7失敗はPausedへのNotifyが現行契約ではResumeするのに、Pausedを維持すると仮定したseedだった。Running中のdurable受付後にPauseするよう同期し、FIFO・通知1件・未配信・元権限・モデル呼出し数のassertionを維持した。いずれも性能改善ではなく`fix:`としてコミットした。

## 第二弾の全体照合

`ae4f8d875`までの変更を含む通常Cargoの最終buildは156.72秒（既存cacheあり）。131 binary/8,045名を列挙し、実行・修正target再実行の結果と照合した。欠落・余剰・区分内重複0。初回8,039名との差は履歴単体検証の5件追加とfailure-tailの1→2分割である。

**7,998成功・46既存ignored・1失敗**を保存した。Journalの`backend_immutable_input_survives_missing_source_and_predispatch_host_reopen`は分割実行でFailed/Pendingの不一致があり、単独再実行は13.41秒で成功した。初回失敗を成功で上書きしておらず、安定した全体成功とはしない。

これは単一のソース状態を一度で完走した結果ではない。`wave2`の通常targets、`wave3`の全Server65区分と更新Standalone、`wave4`の更新Runtime730件とfailure migration9件を、binary/完全なtest名で突合した。各変更targetを再buildして検証した記録である。`final-coverage.json`に全採用log、`final-case-times.json`に全名・時間・状態、`final-coverage.ts`に照合手順を保持した。

| 区分 | ケース時間合計（参考値） | 結果 |
| --- | ---: | --- |
| workspace-server | 8,582.97秒 | 2,304成功・1失敗・44 ignored |
| standalone | 245.34秒 | 216成功・1 ignored |
| worker-runtime | 238.86秒 | 730成功 |
| subjektiv | 236.09秒 | 197成功 |
| worker | 161.19秒 | 1,610成功 |
| ticket | 148.25秒 | 176成功 |

合計はwall timeでもCPU timeでもない。初回と今回の同名成功7,956件は10,912.54→9,724.47秒だが、負荷・分割・個別再実行条件が異なり、名前が変わった大きいmatrixも含まれない。全体削減率としては採用しない。実行前後の判定には上記の独立した反復比較を使う。

追加検証:
- doctest: 9成功・既存21 ignored、buildを含め50.10秒。
- Web unit: 897成功、テスト表示14秒（内部schema生成/buildとは別）。
- Web component: 870成功、Vitest wall 10.56秒。transform累積120.11秒は並列集計でありwallへ加算しない。
- UX unit26成功、検証tool26成功。
- 変更fixtureを使うpermissions実browserは9.87秒で成功。
- root `cargo check`、全体Rust fmt、diff、状態契約命名検査が成功。
- UX全体`deno task check`は生成TSの拡張子なしimport 5件で失敗したが、bundler生成物を許容する設定と実行入口を`d4d30a188`で修正し、再実行は0.95秒で成功。生成Rust/TS型を変更せず、既存の無関係な全ファイル整形は混ぜていない。

## 第三弾: cleanupページ境界とE2Eの実測

### Job cleanupの過大fixture

先頭pageがclaim前に保留されても後続pageを所有できる保証に、固定64件/page・66 Workersは不要だった。private pagerのpage_limitだけを引数にし、製品入口は従来どおり`ABSOLUTE_MAX_CONCURRENT_JOBS`（64）を渡す。テストは2件/page・5 Workersで、満杯の2ページ＋末尾1件を通す。本番cursor・owner・権限・timeout・cleanupの実装は同じである。

| 同一ケース、process内1 thread | 秒 |
| --- | ---: |
| 変更前2回 | 148.66 / 141.45 |
| 変更後3回 | 13.64 / 13.25 / 13.70 |

平均145.06→13.53秒、約90.7%短縮。初回の192.31秒とは負荷条件が異なるため、この比較には使わない。旧artifactを誤選択した3回は`old-binary-extra-*`に分離し、変更後の数値から除外した。全pageのowner、2 sweepで同一集合、実DBのcleanup完了、元worker/run/failureを維持し、removal plan成功・Runtimeからの除去・元Submit ID以外の再送なしも検証する。純粋なpagerのfakeでは、所有権と実削除・保存結果の一致を検出できない。

page 1で打ち切る故障注入はowner数2≠5で失敗した。注入復元・通常workspace再build後のmoduleは26/26成功（107.17秒）、欠落/重複0。親側でも`--features test-support`の別buildで26/26成功（テスト86.72秒）。全Server再実行ではない。記録は`target/t871-cleanup/RESULTS.md`、`module-coverage.json`、各build/run/faultログ。

### E2Eのcompile障害と古いfixture期待値

削除済み`WorkerSpawnRequest.ticket_assignment`参照をtest-supportとrestore seedから除き、seedへ現行の省略値`authorization`、`resolved_core_execution_grant`、`plugin_execution`を明示した。製品APIに旧fieldを復活させていない。CLI helpは共有permission設定とSubject個別policyの分離を説明する現行出力を検証するよう更新した。これらは性能改善ではなくfixtureの`fix:`である。

`cargo test -p yoi-e2e --features e2e,fixture-support --no-run --message-format=json`は成功（最終incremental build 48.36秒）。featureなし通常製品3種を別Cargo invocationでbuild（178.11秒）、TUIはさらに別invocationで`e2e-test`付きyoiをbuild（94.97秒）した。test-supportを製品binaryへ混入させていない。製品の保存済みfixture、稼働サービス、実credentialは借りていない。

| 選択target | 結果 | process wall |
| --- | --- | ---: |
| E2E harness lib | 24成功 | 1.25秒 |
| worker_environment | 2成功 | 44.18秒 |
| auth_passkeys | 8成功、既存browser条件付き2 ignored | 133.23秒 |
| configuration_cli（修正後） | 3成功 | 68.51秒 |
| panel（全製品binaryを明示） | 2成功 | 29.31秒 |
| rewind | 1成功 | 0.55秒 |
| worker_restoreのseed内snapshot単体 | 1成功、E2E4件は選択外 | 0.02秒 |

実行は`target/t871-parent/command.ts`で既存runnerの隔離・回収を使用し、process内1 thread。製品binaryはharness既存の`YOI_E2E_BIN` / `YOI_E2E_SERVER_BIN` / `YOI_E2E_RUNTIME_BIN`で指定した。panel初回はServer/Runtime overrideを指定せずprovider内部buildを含み238.11秒だったため、明示した再測定と区別する。この差を製品高速化とはしない。CLIの初回2成功/1失敗・44.91秒も修正後の性能比較には使わない。

feature限定store fixtureは9/9成功（5.79秒）。最初に公開re-export名でfilterしたrunは0件だったため検証から除外し、実module名`store::fixture_support::tests::`で再選択した。restore E2E4件には外部供給の`YOI_E2E_RESTORE_FIXTURE`が必要で、fixtureを生成・実データで代用していない。checkpoint featureは別選択で未検証。最終root `cargo check`（16.33秒）、全Rust fmt、diff checkも成功。

再現用の各コマンド・stdout/stderr・wall timeは`target/t871-parent/validation/`の`e2e-*`、`fixture-support-tests-selected`、`cleanup-parent`、`final-check-wave5`に保持。時間表はsuite単位であり、今回のE2Eでは全caseのJSON report-timeや各setup phaseの独立計測までは行っていない。

## 残るコストと検証障害

- Job cleanupの最長ケースは第三弾で削減済み。全体runnerの全shardが180秒以内かの一括再確認はまだ行っていない。
- retained Job baseline50..113は131.58秒。古い物理DDL・内容変換が異なるため、識別の入力表だけとみなして機械的に削減していない。Job固有の境界と他suiteの担当をさらに対応付ける余地がある。
- Journal multisessionは82.82秒。UTF-8の分割、48KiB超capture、128KiB envelope、複数Job・実Session purgeの境界を持つ。単に本文を小さくすると保証を失う。setup・連鎖・teardownの独立計測は残る。
- synthetic UX browserの15ファイルを試行。最初の一括runは多数失敗し600秒で中断、未完の7ファイルを別途実行して14件失敗・168.68秒で終了した。成功/失敗のcase時間はログに残したが、成功した性能比較には使わない。
- browserの診断用loopback実行で、Ticket画面が`POST .../tickets/browse`を送る一方、旧テストは`.../tickets/query`の応答ゲートを待つことを確認した。画面は`Ticket browse response contains unknown field`を表示した。旧fixture/selectorの契約確認が必要で、parser緩和・timeout延長はしていない。他のbrowser失敗を全てこの原因とは断定しない。
- Journal再openの失敗ログでは、constructor内のstartup recoveryがdispatch・失敗・cleanupまで進み、テスト側のstop/join後に既にFailedだった。現在のstopはconstructorの返却後・Subjektiv store再open後であり、期待するPending観測点より後へ進める競合がある。単独成功だけでは解決とせず、起動前からの決定的同期をどう保持するかは未修正。
- E2E compile障害は第三弾で解消した。restoreの外部供給fixture、checkpoint、認証browser2件の実行と、残るWeb browserの契約修正・全inventory照合は未完。通常suiteやbuild成功からその成功を推定しない。
- Web browserの他の明示入口、ignored/live APIは未実行部分が残る。稼働環境・実データ・保存credentialを代用していない。

全体の残存失敗と未計測部分を解消する前にT-871を完了とはしない。push/deploy・稼働service/DB変更は実施していない。

主要証拠はTicket T-871とWorkdirの`target/t871-parent/`、`target/t871-busy/`、`target/t871-runtime/`に保持。移行の故障検出対応表は`target/t871-migration/optimized-handoff.md`。target配下は再利用可能なローカル調査資料であり、製品の正本や恒久APIではない。
