# yoi E2E

実際の製品プロセスを隔離したfixtureで検証する。PTY操作とBackend／Runtimeプロセスの検証を担当し、通常のunit testとは分離する。

## 実装の入口

- [`PanelHarness`]: PTY経由の入力、観測イベントによる同期、失敗時のartifact保存。
- [`FixtureWorkspace`]: TUI用WorkspaceとHOME／XDG領域の隔離。
- [`WorkerE2eEnvironment`]: Backend／Runtime用プロセスとfixtureの管理。
- [`FixtureSource`]: 提供資産の全ファイルSHA-256記録、独立copy、原本不変の検証。
- [`WorkerSessionSubscription`]: 公開Worker WebSocketの初期full Snapshot照合と停止確認。モデル呼出しは行わない。
- `tests/auth_passkeys/native_migration.rs`・`browser/native_passkey_migration.ts`: 実Backend／本番shell／Chromium native CTAP2の旧鍵移行。fixture境界・未確認範囲・明示選択コマンドはmoduleに置く。通常suiteやsoftware authenticatorの成功で代替しない。

## 固定Workerの段階起動

1. 提供fixtureを所有crateの型で検証し、`FixtureSource`で原本hashを記録する。各環境の一時領域へcopyする。fixtureが未提供・不適合なら失敗とし、代替Workerを生成しない。
2. `initialize_runtime_identity`で固定Runtime IDに新しい鍵を発行する。Backendの型付きstore APIで新しいAccount認証・Workspace署名identity・Runtime binding・固定Worker登録を準備する。
3. `start_seeded_backend`へ固定Workspace/Runtime IDとその環境の新しいbearerを渡す。Server起動後の公開APIからWorkspaceの公開identityを取得する。
4. `start_runtime_with_seed`はtrust設定後、Runtime processの起動前にseed closureを呼ぶ。ここで実Server URLを使いRuntime所有crateの型付きinstallを呼ぶ。Runtime store rootは`runtime_data_dir.join("runtime")`。
5. Worker一覧とdetailから停止状態・観測tokenを確認し、一つの固定request IDで`restore_worker`を一度だけ呼ぶ。結果不明・拒否は成功扱いせず、tokenを自動更新しない。
6. `WorkerSessionSubscription::connect`で期待`protocol::SessionSnapshot`全体（entry ID、timestamp、順序、本文を含む）を照合する。二環境を同時に保持し、一方のstop/cleanup後も他方の購読・一覧・内容を再確認する。
7. `stop_worker`、購読の`expect_stopped`、環境cleanup、原本`assert_unchanged`を確認する。

所有crateの`test-support`依存はテスト側の非default `fixture-support` featureだけで有効にする。製品binary providerは別Cargo invocationと専用`target/e2e-products`へ`yoi`・`yoi-server`・`yoi-runtime`を追加featureなしでビルドする。support test自身をこのtargetへビルドしてはならない。`YOI_E2E_BIN`等の明示binary overrideを使用する場合も、呼出し側が同じ分離を保証する。既存TUI用`e2e-test` featureは別の`target/e2e-tui-products`へビルドし、restore製品binaryへ混入しない。

固定資産の正常restore・並行隔離・応答喪失回復E2E:

```sh
cargo test -p yoi-e2e --features e2e,fixture-support --test worker_restore
# 親から提供された候補の検証（読取専用、原本の自動採用や生成はしない）
YOI_E2E_RESTORE_FIXTURE=target/fixture-candidate-v1 \
  cargo test -p yoi-e2e --features e2e,fixture-support --test worker_restore
```

標準資産は`tests/e2e/fixtures/worker-codex-v1`の`manifest.json`・`worker-template.json`・`expected.json`・`workspace-config.dcdl`。manifestの各asset hash/size、schemaと固定ID、生成元commit、modelを検証する。Workspace configのimport参照を黙って最新へ置換せず、Gitで生成元commitに対する`resources`と`crates/builtin-source`の一致を確認する（Git履歴がない場合も失敗）。移植したSessionの正規公開projectionを供給された会話行と照合し、その全snapshotを実executionのWebSocketと比較する。欠落fixture・hash不一致・未実行は成功扱いしない。

