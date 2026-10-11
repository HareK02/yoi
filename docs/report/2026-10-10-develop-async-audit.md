# develop 非同期・排他制御監査（2026-10-10）

## 結論

優先すべきは Mutex の一律削減ではなく、**制御系を止める backpressure、deadline の抜け、async executor 上の同期 I/O、キャンセル時の所有権喪失**の解消である。その後に、読み取りの過剰排他と独立した初期化・取得処理の直列化を修正する。

ソース上で根拠を確認した指摘を16件に整理した。P1は5件、P2は10件、P3は1件。条件付きのリスクを含み、以下は本番での発生件数や実測性能の報告ではない。

## 対象と検証方法

- Repository: `main`
- 対象: 取得時の最新 `develop`、`a9a67fd6adaf31e842faddb29d175d0f0b8bc2de`
- HEAD と `git ls-remote origin refs/heads/develop` の一致、および調査開始時の clean 状態を確認。
- 主な調査領域: workspace-server、worker-runtime、worker、workdir、agen、mcp、session-store、standalone、feature-storage、tools。client、TUI、Webの関連経路も確認。
- Mutex/RwLock、同期 filesystem/SQLite、awaitを含むループ、queue/semaphore、timeout/cancellationを探索し、該当関数と呼び出し側を読む静的監査。テスト専用コードのロックを製品問題には数えていない。
- 再現テスト・負荷試験は実施していない。すべての非同期関数を網羅した証明でもない。行番号は上記コミットに固定。
- `cargo check`: 成功。
- state-contract 命名検査: **既存状態で失敗**（unapproved 30行、stale exceptions 9件）。主に frozen repository-key migration と関連テスト・既存レポート。今回の非同期指摘とは別件であり変更していない。
- 実装ソース変更なし。本レポートのみ追加。commit/pushは行っていない。

### 優先度の意味

- **P1**: 条件成立時に進行不能、停止・キャンセル不能、広い executor 阻害につながるため優先修正。
- **P2**: 負荷、低速I/O、競合、キャンセル等で顕在化する応答性・所有権・スケーラビリティの問題。
- **P3**: 正しさを壊さないが、独立処理の不要な待ちを増やす改善。

## 一覧

| ID | 優先度 | 指摘 |
|---|---|---|
| A01 | P1 | Worker protocol の双方向 bounded queue が循環待ちを作る |
| A02 | P1 | Backend の async DB API が同期 Mutex・SQLite を直接実行する |
| A03 | P1 | MCP の request/shutdown timeout が stdin 待ちを覆わない |
| A04 | P1 | Codex refresh の body 待ちが auth Mutex を無期限に保持し得る |
| A05 | P1 | Agen の一部 policy gate が Cancel/Pause を処理しない |
| A06 | P2 | External Workdir の output 待ちが cancel 用の枠まで消費する |
| A07 | P2 | Workdir session cleanup のキャンセルで未完了レコードを失う |
| A08 | P2 | LocalWorkdirSession の async API が同期 filesystem/search を直接実行する |
| A09 | P2 | Bash output pump が無制限に drain し timeout/cancel に戻らない |
| A10 | P2 | Workdir 全体の排他 try_lock が並列 Read/Search を busy エラーにする |
| A11 | P2 | Workspace API cache の全体ロック中に初期化し、Runtime attach も直列 |
| A12 | P2 | FeatureStorage の管理 Mutex 内で fsync・DB open/migration/backup を行う |
| A13 | P2 | 大きな paste の永続化が Worker の async 実行を同期的に止める |
| A14 | P2 | PDF parser の blocking task が tool キャンセル後も残る |
| A15 | P2 | Backend TUI が同期 poll と操作完了待ちを入力ループに置いている |
| A16 | P3 | 独立した画面データ取得を直列化している |

## 詳細

### A01 — Worker protocol の双方向 backpressure による循環待ち

**根拠**

- `crates/workspace-server/src/server.rs:29214–29234`: WebSocket の受信 branch 内で method forwarding を await。その間 `connection.events` を消費しない。
- `crates/workspace-server/src/server/worker_operations.rs:592–607`: Worker ごとの session Mutex を保持して `methods.send(method).await`。
- `crates/workspace-server/src/server.rs:29324–29337, 29366–29397`: methods容量256、events容量512。bridgeは同じ select loop でmethod受信とevent送信を処理し、event送信の容量待ち中にmethodを消費できない。

**発生条件と影響**

