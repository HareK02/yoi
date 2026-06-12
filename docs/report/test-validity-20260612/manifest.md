# テスト妥当性レビュー: manifest
- 評価: 概ね良い

## 確認範囲
- Workspace root: `/home/hare/Projects/yoi`
- Crate/package: `manifest`
- 読み取り専用のレビューのみを行い、ソースファイルと Ticket は変更していない。
- 主な責務を以下で確認した:
  - `crates/manifest/src/lib.rs`
  - `crates/manifest/src/config.rs`
  - `crates/manifest/src/scope.rs`
  - `crates/manifest/src/profile.rs`
  - `crates/manifest/src/paths.rs`
  - `crates/manifest/src/model.rs`
  - `crates/manifest/src/defaults.rs`
- `cargo test -p manifest` を実行: passed、`129 passed; 0 failed`。

## 現在のテストがよくカバーしていること
- この crate には、中核責務である manifest/profile 設定の parse、merge、validate、resolve に対する強い unit-test suite がある。
- `config.rs` のテストは、多くの重要な manifest invariant をカバーしている:
  - 最小限の TOML parse と defaulting;
  - model/tool/web/memory/compaction/reasoning fields;
  - TOML type mismatch を hard error として扱うこと;
  - partial config cascade と upper-layer override behavior;
  - deprecated/removed fields が reject されること;
  - authority-bearing fields における invalid relative paths;
  - feature-flag behavior、特に shell/file/web/memory/tool areas の無効化;
  - prompt/resources/workflow directories と generated memory settings。
- `scope.rs` のテストは、セキュリティ上もっとも重要な path permission behavior をカバーしている:
  - recursive grants と non-recursive grants;
  - read/write permission matching;
  - deny entries が grants を override すること;
  - path traversal rejection;
  - canonicalization/relative-path normalization;
  - delegation subset checks;
  - shared-scope add/remove/clear operations。
- `profile.rs` は、Lua profile behavior について特に広いカバレッジを持つ:
  - 意図された `yoi.profile` API 経由の profile registration;
  - legacy/global APIs の明示的な rejection;
  - builtin role profile defaults と tool-policy boundaries;
  - workspace/profile override precedence;
  - `extend`、helper APIs、TOML loading、model catalog access、Lua module loading;
  - multiple-profile discovery、selector handling、ambiguity、exact/unique match behavior。
- `paths.rs` のテストは、pure resolver functions によって XDG/Yoi directory precedence rules をカバーしており、global environment races を避けている。
- いくつかのテストは、単なる実装機構ではなく、重要な product decisions を regression checks として符号化している。manifest/profile semantics は durable contract なので、この crate ではそれが適切である。

## 不足 / 疑問のあるテスト
- カバレッジはほぼすべて in-module unit tests である。これは概ね適切だが、downstream crates や CLI startup の観点からの integration-style coverage は少ない。変更によってこれらの local unit tests は維持されても、`yoi`、`pod`、または profile loading が manifest API を compose する方法が壊れる可能性がある。
- Path-scope security tests は良いが、adversarial filesystem boundaries に対してはまだ網羅的ではない。workspace 内の symlink が外を指すケース、workspace 外の symlink が中を指すケース、または canonicalized symlink paths を通じた mixed deny/grant interactions に対する専用テストは見当たらなかった。この crate の authority role を考えると、これが主な残リスクである。
- Delegation subset tests は基本的な matrix をカバーしているが、より明示的な negative cases があるとよい:
  - read-only grant は write delegation を満たしてはならない;
  - non-recursive parent grant は child delegation を満たしてはならない;
  - deny entries は direct access を制約するのと同じように delegation を制約するべきである;
  - 同一 path 上の mixed read/write grants は order-independent のままであるべきである。
- 一部の default/builtin profile tests は snapshot-like で、詳細な role policy を assert している。これらは価値があるが、builtin role contracts が意図的に進化すると脆くなりうる。この脆さは、これらを policy-contract tests として扱うなら許容できる。そうでなければ、“must never regress” invariants と “current default snapshot” fixtures に分けるべきである。
- Model/auth schema coverage は manifest/profile coverage より薄い。`ModelManifest::merge` と `AuthRef` variants は単純だが、`codex_oauth`、`secret_ref`、context-window/max-context-window precedence の round-trip tests を追加すると regression risk を下げられる。
- Compact-ratio behavior には known model context lookup を含む positive coverage があるが、以下についてより明確な negative tests があるべきである:
  - model context がなく、unknown model ref である場合の ratio;
  - `context_window` が `max_context_window` より明示的に大きい場合;
  - それらを reject する意図があるなら、invalid/edge ratios。
- Public path helpers は、exported environment-reading functions ではなく pure resolver helpers を通じてテストされている。これは determinism のためには良いが、ごく小さな serial/env-scoped smoke test があれば public functions の配線ミスを検出できる。
- README/doctest coverage は実質的に存在しない（doctests は `running 0 tests`）。これは config crate にとって大きな問題ではないが、README examples が public contract の一部になるなら、実行可能にするか、テストでミラーするべきである。

## 追加を提案するもの
- private helpers だけをテストするのではなく、public API 経由で代表的な manifest/profile fixtures を load する小さな integration test を `crates/manifest/tests/` 配下に追加する。
- `Scope` に symlink boundary tests を追加する:
  - allowed root 内の symlink が外を指す場合は deny されるべきである;
  - 外側の symlink が内側を指す場合は canonical target policy に従って振る舞うべきである;
  - symlink を含む deny paths は、normalization/canonicalization 後も grants を override するべきである。
- permission、recursion、deny、path ancestry の組み合わせをカバーする delegation matrix test table を追加する。
- サポートされているすべての auth variants と context-window precedence について model/auth round-trip tests を追加する。
- missing/unknown context と invalid edge values 周辺の negative compact-ratio tests を追加する。
- snapshot-like builtin profile tests を policy-contract tests としてラベル付けするか、期待値を compact fixtures に移して、意図的な role-policy changes をレビューしやすくすることを検討する。

## 実行したコマンド
- `cargo test -p manifest`
  - Result: passed
  - Summary: `129 passed; 0 failed; 0 ignored; finished in 0.00s`

