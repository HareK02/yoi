# T-871: 全体時間分布とテスト責務の見直し

通常Rustの初回8,039名を計測し、保存形式の履歴組合せを純粋な判定と実DB接続へ分離した。追加で実SQLite Busyの待機、未使用のWasmコンパイル、移行dry-runの反復、Job cleanupの過大fixtureを削減した。専用Workdirで8,045名を再照合し、修正targetの再実行を合わせて7,999成功・46既存ignored、欠落・重複0を確認した。Web実browser計32ファイルも成功。ただし単一の最終ソースを通常runnerで一度に完走した記録ではなく、restore/checkpoint用の固定fixture不整合など未解消項目が残る。全体完了とはせず、最新の採否・検証範囲は末尾の継続検証・fixture再生成の節に記録する。

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
