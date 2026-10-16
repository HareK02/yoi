# テストスイートの実行時間調査

対象: `7042ec7d` (`hare/develop`, develop取り込み後)。以下の初回調査では製品コード・テストコードの変更なし。T-761による最適化後の測定は末尾に追記した。

## 結論

実行できた通常Rustテスト95バイナリの壁時計合計は **644.15秒（10分44秒）**。 `yoi-workspace-server` と `session-store` だけで **72.7%** を占める。 Webの通常テスト群は計約27秒であり、主なボトルネックではない。

全テスト成功という意味ではない。既存の `yoi` CLIテストのコンパイルエラー2件により、そのバイナリは測定対象外になっている。

## 条件・測定方法

- Linux、Rust 1.95.0、Deno 2.7.14、debug/test profile。
- `std::thread::available_parallelism()` は24。`RUST_TEST_THREADS` 未指定。
- リポジトリと `/tmp` は同じNVMe上のext4。専用・無負荷の測定ホストではない。
- crate内はlibtest既定並列、テストバイナリ間は逐次実行。SubWorkerには読み取り専用のコード調査のみを委任。
- ビルドと実行を分離。`cargo build --workspace --exclude yoi-e2e --tests --keep-going --message-format=json` で取得したtest executableを、それぞれのcrateディレクトリから実行。
- `RUSTC_BOOTSTRAP=1 <test executable> -Z unstable-options --format json --report-time` で個別時間を採取。`RUSTC_BOOTSTRAP`はこの計測用テスト実行プロセスに設定し、外側のCargoビルドには設定していない。テストが起動する子プロセスには継承される。
- 95バイナリで5,067 passed / 3 ignored / 0 failed。これはコンパイルできた対象だけの結果。
- 600秒のコマンド実行上限に達したため2バッチで実行。最初の未完了Server試行は集計せず、再実行して完走したServer測定だけを使用。644.15秒は完走した各バイナリの壁時計の合算であり、調査作業全体の所要時間ではない。
- 各条件1回の測定。キャッシュ・他プロセスの負荷による変動を含む。中央値や専用CI性能ではない。
- 個別テストの時間には同じバイナリ内の並列競合・ロック待ちも含まれる。個別時間の合計はスイートの壁時計と一致しない。

## 時間配分

### 外側のビルドとスイート

| 対象                                                                       |   壁時計 | 備考                                                                    |
| -------------------------------------------------------------------------- | -------: | ----------------------------------------------------------------------- |
| Rust test対象ビルド・初回                                                  | 164.86秒 | 既存キャッシュあり。不足する対象をコンパイル。CLIテスト2エラーで終了101 |
| 同じRust test対象ビルド・再実行                                            |   2.32秒 | 温まったキャッシュ。同じ2エラー。正常完走ビルドの時間ではない           |
| Rust unit/integration、95バイナリ                                          | 644.15秒 | 外側のビルドを除く。テスト内部のCargo起動は含む                         |
| Rust doc (`cargo test --workspace --exclude yoi-e2e --doc --no-fail-fast`) |  22.36秒 | コマンド全体。2 passed / 21 ignored、rustdoc起動等を含む                |
| Web unit (`deno task test`)                                                |  10.01秒 | 665 passed、コマンド全体                                                |
| Web component (`deno task test:component`)                                 |  11.30秒 | 413 passed、変換・起動込み                                              |
| Web UX tooling unit (`tools/web-ux: deno task test`)                       |   5.65秒 | 16 passed                                                               |

これらの実行可能なテスト群を逐次足すと、外側のRustビルドを除いて **693.47秒（11分33秒）**。追加の単独再測定・並列数比較はこの合計には含めない。

### Rust crate別（各crate内のバイナリ壁時計合算）

