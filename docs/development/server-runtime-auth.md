# Workspace ↔ Runtime 認証

Yoi の Remote Runtime 認証は Workspace ごとの署名 identity を authority とする。
認証の成否は実際の HTTP / WebSocket request ごとに決まり、保存済みの「verified」状態や過去の接続テスト結果を前提にしない。
Server-global な署名鍵や Runtime 側の trusted-Server catalog は使わない。

## Authority

- Server DB は Workspace ごとの signing identity と Runtime 接続設定（binding）を保持する。
- binding は endpoint、固定した Runtime public key/fingerprint、`binding_id`、authentication mode、管理者による失効を保持する。Workspace key や Runtime 側の trust ID のコピー、認証成功状態は保持しない。
- Runtime は管理者が `trust-workspace` で受理した `WorkspacePublicIdentityBundle` を保持する。
- bundle は `workspace_id`、Workspace `key_id`、Workspace public key/fingerprint、Backend URL を固定する。
- Server → Runtime の各 HTTP / WebSocket request は、その時点の Workspace signing identity で短命な capability token を発行する。署名時には claims の Workspace・key ID・fingerprint が選択した identity と一致することを検証する。秘密鍵materialがない場合は fallback せず拒否する。
- Runtime は現在の明示的な Workspace trust、署名、request method、`path_and_query`、body digest、permission、Workspace、Runtime、必要に応じ Worker、key ID/fingerprint、expiry、JTI を検証する。過去の handshake や verification record は不要である。
- Runtime → Server の source proof は Runtime identity で署名し、対象 Workspace と bundle の Backend URL を audience に固定する。この proof の契約は変わらない。
- Server は現在の Workspace Runtime binding、Runtime public key、Backend public URL、request target、body digest、permission、expiry、replay state を検証する。

旧 Server identity/trust 管理 command と旧 Runtime-side Server trust command、旧 Runtime auth key flags は廃止済みである。これらに相当する Server-global trust を fallback として使ってはならない。

## Provisioning

Runtime 側の Workspace trust 登録と Server 側の Runtime 接続設定は独立した管理操作である。Runtime が発行する `trust_id` を Server にコピーする必要はない。

1. Runtime identity を初期化する。

   ```sh
   yoi-runtime identity init --runtime-id <runtime-id>
   yoi-runtime identity show
   ```

2. Workspace owner が Workspace identity settings から現在の Workspace issuer public bundle を export する。
3. Runtime 管理者が bundle を確認し、Runtime に明示的に登録する。

   ```sh
   yoi-runtime trust-workspace add --bundle <workspace-issuer-bundle.json>
   yoi-runtime trust-workspace show --workspace-id <workspace-id>
   ```

4. Workspace owner が Settings > Runtimes に Runtime public bundle と endpoint を登録する。接続設定には Runtime key を固定するが、Workspace key や Runtime の trust ID は登録しない。
5. 接続テストまたは通常操作を実行する。実際の Workspace-signed request が Runtime の現在の trust と照合される。接続テスト結果はその試行の診断であり、永続的な認証状態を作らず、以後の request の成功を保証しない。

同じ Runtime identity は異なる Workspace から独立して信頼できる。trust record、replay protection、binding、失効はすべて Workspace scope で評価する。

## Runtime auth file

`runtime-auth.toml` は Runtime identity と Workspace issuer records のみを authority とする。
旧 Server trust entry は読み飛ばされ、以後の identity / `trust-workspace` 更新時に書き戻されない。旧 entry を残しても認証には使用されない。

`trust-workspace` の file store は次を fail closed で検証する。

- 最大 8 MiB
- 最大 4,096 records
- exact Workspace / Runtime identity
- `key_id` と public key fingerprint、管理 record の opaque `trust_id`
- normalized Backend URL
- 同じ `key_id` に異なる鍵materialを割り当てないこと。実際の置換・失効は新しい `trust_id` を発行するが、この ID は request token や Server binding に含めない
- list は `offset` / `limit` 必須で、1 page 最大 100 records

## Local token

`--local-token` は明示的な local Runtime 呼び出し専用であり、Remote Workspace binding の代替ではない。Workspace issuer auth が有効な Remote Runtime request は Workspace capability token を使う。

## Key 変更と失効

Workspace signing identity は各 request の発行時に現在の authority から選択する。Workspace public key が変更された場合、Runtime 管理者が現在の public bundle を明示的に `trust-workspace replace` で受理する必要がある。Workspace key のコピーを更新するためだけに Server の Runtime 接続設定を置き換える操作は不要である。これは Workspace key rotation の新しい production API を定義するものではない。

Runtime 管理者は `trust-workspace revoke` で Workspace trust を失効できる。Runtime は現在の trust と一致しない key、失効済み trust、expired token、replayed JTI を拒否する。同じ key の再登録も管理者の明示的な操作で行い、request の自己申告による trust 登録は行わない。

Runtime key や endpoint の変更は Server の接続設定を明示的に置き換える。Server の Runtime cache と authorizer は現在の接続設定（endpoint、Runtime public key/fingerprint、`binding_id`、authentication mode、管理者による失効）と照合し、replaced/revoked binding を使う stale client を拒否する。接続設定の一致は送信先の選択であって、認証成功の証拠ではない。

接続先が到達不能・認証失敗でも、owner は観測した `binding_id` を指定して接続設定を修復できる。取得できた active Worker は置換を遮断するが、Worker 一覧の取得失敗を「idle の証明」とは扱わず、認証成功を修復の前提にもしない。接続設定の置換は Worker の停止・削除ではなく、Runtime 削除の資源確認とは別の操作である。

## 保存形式の移行

Server schema90 は既存 binding の endpoint、Runtime key、binding ID、明示的失効を保持し、古い認証状態と検証記録を履歴専用 archive に移す。履歴は要求の認可には参照しない。既存 Workspace identity や接続設定の作り直しは不要である。

Runtime の旧形式の Workspace trust は、検証済みの公開鍵と管理者の Active/Revoked 設定を保持して読み込む。形式の移行だけでは失効させず、すでに保存されている Revoked 設定を自動で解除することもしない。旧 verification file は認可に使用しない。消費済み token の replay 記録は引き継ぎ、同じ鍵の再登録でリセットしない。

API と capability claims の旧 handshake 契約は維持しないため、Backend・Runtime・Web は対応する版へ更新する。接続テストは更新後の実認証を確認するものであり、登録を有効化する手順ではない。

## 運用確認

Remote Runtime を設定した後は次を確認する。

1. `yoi-runtime trust-workspace show --workspace-id <workspace-id>` が期待する active bundle を表示する。
2. Workspace Settings の Runtime 接続設定が期待する endpoint と固定 Runtime key を示し、失効していない。
3. 接続テストの今回の結果を確認し、Runtime ping、Worker list/create、`worker.protocol` subscription が実際の Workspace-signed request で成功することを確認する。保存済み「verified」表示や verification evidence は確認項目ではない。
4. wrong Workspace、wrong Runtime、wrong target/body、expired token、revoked Runtime trust、replaced/revoked binding、replayed JTI が拒否される。
5. Runtime → Server source proof が configured Backend public URL audience と一致し、spoofed headers だけでは認証されない。

Server / Runtime の再起動は live reload ではない authority 変更を反映するときだけ、通常の運用権限と migration gate に従って行う。実行中プロセスを開発 Worker が無断で停止してはならない。
