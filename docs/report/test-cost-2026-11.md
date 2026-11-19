# T-871: 全体時間分布とテスト責務の見直し

通常Rustの初回8,039名を計測し、履歴組合せの単体化、実SQLite Busyの待機、未使用のWasmコンパイル、移行dry-runの反復、Job cleanupの過大fixtureを削減した。通常runnerはwarm build後、399区分を中断なしで完走。Rustは8,045名＝7,999成功＋46既存ignored、欠落・重複0。Web実browser32ファイルと、再生成した固定fixtureによるrestore4件・checkpoint7件も成功した。前回残したChat接続browserとnavigation reviewの失敗は、追加調査で古いtest/fixtureの原因を確定して修正し、各2回成功した。doctest/Web timeoutも同じCargo targetへseed検証buildを重ねたロック競合と切り分けた。原因未確定のまま完了とした判断は撤回しT-871を再開した。最新の訂正・確証は末尾「未確定原因を残した完了判断の撤回と追加検証」に記録する。途中の失敗・未完記録は経緯として保持する。

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

81通りの履歴はDBなしで識別・途中のtail・残りのplanを検証する。欠落、重複、改名、混在、順序不正の負例を追加した。実SQLiteの移行・保存値保持・close/reopenは14種類全ての物理schemaで維持する。元receiptのversion/nameだけでなく `applied_at` も保持を確認する。これは81入力全てを本番の物理検査・SQL移行へ通したことと同値ではない。論理判定と物理接続の責務を分離し、代表以外の履歴と物理移行の相互作用を総当たりする保証は縮小している。

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

## 専用Workdirでの継続検証

旧Workdirの上記ログはそのまま保持し、継続作業は専用Workdir `001a1f18c4e8c000003`（alias `t871`）、branch `work/t871-test-cost`へ分離した。今回の開始HEADは`24690e2bf`、採用コード最終HEADは`eb9bae58f`。`yoi-origin`への書込み・切替・build・稼働サービス操作はしていない。

### 通常Rustの再照合と残った環境依存

専用cloneはtargetなしで開始。通常workspace test buildは486秒で成功し、実行時間とは別記した。Rust 1.95.0 / Deno 2.7.14 / Linux 24論理CPU。Wasm compile用に既定devshellのlld 21.1.8をPATHへ追加した。通常runnerと同じCargo選択・列挙・隔離・process回収を再利用し、JSON report-timeは実行processだけで有効化した。8 top-level並列、Server/Worker/Runtimeはprocess内1 thread。

- 131 binary / 8,045名 / 394区分。既存`17f2747a7`のServer 16件/区分が適用され、Server lib147区分＋binary1区分は全成功、最長148.47秒。今回の採用区分に180秒timeoutはない。
- 外側Toolが600秒で停止したため、完了JSONのない区分だけを最大24区分のbatchで再実行した。最初の未完ログは残し、完了結果をexact binary/test名で照合した。単一invocationのwall timeや通常runner全体の成功とは呼ばない。
- 初回照合は7,997成功・46 ignored・2失敗。両失敗は長い専用checkout配下でcleanup testのUnix socket bindが`SUN_LEN`を超えたもの。
- `a449bac5d`でLinux test fixtureだけ、所有parent directoryのFDを経由した短い`/proc/self/fd/...`名でbindする。実socketは従来と同じWorkdir内に作られ、FIFO/socketの置換検出・非open/nonconnect・unlink・再試行のassertionを維持。process cwdや製品cleanupは変えない。
- 修正後Runtime lib690件は267.60秒で全成功。前回`24690e2bf`のsnapshot境界caseも成功。Runtime libだけを新結果へ置換した照合は**7,999成功・46既存ignored、欠落・余剰・重複0**。元の2失敗ログは消していない。

### 追加最適化候補の採否

どちらも同じ通常workspace test profile、単独process・1 test threadで前後各2回。専用の無負荷機ではない。buildは別invocationであり表の時間に含めない。

