# 状態契約の命名整理と全件監査

## 判定

現行の識別子・API・生成契約・最新DDLから禁止名称を除去し、具体的な状態契約と再導入検査を実装・検証した。ただし、コードベースの文字列を完全ゼロにはしていない。大小文字無視の最終結果は **32ファイル・546該当行**。内訳は旧保存形式とそのfixture **522行**、移行後の消失などを確かめる検査 **14行**、命名禁止規則とユーザー原文証拠 **10行**。必要性と削除時の影響を以下と限定例外一覧に記録した。

guardは未承認0・stale例外0で成功。これは旧語の完全ゼロを意味せず、残す全行が分類・固定され、それ以外の再導入が拒否されることを意味する。新しい識別子・ファイル名の残存は0。今回の変更は `hare/develop` の `3263d501` を基準に実装・検証した。

元依頼は数値カウンタの廃止だけではなく、コードベース全体から禁止された名称を除去し、何を・どう・いつ確認するのかを明示するもの。先行実装 `c78f7f7d` だけで完了とした判断は誤りだった。今回の開始時監査は追跡テキスト96ファイル・875該当行。途中のHEAD `3263d501` は別作業によるユーザー原文証拠の文書追加のみで、その証拠は改変していない。

## 実装の範囲

現行Rust/API/Web/文書/無関係fixtureと生成物の名前を用途に合わせて整理した。未使用conflict enumは別名で温存せず削除。Workdir一覧の値は実際の公開projectionの `catalog_digest`、MR source解決はGit commit、歴史的な完了記録のRust側フィールドは `legacy_item_marker` として現行判断から分離した。

Server schema89はartifact provenanceと旧secret receiptの列名を変更する。Drive schema3は既存schema2の旧replay列を変更する。保存された値、原request fingerprint、receiptは変更しない。現在のDDLには旧綴りを作らない。旧SQL/JSONの綴りは移行・原文保持・旧fingerprintの再現に限って残す。

## 何を、どう、いつ確認するか

| 境界                  | 対象・比較・時点                                                                                                                                  | 順序や時刻だけでは足りない理由                                                        |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Ticket状態変更        | 書込みtransaction内で、要求時の内容digestとworkflow stateを現在値と比較してからstate/auditを書込む。同一operationの再送は別途receipt照合。        | stateが同じまま受入条件を編集した場合、古い条件による完了判断を通してはいけない。     |
| MR review/integration | 公開Git refをcommitへ解決し、review対象のcommit・現在のTicket内容など、承認が実際に対象としたものを判定時に照合。                                 | 到着順の後勝ちは、未承認commitを承認済みとして扱ってしまう。                          |
| Drive                 | 同一IMMEDIATE transaction内でnodeの最後のmutation request IDを比較してから更新・移動・削除。chunk readも同じIDに固定。                            | 同一時刻の更新、同内容書戻し、移動の往復、複数chunkの新旧混在を時刻では区別できない。 |
| Config                | tree全体または変更entryの実内容digestをcommit前に照合。解決結果cacheは入力とtoolchain/projection条件で識別。                                      | 古い編集結果で他者の内容を消さない。単なる新旧順では内容一致を証明できない。          |
| Subject/Memory        | Memory変更の履歴追加前に観測したchange IDを比較。surface公開前には生成入力のfingerprintと現在値を照合。behavior編集は観測した本文そのものを照合。 | 新しいMemoryの後に到着した古い生成Jobを公開してはいけない。                           |
| Runtime/鍵            | enrollment lifetimeをbinding/trust IDで区別し、鍵の実体をfingerprintで比較して検証・利用する。旧文書は現行認可に復活させない。                    | 同じ鍵の再登録や失効・再登録を時刻や鍵だけでは区別できない。                          |
| standalone保存        | create-new commit marker保持中に観測metadata全体を比較し、確認後にファイル置換。                                                                  | 停止・再開・session pointer変更を古いrecordで戻さない。別の更新番号は不要。           |
| pending FIFO/表示     | stream順の完全置換、再接続時snapshot。Memory historyはServerの順序を保持。                                                                        | この用途では追加の汎用更新番号は不要。IDから順序も推測しない。                        |

実装境界のコメントと `docs/development/web-state-preconditions.md` に対象・比較・時点と理由を記録した。schema/protocol versionは保存形式の選択であり、状態の新旧を任せる番号とは区別する。今回の名称修正でWIPの新番号は追加していない。

## 残す綴りと削除時の影響

各行の固定根拠は `tools/state-contract-name-exceptions.json`、生の全該当行は命名guardの `--report` で再現できる。以下はファイル別の全残存。`crates/` 配下は同prefixを省略した。

