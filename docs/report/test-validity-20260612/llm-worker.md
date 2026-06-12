# テスト妥当性レビュー: llm-worker
- 評価: 概ね良い

## 確認範囲
- Workspace root: `/home/hare/Projects/yoi`
- Crate/package: `crates/llm-worker` / `llm-worker`
- 読んだ範囲:
  - `crates/llm-worker/Cargo.toml`
  - `crates/llm-worker/src/**`
  - `crates/llm-worker/tests/**`
  - `crates/llm-worker/docs/{requirements,architecture}.md`
- ソースファイル、Ticket、設定ファイルは変更していない。

## 現在のテストがよくカバーしていること
- Provider adapter 周りの単体テストはかなり厚い。
  - Anthropic / Gemini / OpenAI Chat / OpenAI Responses の request projection、SSE event parsing、tool call/result、usage、reasoning metadata、prompt cache 関連が細かく検証されている。
  - OpenAI Responses は未知 event/error の保持、reasoning item の round-trip、encrypted content、stateless ZDR default など、最近壊れやすい仕様面もテストされている。
- Worker の主要な in-process 挙動は integration test で押さえられている。
  - text response、tool call、tool call collector、history update、locked/mutable state transition、system prompt preservation、turn count、tool registration など。
  - `trybuild` による compile-fail test があり、typestate 制約の一部を型レベルで検証している点は良い。
- Callback / hook / parallel tool execution のテストがある。
  - text/tool/usage/turn/retry callback、pre/post tool hook、synthetic/skip path、execution context の order / batch id が検証されている。
- Provider fixture smoke test がある。
  - Anthropic / Gemini / Ollama / OpenAI fixture を deserialize し、timeline と usage token の最低限の整合を見る構成になっている。
- Transport/retry 境界にもテストがある。
  - retryable/non-retryable status、`Retry-After` preservation、mid-stream SSE error、response timeout retryability、Codex backend header/compression が検証されている。
- prune / token counter / tool server / truncation など、Worker 周辺の重要な小さい invariant も単体テスト化されている。

## 不足 / 疑問のあるテスト
- Core requirement に見える Pause/Resume 系の実行テストが不足している。
  - `PreToolAction::Pause` / `TurnEndAction::Pause`、pending tool call の保持、`resume()` 後の再開、resume 時に余計な user message を積まないこと、複数 tool call batch の途中停止などは、少なくとも現行テスト名からは直接確認できなかった。
  - typestate や history accumulation は見ているが、「pause を跨いだ Worker の安全性」は別の重要 invariant。
- Cancel / interruption / continuation の安全性テストが薄い。
  - `cancel()` は doc example と API として存在するが、stream 中 cancel、tool 実行前後 cancel、partial assistant content の扱い、未完成 tool JSON/reasoning fragment を history に残さないことのような Worker として重要な保証がテストで明確に固定されていない。
  - stream-started failure 後の continuation / retry path も、単体の timeout/probe test はあるが、history-aware continuation の最終状態を integration で確認するテストが欲しい。
- 一部 integration test が `worker.run(...).await` の戻り値を捨てており、後段で限定的な副作用だけを見る形になっている。
  - 意図的に error path を見ている箇所以外では、`expect` または error 内容の明示 assert にした方が、run 後半の失敗を取り逃がしにくい。
- Fixture tests は smoke としては有用だが、契約テストとしては緩い箇所がある。
  - `common` 側で usage や timeline が不足した場合に warning 寄りの扱いになっている部分があり、provider-specific contract を固定するには弱い。
  - provider ごとの差異を許す helper は便利だが、重要な event sequence は fixture ごとにもう少し strict にしてよい。
- Parallel execution test に wall-clock threshold 依存がある。
  - 現状は短時間 sleep ベースで並列性を確認しているため、遅い/混雑した CI では flaky になり得る。barrier と atomic max-concurrency などで「同時に走った」ことを直接見る方が堅い。
- Doc tests は 24 件 list されるが、すべて `ignore` 扱い。
  - API 使用例の維持には役立ちにくい。外部 API surface が重要な crate なので、可能なものは `no_run` または compile-only doctest に寄せる価値がある。
- 実 provider との E2E はない。
  - crate 単体の妥当性評価としては必須ではないが、provider schema drift を検知する意味では fixture 更新だけでは限界がある。

## 追加を提案するテスト
- Pause/Resume integration tests を追加する。
  - pre-tool pause → pending tool call 確認 → resume → tool result commit → assistant continuation。
  - turn-end pause → history に assistant output が残る → resume で次 request が正しく構築される。
  - 複数 tool calls のうち一部/全部を pause した場合の order、batch id、history item の不変条件。
- Cancel/interruption tests を追加する。
  - stream 中 cancel で partial text/reasoning/tool call fragment が安全に扱われること。
  - tool 実行中 cancel の結果が history/callback/result にどう反映されるか。
  - incomplete tool JSON と reasoning fragment が次 turn の request に混入しないこと。
- Continuation/retry の Worker-level integration を追加する。
  - stream-started 後の transport error → continuation notice callback → safe partial history commit → retry/continue request の形。
  - continuation limit 到達時の error と history post-condition。
- `worker.run` の戻り値を捨てている既存テストを見直す。
  - 正常系は `expect("worker run succeeds")`。
  - 異常系は error kind/message と、その時点までに許される副作用を明示 assert。
- Fixture tests を provider-specific に少し強める。
  - usage が必須の fixture では warning ではなく assert。
  - event sequence の必須 block / tool / usage / reasoning metadata を fixture ごとに固定。
- Parallel execution test の時間依存を下げる。
  - sleep duration で速さを見るより、barrier + concurrent counter で最大同時実行数を assert する。
- 主要 public examples を `ignore` から compile-only doctest に移す。
  - 実行不能な外部 call は `no_run`、擬似コードは明確に `text` fence などに分ける。

## 実行したコマンド
- `cargo test -p llm-worker`
  - Result: passed.
- `cargo test -p llm-worker -- --list`
  - Result: passed/listed.
  - Executable tests listed: unit/integration/trybuild 全体で 237 件。
  - Doc tests listed: 24 件、すべて ignored。
- `cargo test -p llm-worker --quiet`
  - Result: passed.

