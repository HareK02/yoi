# 変更の検証

変更した契約を確認できる検証を選び、実行した範囲と未確認の範囲を区別する。広いcheckの成功で、変更した境界のテストを代替しない。

## 文書とrustdoc

文書だけの変更では`git diff --check HEAD`に加え、移動・削除したファイルへのリンク、ソースのinclude、テストや生成処理の参照を確認する。crate READMEやrustdocを変えた場合は、対象crateのドキュメント生成とdoctestも確認する。通常無効のfeatureでだけ存在するitemへのリンクは、その構成でも検証する。

## Rustコード

[テスト設計規則](rust-testing-strategy.md)に従い、開発中は変更を証明する最小のtarget・filterを使う。

```sh
cargo test -p <crate> --lib <test-or-module-filter>
cargo test -p <crate> --test <test-target> <test-filter>
```

完了前は[AGENTS.md](../../AGENTS.md)に従ってrootのcompile closureと変更crate全体を確認する。

```sh
cargo check
cargo test -p <changed-crate>
cargo fmt --all -- --check
git diff --check HEAD
```

単体・コンポーネント・結合は通常のテストに含める。SQLite、loopback、隔離子プロセスを使うtargetも省略しない。影響する製品feature、test-only target、別crateが所有する結合suiteを追加する。`cargo check`はテストを実行せず、`--lib`だけでは外部integration-test targetを実行しない。

Workspace全体、`--all-targets`、Nix/Dockerなどの重い検証は、変更した境界を狭い検証では確認できない場合か、明示的に求められた場合に選ぶ。既存の無関係なformat差分を混ぜない。公開APIや生成元を変えた場合は、所有する生成処理と差分検査で生成物との一致を確認する。

## Webとレビュー

描画結果が変わるWeb UI変更には、type check・testに加えて実装者自身の[目視確認](ui-ux/visual-review.md)を必要とする。画像の生成、HTTP成功、後段Reviewerへの委託だけでは代替できない。

Ticketに対するレビューでは、差分のもっともらしさだけでなく、現行の要求と受入条件を満たすかを確認する。レビュー判断と実行可能な検証は互いの代替にしない。

## E2EとCI

製品を組み立てる利用場面の検証は、`tests/e2e`の共有harnessを使う。必要なbinary・製品feature、隔離・回収、timeout、診断をsuiteに明記する。稼働中の開発環境を借りたり、crateごとに別の製品起動基盤を作ったりしない。

```sh
cargo test -p yoi-e2e --features e2e --test <target> <test-filter>
```

このpackageは`default-members`に含まれず、対象には`e2e` featureが必要である。通常のroot実行やfeatureなしのWorkspace実行をE2E成功として報告しない。選んだ利用場面と実結果、または未実行を記録する。

通常suiteのCI jobは、主張する結合targetと製品featureを実際に選択する。E2Eは専用jobとし、必須になる変更、手動・定期実行の条件を定める。特殊なplatform・tool・権限が必要なsuiteも適切なjobを持ち、可搬なテストまで例外扱いしない。

suiteの追加・変更時は実行するjobを確認し、選択条件を更新する。未配線なら不足として記録する。CI方針の記載やローカルの成功だけから、CIで実行済みと判断しない。
