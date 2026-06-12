# テスト妥当性レビュー: tools

- 評価: 混在

## 確認範囲

- Workspace: `/home/hare/Projects/yoi`
- Crate: `tools`
- 確認対象:
  - `crates/tools/Cargo.toml`
  - `crates/tools/src/{lib,scoped_fs,tracker,read,write,edit,glob,grep,bash,web,error}.rs`
  - `crates/tools/tests/{integration,edge_cases}.rs`
- この crate は以下の built-in tool の挙動を担っている:
  - filesystem tools: `Read`, `Write`, `Edit`, `Glob`, `Grep`
  - process tool: `Bash`
  - network tools: `WebSearch`, `WebFetch`
  - 共有 safety primitives: `ScopedFs`, `Tracker`, `ToolsError`

## 現在のテストがよくカバーしていること

主要な機能テストのカバレッジは広く、概ね適切な対象に向いている。

- `ScopedFs` tests は中核的な read/write scope enforcement をカバーしている:
  - absolute-path requirement
  - missing files と directory targets
  - out-of-scope と read-only paths
  - write 時の parent creation
  - symlink-to-file behavior
  - symlink escape diagnostics
  - clone 間での動的な `SharedScope` updates
- `Tracker` tests は重要な read-before-edit invariant をカバーしている:
  - record 後の clean verify
  - `NotRead`
  - externally modified hash mismatch
  - clone-shared state
  - symlink variants の canonical key collapse
  - recent-file LRU behavior
  - per-file mutation guard serialization と different-file parallelism
- `Read` / `Write` / `Edit` tests は期待される user-facing contract をカバーしている:
  - Read が history を記録する
  - Write は read なしで new files を作成できる
  - 既存ファイルへの Write/Edit は prior Read を要求する
  - external modification が検出される
  - unique replacement、`replace_all`、missing string、non-unique string
- `Glob` と `Grep` は user-visible options を多くカバーしている:
  - glob matching、hidden files、mtime ordering、invalid pattern
  - grep output modes、line numbers、context、multiline、glob/type filters、pagination、binary skip、invalid regex/type
  - scope-denied results が filter される
- `Bash` tests は重要な runtime behavior をカバーしている:
  - stdout/stderr merge
  - nonzero exit summary
  - stateless cwd
  - timeout
  - long output spill と tail rendering
  - drop 時の spill cleanup
  - background job non-hang regression
- `WebFetch` / `WebSearch` には有用な local-server tests がある:
  - disabled tools が network requests を行わない
  - private address blocking
  - redirect handling
  - Brave request shaping と secret ref use
  - oversized Brave response rejection
  - HTML fallback、local reader extraction、navigation inclusion/omission、output truncation

2つの integration test files は価値がある。`core_builtin_tools()` factory wiring と、cross-tool sharing of `ScopedFs` / `Tracker` を exercise しており、これはこの crate の中心的 invariant の1つである。

## 不足 / 疑問のあるテスト

default の crate test command は現在 green ではない。`cargo test -p tools` は doctests のみで失敗する。理由は、`tracker.rs` の documentation example がまだ `core_builtin_tools(fs, tracker, bash_outputs, None)` を呼んでいる一方で、実際の関数は現在3引数を取るためである。これは test-suite validity issue である。機能テストは通っているが、開発者が実行する default command は failure を報告する。

実質的に最も大きな gap は `web_builtin_tools()` 周辺である。core tool factory は integration-tested だが、web factory については registration names、metadata/schema presence、実際の `Tool` trait object 経由の disabled behavior、public tool surface との parity が確認されていない。

いくつかの領域は happy-path や string-fragment checks でテストされているが、boundary behavior はテストされていない:

- `Read`
  - invalid UTF-8 / binary-ish content rendering behavior の直接的な coverage がない
  - offset-beyond-EOF や `limit = 0` behavior test がない
  - trailing newline / CRLF line-count semantics が固定されていない