bridgeが満杯のevent queueへの送信で停止し、browser側がmethod queueを満杯にして次の送信を待つと、`browser→method容量→bridge→event容量→browser` の循環待ちになる。session Mutexまで保持されるため同じWorkerの別接続からの送信やcleanupも巻き込む。これは単なる遅延ではなく、双方が生存したまま自力で進めなくなる構造。

Standaloneにも類似の制御阻害がある。`crates/standalone/src/host.rs:592–612, 674–678` はevent送信完了までmethod受信を止める。`crates/client/src/transport/in_process.rs:7, 28–29, 73–74` の256件channelが満杯になると、接続を維持した遅いconsumerのためにCancel/Pause/Shutdownのdispatchも止まる。この類似箇所単体の優先度はP2相当。

**改善**: 順序を保つevent writerとmethod/control受信を独立して進行させる。bufferはboundedのまま、lag時の切断・再同期方針と両pumpの共通終了処理を明示する。session/lifecycle排他を無期限のtransport容量待ちに使わない。ただし排他を単に除去してattach/stopとの整合性を崩してはいけない。

**回帰検証**: 双方のqueueをbarrierで満杯にし、Cancelまたは接続終了で規定時間内に収束すること。同じWorkerの別接続も巻き添えで停止しないこと。

### A02 — async と宣言した DB API の中身が同期 Mutex + SQLite

**根拠**

- `crates/workspace-server/src/store.rs:2108–2109, 2156–2172`: 共有 `Arc<Mutex<Connection>>` を同期取得し、そのままSQLを実行。
- 同 `3941–3951, 3954–4011`: `async fn get_workspace` / `update_workspace_display_name` が同期 query/IMMEDIATE transaction を直接実行。
- 同 `12315–12321`: SQLite busy timeoutは5秒。
- `crates/workspace-server/src/worker_projection.rs:167–172`: Tokio taskからも同期DB mutationへ進む。
- `crates/workspace-server/src/server.rs:19146–19161, 19201–19232`: Subjektiv のasync handlerも `open_subjektiv_store` と同期store処理を直呼び。
- `crates/subjektiv/src/lib.rs:624–671`: store openで `FeatureDatabase` を開き、scope確認にもtransactionを使用する。

**影響**: DB競合・fsync・低速storageがTokio executor threadを占有する。同じconnection Mutexを待つリクエストが増えると、複数のexecutor threadが同じDB待ちに使われる。`.await`という表記だけではyieldせず、外からのTokio timeoutも実行中の同期SQLを中断できない。

Subjektivは短命なstoreのopenを行ごとに繰り返す経路もある。他に強参照がない場合、FeatureStorageのweak cacheが失効し、DB初期化経路まで再実行し得る（A12参照）。

**改善**: connection取得からtransaction完了までを、boundedなblocking実行または専用DB threadへ移す。asyncメソッドだけでなくprojection等の同期API callerも対象にする。connection共有の排他とcommit→publication順序は維持する。`std::Mutex`を`tokio::Mutex`へ替えるだけではSQLのblockingは消えない。読み取りpoolはその後、整合性要件と負荷測定を踏まえて検討する。

**回帰検証**: 別connectionでwrite lockを保持し、更新APIが待つ間もtimer/health/control taskが進むこと。queue上限・overload時の応答・transaction順序も検証。

### A03 — MCP の timeout が最も詰まりやすい stdin 書き込みを覆っていない

**根拠**

- `crates/mcp/src/stdio.rs:905–910`: requestのwriteを終えた後に `request_timeout` を開始。
- 同 `1509–1524`: stdin Mutex取得、`write_all`、`flush`にdeadlineがない。
- 同 `1388–1396`: stdout readerからの応答書き込みも同じstdinを使う。
- 同 `1015–1036`: shutdownもstdin Mutex取得後にprocess待ちtimeoutを開始。
- caller: `crates/worker/src/feature/mcp.rs:1085–1094` のtool実行 → call_tool → request。

**影響**: 初期化後にMCP serverがstdinを読まなくなりpipeが埋まると、requestは設定timeoutに到達しない。reader側の返信がstdinを保持して止まった場合、shutdownもterminate/kill fallbackへ進めない。

**改善**: lock取得・frame書き込み・response読み取りを一つのdeadlineで覆う。shutdownはstdin取得失敗でもprocessを止められる経路を持つ。JSON-lineの途中でwriteをキャンセルした場合は接続を再利用しないか、完全frameを所有するwriter taskで扱う。frame混線防止のMutex自体は必要。

