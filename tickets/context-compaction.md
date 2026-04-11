# コンテキスト圧縮: Prune + Compact

## 背景

長時間実行エージェントにとって、コンテキストウィンドウの管理はコア要件。
現状の Worker は history をそのまま保持し、オーバーフロー時の対策がない。

OpenCode は2段階のアプローチを採る:
1. **Prune**: 古いツール出力を削除してトークンを回収
2. **Compact**: 専用エージェントで会話全体を構造化要約に圧縮

Insomnia では Hook ベースで同等の機能を実現できる。

## 方針

### Phase 1: Prune（Worker 層）

`PreLlmRequest` 相当のポイントで、古いツール出力を除去する。

```
history 内のツール出力を走査:
  - 直近 N ターン以内 → 保護
  - それ以前 → 出力を "[pruned — stored as blob {id}]" に置換
```

- Blob Storage にはすでに退避済み（llm-worker-persistence の Stored 出力）
- Prune は参照を残して本文だけ削る操作
- Worker の `Mutable` 状態で history 編集が可能

### Phase 2: Compact（Pod 層）

トークン数が閾値を超えた場合、Controller が要約を挿入する。

```
1. OnTurnEnd でトークン使用量をチェック
2. 閾値超過 → Controller が要約生成を実行
3. history を [system_prompt, compaction_summary, 直近の会話] に圧縮
4. resume で作業を継続
```

要約フォーマット（OpenCode の構造化要約を参考）:

```
## Goal
（元のユーザー指示）

## Accomplished
（完了した作業の箇条書き）

## Key Discoveries
（判明した事実・制約）

## Current State
（ファイル変更・残タスク）
```

## 設計ポイント

- Phase 1 は Worker 層の拡張。llm-worker に `prune` 機能を追加
- Phase 2 は Pod 層の制御。Controller が別の Worker（要約用）を起動する
- 要約用 Worker は短命で、ツールなし・プロンプトのみ
- OpenCode の「Replay」（圧縮後に前回のメッセージを再送）は `resume()` で自然に実現可能
- 設計原則3: 新しい trait は不要。Worker の history 操作 + Controller の制御で完結

## 依存チケット

- ~~[remove-hook-module.md](remove-hook-module.md)~~ — 完了。PreLlmRequest は Pod 層の `hook::Hook<PreLlmRequest>` として利用可能