| 候補 | 変更前 process秒 | 候補 process秒 | 採否 |
|---|---:|---:|---|
| retained Job seed書込みを1 transactionへ | 93.50 / 87.01 | 97.86 / 86.31 | 明確な改善なし。不採用、コードを復元 |
| Journal fresh API初期化に既存PreparedFixtureを使用 | 82.80 / 81.10 | 79.48 / 79.69 | 採用。平均81.95→79.58秒、約2.9%の小幅短縮 |

retained Jobの64経路はそのまま残した。物理DDL・raw content capture・receipt・retirement・reopenが対象で、履歴の識別表だけとみなせない。既にversion別の空DB templateがあり、ゼロからのfixture構築を重複実装する案も採用しない。今回のseed transaction候補のパッチと測定ログはローカル証拠として残す。

Journalは`e8b7edcb3`。新規DBにだけ既存の検証済みpristine schemaを独立copyし、追加接続の重複schema検査を省く。既存DBのrestartはStartup経路のまま。native Host ownership fence、実SQLite、署名付きHTTP、Runtime、Session、Job cleanupを残す。UTF-8入力、48KiB capture境界、128KiB envelope上限、多Session/fork/Job数、全range/hash assertionは削減していない。単体のdecodeだけではこの接続と永続効果を検証できない。

採用候補の最終build後、Journal全97名を照合して全成功、欠落・重複0。最初の独自診断グループ（名前順8件）は1区分が180秒を超え、94件のterminal結果に留まった。その8名全てを4名ずつへ分け、103.94/114.67秒で成功した。最初のtimeoutは保持しており、これは通常runnerの分散配置147区分とは異なる診断グループである。全体の8,045名照合はJournal追加変更前、97件の再検証は追加変更後である。

### Browserの共有buildと最終検証

`tools/web-ux/browser-tests/run.ts`はsuite開始時に一度buildし、各caseは独立HTTP/browser状態を所有する。`eb9bae58f`ではSubject deletionの独自buildも既存`productionBuild` helperへ接続した。明示build未供給の単独実行では従来どおりbuildし、供給時も`index.html`を検査する。test成功をcacheしない。

調査Subworker用に一時追加したartifact出力先override/helperは採用せず、元の出力先へ戻してから全32fileを再実行した。製品parser・fixture応答・selector・assertionを緩めていない。

| 最終選択 | 結果 | 時間・境界 |
|---|---|---|
| UX browser15file | 49成功＋10 steps | build4.59秒、Deno test表示3m29s。実Chromium |
| Web明示browser17file | 26成功＋317 steps | file別process合計239.42秒、既存build再利用、追加buildなし |
| Web unit | 897成功 | Deno表示7秒、内部schema生成/buildとは別 |
| Web component | 870成功 | Vitest10.60秒。実Chromiumとは別 |
| UX unit / 検証tool | 26成功 / 26成功 | 既存の通常入口 |
| doctest | 9成功・21既存ignored | 221.88秒、内部compile込み。通常runnerの180秒以内という証拠ではない |
| root cargo check / Rust fmt / diff / 状態契約命名 | 成功 | check122秒、命名635行/未承認0/古い例外0 |

Browserはproduction shellと合成loopback HTTP/WS fixtureの接続であり、実Backend認可やDB/Runtimeの製品E2Eへ読み替えない。review scriptの`subject_navigation_review.ts`と`chat_ui_review.ts`は未実行。画像の目視レビューは行っていない（製品UIの変更なし）。CIでの実行済みを意味しない。

主な再現入口:

```sh
cargo test --workspace --exclude yoi-e2e --no-run --message-format=json
cargo test -p worker-runtime --lib -- --test-threads=1
cargo test -p yoi-workspace-server --lib server::tests::subjektiv_journal_tests:: -- --test-threads=1
# 上のmodule全件を一processへ集めると時間が長い。今回の診断はexact名で分割。
(cd tools/web-ux && env -u LD_LIBRARY_PATH deno task test:browser)
(cd web/workspace && env -u LD_LIBRARY_PATH deno task test)
(cd web/workspace && env -u LD_LIBRARY_PATH deno task test:component)
cargo test --workspace --exclude yoi-e2e --doc --no-fail-fast
```