**回帰検証**: initialize後にstdinを読まないmock serverとpipe容量超過のrequestを使い、requestとshutdown双方が期限内に終了すること。途中write後の不完全frameを次requestへ継承しないこと。

### A04 — Codex auth Mutex を保持したまま deadline のない response body を待つ

**根拠**

- `crates/agen/src/providers/codex/mod.rs:78, 98–139`: default HTTP clientを使い、auth state Mutexをload・refresh・persist全体で保持。
- `crates/agen/src/providers/codex/refresh.rs:46–63`: 30秒timeoutは `send()` まで。その後の `json().await` / `text().await` は対象外。
- `crates/agen/src/llm_client/transport.rs:545, 648–650`: dynamic header解決はLLM HTTP送信timeoutより前。

**影響**: refresh先がheaderだけ返しbodyを完了しないと、同じproviderを共有する認証要求がすべてMutex待ちになる。明示的キャンセルは別として、自動timeout/retryでは復旧しない。

**改善**: response body消費まで含むrefresh全体のdeadlineとbody size上限を置く。token rotationのsingle-flight性は残す。ロックを外して同じrefresh tokenを同時利用する修正は不適切。

**回帰検証**: header送信後にbodyを停止するHTTP stubで、refreshと後続callerが有限時間で戻り、次回要求を処理できること。

### A05 — Agen の fallback pre-tool / turn-end policy gate が Cancel/Pause を処理しない

**根拠**

- `crates/agen/src/engine.rs:1988–1997`: `execute_tools` のpre-tool interceptorを直接await。
- 同 `2089–2090`: その後provider taskを開始するまでに再度のCancel/Pause確認がない。
- 同 `2952–2960`: assistant-turn-end interceptorも直接await。
- 対照: 同 `1534–1571` のearly admissionはCancel/Pauseを `select!` で処理。
- `AfterResponse` とearly dispatch非対応時のfallbackで到達する（同 `2717–2737`）。

**影響**: policy futureがpendingの間、Cancel/Pauseはqueueに残る。policyが後からContinueを返すと、要求済みキャンセルを処理する前に新しいproviderを開始し得る。turn-end待ちではearly-start済みtoolの制御も遅れる。Worker標準hookには30秒のchain期限があるが、promptなキャンセルとは異なる。

**改善**: early admissionと同じ制御signal監視を適用し、provider所有権の開始直前にも確認する。turn-end中断では開始済みbatchを明示的にterminalizeする。policyや永続化hookを無条件に並列化する話ではない。

**回帰検証**: AfterResponse/fallback双方でpre-toolをpendingにしCancel/Pauseを送り、toolが開始されないこと。turn-end待ちも同様にearly-start済みtoolが回収されること。

### A06 — External Workdir の output 待ちが cancel の admission まで塞ぐ

**根拠**

- `crates/workspace-server/src/server.rs:938–948, 969–982`: 全operationが同じsemaphoreを取得。timeout開始はpermit取得・enqueueの後。
- 同 `1091–1098`: waiting command outputは610秒のtimeout。
- 同 `1455–1463`: command cancelも同じoperation経路。
- 同 `23444–23452, 23532–23538`: connection-wide admissionとpending上限は16。
- `crates/workdir/src/local.rs:1930–1952`: wait=trueのoutputはcommand終了を待つ。

**影響**: 16件のoutput待ちで枠が埋まると、そのcommandを終わらせたいcancel自身が枠待ちになる。cancelの通常30秒timeoutはadmission待ちを制限しない。closeもcommand cancelを経由するため影響する。永久deadlockとは限らないが、長い待ちを自己増幅する。

**改善**: admissionとpending上限の両方でcontrol用capacityを確保し、permit/enqueue待ちを含む絶対deadlineを使う。単にcancelを無制限に通してprovider上限を破らない。

**回帰検証**: output待ちでdata枠を埋めてもcancel/closeが規定時間内に受理されること。provider全体の同時実行上限は維持されること。

### A07 — session cleanup の remove-before-await がキャンセル安全でない

**根拠**

- `crates/workspace-server/src/server.rs:515–568`: session/commandをregistryから取り出して削除。
- 同 `587–609, 618–638`: local Vecを順に `session.close().await` し、失敗項目はloop終了後だけ復元。
- 同 `17524–17539`: attachment単独cleanupも同型。
- 同 `1488–1518`: external sessionは明示的にprovider commandを終了させる必要がある。

