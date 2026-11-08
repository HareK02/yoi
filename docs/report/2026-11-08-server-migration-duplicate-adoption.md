# 既存Server DBの起動を阻害する重複DDL

2026-11-08の調査では、既存DBの履歴欠落ではなく、新規migrationによる適用済みDDLの再実行が起動失敗の原因と確認した。実DBの修復・削除や履歴の変更は行っていない。

## 対象と観測

- 調査sourceはHEAD `5f8432424cd4` に未コミットの変更を含む作業ツリー。
- 起動確認artifactは `target/debug/yoi-server`、SHA-256は `2a1a58e8d67e2d3f48ffabb947b67e157fb81f86394d0c815bd8e75662009296`。
- `cargo run --bin yoi-server -- serve --listen 127.0.0.1:8787` はコンパイル後、`workspace_configuration_authorities` の重複作成で終了した。隔離した新規DBでは起動した。
- 利用者指定の `yoi.local/server/server.db` を読み取り専用で確認した。schema 130までの履歴があり、122のconfiguration adoptionの履歴と対応テーブルが存在した。

## 原因

`crates/workspace-server/src/store.rs` の `migrate_worker_launch_policies_to_v131` が、launch policyとbindingのDDLに続いて `crate::config_adoption::SCHEMA` を再実行していた。このDDLは `migrate_configuration_adoption_to_v122` が既に適用するものであり、通常の更新対象DBではテーブル名が衝突する。新規作成経路では同じDDLを一度だけ実行するため、この不具合を検出できなかった。

## 隔離コピーでの検証

SQLite backup APIで読み取り専用の実DBから、所有者のみアクセス可能な一時ディレクトリへコピーした。次の操作はコピーに限定した。

- 製品binaryの `migrate --database <copy> --dry-run` でも同じ失敗を再現した。
- 現行DDLの順序をトランザクション内で再現し、失敗後のrollbackでschema、既存テーブルの件数、configuration adoptionの保存レコードが変わらないことを確認した。
- 重複するadoption DDLだけを除いたSQLをコピーへ適用し、製品binaryのdry-run検証が成功した。
- 既存の全ユーザーテーブルの件数とadoptionの保存レコードを維持した。新規追加オブジェクトは新規DBの定義と一致し、コピーの `quick_check` は成功、外部キー違反はなかった。
- 最後に実DBがschema 130のままで、新規migrationのテーブルが存在しないことを再確認した。

## 修正方針と未確認範囲

修正対象は131のmigrationに混入したadoption DDLの再実行であり、既存テーブルの削除、適用履歴の書換え、`IF NOT EXISTS` による衝突の隠蔽は不要。新規DBの作成処理と122のmigrationは保持する。

この調査ではコード修正は行っていない。修正後の製品migrationによる更新、130からの回帰テスト、実DBでの再起動は未実施。隔離コピーでのSQL検証は、それらの代替ではない。

## 同日の修正と検証

利用者の修正依頼を受け、131のmigrationからadoption DDLの重複実行だけを除去した。122のmigrationと新規DBの作成処理は変更していない。

`migration_entrypoint_tests.rs` に、130からの更新が既存のadoptionレコードと不変性制約を維持し、途中失敗で新規DDLと適用記録をrollbackする回帰テストを追加した。製品のdry-runが元DBを変更しないこと、apply後の再open、新規DBとの追加schemaの一致も確認する。

- 追加した回帰テスト、既存の暗号化データを含むdry-run/applyテスト、schema 84からの復旧を含む更新テストが成功した。
- root `cargo check` と `cargo build -p yoi-workspace-server --bin yoi-server` が成功した。
- 修正後artifactのSHA-256は `afe8b12cd639a5e037ec3797da561644faba4f09ccbc7e5b5f9af375ffcf810e`。
- このartifactで実DBに `migrate --database yoi.local/server/server.db --dry-run` を実行し、130から131への検証が成功した。元DBは130のままで、新規テーブルは作成されていない。
- 追加テストの整形と差分検査は成功した。Workspace全体の整形・状態契約命名検査は、今回の修正対象外の作業中変更に指摘が残る。

最初のテストビルド中に `target/` が消失し、コンパイラが出力先不在で失敗した。消失の原因は未特定だが、生成物を再作成した後の上記ビルド・テストは成功した。

crate全体のテストと実DBへのmigration適用・再起動は実施していない。稼働中のYoiプロセスは停止していない。