### restore/checkpointの具体的な阻害と残件

固定資産`tests/e2e/fixtures/worker-codex-v1`は存在する。「環境変数未設定でfixtureがない」だけではない。manifestの生成元`1639be98411921308238b3a6a6e75c784c045156`に対して、現行`resources`と`crates/builtin-source`は異なり、`job.dcdl`等の参照を現在の資源へ黙って置き換えることを検証器が拒否している。Git差分を確認した。

- 適合する固定fixtureの供給・更新が必要。古いSessionの生成元commitだけの書換え、hashの再計算による出自の偽装、検証skipは行わない。
- 既存Sessionを保持したfrozen-reference形式を追加するなら、別途形式と参照閉包の完全性・実際の評価入力を設計/検証する必要がある。今回の速度改善としてガードを削らない。
- restore通常4件とcheckpoint7件は、この制約を解消した最終実行が未完。以前の部分E2E結果、seed projection成功、通常Rust全名照合では代替しない。実credential・稼働データ・外部モデルを借りてfixtureを生成していない。
- 明示E2Eの内部phase計測、残るignored/live APIとmanual review入口、通常runnerによる単一最終ソースでの完走は未完。doctestは新cloneの内部compile込みで180秒を超えた点も別に扱う。

今回の証拠は専用Workdir `target/t871-final/`（build結果、inventory/tasks、各case JSON/stdout/stderr、coverage、候補比較、最終Journal97件照合、Web/UX/check/fmt/docログ）と `target/t871-browser/`（全入口inventory、file別command・時間・結果、初回/最終ログ）。`measure.ts` / `resume.ts` / `aggregate.ts` / `journal-final.ts` / `journal-split.ts` / `verify.ts`は計測・照合手順を保持する。最初の失敗・中断・不採用候補も残す。T-871はopenのままで、push/deploy・元環境の変更はない。

## 現行ソースからのfixture再生成と次の阻害

ユーザーの再生成指示を受け、通常テストからは実行されない明示取得exampleを追加した。`e40316bac9507fc772a8f960fe58da7bb71104bd`のclean tracked sourceを生成元として、featureless製品3種を`target/e2e-products`へ別Cargo invocationでbuildし、隔離Server/Runtime・実Codex OAuth (`codex-oauth/gpt-6.1-sol`) の人工会話2往復から`tests/e2e/fixtures/worker-codex-v2`を取得した。上の「生成していない」はこの再生成以前の状況である。

生成は公開APIのWorker作成・入力・停止と実WS snapshotを用い、停止・shutdown後に型付きoffline抽出した。live/offlineの会話4行、元Session bytes/hash、生成元commit、asset size/hashを照合。会話内容、system promptが公開定型文のみであること、Workdirが空であることも確認した。取得元Runtime/Server shutdownとroot cleanupは全て成功。稼働Workspaceや保存DBは使用せず、認証情報・元Worker metadata・実行grantをfixtureへ保存していない。認証は取得processだけに明示し、restore testへは渡していない。

取得過程では公開createのRunAccepted契約、usageと会話行の分離、失敗runの検出に追従した。さらに抽出器が現行writerを拒否する2点を修正した:

- Workdir公開定型句の`authorization`を秘密値と誤検出。完全一致する既知の1文だけを検査上除外し、他の同語・Bearer/token/URL/host path等の拒否は維持する。Session bytesは一切書換えない。
- `worker.protected_inputs.v1`の空checkpointを未対応として拒否。`{"version":1,"kinds":[]}`だけを受け入れ、保護対象あり・未知field・未知versionは拒否する。抽出・installのbytes保持を検証した。

fixture-support回帰は15成功・既存1 ignored、実行1.93秒（build別）。診断には型・位置・typed record kindだけを出し、private payloadやserdeの値引用を出さない。再生成失敗ログも`target/t871-acquisition/acquire-{1..8}.*`に残し、成功は`acquire-9.*`。秘密値は記録しない。

