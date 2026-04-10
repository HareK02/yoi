# max_turns: マニフェストによるターン数制限

## 背景

Pod は長時間自律実行を前提としているが、暴走防止のガードレールがない。
OpenCode は Agent 定義に `steps`（最大ツール呼び出し回数）を持ち、
サブエージェントが無限ループに陥ることを構造的に防いでいる。

Insomnia では Worker の `OnTurnEnd` 相当の制御ポイントで同様の保護が可能だが、
マニフェストに宣言がないため「設定忘れ」が暴走を許す。

## 方針

`[worker]` セクションに `max_turns` を追加し、Worker の実行ループで強制する。

```toml
[worker]
system_prompt = "You are a code reviewer."
max_tokens = 4096
max_turns = 50   # 省略時: 制限なし（明示的な無制限）
```

## 設計ポイント

- Worker の turn loop 内でカウントし、超過時は `PodRunResult::Finished` で正常終了
- 「制限に達した」ことを Event として通知（`TurnEnd` の `result` に `LimitReached` を追加）
- 省略時は制限なし。長時間実行 Pod は意図的に省略する
- `max_turns = 0` はエラー（0ターンの Pod に意味はない）

## TurnResult の拡張

```rust
pub enum TurnResult {
    Finished,
    Paused,
    LimitReached,  // 追加
}
```

## 実装場所

- `WorkerManifest` に `max_turns: Option<u32>` を追加
- `apply_worker_manifest` で Worker に設定を反映
- Worker の turn loop でカウント・判定
