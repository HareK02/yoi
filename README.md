# 夜居 / Yoi

Yoiは、LLM Workerの実行履歴と明示的な権限を管理するエージェント実行基盤である。Workspaceの記録と操作権限をBackendが管理し、RuntimeがWorkerと作業ディレクトリの実行資源を管理する。単一プロセスで実行するStandaloneは、Workspaceの権限やRuntime Workerの登録とは分離している。

## 実装の入口

- [agen](crates/agen/README.md)：LLMターン、履歴、ツール呼出し。
- [worker](crates/worker/README.md)：セッション、機能、ツールと実行権限の接続。
- [workspace-server](crates/workspace-server/README.md)：Workspaceの正本とBackend操作。
- [worker-runtime](crates/worker-runtime/README.md)：Workerの実行とRuntime資源。
- [standalone](crates/standalone/README.md)：プロセス内の独立した実行ホスト。
- [yoi](crates/yoi/README.md)・[tui](crates/tui/README.md)・[Web](web/workspace/README.md)：利用者との接点。

各crateのREADMEをcrateドキュメントとして使い、具体的な設計理由はmodule・itemのrustdocに置く。文書の追加・維持・削除は[文書管理規則](docs/README.md)、変更の検証は[検証規則](docs/development/validation.md)に従う。

ライセンスは[MIT](LICENSE)。
