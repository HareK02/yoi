# テスト妥当性レビュー: llm-worker-macros
- 評価: 混在

## 確認範囲

- 対象 crate: `crates/llm-worker-macros`
- 読んだ主なファイル:
  - `crates/llm-worker-macros/src/lib.rs`
  - `crates/llm-worker-macros/Cargo.toml`
  - `crates/llm-worker-macros/README.md`
  - `crates/llm-worker/tests/tool_macro_test.rs`
- 変更はしていない。

`llm-worker-macros` 自体にはローカル unit/integration test がなく、`cargo test -p llm-worker-macros` では実質的に挙動を検証していない。一方で、下流 crate の `llm-worker` 側に `#[tool_registry]` / `#[tool]` の実利用 integration test があり、主要な happy path と一部 error path は確認されている。

## 現在のテストがよくカバーしていること

`crates/llm-worker/tests/tool_macro_test.rs` は、この proc macro が実際に `llm_worker::tool::Tool` と組み合わさって動くことを確認しており、単なる helper unit test より有用な面がある。

特に以下はよく押さえられている。

- `#[tool_registry]` が `*_definition()` を生成し、`ToolMeta` と `Tool` instance を返せること。
- tool 名が method 名由来になること。
- doc comment が `ToolMeta.description` に入ること。
- JSON 引数が deserialize され、method に渡されること。
- 複数引数の tool が動くこと。
- 引数なし tool が `{}` で実行できること。
- 不正な引数、少なくとも必須 field 欠落が error になること。
- `Result<T, E>` 返却の success/error mapping が動くこと。
- async method と sync method の両方が wrapper 経由で実行できること。
- `ToolExecutionContext` typed parameter が JSON schema/input からではなく実行時 context から渡されること。
- definition factory を複数回呼んでも `ToolMeta` が安定していること。

この crate の中心責務は「compile-time macro expansion による runtime tool definition 生成」なので、下流 crate で実 macro expansion を通している点は妥当。

## 不足・疑問のあるテスト

- `llm-worker-macros` package 自身の test は 0 件。`cargo test -p llm-worker-macros` 単体では回帰検知力がほぼない。
- proc macro crate として重要な compile-pass / compile-fail matrix がない。現在は runtime integration test 中で使われる形だけが通っている。
- `#[description = "..."]` parameter attribute の挙動が未検証。実装は `#[schemars(description = ...)]` に変換するが、schema に description が出るか、parameter attribute として期待通り扱えるかをテストしていない。
- JSON Schema の検証が弱い。`properties` の存在だけを見ており、以下を確認していない。
  - field 名
  - `required`
  - field type
  - `ToolExecutionContext` が schema から除外されること
  - `#[description]` が schema に反映されること
- 不正引数 test が error variant や message を確認していない。`ToolError::InvalidArgument` であることまで見るべき。
- 引数なし tool は invalid JSON も `unwrap_or` で通す実装になっているが、この仕様が意図通りかを固定する test がない。現在の test は `{}` のみ。
- macro expansion が参照する外部 path/import の契約が未検証。生成コードは `serde`, `schemars`, `serde_json`, `async_trait`, `::llm_worker` に依存しているため、利用側 crate で何を明示 import/dependency すべきかを compile test で固定した方がよい。
- unsupported input の扱いが未検証。
  - destructuring pattern 引数
  - `self` 以外の receiver 形
  - generic method
  - generic impl / lifetime 付き impl
  - `Result` alias や path-qualified result
  - `ToolExecutionContext` の path-qualified type / alias
- negative compile tests がないため、「サポート外 syntax が分かりやすく失敗するか」は不明。
- assert が `summary.contains(...)` に寄りがちで、`Debug` formatting 前提の出力仕様を明確に固定していない。脆すぎるわけではないが、macro の期待仕様としてはやや曖昧。
- doctest はすべて `ignore` なので、README/API example の鮮度は保証していない。

## 追加の提案

- `trybuild` による compile-pass tests を追加する。
  - 最小構成の consuming crate で `#[tool_registry]` が展開できること。
  - async/sync/Result/no-arg/multi-arg/context-injected/description-attribute の pass cases。
  - 必要な dependency/import 契約が明確になる fixture。
- `trybuild` による compile-fail tests を追加する。
  - destructuring pattern 引数。
  - unsupported receiver/method form。
  - `Clone` できない context。
  - 期待していない attribute placement。
  - サポート外 generic form を意図的に禁止するなら、その diagnostic。
- schema の exact-ish assertions を追加する。
  - `properties.message` 等の field 名。
  - `required`。
  - numeric/string type。
  - `ToolExecutionContext` が schema に出ないこと。
  - `#[description = "..."]` が field schema に反映されること。
- error path の assertion を強くする。
  - invalid JSON syntax。
  - missing required field。
  - wrong type。
  - それぞれ `ToolError::InvalidArgument` になること。
- 引数なし tool の invalid input 仕様を明文化して test する。現在の「parse 失敗でも空 args として続行」はかなり permissive なので、意図した仕様なら固定し、意図しないなら変更前提の regression test を作る。
- `llm-worker-macros` crate 自身にも、少なくとも compile test harness を置くか、下流 `llm-worker` 側の macro tests がこの crate の実質テストであることを明示する。

## 実行したコマンド

```sh
cd /home/hare/Projects/yoi && cargo test -p llm-worker-macros
```

結果:

- 成功。
- Unit tests: `0 passed`。
- Doctests: `0 passed`, `2 ignored`。
- この crate 単体の test 妥当性評価としては、挙動検証はほぼない。

```sh
cd /home/hare/Projects/yoi && cargo test -p llm-worker --test tool_macro_test
```

結果:

- 成功。
- `9 passed`。
- `llm-worker` 側の integration test としては、macro の主要 runtime behavior をある程度検証できている。