**影響**: cleanup futureがpending中にdrop/abortされると、未処理項目・close中の項目・復元前の失敗項目を失う。retryがregistryから発見できず、external commandが動作したまま未追跡になる可能性がある。通常のHTTP切断だけで必ずhandlerがキャンセルされる、とまでは主張しない。

**改善**: unresolved項目をregistry内に保持するか、drop時に同期復元できるRAII所有権を用いる。あるいはrequest futureから独立したsupervised cleanupへ所有権を移す。成功確認後にだけ恒久削除する。

**回帰検証**: closeをbarrierで停止してcleanup futureをdropし、retryが全未完了項目を発見・closeできること。直列loopを並列化する前にこの所有権契約を直す。

### A08 — LocalWorkdirSession の async interface が同期 filesystem/search を直呼び

**根拠**

- `crates/workdir/src/local.rs:1471–1517, 1724–1828`: checkout search / read / write / edit / list / glob / grepが `fs_operation::run_*` を直接実行。
- `crates/worker-runtime/src/http_server.rs:880–924`: Runtime HTTP handlerがsession dispatcherを直接awaitし、ここにはblocking executorへの移動がない。

**影響**: 大きなtreeのgrep、低速mount、file I/Oがexecutorを占有する。同じthread上のtimer・control・別requestはその間進めない。async signatureもFuturesUnorderedも、poll内の同期処理を非同期化しない。

**改善**: provider内部でownedなrequest、session/resource guardをblocking側へ移し、同時実行数を制限する。scope検証・観測条件・mutationの順序を維持する。outer futureが消えても実I/Oが残るため、resource所有権をwaiterだけに持たせない。

**回帰検証**: 制御可能な遅いfilesystem adapterで処理を止め、同一executor上のtimerと別sessionのcontrolが進むこと。closeとblocking I/Oのresource lifetimeも検証。

### A09 — Bash output pump は select 内でも協調的に戻らない

**根拠**

- `crates/workdir/src/local.rs:2256–2298`: timer/cancel/child waitとoutput tickをselect。
- 同 `2371–2400`: output tick内の `publish_available_output` が同期readを、EOFまたは短いreadまで繰り返す。chunkは有限だが、一回の呼び出しで処理する総量は無制限。

**影響**: stdoutがread/telemetry処理より速く増え続けるとdrainが戻らず、stderr・timeout・cancelをpollしない。有限でも大量backlogで長い遅延になる。`select!`で囲っただけではbranch内の長い同期loopをプリエンプトできない。

**改善**: 1回のpumpにbyte/chunk budgetを設け、制御loopへ戻る。disk readはblocking境界または専用pumpへ移す。stdout/stderrの公平性と最終drainの完了条件を分ける。

**回帰検証**: 常にfull chunkを返す制御可能なreaderで、1 tickが有限量で終了すること。大量出力中もtimeout/cancelが規定範囲で処理されること。

### A10 — Workdir 共通 try_lock が独立した読み取りを失敗させる

**根拠**

- `crates/workdir/src/local.rs:650`: session内の `checkout_lock: StdMutex<()>`。
- 同 `1477–1513, 1526–1544, 1563–1567`: search/observe/execute全体で排他。
- 同 `1734–1751`:通常readも同じ排他 `try_lock`。競合時は `filesystem provider busy` を返す。

**影響**: 同じsessionの異なるファイルのReadやread-only searchまで排他になる。複数executor threadから重なると待つのではなく一方が失敗する。単一threadでは同期I/Oのため事実上直列に見えるが、A08を修正して並列性を出すだけではbusyエラーが増える。

**改善**: 何を守る排他かを分ける。観測済み対象のread-only処理を共有許可し、mutationのvalidator確認＋更新は必要な単位で排他。same-target競合、親directory変更、scope/checkout lifecycleは維持する。単にMutexを消すのではなく、read gateとmutation gateを設計する。

**回帰検証**: 同じsessionで別ファイルを同時readして両方成功すること。writeとの競合では古い観測に基づくmutationを引き続き拒否すること。

### A11 — Workspace API cache の全体ロックで初期化し、Runtime attach も直列

**根拠**

- `crates/workspace-server/src/server.rs:2224–2238`: `apis` 全体Mutexを保持して `WorkspaceApi::new(...).await`。
- 同 `3516–3541`: config/storage初期化とRuntime projection attachを含む。
- 同 `3525–3529`: Runtimeをfor loopで逐次attach。
- `crates/workspace-server/src/worker_projection.rs:131–146`: 全Runtime共通のattach_gateを保持し、初回snapshotを最大2秒待つ。

