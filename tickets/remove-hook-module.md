# Hook モジュールの llm-worker からの除去

## 背景

llm-worker は低レベル基盤に徹するべきだが、現行の `hook` モジュールは
高レベルのオーケストレーション関心（承認フロー、ターン制御等）を含んでいる。
Claude Code の Hooks のような機能は insomnia 層の責務。

低レベルのストリーム介入は、クロージャベースの Subscriber API で十分カバーできる。

## 方針

`hook` モジュールを llm-worker から除去し、責務を分離する。

### Subscriber で代替（削除）

ストリーム観測・介入はクロージャ Subscriber で対応:

- `OnTextDelta`
- `OnToolCallDelta`
- `OnStreamChunk`
- `OnStreamComplete`

### insomnia 層に移動

高レベルオーケストレーションは上位層が担う:

- `OnPromptSubmit`
- `PreLlmRequest`
- `PreToolCall` / `PostToolCall`
- `OnTurnEnd`
- `OnAbort`

## 設計ポイント

- Worker の実行ループは「ストリーム受信 → ツール実行 → 次ターン」に集中させる
- 介入ポイント（承認、中断、ターン継続判断）は insomnia 層が提供する
- `HookEventKind` / `Hook<E>` の型設計自体は良いので、insomnia 層で再利用可能

## 依存チケット

- [subscriber-closure-api.md](subscriber-closure-api.md) — ストリーム系 Hook の代替先
