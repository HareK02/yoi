# yoi E2E

実際の製品プロセスを隔離したfixtureで検証する。PTY操作とBackend／Runtimeプロセスの検証を担当し、通常のunit testとは分離する。

## 実装の入口

- [`PanelHarness`]: PTY経由の入力、観測イベントによる同期、失敗時のartifact保存。
- [`FixtureWorkspace`]: TUI用WorkspaceとHOME／XDG領域の隔離。
- [`WorkerE2eEnvironment`]: Backend／Runtime用プロセスとfixtureの管理。