新候補を明示したfeatureless `worker_restore` targetは **1成功（seed projection）・4失敗**、実行45.04秒（事前build別）。2件は直接restore HTTP 403、2件はresponse-loss relayのupstream non-200で停止した。fixture整合性チェックとoffline installは通ったが、復元成功ではない。コード照合では、旧`seed_fixed_worker`はcatalog/reservationだけを作り、現行restoreが`subject_authorization::validate_bound_worker`で要求する実行権限を作っていない。実際のHTTP error bodyは秘密保護のため収集しておらず、認可拒否の具体的reasonとの照合は未実施。5つの隔離環境のcleanup成功・root不存在を確認した。

新候補は保存するが標準fixtureはまだ切替えない。次は新規の隔離seedに現行の正規実行権限を整合させ、認可を迂回せず4 restoreケースと7 checkpointケースを再検証する。checkpoint候補実行、phase別計測、通常runnerの最終単一ソース完走は引き続き未完。T-871全体完了や全E2E成功は主張しない。

## 前回の照合と独立した残件（a15f99301時点の記録）

以下の「原因未確定」「cold内部build」および完了判断は当時の記録であり、末尾の追加検証で訂正した。

上の阻害記録以後の最終結果。専用`work/t871-test-cost`で実施し、通常runnerの採用ソースは`77ab0dc0f`。その後の`cb1a75821`は取得exampleの内部変数命名と既存保存keyの例外登録、`9bbd796bb`はrestoreテストの単調時計記録だけで、通常runner対象の製品コード・検証条件は変えていない。各変更targetは別途検証した。元の`yoi-origin`、稼働サービス、`yoi-data`は変更せず、push/deployはしていない。

### Restore seed修正・標準fixture採用

`77ab0dc0f`で、隔離bootstrap ownerから現行Profile/permissionを解決し、本番launch policy検証器を通した新規の実行grantとfrozen launch bundleをoffline seedへ保存するよう修正した。元fixtureのgrantの再利用や本番restoreの認可緩和はない。owner不一致、明示empty、binding/digest、revocation、後段失敗時のrollbackを含むseed回帰10件（7.57秒）と、関連authority回帰24件（54.73秒）が成功。harness libは25件（1.24秒）成功。

候補のrestore4件とcheckpoint7件の成功後、標準資産をv2へ切替え、overrideなしでも再実行した。READMEがasset rootにあると安全性検査が拒否したため、説明を隣接`worker-codex-v2.md`へ移動した。許可ファイルの検査は維持し、取得済み4資産のbytes/hash/出自は変更していない。元HTTP403、README拒否、初回compile失敗のログは保存した。

### 通常runnerの一括結果

`deno task test --jobs 8`の単一invocationが**399区分成功・失敗0**で終了した。通常Rust394区分＋doctest/Web/tool5区分。実行1,508.19秒（約25分8秒）、外側warm build8.61秒を別計上。131 binary・8,045選択名と結果数が一致し、7,999成功・46既存ignored、欠落・余剰・重複0。分割再実行の合成ではない。以前のcase別JSON計測とは別の通常runner結果である。

| 通常runnerの非Rust区分 | 結果 | process秒 |
|---|---|---:|
| doctest | 9成功・21既存ignored | 38.40 |
| Web unit | 897成功 | 22.26 |
| Web component | 870成功 | 7.00 |
| UX unit | 26成功 | 4.32 |
| 検証tool | 26成功 | 7.72 |

最長Server shardは179.44秒、続いて178.01/176.55/174.25秒。180秒制限は変更しておらず、**余裕は小さい**。この一回の成功を負荷に依存しない安定完走や60秒目標の達成としない。

先行invocationは外側build322.14秒後、内部compileを含むdoctestとWeb unitが各180秒でtimeoutし、全体も未完だった。その記録は`normal-runner.log`と07-05のrunner directoryへ保持した。warm状態の全体成功で、cold内部build競合の問題が解消したとはしない。外側buildと内部buildの分離は残る改善候補である。

### 明示E2E・browser・手動入口