| crate                  |         秒 | 主な内容                                                       |
| ---------------------- | ---------: | -------------------------------------------------------------- |
| `yoi-workspace-server` | **376.68** | DB移行、DBを伴うAPI/authority等。Rust全体の58.5%               |
| `session-store`        |  **91.40** | 巨大な旧形式inline画像の1テストが90.07秒                       |
| `agen`                 |      34.94 | compile-fail/trybuildが27.35秒                                 |
| `worker-runtime`       |      32.44 | timeout解除確認の1テストが31.02秒                              |
| `api-macros`           |      24.51 | compile-failが19.32秒、renamed dependencyのCargo checkが4.96秒 |
| `yoi-plugin-pdk`       |      18.39 | wasmテンプレートのCargo check。2テストが並列で各約18秒         |
| `ticket`               |      16.76 | SQLite schema/assignment/tag等                                 |
| `standalone`           |      13.97 | Subject/Job等のintegrationも含む                               |
| `worker`               |       9.45 | unitとintegration 9バイナリ                                    |
| `tools`                |       8.29 | 大きいbinary readの1テストが5.74秒                             |
| `subjektiv`            |       7.89 | Journal Unicode/budgetケース等                                 |
| `merge-request`        |       2.73 | 主にstore integration                                          |
| `workspace-drive`      |       2.60 | 2バイナリ                                                      |
| `workdir`              |       1.71 | 6バイナリ                                                      |
| その他                 |     約2.39 | server-api等                                                   |

## 1. Workspace Server：DB fixtureの反復と同時実行時の増幅

### 遅いケースの単独比較

同じコンパイル済みバイナリ、同じ入力をフィルタして1テスト・1スレッドで実行した。

| テスト（名前の共通prefixを省略）                                                                                           | 全体実行中 |       単独 |
| -------------------------------------------------------------------------------------------------------------------------- | ---------: | ---------: |
| `store::tests::migration_resumes_from_a_valid_partially_applied_chain`                                                     |    95.94秒 | **4.71秒** |
| `store::tests::schema_v50_plans_and_applies_the_complete_retained_chain`                                                   |    80.77秒 | **4.88秒** |
| `store::tests::schema_v89_runs_auth_then_cases_preserving_legacy_states_evidence_and_responsibility`                       |    63.80秒 | **6.24秒** |
| `server::tests::assignment_start_tests::orchestrator_start_and_assign_reject_missing_stale_cleared_or_reissued_delegation` |    81.03秒 | **7.46秒** |

ほかに、`migration_dry_run_and_apply_share_the_original_repository_secret_authority` 87.11秒、`opening_schema_84_with_uncertain_removal_preserves_old_server_recovery` 83.41秒、Journal erased-archiveケース78.79秒、v82移行67.32秒、v90移行63.59秒。

**「95秒の単独処理が必要」ではない。** 並列実行条件で所要時間が大幅に増幅する。一方で、Server全体を `--test-threads 4` にすると **544.25秒** と既定並列の376.68秒より44.5%遅かった。同じ1,126件成功・2件ignored。個別の待ち時間を短縮しても全体の処理量が減るとは限らず、並列数を減らすだけの対策は採用できない。

単独比較と並列数比較はそれぞれ1回なので、CPU・ディスク・SQLite内部処理の寄与率までは確定していない。`fsync`回数やI/O待ち率のprofileは未取得。

### ソースで確認できた反復処理

- `crates/workspace-server/src/store.rs:17521–17600` の `prepare_schema_v52()` は実ファイルDBに凍結Ticket schema、MR schema、v85凍結schemaを構築後、旧schemaへ戻す。`prepare_schema_v50()` はさらにそれを加工する（同`:17603–17656`）。
- `create_frozen_schema_v85()`（同`:17407–17500`）が読む凍結SQLは1,654行。fixture全体を包むtransactionはなく、多数のDDL・seedを個別に実行する。
- `migration_plan()` は単なる版一覧ではなく、DBをメモリへbackupし実際にmigrationを適用する（同`:2501–2518`）。
- `migrate_database()` は再度planを行ってから実DBを移行する（同`:2521–2525`）。v50完全連鎖テストは51→98の連鎖を計3回適用する。
- 通常APIテストも `server.rs:56594–56608` 等で新しいDBを繰り返し構築する。Serverの遅さは旧schemaテストだけではない。
- SQLite設定はWAL・foreign keys・busy timeout 5秒（`store.rs:13515–13529`）。5秒timeoutは上限であって、毎回5秒待っている証拠ではない。
- `store.rs` に固定sleepや `serial_test` はなく、store mutexもインスタンス単位。

改善候補は、fixture構築のtransaction化、通常APIテスト用の構築済みDBテンプレートから独立DBを作る方法、fixtureの不要な重複構築の削減。移行そのものの正しさ・durabilityを試すテストと、APIの前準備を分ける。実装・改善後の実測はまだ行っていない。

## 2. Session Store：1テスト90秒、本番readerの二次量処理

対象: `public_snapshot::tests::large_legacy_inline_images_project_as_bounded_refs_and_fetch_from_retained_storage`

