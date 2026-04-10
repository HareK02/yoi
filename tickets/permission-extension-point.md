# パーミッション: パターンベースのツール実行制御

## 背景

現状の `Scope` はディレクトリ単位の書き込み制約で、静的な境界線。
実際のエージェント運用では、ツール単位・引数パターン単位の動的な権限制御が必要になる。

OpenCode はパターンベースのルール（tool × pattern → allow/deny/ask）を持ち、
`*.env` への書き込み拒否や `rm -rf` の実行拒否を宣言的に設定できる。

## 方針

`PreToolCall` Hook として実装する。マニフェストにルールを宣言し、
insomnia 層の Hook 実装がツール呼び出し時に評価する。

```toml
[[permission]]
tool = "bash"
pattern = "rm *"
action = "deny"

[[permission]]
tool = "file_write"
pattern = "*.env"
action = "deny"

[[permission]]
tool = "*"
pattern = "*"
action = "allow"
```

評価順序（OpenCode に倣う）:
1. 最初にマッチした `deny` → 拒否
2. すべてマッチする `allow` → 許可
3. それ以外 → `ask`（ユーザー確認）

## 設計ポイント

- 設計原則3: 新しい trait は作らない。`PreToolCall` Hook として実装
- 設計原則2: マニフェストに宣言した以上、insomnia 層が解決する
- `ask` アクションは Pod Protocol の拡張が必要（Method に `PermissionReply` を追加）
- `Scope` との関係: Scope は書き込みの物理的境界、Permission はツール実行のポリシー。補完関係
- ルール評価はパターンマッチのみ。コンテキスト依存の判断はしない（シンプルに保つ）

## 段階的実装

1. **拡張ポイントの記録**（今）: docs/pod.md の拡張ポイント表に追加
2. **deny/allow の実装**（ツール実装時）: PreToolCall Hook でパターン評価
3. **ask の実装**（Protocol 拡張時）: Method/Event に Permission 関連メッセージを追加

## 依存チケット

- [remove-hook-module.md](remove-hook-module.md) — PreToolCall が insomnia 層に移動してから実装