| ファイル                                                   | 該当行数 | 必要性・対象                                                        |
| ---------------------------------------------------------- | -------: | ------------------------------------------------------------------- |
| `manifest/src/lib.rs`                                      |        8 | 旧Memory settings snapshotの読取り・復元fixture                     |
| `merge-request/src/lib.rs`                                 |        2 | 保存済みcompletion属性のroundtrip、旧review archive移行             |
| `merge-request/tests/store.rs`                             |        2 | 旧review/completion原文保持のfixture                                |
| `session-store/src/system_item.rs`                         |        1 | 旧Session履歴の読取りfixture                                        |
| `standalone/src/job_store/legacy_migrations.rs`            |       14 | schema1/2 Job request/attempt/receipt/再送の移行・rollback          |
| `standalone/src/store.rs`                                  |        4 | schema1 Worker metadata復元と旧key除去                              |
| `subjektiv/src/legacy_migrations.rs`                       |      113 | 旧Memory履歴・参照edges・decision receipt・surface provenanceの移行 |
| `ticket/src/lib.rs`                                        |        1 | 旧receiptを現行判断の証拠に昇格させないfixture                      |
| `worker-runtime/src/fs_store.rs`                           |       14 | 旧Job/request/config/SSH evidenceの変換・fixture                    |
| `worker-runtime/src/main.rs`                               |        2 | 旧enrollmentの検出・原文保持・失効                                  |
| `worker-runtime/src/working_directory.rs`                  |        4 | 旧SSH evidenceの読取りとfresh access必須化                          |
| `worker-runtime/src/workspace_issuer.rs`                   |        8 | 旧trust/verificationの読取りと旧authority失効                       |
| `workspace-drive/src/migrations.rs`                        |       25 | 旧node/receipt移行、旧typed payloadのfingerprint再現、schema2列改名 |
| `workspace-server/src/frozen_content_state_migration.rs`   |       14 | 旧Config/Memory/reservation/Job保存形式の移行                       |
| `workspace-server/src/frozen_content_state_tests.rs`       |       31 | 同移行の実形式fixture・消失・rollback                               |
| `workspace-server/src/frozen_flow_content_migration.rs`    |        7 | 旧Flow source/compiled definitionを再compileせず移行                |
| `workspace-server/src/frozen_flow_content_tests.rs`        |       13 | 旧Flow履歴保持と不整合rollback                                      |
| `workspace-server/src/frozen_legacy_migrations.rs`         |       49 | retained migration chainの旧列・表・FK・index                       |
| `workspace-server/src/frozen_repository_key_migration.rs`  |       39 | 旧secret復号/reseal、operation fingerprint、SSH retention/audit     |
| `workspace-server/src/frozen_repository_key_schema.sql`    |        2 | schema87/88の旧receipt列、schema89への移行入力                      |
| `workspace-server/src/frozen_repository_key_tests.rs`      |       22 | 旧暗号secret/SSH fixture、再送・archive・rollback                   |
| `workspace-server/src/frozen_runtime_binding_migration.rs` |       25 | 旧binding/policy/署名鍵/provisioning receiptの移行                  |
| `workspace-server/src/frozen_runtime_binding_tests.rs`     |       17 | 同移行の実形式fixtureと失敗時保持                                   |
| `workspace-server/src/frozen_schema_v85.sql`               |       60 | 旧DB移行を検証する凍結DDL                                           |
| `workspace-server/src/legacy_evidence_column_migration.rs` |        3 | 旧artifact/SSH receipt列を値保持して改名する入力                    |
| `workspace-server/src/legacy_evidence_column_tests.rs`     |        2 | 旧artifact値保持・現行DDLの旧名消失                                 |
| `workspace-server/src/migration_entrypoint_tests.rs`       |        5 | 公開migration入口の旧policy/removal/SSH fixture                     |
| `workspace-server/src/store.rs`                            |       44 | 旧schema reader/validation/fixture、保存済みmigration名             |
| `workspace-server/src/workspace_signing_identity.rs`       |        5 | version1のprivate materialを同じ鍵のまま復元                        |
| `AGENTS.md`                                                |        1 | 禁止対象の明示                                                      |
| `tools/check-state-contract-names.ts`                      |        1 | 検出対象の定義                                                      |
| `docs/report/2026-10-10-compaction-user-directive-loss.md` |        8 | ユーザー指示と指示脱落の原文証拠                                    |
| **合計**                                                   |  **546** | **32ファイル、全行に限定例外の根拠あり**                            |

特に `store.rs` のmigration名2行は `verify_schema_history()` が保存済み `__yoi_schema_migrations.name` と完全一致を要求する。これは単なる表示文ではなく、改名すると既存DBが非canonicalとして拒否される保存形式境界。一方、診断文・triggerエラー文・ローカル変数・汎用unknown-field fixtureの14行は監査で不要と判定し、例外へ追加せず修正した。

- 旧DBの列・表・制約名：既存DBを特定して移行するため。削除すると移行SQLが旧データを読めない。
- 旧保存JSON/TOMLのキー：履歴・snapshot・receiptを読み、元記録を保持するため。現行Rustフィールドや新規入力契約とは分離する。削除すると旧データが読めなくなる、または原文を欠落させる。
- 旧fingerprint/AADのbyte encoding：キー名と順序を含む旧payloadを正しく照合・復号するため。変更すると同一の正当な再送や暗号化保存データを読めなくなる。
- 旧形式fixtureと消失検査：上記移行とrollbackを実際の旧入力で証明するため。無関係な旧名fixtureや重複した不存在assertは除去した。
- 禁止規則自身とユーザー原文証拠：検出対象の定義と、実際の依頼を狭めないための証拠。現行状態モデルの名前ではない。