全6 E2E integration targetを実行。製品3種のfeatureless build、checkpoint Runtime、TUI用buildは別Cargo invocation/targetで分離した。テストは原則process内1 thread、並行隔離ケース内の2環境は並行のまま。認証browser2件は既存ignoredを明示選択し、成功へ数えた。

| 選択target/入口 | 最終結果 | テスト/実行秒（build別） |
|---|---|---:|
| worker_restore、featureless | 製品4成功＋seed projection1成功 | 55.55 |
| worker_restore、checkpoint選択 | 7成功、上記5件はfilter外 | 90.71 |
| auth_passkeys、ignoredを含む | 10成功 | 220.83 |
| configuration_cli | 3成功 | 52.82 |
| worker_environment | 2成功 | 51.21 |
| panel | 2成功 | 86.40 |
| rewind | 1成功 | 0.52 |
| Ticket実API/Web roundtrip | 1成功 | 8.40 |
| Drive実API/Web adapter | 1成功 | 4.83 |
| Repository実API/Web receiver | 1成功 | 3.67 |
| Ticket browse performance | 1成功 | 149.04 |
| Chat実Backend/browser接続 | **1失敗** | 34.75 |
| UX browser15file | 49成功＋10 steps | Deno表示2m59s |
| Web browser17file | 26成功＋317 steps | file別process合計235.78 |

E2Eの製品ケース合計は29成功（seed projection1件は別）。harness lib25件と取得example単体1件も別計上。再生成時のRuntime fixture抽出回帰は15成功・private helper1 ignored。これらは全てを同一invocationへまとめた結果ではない。checkpoint製品のbuildは4m20s、認証targetのbuildは2m42s、取得example回帰のbuildは43.58秒。全体cold build費用や性能比較へ混ぜない。

UX browserはsuite共通buildを一度使用（Viteの二段出力2.10/1.50秒）。Web17fileは既存production buildを再利用した。mock HTTP/WSの32file成功を、実Backend接続Chatの失敗の代替証拠にしない。`chat_ui_review.ts`はlight/dark・390/768/1440pxでdraft保持/復帰まで完走したが、画像の目視承認は行っていない。`subject_navigation_review.ts`は`Operation` comboboxの15秒待ちで失敗した。

通常Rustの46 ignoredを全件列挙したところ、41件は親テストがexact subprocessとして起動するprivate helper、5件は表の実API/Webとbrowse入口だった。helperを環境なしで単独起動して成功を水増ししない。5件の明示選択結果は4成功・Chat1失敗で、通常runnerのignored数とは別の帳簿にした。doctest21 ignoredは既存の非実行例のまま。実モデルの一般的なlive API試験、物理認証器/実スマホ、全feature組合せや全platformの成功は検証範囲に含めていない。実Codexの使用は前節の人工会話fixture取得に限る。

### Restore内部の計測

`restore-phases.jsonl`へprocess-localの単調時計とphase名だけを追加し、要求・Session・credentialのpayloadは出さない。通常restoreの5隔離環境とcheckpointの7環境について、binaryの保存hashと現在bytesの一致、cleanup成功、root不存在を照合した。次の値は同一環境の隣接phase差分。checkpointの一部はRAII/panic回収を検証するため、全phaseを共通stop経路で測れない。

| 準備区間 | 通常5環境の平均秒 | checkpoint7環境の平均秒 |
|---|---:|---:|
| fixture copy/verify | 0.056 | 0.044 |
| offline bootstrap/Profile/Worker seed | 1.343 | 0.791 |
| seeded Backend起動＋公開identity取得 | 9.946 | 9.492 |
| Runtime trust初期化 | 0.041 | 0.044 |

seedの初回を含む範囲は通常0.679–3.379秒、checkpoint0.475–2.546秒。通常のrestore→live subscribeは3環境で0.332–0.685秒、snapshot検証は0.036–0.095秒。通常cleanupは5環境で0.063–0.070秒。Backend起動区間が支配的だが、内側のDB open・process readiness・公開identity取得は未分離であり、約10秒全てを単なる固定待機やfixture overheadとは断定しない。実process/公開認可/永続復元がこのE2Eの対象なので、単体fakeへの置換は採用しなかった。各環境時間の和は並行ケースを含むsuite wall timeと異なる。

