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

完了前にはworkspace rootで必ず`cargo check`を実行し、TUI・Serverを含む通常のcompile closureと変更crate全体を確認する。公開型の変更ごとにreverse dependencyを推測して`-p`を列挙する運用にはしない。

```sh
cargo check
cargo test -p <changed-crate>
cargo fmt --all -- --check
git diff --check HEAD
```

単体・コンポーネント・結合は通常のテストに含める。SQLite、loopback、隔離子プロセスを使うtargetも省略しない。影響する製品feature、test-only target、別crateが所有する結合suiteを追加する。`cargo check`はテストを実行せず、`--lib`だけでは外部integration-test targetを実行しない。

Workspace全体、`--all-targets`、Nix/Dockerなどの重い検証は、変更した境界を狭い検証では確認できない場合か、明示的に求められた場合に選ぶ。既存の無関係なformat差分を混ぜない。公開APIや生成元を変えた場合は、所有する生成処理と差分検査で生成物との一致を確認する。

Linuxで通常suite全体を選ぶ場合は、rootの `deno task test` を使える。Cargoが選択した通常Rust test targetを毎回build・列挙し、doctest、Web unit/component、検証toolのテストも実行する。大きいRust targetは列挙した全ケースを重複なくprocessへ分配し、同一process内の共有資源への競合を減らす。`--jobs N` は同時実行するtop-level suite数の上限で、内部のCargoやWeb runnerの並列性とは別である。個別調査には引き続きCargoのtarget/filterを使う。

外側buildと実行の壁時計時間を分け、通常テストの失敗・timeout・列挙失敗は全体の失敗とする。各processの途中ログ、選択したケース、結果は `target/normal-tests/` に保存する。timeout・取消時の回収はLinuxの同じprocess group内を対象とし、自ら別sessionへ離脱した子孫までは保証しない。E2E/browserなど明示選択のsuiteは別に実行し、このrunnerの成功で代替しない。

## 状態契約の命名検査

完了前に次を実行する。例外一覧を機械的に増やして合格させず、必要な保存互換・検証・禁止規則の記述だけを理由付きで扱う。

```sh
env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts
```

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