- 実測 **90.07秒**。crate全体91.40秒のほぼすべて。
- `crates/session-store/src/public_snapshot.rs:4395–4404` で4画像・計18,373,509 bytesを用意。base64部分だけで24,498,012 bytesの単一JSONL行になる。
- `crates/session-store/src/worker_session_store.rs:60,92–94,149–162` の逆方向readerは64 KiB読むたびに、蓄積済みbuffer全体を改行検索し、全bufferを新chunkへコピーする。
- 巨大な一行では、検索・コピーが概ね `O(N²/chunk_size)` になる。この計算量はソース上確認済み。
- テストはこの巨大行を逆方向に5回取得する。静的概算で検索約23 GB・コピー約23 GB相当の累積メモリ処理となる。これはディスク読取量ではない。
- さらにJSON/base64 decode・hash・cloneが重なる。90秒の内訳をCPU profileで割合分解したわけではない。

改善候補は、改行検索を新規chunkだけに限定し、蓄積bufferの毎回prependコピーを避ける局所的な線形化。画像fixtureを縮小してテストだけ軽くするより、本番の巨大inline画像取得も同時に直す方がよい。入力と回帰条件は維持する。

## 3. Runtime：31秒の固定待ち

`worker_source::tests::runtime_owned_workspace_client_has_no_fixed_request_timeout` は **31.02秒**。

`crates/worker-runtime/src/worker_source.rs:1818–1836` はreqwest既定30秒timeoutの解除を確認するため、明示的に31秒遅延を指定する。helperの`:1889` に `std::thread::sleep(delay)` がある。ネットワークや大入力ではなく、意図された固定待ち。

単にsleepを短くすると `.timeout(None)` を削除してもテストが通るため、回帰検知が弱まる。短い初期deadlineを持つbuilderに本番と共通のtimeout設定を適用し、その解除を小さい実時間で検証するなど、テスト境界の見直しが必要。現段階では提案のみ。

## 4. テスト内部のビルド

- `agen/tests/compile_fail.rs`: trybuildの2ケースで27.35秒。
- `api-macros/tests/compile_fail.rs`: UI compile-fail群で19.32秒。
- `api-macros/tests/renamed_dependency.rs`: 一時プロジェクトのoffline Cargo checkで4.96秒。
- `yoi-plugin-pdk/tests/template.rs`: wasmテンプレート2ケースが各18.36 / 18.13秒、並列のためスイート全体は約18.39秒。

これらは外側のビルドとは別にテスト実行中にRustコンパイラを起動する。外側がwarmでもゼロにはならない。キャッシュを安全に再利用できるかは、依存rename・compile-fail・テンプレート自立性の検証目的を維持した上で別途検討する必要がある。

## 範囲外・既知の失敗・再実行

- `yoi` CLIテストは `crates/yoi/src/ticket_cli.rs:1382` の `edit_post_with_snapshots` 未実装、同`:1306` の `TicketMeta.created_by` 欠落でコンパイル不可。今回修正していない。
- opt-inのprocess E2E（`yoi-e2e --features e2e`）、Playwright/browser suite、ignored real-APIテスト・性能benchmarkは実行していない。
- Nixの `checks.default` はパッケージビルドで、`doCheck = false`。通常Rust全テストがそこで走るわけではない。
- このcheckoutには実行中と確認できるCI workflow定義がなく、今回の数字をCIの所要時間とは扱わない。
- テストによる `ConfigTreeSnapshot.ts` の自動生成フォーマット変更は、実行前の内容へ戻した。

測定用スクリプト・生ログはGit管理外の `target/test-timings/` に保持:

- `profile.ts`: build/run/single/summary
- `artifacts.json`, `results.json`, `summary.json`: 対象と全Rustテスト時間
- `build.meta.json`, `build-errors.json`: 外側のビルド
- `single-*.stdout`, `server-threads4.*`: 単独比較と並列数比較
- `remaining.ts`, `remaining-results.json`, `web-components.json`: Web/doc/warm build

例:

```sh
deno run -A target/test-timings/profile.ts build
deno run -A target/test-timings/profile.ts run
# ツール側の実行時間上限で中断した場合（完了結果は各バイナリごとに保存）
deno run -A target/test-timings/profile.ts run --resume
deno run -A target/test-timings/profile.ts summary
deno run -A target/test-timings/profile.ts single yoi-workspace-server \
  store::tests::schema_v50_plans_and_applies_the_complete_retained_chain 1 single-v50
```

## T-761：最適化後の測定