**影響**: 一つのWorkspaceの初期化が、別Workspaceのcache hitや作成・削除準備まで待たせる。snapshotが来ないRuntimeがN件なら、初期化のsnapshot待ちが概ね2秒×Nに積み上がり得る。cached routerへの通常dispatchはapi cacheを通らない早期returnがあるため、「全APIが止まる」とはしない。

**改善**: map lockをlookup/publicationだけに短縮し、Workspace単位のsingle-flightを外で行う。publish時には削除/lifecycle状態を再確認。Runtime attachは同一Runtimeの置換だけを直列化し、異なるRuntimeはbounded concurrentにする。callerをjoin_allにするだけでは全体attach_gateで再直列化される点に注意。projection commit順序のgateは維持する。

**回帰検証**: Workspace Aの初期化を止めてもBのcache hitが戻ること。複数Runtimeの初期snapshot待ちが重なること。同一Runtime置換で旧consumerのeventが混入しないこと。

### A12 — FeatureStorage の管理 Mutex に disk/SQLite lifecycle 処理を含めすぎる

**根拠**

- `crates/feature-storage/src/lib.rs:258–290`: manager state Mutexを保持したままdirectory作成・fsync、SQLite open/configure/migrate。
- 同 `719–742`: configured/resolved ancestorへのdirectory sync。
- 同 `750–795`: WAL設定・busy retry・IMMEDIATE migration。
- 同 `293–334`: scope backupもmanager lockを全処理中保持。

**影響**: 一つのDB open/migration/backupが、同じmanagerの別Featureのcache hitやopen/lifecycle操作まで塞ぐ。接続処理の遅さをregistry全体の待ちへ広げている。BackendはWorkspaceApiごとにmanagerを構築しているため、現行Backendのこのlock単体が全Workspaceをまたぐとは主張しない。backupの本番利用頻度は未測定。

**改善**: scope/Feature単位のinitializing状態とsingle-flightを使い、global管理lock外でI/Oする。open中のdelete/shutdownを防ぐreservation・fenceを設け、publish前に確認する。backupのmaintenance exclusionも対象scope単位に限定する。

**回帰検証**: Feature Aのopenを止めても既存Feature Bの取得が戻ること。open途中のdelete/shutdownでconnectionやdirectoryを再生しないこと。

### A13 — paste 永続化の同期 lock/fsync が async Worker を止める

**根拠**

- `crates/worker/src/worker.rs:4793–4813`: async runの中で `materialize_large_pastes` を同期呼び出し。
- 同 `5449–5454`: Storeのpaste artifact書き込みへ進む。
- `crates/session-store/src/fs_store.rs:425–435`: append Mutexを保持して保存。
- `crates/session-store/src/paste_artifact.rs:83–91, 136–149`: blocking filesystem lock、aggregate使用量scan、write/fsync/rename/directory fsync。
- `crates/standalone/src/host.rs:681–690`:Standaloneの実StoreはこのFsStore。

**影響**: 遅いstorageやaggregate lock競合で、Workerをpollするexecutorが停止する。特にcurrent-thread runtimeではtimer/controlまで止まる。複数threadでもexecutor枯渇要因。

**改善**: ownedな保存jobをbounded blocking実行または専用persistence serviceへ移し、完了ack後にinputをpublishする。aggregate quota、append順序、durable commit-before-publicationは正しい要件であり削除しない。

**回帰検証**: aggregate lockを別threadで保持し、保存待ち中にtimer/controlが進むこと。ack前にはinput commitを公開しないこと。

### A14 — PDF extraction はキャンセル後も blocking task が走り続ける

**根拠**

- `crates/tools/src/web.rs:825–834`: `spawn_blocking(pdf_extract...)` のJoinHandleをその場でawait。
- `crates/agen/src/tool.rs:474–491, 593–604`: force-closeはouter async taskをabortし、default cooperative cancellationは何もしない。

**影響**: parser開始後にouter futureをdropしてもblocking closureは止まらない。toolがOutcomeUnknownで終わってもCPU/memory消費は続く。繰り返しキャンセルで処理が残り得る。入力byte上限とHTTP timeoutはparser実行時間の上限ではない。

**改善**: 実blocking処理が保持するconcurrency permitとprovider側のtask所有権を用意する。厳密な時間・停止保証が必要ならkill可能なprocessか協調キャンセル可能なparserを使う。JoinHandleにtimeoutを付けるだけでは実処理を停止できない。

