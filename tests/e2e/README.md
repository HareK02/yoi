# yoi E2E

実際の製品プロセスを隔離したfixtureで検証する。PTY操作とBackend／Runtimeプロセスの検証を担当し、通常のunit testとは分離する。

## 実装の入口

- [`PanelHarness`]: PTY経由の入力、観測イベントによる同期、失敗時のartifact保存。
- [`FixtureWorkspace`]: TUI用WorkspaceとHOME／XDG領域の隔離。
- [`WorkerE2eEnvironment`]: Backend／Runtime用プロセスとfixtureの管理。
- [`FixtureSource`]: 提供資産の全ファイルSHA-256記録、独立copy、原本不変の検証。
- [`WorkerSessionSubscription`]: 公開Worker WebSocketの初期full Snapshot照合と停止確認。モデル呼出しは行わない。

## 固定Workerの段階起動

1. 提供fixtureを所有crateの型で検証し、`FixtureSource`で原本hashを記録する。各環境の一時領域へcopyする。fixtureが未提供・不適合なら失敗とし、代替Workerを生成しない。
2. `initialize_runtime_identity`で固定Runtime IDに新しい鍵を発行する。Backendの型付きstore APIで新しいAccount認証・Workspace署名identity・Runtime binding・固定Worker登録を準備する。
3. `start_seeded_backend`へ固定Workspace/Runtime IDとその環境の新しいbearerを渡す。Server起動後の公開APIからWorkspaceの公開identityを取得する。
4. `start_runtime_with_seed`はtrust設定後、Runtime processの起動前にseed closureを呼ぶ。ここで実Server URLを使いRuntime所有crateの型付きinstallを呼ぶ。Runtime store rootは`runtime_data_dir.join("runtime")`。
5. Worker一覧とdetailから停止状態・観測tokenを確認し、一つの固定request IDで`restore_worker`を一度だけ呼ぶ。結果不明・拒否は成功扱いせず、tokenを自動更新しない。
6. `WorkerSessionSubscription::connect`で期待`protocol::SessionSnapshot`全体（entry ID、timestamp、順序、本文を含む）を照合する。二環境を同時に保持し、一方のstop/cleanup後も他方の購読・一覧・内容を再確認する。
7. `stop_worker`、購読の`expect_stopped`、環境cleanup、原本`assert_unchanged`を確認する。

所有crateの`test-support`依存はテスト側の非default `fixture-support` featureだけで有効にする。製品binary providerは別Cargo invocationと専用`target/e2e-products`へ`yoi`・`yoi-server`・`yoi-runtime`を追加featureなしでビルドする。support test自身をこのtargetへビルドしてはならない。`YOI_E2E_BIN`等の明示binary overrideを使用する場合も、呼出し側が同じ分離を保証する。既存TUI用`e2e-test` featureは別の`target/e2e-tui-products`へビルドし、restore製品binaryへ混入しない。

固定資産の正常restore・並行隔離E2E:

```sh
cargo test -p yoi-e2e --features e2e,fixture-support --test worker_restore
# 親から提供された候補の検証（読取専用、原本の自動採用や生成はしない）
YOI_E2E_RESTORE_FIXTURE=target/fixture-candidate-v1 \
  cargo test -p yoi-e2e --features e2e,fixture-support --test worker_restore
```

標準資産は`tests/e2e/fixtures/worker-codex-v1`の`manifest.json`・`worker-template.json`・`expected.json`・`workspace-config.dcdl`。manifestの各asset hash/size、schemaと固定ID、source revision、modelを検証する。Workspace configのimport参照を黙って最新へ置換せず、Gitでsource revisionに対する`resources`と`crates/builtin-source`の一致を確認する（Git履歴がない場合も失敗）。移植したSessionの正規公開projectionを供給された会話行と照合し、その全snapshotを実executionのWebSocketと比較する。欠落fixture・hash不一致・未実行は成功扱いしない。

通常のharness回帰試験（製品processは起動しない）:

```sh
cargo test -p yoi-e2e --lib --no-default-features
```

既存の製品process E2Eは別結果として実行・報告する:

```sh
cargo test -p yoi-e2e --features e2e --test worker_environment
cargo test -p yoi-e2e --features e2e --test panel --test rewind
```
