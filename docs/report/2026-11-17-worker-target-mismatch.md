# Ticket target変更によるWorker取得の巻き添え障害

## 観測と原因

`HEAD`指定で作成・接続したWorkdirを保持するWorkerについて、割当Ticketのtargetが後から`develop`へ変更された。Serverは現在のtargetと作成selectorの不一致を拒否し、その拒否を一覧の表示投影からそのまま返していた。さらに個別取得も全件一覧を経由していたため、無関係なWorkerのConsoleも接続先identityを取得できなかった。

今回の修正は、明示的なtarget不一致だけを表示用の権限なし・警告へ変換し、実操作の拒否と保存済みgrantを維持する。個別取得は対象1件の保存済みprojectionを読む。DB障害を不一致として扱うfallbackは追加していない。

## 検証の識別と結果

既存の手元変更は`83f1aac3e`に分離した。その上の`server.rs`と`server/tests/worker_registry_access_tests.rs`の修正を検証した。検証ログ・列挙結果は`target/validation-worker-access/`に保存したローカル資料であり、製品の正本ではない。

- `cargo check`、`cargo fmt --all -- --check`、`git diff --check HEAD`、状態契約の命名検査は成功した。
- 新規3ケースは成功した。selector変更・target削除後の一覧／個別取得／入力受付、Workdir拒否維持、grant不変、一覧上限からの独立、DB障害の非隠蔽を確認した。
- `tools/test.ts`の隔離処理を利用した関連19ケースは全件成功した。既存Worker割当、保存projection、protocol proxy、restore後の親子入力も含む。`related-run.log`に選択名と結果を残した。
- `yoi`、`yoi-server`、Runtimeの製品binaryをビルドした。ビルド済みbinaryを明示した`yoi-e2e`の`worker_environment::starts_server_and_runtime_from_empty_storage`は成功した。共有harnessが別DB・Runtimeデータ・ポートと実認証・署名要求を扱い、cleanupも確認する。これはtarget変更の製品E2Eを実行した主張ではない。
- 稼働中のServer／Runtimeの停止・更新、保存データの修復は行っていない。

## 全体テストの制限

crate全体は成功していない。

最初の`cargo test -p yoi-workspace-server`は600秒で中断した。restore系の単独再実行では、テスト専用の隔離環境が未指定であるという安全preflightの失敗を確認した。以後は維持されている`tools/test.ts`の列挙・分割・隔離・実行helperを使った。

分割再実行では、2339件（ignoredを含む）を66 shardへ列挙したが、外側の600秒上限で全件終了前に中断した。個別shardの180秒timeoutと、以下2ケースのassert失敗を確認した。既存不具合か今回の回帰かはbaseline比較をしておらず、未確定とする。

- `chat_public_receive_busy_messages_commit_before_next_model_input_and_idle_notify_activates`: 失効したsourceのrestore拒否を期待した箇所で`accepted`になった。
- `body_start_and_restore_do_not_repair_missing_session_record_permission`: 拒否を期待したrestoreが`accepted`になった。

`sharded-run.log`、`sharded/inventory.json`、各shardのstdout／stderrに範囲と結果を残した。中断後に当該テストbinaryの残存プロセスがなくなったことを確認した。上記失敗を今回のWorker表示修正で修復済みとは扱わない。

## 別経路の要確認事項

読取りレビューと親側のソース確認で、保持済みcommandの`CommandOutput`はattachment aliasとhandle所有を確認する一方、通常のsession-open経路と異なりTicket targetの現在の制約を再評価しない経路を確認した。これは今回の表示修正が新設した経路ではなく、実行再現は未実施である。

現在のtarget変更後も保持済み出力を読めるかを隔離テストで確認し、出力取得前のcapability再評価を検討する必要がある。既存効果の確認・停止に使う`CommandStatus`／`CommandCancel`とは区別する。今回の修正で全Workdir経路の再認可を保証したとは扱わない。

また、読取りSubWorkerにcrateと`docs`のscopeを渡してcrateをcwdにした際、別階層の必須資料を親相対・絶対パスで読む試行がscope検査に拒否された。技術レビューは継続したが、そのSubWorker自身による必須資料の確認は未完了である。親側では資料を読んでいる。今後の委譲では資料を読めるcwd・scope構成を選び、ツール側のcwdとscope解決の整合性は別途確認する。