`7042ec7d`上の作業差分で、変更した3 crate・通常8バイナリを再測定した。**合計500.52秒から317.72秒へ、182.80秒（36.5%）短縮**。1,796 passed / 2 ignored / 0 failed。ignoredは既存のままで、新しいskipや並列数制限は追加していない。

### 変更と検証範囲

| 変更                                | 小さい検証                                                                                                                                                                                          | 残した重要な接続                                                                                                                                                         |
| ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Session逆方向readerの線形化         | `retained_reader_*` 4件でchunk前後・正確な境界、budget不足からの再開、内容・offset・追加読取り量、不正行を検証                                                                                      | `large_legacy_inline_images_project_as_bounded_refs_and_fetch_from_retained_storage` の約24.5MB行と実storage再読取りは不変。legacy IDの既存検証も維持                    |
| Runtimeの31秒待ち除去               | `blocking_workspace_client_clears_initial_request_timeout` は初期20ms deadlineを渡し、本番の要求準備・timeout適用・HTTP送信・応答読取りを通す。要求受信後40ms経過を障害刺激にし、実応答で完了を判断 | `RuntimeOwnedWorkspaceClient` のdefault timeout設定、実HTTP成功、明示timeout指定とエラー報告を既存loopback fixtureで確認                                                 |
| 通常APIの空schema構築共有           | `api_fixture_databases_isolate_writes_and_reopen_persisted_rows` は同一PKへの並行書込みを独立DBで行い、再openの保存値、pristine DBへの非混入、WAL/FKとFK拒否を確認                                  | 本番schema初期化の結果だけをSQLite Backupで各ケースの実DBへ複製。認証router・API seed・store・executorは各ケースのまま。既存ファイルは通常openし、保存内容を初期化しない |
| 旧schema fixture構築のtransaction化 | `prepare_schema_v52` / `prepare_schema_v50` の準備用DDL・seedをまとめる                                                                                                                             | 本番migrationとFK設定切替はtransaction外。全移行連鎖、途中再開、rollback、保存値・制約の既存試験を非template経路で維持                                                   |

局所入力規則には既にDB不要のテストがあり、入力表の削除や大量のテスト再配置は行っていない。通常API用の3 constructorだけをtemplateへ変更し、移行・fresh作成試験が共用する `open_for_test` 自体は変更していない。

### 時間比較

初回と同じWorkspace test buildのfeature構成、debug profile、libtest既定並列、バイナリ間逐次、JSON report-timeで測定。外側のビルドは除外し、変更crateの全通常バイナリを選択した。最終sourceの測定は1回であり、専用・無負荷ホストの中央値ではない。

| crate                  |     初回 | 最終差分 | 短縮率 |
| ---------------------- | -------: | -------: | -----: |
| `session-store`        |  91.40秒 |   9.77秒 |  89.3% |
| `worker-runtime`       |  32.44秒 |  20.25秒 |  37.6% |
| `yoi-workspace-server` | 376.68秒 | 287.70秒 |  23.6% |
| 合計                   | 500.52秒 | 317.72秒 |  36.5% |

巨大画像テストは90.07秒から8.01秒へ短縮。Runtimeの新timeoutテストは0.078秒だが、crate全体では他の復旧・Worker構築試験が支配的になり、31秒分がそのまま短縮されるわけではない。

先行測定でもSession 9.83秒、Server 291.02秒だった。Runtimeは先行測定19.05秒だが、後述のレビュー修正前なので最終sourceとの統計平均には使わない。

**通常Rust全95バイナリは再実行していない。** 未変更87バイナリの以前の143.62秒を足すと約461.35秒（7分41秒）、以前の644.15秒より約28.4%短い見積りになるが、これは全体の再実測値ではない。Web、Workspace全doctest、opt-in E2E/browserも再測定していない。

### 検証とレビュー