### 最後に残した独立項目

1. **Chat実Backend/browser接続失敗**：画面には`Conversation history could not be loaded.`、テストは`outcome could not be confirmed` alertを30秒待って失敗。原因が製品・fixture・同期のどれかは未確定。timeout延長、assertion削除、製品parser緩和はせず、独立した問題として修正をスキップした。
2. **navigation review失敗**：`Operation` comboboxが15秒以内に現れない。現行画面とreview scriptの契約照合は未完。上記32fileの自動suiteとは独立して記録した。
3. **cold内部buildとshard余裕**：最終一括成功はwarm。coldでのdoctest/Web内部build timeoutは残り、最長shard179.44秒も余裕が小さい。現在のrunner成功を一般的な性能保証にしない。
4. **非実行範囲と残るコスト**：目視レビュー・実物認証器・一般live API・全feature/platform組合せは未検証。retained Jobの64物理移行とJournalの実Session/Job境界は残す。seed transaction候補は効果がなく復元済み。全setup内部を独立stopwatchで測り切ったわけではなく、特にBackend起動区間は今後の内訳候補である。

利用者の「独立した問題はスキップし最後にまとめる」指示に従い、実行可能な選択targetの検証・採用変更・集計をここで完了する。全テストgreenや全費用最小化を意味しない。

### 再現と証拠

通常runner: `target/normal-tests/2026-11-19T07-21-24.395Z-86810a66-ec0e-48cd-a004-ff0b957f75e1/{inventory,summary}.json`と全区分stdout/stderr。`target/t871-completion/aggregate.mjs`がselected名/親summary件数を照合し、`coverage.json`に重複/欠落と上位時間を記録する。初回・最終ログを別保存している。

明示試験: `target/t871-completion/*-tests.log`、`restore-timed.log`、`checkpoint-timed.log`、各実API/Webの`.json/.stdout/.stderr`、`ux-browser.log`、`navigation.log`、`chat-review.log`。phaseは`phase-summary.json`、元環境は`target/e2e-artifacts/worker-201640-*`と`worker-205937-*`。seed/authority回帰・標準採用前後は`target/t871-restore-authority/RESULTS.md`と`artifact-index.json`。既存ログは消していない。これらtarget配下の資料はローカル証拠でありGit管理対象ではない。

```sh
# 既定devshellのlld、Deno依存とPlaywright Chromiumを事前供給。
# 共有ライブラリ注入がbrowserと干渉する環境ではenv -u LD_LIBRARY_PATHで実行。
deno task test --jobs 8
# tests/e2e/README.mdに従い、製品buildをテストbuildと分離し
# YOI_E2E_BIN / YOI_E2E_SERVER_BIN / YOI_E2E_RUNTIME_BINを明示する。
cargo test --locked -p yoi-e2e --features e2e,fixture-support --test worker_restore -- --test-threads=1
cargo test --locked -p yoi-e2e --features e2e,restore-checkpoints --test worker_restore checkpoint:: -- --test-threads=1
cargo test --locked -p yoi-e2e --features e2e --test auth_passkeys -- --include-ignored --test-threads=1
cargo test --locked -p yoi-e2e --features e2e --test configuration_cli --test worker_environment -- --test-threads=1
# panel/rewindのみTUI用e2e-test buildを供給する。
cargo test --locked -p yoi-e2e --features e2e --test panel --test rewind -- --test-threads=1
```

最終root `cargo check`、全Rust format、`git diff --check`、状態契約命名検査は成功。命名検査は639該当行・未承認0・古い例外0。取得exampleの内部変数は`source_commit`へ変更し、既存fixture formatの保存keyだけを出自の説明付きで例外登録した。fixtureの保存bytesは書換えていない。

## 未確定原因を残した完了判断の撤回と追加検証