応答喪失ケースは同じ`WorkerE2eEnvironment`内の単発loopback relayで公開restore要求をそのまま転送し、完全なupstream応答を受け取った後に呼出元へ0byteで切断する。呼出元のtransport失敗とrelay側のAccepted観測は別の事実として記録する。回収は元のrequest ID・観測tokenをそのまま使い、fresh intentへの古いtokenはHTTP409を要求する。

process終了ケースはそのAccepted receipt確認後に環境が所有するRuntime childだけをkill/reapし、同じroot・Runtime identity・trustから再起動する。seedの再実行・tokenの差替えはしない。再起動前後のcanonical owner全体のhash・operation ID・owner数が一致し、公開APIの保存結果とfull Session snapshotを回収できることを確認する。公開Worker投影の観測tokenは再起動で変わるため、そのfieldだけはcurrent detailとの一致を別に確認する。canonical診断はboundedな読取りのみで、要求・receiptの内容や認証情報はartifactへ保存しない。

これは**commit済みかつterminal receipt保存後**のprocess終了であり、未commit候補やcommit直後・receipt前の終了証明ではない。固定Codex資産へSubmit/Notifyを送らないため、通常入力・Paused admissionの製品E2Eの証明でもない。検証jobはこの明示targetを選択する必要があり、通常suiteだけでは代替しない。

## Opt-in Runtime checkpoint process試験

上のfeatureless製品4case（target内の既存seed projection試験1件とは別）に加え、`restore-checkpoints`を明示した実Runtimeでdecision前後とactivation後・terminal前のlive windowを試験する。このfeatureはoffline fixture-supportと独立しており、通常RuntimeではCLI入口も環境変数による入口も無い。

```sh
# 同期を有効にしたRuntimeだけを専用targetへ。通常製品binaryを上書きしない。
cargo build --locked -p worker-runtime --bin yoi-runtime \
  --features restore-checkpoints --target-dir target/e2e-checkpoint-products
cargo test --locked -p yoi-e2e --features e2e,restore-checkpoints \
  --test worker_restore checkpoint:: -- --test-threads=1
```

`WorkerE2eEnvironment::use_checkpoint_runtime`はその環境のRuntime binary/hashと、隔離root内の明示configだけを選択する。global provider・環境変数・通常起動への暗黙設定は変更しない。configは一度だけCLIへ渡し、再起動時は渡さない。test側のlink featureをfeatureless製品binaryへ混入させない。

共有harnessはloopback ackのstage/Worker/operation/PID/nonceを確認し、直ちに所有childをkill/reapする。**offline正本を確認してから**同roots/identity/trustを再起動するため、ack後のdeadlineとの競合を成功扱いしない。NotCommittedの実私有候補は同operationのcleanup-only→RolledBack/Stopped、Committed/preterminalは同じdecision→実protocol/full Session snapshot→Acceptedへ回収する。元のrequest/token、owner数1、同operation、stale fresh intent拒否、保存source hash、restore run path数、最終cleanupを検証する。JSONでpending状態を作らず、seedは初回のみ。

orphan upload pinの代表repairは、隔離copyへ正規Session storeのupload/pin APIで作った未結付けpinを、実Profileのcommit後activationが解放する経路を使う。Prepared窓は未修復のsource保持、Activated窓は実修復bytes/hashの保持を区別し、kill/reap後に正本を読んでから再起動する。materialized headの固定資産を巻き戻さないため、この試験は先行repair済みNotCommittedのprocess証明ではない。Activated後のstale allocationは通常登録と同じallocation authorityで回収し、live ownerが残る場合はrun artifactsを回収しない。

明示解除・peer EOF・observer scopeの意図的panicも試験する。gateの8秒deadlineとscoped HTTP callの15秒deadlineが待機を制限し、通常operationの継続後に環境のRAIIがprocess/rootを回収する。これはtest scope失敗の回収証明であり、OSによるharness強制終了や電源断の一般的な保証ではない。原fixtureへのSubmit/Notifyや実モデル呼出しは行わない。checkpoint ack/at-death/recovery、kill/reap、binary/hashとcleanup artifactはfeaturelessログと分けて報告する。

通常のharness回帰試験（製品processは起動しない）:

```sh
cargo test -p yoi-e2e --lib --no-default-features
```

既存の製品process E2Eは別結果として実行・報告する:

```sh
cargo test -p yoi-e2e --features e2e --test worker_environment
cargo test -p yoi-e2e --features e2e --test panel --test rewind
```
