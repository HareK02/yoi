# schema86→87のSSH移行: 異なる時刻形式の文字列比較でfingerprintがNULLになる

調査日: 2026-10-10
報告された起動エラー:

`yoi-server: store error: workspace schema migration failed: sqlite error: NOT NULL constraint failed: workdir_create_credential_candidates.credential_fingerprint`

## 原因

`crates/workspace-server/src/frozen_repository_key_migration.rs:262–266`のcredential candidate移行は、旧credential ID/revisionから`repository_key_cutover`へLEFT JOINし、`m.fingerprint`を新しいNOT NULL列に挿入する。

JOIN条件の`m.created_at <= o.updated_at`が、RFC3339の鍵作成時刻とepoch-millis文字列のWorkdir更新時刻を、そのままTEXTとして比較している。日時としての順序を判定していないため、有効な古い鍵のJOINが外れ、fingerprintがNULLになる。

- 鍵作成時刻: `repository_access.rs:2256`の`Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)`。
- Workdir作成・更新時刻: `server.rs:30936–30940`の`now_registry_timestamp()`はepochミリ秒の文字列。reserveでもこれを使用する（`server.rs:24272–24308`）。

同じ異形式比較はoperation本体のcredential/host trust移行にもある（migration L260–261）。candidateだけ直してもoperation snapshotのfingerprintが欠落する問題が残る。

## 実データによる確認

ユーザーから共有済みの`yoi.local/server/server.db`をNodeの`DatabaseSync({readOnly: true})`と`PRAGMA query_only=ON`で照合した。秘密鍵、暗号化envelope、master keyは取得していない。migrationやDB更新も実行していない。

- DBはschema86のまま。
- 新candidateへ移される5件すべてがfailed Workdir createのcredential候補で、旧retentionが各1件あるため成功済み・unretainedのarchive経路には入らない。
- 5件とも参照先の鍵は存在する。対応するmutation receiptも各1件あり、証拠欠落ではない。
- 5件ともSQLiteのTEXT比較はfalse、実際の時刻比較はtrue。

代表例:

| 項目 | 値 |
| --- | --- |
| operation | `call_mgmyCNofdQMZ8uyI5tg0mBhv` |
| credential | `workspace-default`、旧revision 1 |
| 鍵created_at | `2026-09-16T18:44:56.736Z` |
| Workdir updated_at | `1790173821030` |
| Workdir updated_atを日時に変換 | `2026-09-23T14:30:21.030Z` |
| SQLiteの文字列比較 | false |
| 日時としての比較 | true |

他の4件では鍵作成が`2026-09-10T13:09:52.113Z`、Workdir更新は9/23または9/29で、同じ問題になる。

## 修正方針（この調査では未実装）

- frozen migration内で旧データの時刻形式を明示的に読み、同じ時刻型/epoch-millisへ正規化して比較する。文字列比較やSQLの安易なCASTに頼らない。
- `created_at <= updated_at`の安全確認そのものは削除しない。旧ID/revisionが削除後に再利用された鍵を、過去のcandidateに誤って結び付けることを防ぐ条件である。
- candidateだけでなくoperation本体のcredential/host trust等、同じ境界の比較を確認する。
- 正規化不能・不正な時刻や真に必要な証拠が欠落した場合は、NOT NULLエラーになる前に文脈付きで拒否し、transactionをrollbackする。

既存の`frozen_repository_key_tests.rs:35–42`の共通fixtureは、鍵とWorkdirの時刻をすべて`created`に揃えており、実データの異形式を再現していない。RFC3339鍵時刻＋epoch-millis Workdir時刻での移行成功と、実時刻が操作より後の再作成鍵を拒否するケースが必要。テスト追加時は`docs/development/rust-testing-strategy.md`に従う。

## 影響・実施範囲

schema86→87はtransaction内で実行され、version87の記録とcommitは最後に行う（migration L29、L349–350）。今回のDBでもschema86と旧テーブルを確認した。ただし、それより前のschema更新まで含めた全起動処理が一括で元に戻るという意味ではない。

NOT NULLを外す、適当なfingerprintを入れる、candidate/retentionを消して通す、といったDB修復は行うべきではない。正しい既存データをmigrationが誤判定しているので、移行コード側を修正する。

調査時点で追加したのは本報告のみ。以下は、その後のユーザー指示による修正記録。

## 修正と検証（2026-10-10）

- RFC3339/epoch-millisをRustで読み、temporary tableのUTC秒・ナノ秒へ正規化して比較するよう修正。credential/host trustのoperation snapshot、candidate、旧receiptの時刻条件に適用した。
- タイムゾーン・sub-millisecondの順序を維持し、元の履歴時刻やreceipt照合用の文字列、暗号化AADは書き換えない。永続schemaの変更・version bumpは不要。
- 不正な時刻、欠けた鍵、操作より新しい再利用鍵は文脈付きで拒否しrollback。NOT NULLを緩めたり候補を削除したりしない。
- 独立コード確認で見つかった、親operationのない旧candidateがINNER JOINから落ちて消える経路も、コピー前の拒否とrollbackテストで保護した。追修正に追加のblocking findingなし。
- 偽時刻`created`や日付だけのfixtureは実際のRFC3339へ修正。新規回帰は混在形式・同時刻・offset・ナノ秒精度、各SSH参照での新しい鍵拒否、不正時刻、孤立候補の保持を検証する。
- 最小の混在形式テスト、migrationモジュール、公開migration dry-run/applyテスト、孤立候補テストを順に実行して成功。最終の`cargo test -p yoi-workspace-server`はlibrary **815 passed / 1 ignored**、binary **11 passed**、doctest失敗なし。
- root `cargo check`、`cargo fmt --all -- --check`、変更したincludeファイルの明示的`rustfmt --check`、`git diff --check HEAD`も成功。
- 稼働DBでのmigration、ビルド済みバイナリの配備、サービスの停止・再起動は行っていない。更新はユーザー操作のまま。

作業中、テスト担当SubWorkerはcwd基準の読取で、scopeに付与した別ディレクトリのテスト規則を参照できなかった（絶対pathもinvalid logical filesystem pathで拒否）。編集なしを確認して停止し、親でテストを実装した。読取専用レビュー担当はroot cwdで規則・ソースを読めた。cwdとscopeにまたがる論理pathの案内・検証がツール上の改善点。
