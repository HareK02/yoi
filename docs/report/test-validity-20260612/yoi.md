# テスト妥当性レビュー: yoi
- 評価: 混在

## 確認範囲

`/home/hare/Projects/yoi/crates/yoi` 配下の `yoi` crate を確認した:

- `crates/yoi/Cargo.toml`
- `crates/yoi/src/main.rs`
- `crates/yoi/src/ticket_cli.rs`
- `crates/yoi/src/objective_cli.rs`
- `crates/yoi/src/memory_lint.rs`
- `crates/yoi/src/session_cli.rs`

この crate はプロダクトのバイナリ境界である。責務はトップレベル CLI の parse/dispatch、TUI/panel 起動選択、`yoi pod` 委譲、`yoi ticket ...`、`yoi memory lint`、`yoi session analyze`、および objective-record CLI の処理である。深いドメイン挙動の大半は他 crate（`ticket`、`memory`、`session-analytics`、`tui`、`pod`）に委譲されている。

この crate には 48 個の unit test があり、独立した integration test ファイルはない。

## 現在のテストがよくカバーしていること

- `main.rs` のトップレベル引数 parsing は、重要な routing 判断をカバーしている:
  - positional Pod name vs `--pod`
  - `pod` runtime subcommand の pass-through
  - `ticket`、`session`、`memory lint`、`panel`、`keys`、`setup-model`
  - `--profile` の互換性制約
  - `--resume` / Pod-name の衝突
  - Pod identity を伴う明示的な `--session`
  - `--multi` が launch alias として受理されないこと

- `ticket_cli.rs` には `TempDir` を使った比較的強い in-process CLI test がある:
  - `init` scaffold 作成と非上書き挙動
  - `create`、`list`、`show`、`comment`、`review`、`state`、`close`、`doctor`
  - configured backend root
  - relation add/list/show rendering
  - list bounding と title truncation
  - 曖昧な body source と review result の拒否
  - `state closed` が専用の `close` command を要求すること

- `memory_lint.rs` は主要な CLI 固有 invariants をカバーしている:
  - option parsing と usage error
  - 意図した memory/Knowledge record path だけが lint されること
  - invalid record が lint failure になること
  - `--warnings-as-errors` が status を変えること
  - JSON output が parse 可能で、期待される counts を含むこと

- `objective_cli.rs` は基本的な record lifecycle をカバーしている:
  - create/list/show
  - 生成される opaque record id の形
  - linked Ticket validation
  - linked-ticket state に対する doctor success/failure

- `session_cli.rs` は以下をカバーしている:
  - `analyze <path> --json` parsing
  - 最小 JSONL fixture からの output が valid JSON であること
  - 初期 non-JSON output mode の拒否

## 不足 / 疑問のあるテスト

- 現在の `yoi` crate test suite は pass しない。`cargo test -p yoi` は `ticket_cli::tests::ticket_cli_init_writes_explicit_ticket_config_scaffold` で決定的に失敗する。
  - 失敗している assertion は、scaffold role profiles に `profile = "builtin:default"` が含まれることを期待している。
  - 現在の `ticket::config::ticket_config_scaffold()` は、`role.default_profile()` 経由で `builtin:intake`、`builtin:orchestrator`、`builtin:coder`、`builtin:reviewer` のような role-specific builtin profiles を出力する。
  - 対応する `ticket` crate の scaffold test は pass しているため、これは scaffold generation 自体が壊れている証拠というより、`yoi` boundary test 内の stale duplicate assertion に見える。
  - 影響: この suite は現在、この crate に対する有効な green regression signal になっていない。

- `ticket_cli_init_writes_explicit_ticket_config_scaffold` は copied scaffold text に密結合しすぎている。`yoi` crate は、おそらく `yoi ticket init` がファイルを書き、そのファイルが期待される semantic config として parse できることを assert すべきであり、正確な role default は主に `ticket` crate tests の責務である。

- installed binary behavior に対する subprocess-level test がない。in-process test は有用で速いが、以下を検証していない:
  - `main` からの実際の exit code
  - parse/runtime error における stderr formatting
  - real binary 経由の stdout behavior
  - `std::env::current_dir()` を呼ぶ command の current-directory behavior
  - packaging/runtime resource の影響

