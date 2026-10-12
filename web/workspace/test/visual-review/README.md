# Repository任意作成の視覚検証

production static Workspace画面と共有 `tools/web-ux` harnessを使い、RepositoryなしのWorkspace作成、catalog、Worker起動を検証する。loopback proxyはAPI fixtureだけを拡張し、shell、style、componentはproduction実装を使う。

- `optional-repository.json`: 空Workspaceとcatalogの表示条件。
- `optional-repository-interactions.json`: 作成操作と意図した拒否応答。

実Workspace、account、Repository、Runtime、Workerは作成しない。拒否fixtureの診断を正常系の成功として扱わず、captureの終了状態だけで視覚面を承認しない。横断検証規則は [Visual Review](../../../../docs/development/ui-ux/visual-review.md) に従う。
