# T-871: 全体時間分布とテスト責務の見直し

通常Rustの全選択名を計測し、最初の改善として保存形式の履歴組合せを純粋な判定と実DB接続へ分離した。これは全テスト最適化の完了報告ではない。browser/E2E、残る重い接続群の内部内訳、変更後の全体実行は未完である。

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

最終sourceのworkspace test build、root `cargo check`、`cargo fmt --all -- --check`、`git diff --check`、状態契約命名検査は成功した。全体テストの変更後再実行は未完である。

## 残る調査・検証

- combined failure、backend_job upgradeなどの別の巨大行列も同じ責務分離が適用できるか調べる。
- Standaloneは実SQLite busy_timeoutを受ける8passの検証。実Busy・retained recovery・元権限保持は必要だが、50秒自体が契約か、既存接続に短い待機上限を注入できるかは未解決。
- Chat/Journalの認可再確認・実Session purgeは結合が必要。共通fixture内の付随認可検証、HTTP client構築、設定解析等の各寄与は未計測。
- Web unitは成功。componentは867件中855成功・12失敗、Web UXは既存fixtureのTypeScript型エラー。doctestの先行試行は並行編集中のsourceをcompileして失敗したため、baseline/最終検証の根拠から除外する。
- browser/E2Eの選択条件と前提を確認したが、実行時間の収集は未完。ignored/live APIを実行済みとしていない。
- 最終sourceの全体回帰、変更後全時間分布、未改善コストを確認してからTicketの充足を判断する。

主要証拠はTicket T-871とWorkdirの `target/t871-parent/`、移行の故障検出対応表は `target/t871-migration/optimized-handoff.md` に保持。target配下のログ・scriptは再利用可能なローカル調査資料であり、製品の正本や恒久APIではない。