- 親でreader局所テスト、巨大画像、Runtime timeout/実HTTP、DB隔離、v50完全連鎖・途中再開を先に実行して成功。その後、変更3 crateの全通常8バイナリを既定並列で測定し成功。
- root `cargo check`、3 crateのdoctest選択（対象0件）、`cargo doc -p yoi-workspace-server --no-deps`、全体fmt/diff、状態契約命名検査が成功。既存のServer dead-code警告3件は残る。
- 初回と同じ全Workspace test buildで測定用バイナリを生成した。既知のTicket CLIテスト2エラーは継続し、build全体はexit 101。変更対象8バイナリは生成・実行できており、このbuild失敗を全テスト成功へ読み替えていない。
- readerとServer/Runtimeを独立にレビュー。Runtime初版はtimeout helperだけを通っており、本番呼出し漏れを検知できないというP2指摘を修正した。最終版は本番 `execute_blocking_inner` を通る。
- 本番のtimeout適用を一時的に `Some` の場合だけへ変更する故障注入で、新テストが0.04秒で実timeoutを検出して失敗。復元後0.07秒で成功。再レビューでP2解消、追加P1/P2なしを確認。
- 通常テストのfeature/target選択は変更なし。checkoutに通常suiteを実行するCI workflowは確認できず、Nixの`doCheck = false`も不変。ローカル成功をCI成功とは扱わない。

残る大きなコストはServerの実移行・復旧試験であり、並列実行中には完全連鎖等の個別時間が依然として長い。今回の短縮はクリティカルな接続を削除せずに得たもので、全suiteのボトルネックを解消したという主張ではない。

追加の生ログ・測定用scriptはGit管理外の `target/test-timings/` に保持する。

- `optimized.ts` / `optimized-artifacts.json`：初回と同じbuild方式と変更8バイナリの選択。
- `optimized-results-1.json` / `optimized-results-2.json`：先行測定／最終差分の測定。各バイナリのstdout/stderr/metaも保存。
- `timeout-mutation.stdout` / `.stderr`：None適用漏れの故障注入結果。
- `optimized-root-check.*` / `optimized-doc.*` / `optimized-doc-build.*`：compile・doctest・rustdoc結果。

再測定は `deno run -A target/test-timings/optimized.ts build`、続いて `deno run -A target/test-timings/optimized.ts run <識別子>`。通常の回帰検証には `cargo test -p session-store -p worker-runtime -p yoi-workspace-server` を選択できる。

## T-761追加要求：全通常suiteを1分以下へ

前節は最初の部分最適化であり、追加要求に対する全体再実行の結果ではない。以下ではCLIの既存test compile errorも修正し、通常Rust全96バイナリ、Workspace doctest、Web unit/component、Web UX tooling、root検証toolのテストを対象にする。外側buildは分離し、test内部の実Cargo起動やWeb変換、Rustケース列挙は実行時間に含める。以前から明示選択のE2E/browserとignoredは別枠のまま。

### 粒度・準備コストの変更

- Serverのconfig候補検証・cleanup分類・Ticket checkerの応答検証/描画など、局所規則を実DBなしのunitへ移した。実store入口の拒否と保存値不変、認証付き保存→Job結果→advisory配送は代表結合として残す。fakeモデル応答による分類試験を、モデル自体の判断精度の証明とは扱わない。
- 通常fixtureの空schemaを独立した実DBへ複製し、準備済みDBへの追加connectionだけ重複schema検証を省く。既存DB・再open・本番startupは検証を通し、trigger欠落の拒否試験を残す。旧schema templateはseed前の構造だけ共有し、対象区間のmigration試験と最古→最新の完全連鎖を分離する。実transaction・FK・WAL・rollback・保存値のassertionは維持。
- Driveページ上限試験の201件は、読取り前の準備だけ実ownerのmutationで作成する。認証付きTools/WIP読取り、ページ件数・cursor・全ID一致は変更しない。
- Journal復旧の3つのloopを6+2+4個の独立ケースへ分割。各ケース内の障害注入→復旧→再読取りは逐次のまま、DB/Runtimeは毎回独立。旧assertionと実reopenの全組合せは前後diffで独立レビュー済み。
- Runtimeの保存Manifest入力行列をunitへ分離し、実保存/再読取りとfactory入口の拒否を残した。論理timeoutには仮想時間、取消完了には開始/Drop通知を使い、固定sleepを除去。
- ViewImageの16 MiB caller-limit規則は局所readerへ移し、LocalWorkdirとの実接続はchunk境界を跨ぐファイルで検証する。Session Storeの巨大legacy行試験は入力サイズを維持。
- macroのrenamed consumerとWASM templateは、毎回独立sourceから実Cargo checkを行い、依存artifactのみ再利用する。成功判定はキャッシュしない。warm cacheに`compile_error!`を注入したtemplateが失敗し、復元後成功することを確認（`template-cache-mutation.*` / `template-cache-restored.*`）。
- test profileのSQLite Cコードをopt-level 2、Workspace Serverをopt-level 1とした。Rustのtest/debug assertionsとoverflow checksは無効化しない。通常のdev/release profileは変更しない。