- `Write` / `Edit`
  - mutation serialization はテストされているが、`write_then_edit_same_file_same_batch_uses_call_order` は脆く見える。coordinator は path ごとに serialize するが、test name は call-index ordering を示唆している。この test は ordering invariant を証明するというより、polling order によって通っている可能性が高い。
  - `old_string == new_string`、empty `old_string`、non-UTF-8 edit target の明示的な test がない
  - write/edit の broken symlink handling は read/symlink escape より直接的な coverage が少ない
- `Glob`
  - result cap at 1000 がテストされていない
  - relative explicit `path`、non-directory `path`、out-of-scope explicit `path` が直接カバーされていない
  - symlink directory behavior は `Glob` でカバーされているが、direct path としてのみである
- `Grep`
  - description では `.gitignore` を honor するとされているが、直接的な `.gitignore` regression test は見当たらなかった
  - `-n: false`、個別の `-A` / `-B`、zero `head_limit`、context lines 付き content-mode pagination がカバーされていない
  - `Glob` test と同様の symlink-directory rejection がカバーされていない
- `Bash`
  - timeout behavior は確認されているが、process-tree cleanup や partial output を伴う timed-out command は確認されていない
  - tail read budget を超える very large output と partial UTF-8 tail boundaries がカバーされていない
  - unusual shell syntax を含む multiline commands などの command wrapping edge cases は、間接的にしか軽くカバーされていない
- `WebFetch`
  - URL parser constraints の coverage が不足している: non-http schemes、embedded credentials、missing host
  - content-type rejection、binary body rejection、invalid UTF-8、JSON/XML/text rendering、`Content-Length` なしの response-size truncation、redirect limit exceeded、redirect-to-private blocking が直接カバーされていない
  - private-address tests は有用だが必然的に local であり、DNS/rebinding-style behavior はここでは深くテストできない

一部の tests は exact または near-exact な diagnostic strings を assert している。remediation-focused user diagnostics に対しては正当化できるが、invariant-focused checks よりもいくつかの tests を脆くしてもいる。

## 追加提案

1. `tracker.rs` の stale doctest を修正するか、意図的に mark する。`cargo test -p tools` が今日失敗するため、これは最初の test-suite maintenance item として扱うべきである。

2. 小さな `web_builtin_tools_registers_full_set` integration test を追加する:
   - `WebSearch` と `WebFetch` を期待する
   - non-empty descriptions と object schemas を verify する
   - trait object 経由で disabled tools を execute し、no-network disabled error を assert する

3. focused boundary tests を追加する:
   - `Read` offset beyond EOF、`limit = 0`、CRLF/trailing newline behavior
   - `Edit` empty `old_string`、identical strings、non-UTF-8 target
   - `Glob` explicit relative/out-of-scope/non-directory path と 1000-result cap
   - `Grep` `.gitignore`、`-n: false`、個別の `-A`/`-B`、symlink directory rejection、zero/low `head_limit`
   - `Bash` partial output 付き timeout と `TAIL_READ_BUDGET` を超える huge output
   - `WebFetch` unsupported scheme、credentials in URL、unsupported content-type、binary body、invalid UTF-8、redirect limit、redirect-to-private

4. `write_then_edit_same_file_same_batch_uses_call_order` を作り直すか rename する。call-index ordering が intended invariant なら、implementation と test がそれを deterministic にすべきである。same-file serialization のみが intended なら、test name と assertion は ordering guarantees を示唆しないようにすべきである。

## 実行したコマンド

- `cargo test -p tools`
  - Result: failed.
  - Unit/integration portions passed:
    - lib tests: `99 passed`
    - `tests/edge_cases.rs`: `10 passed`
    - `tests/integration.rs`: `14 passed`
  - Doctest failed:
    - `crates/tools/src/tracker.rs - tracker (line 29)`
    - error `E0061`: `core_builtin_tools` は現在3引数を取るが、doctest は4引数を渡している。

- `cargo test -p tools --lib --tests`
  - Result: passed.
  - `99` lib tests、`10` edge-case tests、`14` integration tests が passed。

- `cargo test -p tools --doc`
  - Result: 同じ stale `tracker.rs` doctest error で failed。

