# Workspace ↔ Runtime 認証

Yoi の Remote Runtime 認証は Workspace ごとの署名 identity を authority とする。
Server-global な署名鍵や Runtime 側の trusted-Server catalog は使わない。

## Authority

- Server DB は Workspace ごとの signing identity と Runtime binding を保持する。
- Runtime は `trust-workspace` で受理した `WorkspacePublicIdentityBundle` を保持する。
- bundle は `workspace_id`、Workspace `key_id`、Workspace public key/fingerprint、Backend URL を固定する。
- Server → Runtime の各 HTTP / WebSocket request は、対象 Workspace の signing identity で短命な capability token を発行する。
- Runtime は request method、`path_and_query`、body digest、permission、Workspace、Runtime、key ID、現在の trust ID、expiry、JTI を検証する。
- Runtime → Server の source proof は Runtime identity で署名し、対象 Workspace と bundle の Backend URL を audience に固定する。
- Server は現在の Workspace Runtime binding、Runtime public key、Backend public URL、request target、body digest、permission、expiry、replay state を検証する。

旧 Server identity/trust 管理 command と旧 Runtime-side Server trust command、旧 Runtime auth key flags は廃止済みである。これらに相当する Server-global trust を fallback として使ってはならない。

## Provisioning

The enrollment input includes the exact Runtime-issued `workspace_trust_id` from
`trust-workspace add/show`. Export the current Workspace public bundle first,
accept it on the Runtime, then register the Runtime public bundle, endpoint and
trust ID. Server verification binds that enrollment ID; it must never invent an
ID from the key fingerprint or assume an initial counter value. Re-enrollment
requires registering the newly issued trust ID, even when the key is unchanged.

1. Runtime identity を初期化する。

   ```sh
   yoi-runtime identity init --runtime-id <runtime-id>
   yoi-runtime identity show
   ```

2. The Workspace owner exports the current Workspace issuer public bundle from Workspace identity settings.
3. The operator accepts that bundle on the Runtime and reads the issued trust ID.

   ```sh
   yoi-runtime trust-workspace add --bundle <workspace-issuer-bundle.json>
   yoi-runtime trust-workspace show --workspace-id <workspace-id>
   ```

4. The owner registers the Runtime public bundle, endpoint and exact `workspace_trust_id` in Settings > Runtimes.
5. The Server sends a signed challenge bound to that trust ID, the Workspace key fingerprint and the binding ID, then verifies the Runtime proof and acknowledgement.
6. Normal Workspace-signed requests become available only after the verified binding is committed. Revoking and re-enrolling the same key still requires the newly issued Runtime trust ID.

同じ Runtime identity は異なる Workspace から独立して信頼できる。trust record、replay protection、binding、失効はすべて Workspace scope で評価する。

## Runtime auth file

`runtime-auth.toml` は Runtime identity と Workspace issuer records のみを authority とする。
旧 Server trust entry は読み飛ばされ、以後の identity / `trust-workspace` 更新時に書き戻されない。旧 entry を残しても認証には使用されない。

`trust-workspace` の file store は次を fail closed で検証する。

- 最大 8 MiB
- 最大 4,096 records
- exact Workspace / Runtime identity
- `key_id` と public key fingerprint、opaque `trust_id`
- normalized Backend URL
- 同じ `key_id` に異なる鍵materialを割り当てないこと。実際の置換・失効は新しい `trust_id` を発行する
- list は `offset` / `limit` 必須で、1 page 最大 100 records

## Local token

`--local-token` は明示的な local Runtime 呼び出し専用であり、Remote Workspace binding の代替ではない。Workspace issuer auth が有効な Remote Runtime request は Workspace capability token を使う。

## Rotation と失効

Workspace signing key または Runtime key の変更は、現在 binding を置き換える明示的な provisioning 操作として行う。古い key/trust ID、古い Runtime key、revoked binding、失効済み token、replayed JTI は即時拒否する。

Server の Runtime cache は現在の persisted binding 全体と照合する。endpoint、Runtime public key/fingerprint、`binding_id`、Workspace `key_id`、`workspace_trust_id` の変更を検知した場合、stale client を利用しない。

## 運用確認

Remote Runtime を有効化した後は次を確認する。

1. `yoi-runtime trust-workspace show --workspace-id <workspace-id>` が期待する bundle を表示する。
2. Workspace Settings の Runtime binding が `verified` で、現在の `binding_id`・`workspace_key_id`・`workspace_trust_id` と verification evidence を表示する。
3. Runtime ping、Worker list/create、`worker.protocol` subscription が Workspace-signed token で成功する。
4. wrong Workspace、wrong Runtime、wrong target/body、expired token、revoked/replaced binding、replayed JTI が拒否される。
5. Runtime → Server source proof が configured Backend public URL audience と一致し、spoofed headers だけでは認証されない。

Server / Runtime の再起動は live reload ではない authority 変更を反映するときだけ、通常の運用権限と migration gate に従って行う。実行中プロセスを開発 Worker が無断で停止してはならない。