- トップレベル parser coverage は良いが、hand-written parser としては完全ではない。欠けている、または薄い case には以下がある:
  - 引数なしの default spawn mode
  - `--workspace=...`、`--pod=...`、`--socket=...`、`--profile=...`、`--session=...`
  - 重複した positional Pod names
  - Pod name なしの `--socket`
  - `--session` と併用した `--socket`
  - `--session` と併用した `--resume`
  - `panel` の malformed argument cases

- `ticket_cli.rs` は良い happy-path lifecycle をカバーしているが、多くの CLI error と file-boundary case がまだない:
  - ほとんどの flag に対する required value 欠落
  - invalid `--state`、`--limit`、`--role`、relation kind
  - `--message`/`--file` 欠落
  - 読めない `--file`
  - comment/review/close に対する `--file` bodies
  - invalid configured backend provider/root config
  - malformed Ticket records に対する doctor failure rendering

- `objective_cli.rs` は自身の Markdown records を書き、validate するため、test surface は現在より強くあるべきである。欠けている case には以下がある:
  - missing/invalid frontmatter
  - invalid state
  - required section headings の欠落
  - duplicate linked tickets warning
  - `show` における path traversal / invalid id rejection
  - paused/done/archived/all に対する list filtering

- `memory_lint.rs` には妥当な focused tests があるが、decisions と Knowledge records を明示的にはカバーしておらず、symlink/directory の特殊性、読めない file、summary fragments を超えた stdout human-output details もカバーしていない。

- `session_cli.rs` は意図的に薄いが、ほとんどは最小 fixture を通じて delegated analytics output shape を検証している。より広い analytics correctness は `session-analytics` に属する。`yoi` crate には、unknown subcommands/options と duplicate path handling の test がまだない。

## 追加提案

- まず失敗している scaffold test を修正する。以下のどちらかを推奨する:
  - `builtin:default` ではなく `role.default_profile()` を assert するように更新する、または
  - `yoi` test を semantic にする: `ticket init` を実行し、生成された config を `TicketConfig::from_toml` / `load_workspace` で parse し、各 fixed role が launch config を持つことを assert する。正確な scaffold-string coverage は `crates/ticket` に残す。

- 完全な E2E design の前でも、binary boundary に対するごく小さい subprocess smoke suite を追加する:
  - `yoi --help` が 0 で exit し、core commands を出力する
  - unknown top-level arg が non-zero で exit し、`try yoi --help` hint を書く
  - temp cwd で `yoi ticket init` が 0 で exit し、`.yoi/ticket.config.toml` を作る
  - `yoi memory lint --workspace <temp> --json` が適切に 0/1 で exit する
  - `yoi session analyze <fixture> --json` が 0 で exit し、JSON を出力する

- トップレベル edge cases に対して、特に `--flag=value` forms と invalid combinations の table-driven parser tests を追加する。これは安価で、既存の unit-test style に合っている。

- `ticket_cli` parsing と body-file handling に対する CLI 固有の negative tests を追加する:
  - missing `--title`
  - invalid list state / limit
  - missing body source
  - unreadable `--file`
  - invalid relation kind
  - invalid comment role

- malformed persisted records 周辺の objective CLI tests を強化する。この code は validation を別 crate にすべて委譲するのではなく、自身で storage format を所有しているためである。

- domain-deep tests は owning crates に置き続ける。`yoi` では、CLI mapping、filesystem boundary behavior、output/status contract、delegated crates との integration に focus する。

## 実行したコマンド

- `cargo test -p yoi`
  - Result: failed.
  - Summary: 47 passed, 1 failed.
  - Failure: `ticket_cli::tests::ticket_cli_init_writes_explicit_ticket_config_scaffold`
  - Failure reason: assertion が generated role config に `profile = "builtin:default"` が含まれることを期待していた。

- `cargo test -p yoi ticket_cli::tests::ticket_cli_init_writes_explicit_ticket_config_scaffold -- --exact`
  - Result: failed.
  - 失敗する test が deterministic かつ isolated であることを確認した。

- `cargo test -p ticket config::tests::scaffold_config_includes_backend_and_all_fixed_roles -- --exact`
  - Result: passed.
  - これは、owning `ticket` crate の scaffold test は現行であり、`yoi` test に stale duplicate expectation があるという解釈を支持する。

- `cargo test -p yoi -- --list`
  - Result: 48 tests, 0 benchmarks を listed。

