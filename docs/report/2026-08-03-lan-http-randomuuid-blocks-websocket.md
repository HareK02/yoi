# LAN の HTTP アクセスで `crypto.randomUUID()` が WebSocket 接続開始を阻害する

## 概要

Workspace WebUI の Vite dev server を `--host` 付きで LAN に公開し、別端末から
`http://192.168.1.32:5173` にアクセスすると、Worker Console の protocol 状態が
`connecting` のまま進まなかった。

原因は WebSocket や Vite proxy ではなく、WebSocket multiplexing の相関 ID 生成に
`crypto.randomUUID()` を直接使っていることである。`http://localhost` はブラウザから
potentially trustworthy origin として扱われる一方、LAN IP 上の平文 HTTP は secure
context ではない。`crypto.randomUUID()` は secure context 限定なので、LAN アクセスでは
接続処理が WebSocket の生成前に例外終了する。

## 発生経路

Worker Console の `connectProtocolTransport` は、最初に `protocolState` を
`"connecting"` に設定してから `WorkspaceMultiplexer.subscribe()` を呼ぶ。

- `web/workspace/src/routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte`
  - `protocolState = "connecting"`
  - 直後に `workspaceMultiplexer(...).subscribe(...)`
- `web/workspace/src/lib/workspace/multiplexer.ts`
  - `subscribe()` の先頭で `const clientId = crypto.randomUUID()`
  - その後の `#ensureConnected()` で初めて `new WebSocket(url)` を実行する

このため `crypto.randomUUID()` が利用できないブラウザでは、画面状態だけが
`connecting` に更新された後に同期例外が発生し、WebSocket request 自体が送信されない。
接続失敗を示す `closed` や diagnostic にも遷移しないため、見た目からは WebSocket の
接続待ちに見える。

同じ multiplexer では `crypto.randomUUID()` を次の3用途に使っている。

1. ブラウザ内で subscription を区別する `clientId`
2. `subscribe_events` request と response を対応付ける `request_id`
3. `unsubscribe_events` の `request_id`

いずれも暗号化や認証のためではなく、ローカル Map のキーまたはプロトコル上の相関 ID
である。secure-context-only API を要求する必要性はない。

## 切り分け結果

開発ホスト上から次の条件で `/api/w/<workspace-id>/protocol/ws` に WebSocket upgrade を
送ると、backend への直接接続と Vite の `5173` proxy 経由の両方で
`101 Switching Protocols` が返った。

- `Host: 192.168.1.32:5173`
- `Origin: http://192.168.1.32:5173`

したがって、現在の Vite 設定にある `proxy["/api"].ws = true` と、loopback に bind した
backend への proxy はこの現象の直接原因ではない。

## 改善案

- UI 全体で使う ID 生成 helper を用意し、secure context に依存しない実装にする。
  `crypto.getRandomValues()` から UUID v4 相当を生成する方法で十分である。
- `WorkspaceMultiplexer.subscribe()` の同期初期化失敗を Console の diagnostic/state に
  反映し、初期値の `connecting` に留まらないようにする。
- LAN 上の平文 HTTP を開発時の対応経路とするなら、insecure context から subscription
  初期化できることを regression test として固定する。
- 本番相当の LAN 公開では HTTPS を使う。Passkey/WebAuthn も secure context を要求し、
  現在の開発設定は `rp_id = localhost` なので、LAN origin を正式対応する場合は auth の
  origin/RP ID 設計も別途必要になる。

## 補足

`--host` は Vite の listener を LAN に bind するだけであり、配信 origin を secure
context に変えるものではない。localhost で正常に動くことだけでは、LAN IP の HTTP
アクセスでも同じブラウザ API が利用できることを証明できない。