**回帰検証**: parserをbarrierで停止してouter toolをキャンセルし、残存処理が同時実行枠を保持していること、無制限に後続parserを増やさないことを確認。

### A15 — Backend TUI の event loop が同期pollと操作待ちを抱える

**根拠**

- `crates/tui/src/backend_dashboard.rs:84–105`: async run_loop内で `event::poll(100ms)` / `event::read`。idle時はawaitせずloop継続。
- 同 `148–186`: reload、Intake/Orchestrator起動を入力branch内で完了までawait。
- 対照: `crates/tui/src/console/mod.rs:673–717` は専用terminal reader threadを持つ。

**影響**: idle中もasync taskが同期pollを繰り返し、pollからreturnしない。さらにHTTPをspawn_blockingへ移していても、UI loopがその完了を待つため、その間q/Esc、navigation、再描画を処理できない。I/OのoffloadとUI応答性は別問題。

**改善**: consoleと同様のevent readerを共有し、入力・結果・tickをselectする。ネットワーク操作は所有権を追跡したtaskとして走らせ、UIはpending表示と結果message処理を行う。キャンセルでmutation結果が不明な場合は失敗扱いで再送しない。

**回帰検証**: Backend応答を停止してreload/launchし、q/Escと再描画が引き続き動くこと。二重起動を許さないこと。

### A16 — 独立した画面データ取得が不要に直列

**根拠**

1. `web/workspace/src/routes/w/[workspaceId]/settings/runtimes/+page.ts:7–31`: Runtime listの完了後にsigning identity取得を開始。どちらも入力はworkspaceIdだけ。
2. `crates/tui/src/backend_dashboard.rs:62–70`: Ticket一覧とObjective一覧を一つのblocking closure内で順番に取得。
3. `crates/tui/src/backend_worker_picker.rs:96–116`: active/stopped一覧を順番に取得。include_stopped時は独立に開始できる。

**影響**: 依存関係がないのに待ち時間が各requestの和になる。実測値ではないが、二つの独立待ちの理想的critical pathは和ではなく最大値。

**改善**: WebはPromise.all、TUIは二つのtaskを同時開始してjoin、またはasync clientを直接使用。現在の部分失敗diagnosticsと表示順は維持する。単にjoinするだけで同じblocking closure内を並列にできるわけではない。

**回帰検証**: 一方のresponseを止めても他方が開始済みであることをbarrierで検証。失敗時の個別error表示も維持。

## 問題と数えなかったもの

- 短い共有state/registry更新の `std::Mutex`。awaitを跨がず短時間なら妥当であり、async Mutexへの一律置換は不要。
- `crates/agen/src/tool_server.rs:123–132`: toolをcloneしregistry lock解放後に実行する構造。
- 同一対象のmutation、SQLite transaction、durable commitとpublication順序の直列性。
- MCPのstdin frame混線防止、Codex token refreshのsingle-flightそのもの。
- `workspace_drive_host.rs:77–98` のlazy-open排他。初期化重複を防ぐ目的があり、今回の指摘はそれ自体ではない。
- Web dashboardは `web/workspace/src/lib/workspace/home/dashboard.ts:116–125, 165–206, 225` で独立requestを開始し、HOME_LIMIT=5のdetailをPromise.allで取得している。for/awaitの存在だけで直列化と判定していない。
- Workdir lifecycleや権限境界の排他は、単純な性能改善のために除去しない。

## 推奨する修正順と設計上の注意

1. **制御の進行保証**: A01/A03/A04/A05。満杯queue、partial body、policy pendingを使う決定的テストを先に用意する。
2. **executor上のblocking解消**: A02/A08/A09/A13。boundedな実行数、実処理側resource所有権、commit ackをセットで導入。無制限spawn_blockingへの置換はしない。
3. **キャンセル・cleanup所有権**: A06/A07/A14。control用capacityと実task lifetimeを明示する。
4. **排他粒度と独立処理の並列化**: A10/A11/A12/A15/A16。まず保護対象を明確にし、read-only/別Runtime/別Workspaceを分ける。

A08とA10は同時に扱う必要がある。同期I/Oをoffloadして並列に動くようにすると、現在のtry_lockによるbusy拒否が顕在化しやすくなる。またA11はcallerのjoin_allだけではattach_gateで再直列化される。

性能改善の測定は、mutex取得待ち/保持時間、DB job queue待ち、blocking job稼働数、control dispatch遅延、初期化critical pathを分けて行う。高負荷時のp95/p99とキャンセル完了時間を観測し、根拠なくmutex数の減少を成果指標にしない。
