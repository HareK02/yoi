# テスト設計

## 背景

各クレートのテスト方針が未策定。クレート間の依存関係と非同期処理が絡むため、
テストの層（単体/結合/E2E）と mock 境界を明確にする必要がある。

## 検討事項

- `llm-worker`: LlmClient の mock 実装によるターンループ・ツール実行のテスト
- `llm-worker-persistence`: FsStore / FsBlobStore のファイルシステムテスト（tempdir）
- `pod`: PodController / SocketServer の結合テスト
- `protocol`: シリアライズ/デシリアライズの往復テスト
- `manifest`: パースのバリエーション（既存テストの拡充）