利用者の「独立した問題はスキップして報告」は、原因調査も打ち切ってよいという意味ではなかった。原因未確定のままCompletedとした判断を撤回し、T-871をreopenした。以下は`a15f99301`からの追加調査。前回の失敗ログは保持し、製品・保存fixture資産・timeoutは変更していない。

### 1. Chat接続browser: 現行UIへ追従していないdriver

最初の失敗を34.65秒で再現。安全な診断fieldを追加し、`open`/`subscribe`送信後に実BackendのSubscribe ACKをdriverが意図どおり保留したこと、保留中のまま、page errorなしで`Conversation history could not be loaded.`が表示されたことを確認した。これは認可拒否や履歴破損ではなく、意図したACK喪失の表示である。driverは旧`ConversationApiError`の汎用文言`outcome could not be confirmed`を待っていた。

`abf8439a9`で変更された`SubjectConversation.svelte`/chat pageに対し、明示driverだけが古かった。最初のalertだけを修正すると履歴は読み込めたが、次に旧`Start conversation body`が存在せず失敗した。現行DOM/sourceとの照合により、次も同じ追従漏れと確定した:

- body操作は折畳み`Subject controls`内の`Start Subject`/`Stop Subject`。
- 履歴listは`Conversation messages`。送信結果は長い説明文ではなくbubbleの`Retry message`/`delivered`。
- 送信ACK不明のalertは`Delivery could not be confirmed.`。

`f11835b77`でdriverを修正。旧文言を消すだけでなく、初回`delivery.state=unknown`、最後の同一入力retryは`accepted`、Subscribe/POST ACKが実際に保留・喪失したことを明示的に検証する。途中の喪失応答は保存済み`unknown`でも`accepted`でもよく、必ずacceptedとした追加診断assertは不適切だったため撤回した（中間失敗ログ保持）。同一の全入力3送信、model call数3、公開履歴2件、内部assistant文非公開、再読込み時のpending入力保持・重複なし、遅延ACK購読解除、body再起動、他Subject/他account分離は維持した。

修正後の実Browser/Backend/SQLite/signed Runtimeケースは**42.77秒・50.56秒の2回成功**。HTTP chat polling0、Subscribe/POST ACK故障注入あり、page errors0、各6 captures。既存binaryと同じproduction Web buildを使用し、製品変更で試験を通したわけではない。

証拠: `target/t871-cause-investigation/chat-before.*`, `chat-after-alerts.*`, `chat-current-ui.*`, `chat-current-ui-2.*`, `chat-repeat.*`。最初のprotocol/DOM証拠は`target/t855-validation/browser/messaging-1795076993565/review-context.json`、成功は`messaging-1795077273552`/`messaging-1795077538966`。診断は状態名・command種別のみでcredential/request本文は追加保存しない。

### 2. Navigation review: 削除済みUIと未対応WebSocket fixture

SubWorkerの保存Session・具体的差分・実行artifactを親が照合した。元の15秒timeoutを再現し、policyがロード済みでも`Operation` comboboxが存在しないことを確認。`f19b69061205bdec710dd843026b89549308e673`で旧editorが廃止され、現行`SubjectPermissions.svelte`はFeature参照とresolved operation keysを表示する。readerも旧`role=status / Read-only.`ではなく`Read-only policy access.`。policy応答自体は現行契約であり変更不要だった。

owner locatorだけを修正してreaderの旧期待値で失敗する対照実行を残した。両方を直すと次の追従漏れが順に現れたため、個別に観測・修正した:

- chatはHTTPではなくWebSocket `messaging_request/open`を送信するが、fixtureは`subscribe_events`以外を無視していた。送信frameと未応答を記録。navigation mode限定で空会話のopen/subscribe/unsubscribe応答を追加し、使われないHTTP mockを削除した。送信/Runtime処理をfake成功にせず、未対応commandは拒否する。
- chat入力は`textarea`ではなくCodeMirrorのcontenteditable textbox。DOMを記録してrole locatorへ変更した。
- body actionsはDOMに存在するが折畳みdetails内で非表示。summaryを開いてから従来の表示/no-alert assertionを検証する。