これは旧設計を現在の操作へ延命する許可ではない。既存保存形式の移行・再送・履歴保持サポートを廃止すれば互換literalは削除できるが、その場合に読めなくなるデータがある。文字列分割・符号化で検索を逃れる処置はしていない。

## 再導入防止

`AGENTS.md` に命名禁止と具体的な状態契約の規則を置いた。`tools/check-state-contract-names.ts` はGit管理ファイルと非ignored未追跡ファイルのパス・本文を大小文字無視で検査し、binaryも対象とする。例外はファイルごとの理由、該当行のdigest、出現数で固定し、追加・変更・別ファイルへのコピー・削除による古い例外を拒否する。directory単位の免除はない。`--report` は該当箇所の実際の文字列を出力する。

```sh
env -u LD_LIBRARY_PATH deno test tools/check-state-contract-names.test.ts
env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts --report
```

Git履歴・stash・他作業のWorkdir・外部依存・旧実行ログは現在のコードとは分けて扱い、検索をゼロに見せる目的で消していない。ignored inventoryはtarget配下162,446件、node_modules 13,975件、Web生成領域2,336件、その他55件（検査時点）。targetの旧バイナリ/証拠ログ、外部依存、symlink先、zipバックアップ、別Workdirの内容までゼロ化したという主張はしない。

ignored内の自作Web build/生成server・client出力/ローカル文書については、最終再生成後の通常ファイル2,135件を別監査し一致0。旧検証ログの名前/内容とローカルwasm-bindgen-test-runnerには残存があるが、履歴/外部toolを削除して検索結果を隠してはいない。Gitの非ignored監査とは別に `target/state-name-ignored-inventory.json`、`target/state-name-ignored-owned-audit.json`、`target/state-name-ignored-current-generated-final.json` に対象と結果を記録した。

## 生成の再現

正規generatorを使用した。主な再生成コマンドは次のとおり（rootで実行）。`generate_legacy_typescript` のstdoutは `web/workspace/src/lib/generated/legacy-server-api.ts` と比較・同期する。

```sh
cargo run -p server-api --example export_openapi -- openapi/server-api.json
for generator in generate_repository_openapi_types generate_runtime_api_types generate_ticket_api_types generate_workdir_api_types generate_worker_launch_api_types generate_companion_api_types; do
  cargo run -p server-api --features typescript --example "$generator"
done
cargo run -p server-api --features typescript --example generate_legacy_typescript
cargo run -p protocol --features typescript --example generate_typescript
```

6つのAPI generatorとprotocol generatorには `-- --check` を付けてfreshnessも確認した。`cargo test -p server-api --features typescript` の96 testsと、protocol TypeScript featureの85 testsも成功した。

## 検証

- 変更14 Rust crateをまとめて検証（server-apiのTypeScript featureを含む）：3,209 passed、3 ignored、0 failed。
- Server schema89：保存値/NULL/FK/current DDLを検査、最後のhistory INSERT失敗時の全rename rollbackと再試行も成功。
- Drive schema3：既存schema2のevidence/digest/replay結果/live nodeと再open後の動作を保持。
- protocolのTypeScript feature、api-macrosのOpenAPI feature、E2E fixtureのcompileを追加検証。E2Eの実プロセスは起動しない。
- 正規generatorによるOpenAPI/API/protocol同期とfreshness check成功。途中の未同期による失敗は生成後の統合テストで解消。
- 生成後Web標準591 passed、component315 passed/33 files、checkは0 errors/0 warnings、build成功（chunk-size warning）。
- root cargo check、workspace format、included migration/test filesのformat、diff check成功。最終命名guardは546該当行・未承認0・stale例外0。
- 最後の不要14行修正後、対象25 testsと変更3crate全体（Subjektiv/Runtime/Server）を再実行して成功。例外一覧を増やさずguardを通した。
- 命名guardの5 tests成功（大小文字・複合語・ファイル名・binary・限定例外の改変/増殖/残骸を検査）。
- 独立read-only reviewでは今回のmigrationとguard本体/直接testsにblocking指摘なし。schema同等性検査はCHECK式やpartial-index述語の完全な同等性証明ではない。
- 追加実行した非標準Webテストには既存のsource-text assertion失敗が1件ある。`Current requirement evidence` を変更していないTicket routeへ要求するもので、HEADでも同文字列が無いことを確認。無関係な修正はしていない。`target/state-name-nonstandard-existing-failure.log` に記録。

今回の主証拠ログは `target/state-name-rust-combined-final.log`、`target/state-name-root-check-final.log`、`target/state-name-api-freshness.log`、`target/state-name-generated-web-{standard,component,check,build}.log`。前回の検証件数を今回の結果として流用していない。

live Server/Runtime/DBは変更せず、push・restart・rolloutは実施していない。既存stash2件は保持している。
