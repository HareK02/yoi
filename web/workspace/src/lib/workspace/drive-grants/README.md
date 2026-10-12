# Worker Drive grants

既存Workers画面内のDrive grant管理を担当する。Worker一覧と管理permissionは親画面のBackend projectionを使い、独自のWorker registryやProfile編集機能は持たない。

- `WorkerDriveGrants.svelte`: Worker選択、grant表示、作成と取消しの状態管理。
- `api.ts`: 認証済みgrant APIと応答検査。
- `WorkerDriveGrants.browser.spec.ts`: 実componentを使うHTTP fixture検証。

変更時はcomponentとAPI境界のfixture検証を保ち、Worker catalogの所有者をこのcomponentへ移さない。
