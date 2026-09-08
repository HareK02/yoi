# Workspace ↔ Runtime 認証

Yoi の Remote Runtime 認証は Workspace ごとの署名 identity を authority とする。
Server-global な署名鍵や Runtime 側の trusted-Server catalog は使わない。

## Authority

- Server DB は Workspace ごとの signing identity と Runtime binding を保持する。
- Runtime は `trust-workspace` で受理した `WorkspaceIssuerAuthorizationBundle` を保持する。
- bundle は `workspace_id`、Workspace key id/generation、Workspace public key、Backend URL、許可された Runtime identity を固定する。
- Server → Runtime の各 HTTP / WebSocket request は、対象 Workspace の signing identity で短命な capability token を発行する。
- Runtime は request method、`path_and_query`、body digest、permission、Workspace、Runtime、key generation、expiry、JTI を検証する。
- Runtime → Server の source proof は Runtime identity で署名し、対象 Workspace と bundle の Backend URL を audience に固定する。
- Server は現在の Workspace Runtime binding、Runtime public key、Backend public URL、request target、body digest、permission、expiry、replay state を検証する。

旧 Server identity/trust 管理 command と旧 Runtime-side Server trust command、旧 Runtime auth key flags は廃止済みである。これらに相当する Server-global trust を fallback として使ってはならない。

## Provisioning

1. Runtime identity を初期化する。

   ```sh
   yoi-runtime identity init --runtime-id <runtime-id>
   yoi-runtime identity show
   ```

2. Workspace owner が Settings → Runtimes から Runtime public bundle と endpoint を登録する。
3. Server が Workspace issuer bundle と challenge を発行する。
4. operator が bundle を Runtime に追加する。

   ```sh
   yoi-runtime trust-workspace add --bundle <workspace-issuer-bundle.json>
   yoi-runtime trust-workspace show --workspace-id <workspace-id>
   ```

5. Runtime が challenge proof を生成し、Workspace owner が Server に submit する。
6. Server が verified binding を commit した後、通常の Workspace-signed request が利用可能になる。

同じ Runtime identity は異なる Workspace から独立して信頼できる。trust record、replay protection、binding、失効はすべて Workspace scope で評価する。

## Runtime auth file

`runtime-auth.toml` は Runtime identity と Workspace issuer records のみを authority とする。
旧 Server trust entry は読み飛ばされ、以後の identity / `trust-workspace` 更新時に書き戻されない。旧 entry を残しても認証には使用されない。

`trust-workspace` の file store は次を fail closed で検証する。

- 最大 8 MiB
- 最大 4,096 records
- exact Workspace / Runtime identity
- key id/generation と public key fingerprint
- normalized Backend URL
- replace 時の expected current generation
- list は `offset` / `limit` 必須で、1 page 最大 100 records

## Local token

`--local-token` は明示的な local Runtime 呼び出し専用であり、Remote Workspace binding の代替ではない。Workspace issuer auth が有効な Remote Runtime request は Workspace capability token を使う。

## Rotation と失効

Workspace signing key または Runtime key の変更は、現在 binding を置き換える明示的な provisioning 操作として行う。古い generation、古い Runtime key、revoked binding、失効済み token、replayed JTI は即時拒否する。

Server の Runtime cache は現在の persisted binding 全体と照合する。endpoint、Runtime public key/fingerprint、binding revision、Workspace key generation の変更を検知した場合、stale client を利用しない。

## 運用確認

Remote Runtime を有効化した後は次を確認する。

1. `yoi-runtime trust-workspace show --workspace-id <workspace-id>` が期待する bundle を表示する。
2. Workspace Settings の Runtime binding が `verified` で、現在の key id/generation と verification evidence を表示する。
3. Runtime ping、Worker list/create、`worker.protocol` subscription が Workspace-signed token で成功する。
4. wrong Workspace、wrong Runtime、wrong target/body、expired token、revoked/replaced binding、replayed JTI が拒否される。
5. Runtime → Server source proof が configured Backend public URL audience と一致し、spoofed headers だけでは認証されない。

Server / Runtime の再起動は live reload ではない authority 変更を反映するときだけ、通常の運用権限と migration gate に従って行う。実行中プロセスを開発 Worker が無断で停止してはならない。
