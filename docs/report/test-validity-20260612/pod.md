# テスト妥当性レビュー: pod

- 評価: 混在

## 確認した範囲

- `crates/pod/Cargo.toml`、`crates/pod/README.md`、`crates/pod/src/lib.rs`、`crates/pod/tests/*.rs` の integration suite、テスト一覧、および選択した失敗中の prompt-resource assertion を確認した。
- `pod` crate の責務: `agen` を中心とした Pod lifecycle/runtime authority、socket protocol serving、manifest/profile startup、スコープ付き built-in tools、session/pod-store persistence、spawned child Pod orchestration、prompt/resource assembly、compaction/extraction/consolidation、および runtime metadata integration。
- `cargo test -p pod -- --list` によるテスト一覧:
  - 310 件の lib/unit tests。
  - 9 件の integration test target、合計 106 件の integration tests。
  - 0 件の doctests。
  - 一覧上の合計: 416 tests。

## 現在のテストがよくカバーしている点

- suite は広範で、crate の実際の責務によく対応している。
- Controller/runtime behavior は意味のある state-machine tests でカバーされている: idle/running transitions、double-run rejection、pause/resume/cancel behavior、empty-turn rollback、provider stream error recording、snapshots、event broadcast、socket method errors、notification handling。
- Spawned Pod behavior は重要な authority invariants 周辺を強くカバーしている: workspace root と process cwd の区別、explicit delegation scope、parent authority 外の scope rejection、non-recursive scope rejection、omitted cwd behavior、`Run` の送信、予約済み child socket が一度も現れない場合の rollback。
- Pod communication tools は純粋な mock だけではなく mock Unix sockets に対してテストされている: send/read/stop behavior、initial alert/snapshot draining、already-running errors、unreachable child stop behavior、durable registry restore、scope reclamation が表現されている。
- Persistence-adjacent behavior は `FsStore`/`FsPodStore` tempdirs でカバーされている: active/pending segment restore、session-log drift auto-forking、metrics のない old sessions、durable spawned-child state、restore rejection cases。
- Compaction、pruning、extraction、memory consolidation は通常以上に良い invariant coverage を受けている: token split boundaries、tool-call/result pair balancing、threshold behavior、circuit breakers、warning/error events、audit-only skips、lock behavior、staging cleanup、extract pointer reset。
- Prompt/resource handling は単純な文字列読み込みを超えてテストされている: loader prefix resolution、include semantics、override order、system prompt trailing sections、AGENTS.md injection、memory guidance gating、compaction を跨ぐ runtime prompt materialisation。
- Feature/plugin-adjacent boundaries はよくカバーされている: declared contributions と host authorities の区別、ticket feature registration と fail-closed cases、task tool replay/reminder behavior、hooks/interceptors、permission matching、fs-view scope filtering、runtime directory/shared-state basics。

## 不足 / 疑問のあるテスト

- suite は現在 red。`cargo test -p pod --no-fail-fast` は 414 passed、2 failed で終了する。どちらの failure も古くなった prompt-prose substring assertions:
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
  - どちらも `rendered.contains("worktree status, diff, and test results")` を assert している一方、`resources/prompts/common/pod-orchestration.md` は現在 `worktree state, diff, and test results` と書いている。
- この 2 件の failing tests は、Rust tests 内で model-facing prose を正確に重複しているため疑問がある。prompt-resource drift は検出できるが、失敗している phrase は stable API invariant ではない。そのため semantic guidance が保たれていても、通常の prompt wording edits で crate が壊れる。
- 真の end-to-end Pod process smoke test がない。integration tests は mock `LlmClient`、temp stores、mock Unix listeners、injected runtime commands を使っており、速度面では適切だが、実際の `yoi pod`/entrypoint/runtime-dir/socket/session path は全体として exercise されていない。
- いくつかの async/socket tests はまだ fixed sleeps と process-wide environment mutation (`YOI_RUNTIME_DIR`, `YOI_HOME`, `XDG_RUNTIME_DIR`) に依存している。一部ファイルでは env-changing tests を guards で serialize しているが、fixed timing と global env は高負荷 CI や parallel execution 下で flakiness risk のまま。
- Real provider wire behavior は意図的に `pod` の外側だが、crate と streaming edge cases の interaction はまだ大部分が mock されている。tests は重要な `Worker` outcomes をカバーしているが、malformed/partial provider streams を real provider adapter 経由では exercise していない。
- Prompt tests は数が多く有用だが、一部は behavior-coupled というより prose-coupled である。critical safety wording に対して、その文字列を意図的に stable contract として扱う場合だけこれは許容できる。そうでなければ maintenance noise になる。
- startup profile resolution、socket server、`Method::Run`、session persistence、shutdown、restore を跨ぐ full lifecycle integration は slice ごとにしかカバーされておらず、1 つの scenario としてはカバーされていない。

## 追加を提案するもの

- 古くなった phrase ではなく durable semantic invariant を assert する形で、2 件の failing prompt tests を修正する。たとえば、section が含まれること、child notifications が hints であること、required evidence に `diff` と `test results` が含まれることの anchors は保ちつつ、2 つの Rust tests で文全体を重複しない。
- まずは ignored または feature-gated でもよいので、最小限の end-to-end smoke test を 1 件追加する。temp `YOI_HOME`/runtime dir で実際の Pod runtime を起動し、socket に接続し、単純な method を送信し、status/events を観測し、session/pod metadata を検証し、clean に shutdown するもの。
- profile selection と prompt-resource loading 周辺に real startup/restore smoke を追加する。`pod` は manifest/profile/resource resolution が running Pod になる境界を所有しているため。
- controller/socket tests の fixed sleeps を、bounded `timeout` で包んだ condition-based waits に置き換える。特に status、socket accept、event propagation を待っている箇所。
- `catalog` と `system` tests が脆い prose anchors を重複しないように、prompt-orchestration guidance assertions を 1 つの helper または snapshot-style semantic check に集約する。
- simultaneous child reservation や duplicate child name/scope acquisition に対する focused race test を追加する。その invariant が `pod-registry` だけでなく `pod` によって enforce されることが期待される場合。
- production が current executable path を使う境界で runtime-command resolution failure/override のテストを 1 件検討する。child Pod spawning はその path に運用上敏感なため。

## 実行したコマンド

- `cargo test -p pod -- --list`
  - 成功。
  - 310 件の lib tests、106 件の integration tests、0 件の doctests を列挙した。
- `cargo test -p pod`
  - integration targets に到達する前に lib test target で失敗。
  - 失敗時点の結果: 308 passed、2 failed。
- `cargo test -p pod --no-fail-fast`
  - lib target が失敗したため全体として失敗。
  - Lib target: 308 passed、2 failed。
  - Integration targets: 106 passed、0 failed。
  - Doctests: 0。
  - Failing assertions はどちらも `worktree status, diff, and test results` に対する古い exact prompt-prose checks。