### 全体runner

root `deno task test`（Linux）を追加。Cargoが選択したtest artifactを毎回列挙し、大きいバイナリを全ケース一意の複数processへ分配する。既存ignoredを含む全選択名は`inventory.json`に保存し、結果と対応付ける。未知listing・重複名・列挙失敗も全体失敗とし、成功結果によるskipは行わない。

並列上限はtop-level suite/shard数であり、内部Cargo/Vitest等まで含む総プロセス数ではない。実行上限180秒・外側build上限1,800秒は、性能目標60秒とは別のハング検出用。途中ログを保存し、timeout・SIGINT/SIGTERM時は同一process groupへTERM→KILLして結果を保存する。別sessionへ自ら離脱した子孫の回収は保証しない。runnerの独立レビューで初版の無期限待機を指摘され、修正後の再レビューではP1/P2なし。

### 途中の全体測定

- 初回runner：全101 suites成功、実行238.35秒、外側build75.20秒。1分未達。
- 32 top-level並列・Server32 shards版：96 binaries / 5,225選択名、全210 suites中205成功・5失敗、実行71.49秒、warm build0.40秒。失敗4件は待機上限超過、1件はWorkerの完了前履歴参照。この結果は成功扱いしない。`target/normal-tests/2026-10-16T05-39-04.512Z/`、`target/test-timings/final-normal-1.log`に保存。
- 同runの外側`time -p`はreal71.98/user702.72/sys352.49秒。平均約14.7 CPU相当であり、利用可能24論理CPUを有効計算で使い切った性能限界ではない。共有資源競合・I/O・後半の重いsuite待ちを含む。個々のCPU使用率やI/O寄与率の厳密な内訳をこの値だけから断定しない。

### 最終差分の検証結果（1分目標は未達）

同期修正後の最終runは **実行71.06秒・全210 suites成功・0失敗**。外側の変更分buildは48.56秒、コマンド全体は119.70秒。実行にはdiscovery 0.28秒を含む。キャッシュのある同一Linuxホストで1回の成功実測であり、cold build込み1分、CI性能、反復安定性の達成は主張しない。

- Rust通常96 binaries / 205 suites・shards：**5,222 passed / 3既存ignored**。実行結果合計とinventoryの全5,225選択名が一致することを別途検算した。
- Workspace doctest、Web unit **665**、Web component **413**、Web UX **16**、root tool **22** が成功。E2E/browserは未実行。
- root `cargo check`、Rust fmt、Deno fmt/lint、diff検査、状態契約命名検査（未承認0・stale0）が成功。
- Controllerは初期Idleではなく同じSubmitの完了fenceまで待つ。Protocolは2秒の実session lock解放検証を追加し、実Stop全体のwatchdogだけ15秒へ分離。Attentionは不要Git準備を削除し、実Restore中のrelease待ちだけ15秒へ変更。いずれも独立レビューでP1/P2なし。
- Runtime cleanupのサイズ比較は、同じGit入力から独立checkoutを作るよう修正。毎回別commitを作るとobject-prefixディレクトリ数も変わり得るため、「サイズだけ変える」という比較条件を固定した。256 bytes/128 MiB・失敗注入・再起動retry・body非アクセス・walker件数のassertionを維持し、独立レビューでもP1/P2なし。

途中の48 top-level並列runは67.55秒だったが4件失敗したため採用していない。既定は32 top-level（24論理CPU時）、大きいRust binaryは約36ケース/process・最大32 shards・各2 test threads。`--jobs 1..64`で明示指定可能だが、増やせば速く安定するとは扱わない。最終成功前の32並列runも71.18秒・1件失敗であり、最終成功値とは分離する。

最終ログは `target/normal-tests/2026-10-16T05-54-44.954Z/` の `summary.json`、`inventory.json`、`count-audit.json`、各processのstdout/stderr。全体出力は `target/test-timings/final-normal-4.log`。比較試行は `final-normal-1.log`〜`final-normal-3.log` に保存。外側buildと内部Cargo依存artifactは温まった状態で、成功結果の再利用による省略はない。

**要求の60秒までは約11秒残っているため、T-761は未完了。** Serverの実Worker起動・永続化・復旧を含む重いケースと、後半に開始するshardが残る。全コアを有効に使い切った性能限界を測定したわけではなく、さらなる短縮には残るfixture反復と実行順・競合の分析が必要。ここまでの変更は未commit・未pushで、稼働反映なし。
