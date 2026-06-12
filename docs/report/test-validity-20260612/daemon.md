# テスト妥当性レビュー: daemon
- 評価: 混在

## 確認範囲
- `crates/daemon/Cargo.toml`
- `crates/daemon/src/lib.rs`
- `crates/daemon/README.md`
- リポジトリ内の `daemon` 参照
- `cargo test -p daemon`

`daemon` は現時点では意図的な placeholder crate です。`README.md` では、将来の long-lived Pod lifecycle management のために予約されており、現在の Pod socket serving、CLI/TUI startup、registry mechanics をまだ所有してはいけないと説明されています。`src/lib.rs` は空です。

## 現在のテストがよくカバーしていること
- `cargo test -p daemon` は成功します。
- crate が package target としてビルドできることは確認されています。
- unit test と doc test は正常に実行されますが、どちらも `0` tests です。
- 現在の空実装では、この crate 内で直接テストできる振る舞いはありません。

## 不足・疑わしいテスト
- テストは存在しないため、現在の test suite が証明しているのは placeholder crate がコンパイルできることだけです。
- `README.md` に書かれた placeholder boundary はテストで強制されていません。たとえば、具体的な設計なしに runtime lifecycle authority が誤ってここへ追加されることを防ぐ自動 guard はありません。
- `Cargo.toml` は `manifest`、`protocol`、`tokio` への依存を宣言していますが、crate にはそれらを使うコードがありません。テストからは、これらの依存が意図的な scaffolding なのか、古い placeholder baggage なのかは分かりません。
- 実装が存在しないため、テスト妥当性は「空 crate がコンパイルできる」以上にはほとんど評価できません。

## 追加を提案するテスト
- `daemon` に具体的な責務が生まれるまでは、behavioral test は追加しないでください。
- crate をしばらく placeholder のままにするなら、予期しない public API や非空の runtime 実装を検出する軽量な policy/lint-style check を crate 外に置くことは考えられます。ただし任意であり、過剰かもしれません。
- daemon 機能を導入するときは、実装詳細ではなく実際の invariant に対するテストを追加してください。
  - long-lived Pod management の lifecycle state transition
  - persistence / recovery behavior
  - shutdown、cancellation、restart semantics
  - `daemon`、`pod`、`pod-registry`、`yoi`、`tui` の authority boundary
  - socket/path handling と failure case
  - 該当する場合は manifest/profile-derived configuration との interaction

## 実行したコマンド
```sh
cd /home/hare/Projects/yoi && cargo test -p daemon
```

結果: 成功。`daemon` をビルドし、`0` unit tests と `0` doc tests を実行しました。