`e1a0882af`で修正し、**light/dark × 1440/768/390の6条件・各56 capturesを2回完走**。失敗0・予期しないfault0。15秒timeoutを維持。navigation・history・canonical/legacy URL・overflow・denied access等のassertionは維持した。shared fixtureを使うMemory/Journal browserも親側で3 tests＋10 steps成功（32秒）。

証拠: `target/t871-navigation-investigation/RESULTS.md`、before/段階修正log、WebSocket/DOM probe、`target/web-ux/t817/t871-cause-{before,corrected,corrected-repeat}`。同じbuild index hashで比較した。自動検証の成功であり、画像の目視承認や実Backend認可の証拠とは区別する。

### 3. doctest/Web timeout: 同一targetへ重ねたseed buildのCargo lock競合

前回の「cold内部buildが180秒を超えた」という説明を訂正する。doctestのstdoutは空で、stderrはpackage cache/artifact directoryのlock待ちだけだった。Webも`generate:profile-schema`の`cargo run -q -p manifest --example profile_authoring_schema`で止まり、Deno testは一件も開始していない。**テスト実行が遅くてtimeoutしたのではない。**

保存logの生成/更新時刻とbuild内容を照合した:

| 証拠 | 時刻（UTC、11/19） |
|---|---|
| 通常runner外側build | 07:05:11.946–07:10:33.964 |
| 別途起動したseed検証build log | 07:06:41.426–07:13:54.143 |
| doctest lock待ち→timeout | 07:10:34.897–07:13:34.902 |
| Web schema生成待ち→timeout | 07:10:34.927–07:13:34.933 |

`target/t871-restore-authority/seed-tests.log`は最初にartifact lock待ち、その後同じroot targetでtest-support付き依存/Serverをcompileし、`owner_account_id`不足で終了していた。外側build完了を、以後にCargoを使わない純粋なテスト実行とみなして別のbuildを重ねた私の実行調整が誤りだった。通常runner内には引き続きCargoを使うdoctestとWeb schema生成がある。

対照実験として、この専用checkoutの`target/debug/.cargo-lock`だけを`flock`で保持し、元と同じdoctest/Cargo schema生成commandを既存runnerの隔離・process回収で起動した。診断用3秒deadlineまで両方が進まないことを確認。doctestは元と同じ3つのpackage cache待ち＋artifact directory待ち、`cargo run -q`は診断を出さず待機するログを再現した。holderを回収した後、**外部buildを重ねず元の2入口を同時実行してdoctest23.39秒、Web unit7.43秒で成功**（9成功/21ignored、897成功）。製品・テスト・runnerの180秒timeoutは変更していない。

この原因への対処は、同一targetのcompileを親で直列調整し、必要なら明示的な別targetを使うこと。Web schema生成とdoctestを別々の重いbuildへ無条件分離する製品/runner改修は今回の原因修正として不要。warm一括完走とこの対照実験をcold全依存buildの性能保証へ読み替えないが、報告した2 timeoutの原因を「cold」や「不明」のまま残さない。

証拠: `target/t871-cause-investigation/lock.ts`, `lock-reproduction.json`, `lock-{doctest,schema-quiet}.*`, `uncontended.ts`, `uncontended.json`, `uncontended-{doctest,web-unit}.*`。既存seed失敗と07-05の元logは保持。

### 追加検証の境界

変更は明示browser driver/合成fixtureの3ファイルと本報告だけ。通常Rust/製品コード・固定restore資産は不変なので、通常399区分を再び全実行したとは主張しない。今回の実接続Chat2回、navigation2回、shared fixture回帰、Web unit/doctest対照実行、編集3ファイルのDeno typecheck、diff/命名検査で変更と原因を検証した。以前の全体計測における179.44秒shardの余裕の小ささは実測値として残すが、失敗原因不明と同一扱いにしない。未実行の物理認証器/全platformや起動区間の未細分化は検証・計測範囲の限界であり、観測済み失敗の原因として推測しない。
