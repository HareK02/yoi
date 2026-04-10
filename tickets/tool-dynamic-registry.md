# ツールの動的追加/削除

## 背景

現状の `ToolServer` はツールの登録のみで、実行中の unregister / replace ができない。
エージェントが状況に応じてツールセットを切り替えるユースケース（例: フェーズ遷移、権限変更）に対応できない。

## 方針

`ToolServer` / `ToolServerHandle` に動的操作を追加する。

```rust
// 削除
tool_server.unregister("tool_name")?;

// 置換（同名ツールを新しい実装で上書き）
tool_server.replace(new_tool_definition)?;
```

## 設計ポイント

- 実行中のツール呼び出しとの競合を考慮（呼び出し中のツールは削除をブロックするか、完了を待つか）
- LLM に渡すツール定義リストは次の `PreLlmRequest` 時点で反映される（遅延反映で十分）
- Builder API（worker-builder-api.md）との整合: `reconfigure()` で静的に差し替える方法と、動的に差し替える方法の使い分け
