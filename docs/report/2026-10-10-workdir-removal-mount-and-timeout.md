# Workdir 削除のマウント残留と結果不明の診断

## 調査範囲

ユーザーの「workdirを消せない原因を調査して」という依頼に対し、保存済み Server DB を
`mode=ro` / `PRAGMA query_only=ON` で照合し、対象ディレクトリ・マウント・現行実装を
観察した。削除再実行、アンマウント、DB 更新、サービス/プロセス操作、実装修正は行っていない。
調査中にも operation の記録が更新されたため、以下は最後に読んだ状態であり、過去の全試行に
同じ原因があったと断定するものではない。

Workspace: `0197a949-4b6b-7f2a-9d9a-1f87e3a4c5b6`、Runtime: `arcadia`。
表示に含まれる次の2つの Workdir ID は別の対象である。

## T-721: 残留した JuiceFS マウントによる拒否（確認済み）

- Workdir: `001a11ab3e830000007` / `T-721 JuiceFS Drive foundation`
- operation: `wdr_1c0840acc66f27b7e048bdfff8f80b76`
- 最後に読んだ attempt: `01a12682-3b33-7e43-b4c6-4e57eb469912`
- 更新時刻: `2026-10-10T15:50:31.515343065+00:00`
- 結果: `failed` / `mount_present` / `retryable=1`

`findmnt` で、次の2つが `JuiceFS:yoi-t721` / `fuse.juicefs` のマウントとして残っていることを
確認した（パスは `yoi.local` 以下の論理パス）。

```text
workdirs/001a11ab3e830000007/checkout/.yoi/dev/juicefs-t721/mount
workdirs/001a11ab3e830000007/checkout/.yoi/dev/juicefs-t721/mount2
```

両エントリの属性参照は `Transport endpoint is not connected` で失敗した。
`ps -C juicefs` に現在のプロセスはなかった。マウント解除されていない FUSE の残留状態と
整合するが、プロセスが終了した経緯までは今回確認していない。

Registry の cleanliness は `clean` だが、これはマウントを安全に削除できる証明ではない。
実ディレクトリの `materialization.json` は `cleanup_pending`。
現行 `crates/worker-runtime/src/working_directory/cleanup.rs` は、削除前にマウント境界を
検査し、外部/未知のマウントを自動解除しない。Server は
`working_directory_cleanup_mount_present` を `mount_present` として扱う。
これは安全性を維持する拒否であり、cleanliness 判定の不具合とは異なる。

解消には、所有者が保存先と利用状況を確認して両マウントを安全に解除することが必要。
その後、マウントがないことを確認して通常の Workdir 削除を再実行する。
マウント先を含む `rm -rf`、強制的なメタデータ改変、未知のプロセス停止は解消手段にしない。
マウント解除後も ignored な実験データ等の保護により別の拒否が起こる可能性があり、今回の
確認は次回の削除成功を保証しない。

## 別 ID: HTTP タイムアウトと物理削除の時間差が疑われる（未確定）

- Workdir: `001a111ca9d33000002`
- operation: `wdr_fccf7c6e9ee6cbb931ea2f9b82198650`
- attempt: `01a12682-411e-7720-b829-f9c9c0148886`
- created: `2026-10-10T15:50:32.984845992+00:00`
- updated: `2026-10-10T15:50:43.018819445+00:00`
- 結果: `failed` / `provider_cleanup_failed` / `retryable=1`

要求から失敗まで約10.034秒。Registry には `corrupted` / cleanliness `unknown` として
残っている一方、調査時点で `yoi.local/workdirs/001a111ca9d33000002` は存在しない。
`.cleanup-authority` にも残存ファイルはなかった。

現行実装では `RemoteRuntimeConfig::new` の timeout は10秒で、DELETE はその共通
blocking HTTP client を使う。Runtime の HTTP handler は `spawn_blocking` で同期的な
検査/削除の完了を待つ。クライアントのタイムアウトは、この blocking 削除処理の取り消しや
物理削除の失敗を意味しない。

このため「Server が10秒で応答待ちを打ち切った後、Runtime の物理削除が完了した」という
説明と整合する。ただし、元の `remote_runtime_timeout` 等の診断を今回の観察面で取得できず、
別の操作でディレクトリがなくなった可能性も排除できない。物理パスがないだけで Backend の
削除完了を捏造してはいけない。通常の再試行で Runtime の厳密な not-found 応答を確認できれば、
現行 Server は registry 削除を完了する経路を持つ。今回、その再試行は行っていない。

## 診断面の障壁と改善案

`crates/workspace-server/src/server.rs` の `workdir_cleanup_failure_category` は、既知の
cleanup code を分類し、それ以外を `provider_cleanup_failed` に落とす。
`crates/workspace-server/src/hosts.rs` は transport timeout を `remote_runtime_timeout`
に区別するが、上記分類では失敗結果不明との区別が失われる。

また removal operation は bounded category のみを永続化し、元の diagnostic code を残さない。
これは秘密情報を含み得る message を保存しない方針とは両立するが、現行の失敗ログも
Workdir/operation/runtime/category のみで、ユーザー案内の「相関 Runtime diagnostics を調べる」
を保存済み情報だけでは完遂できなかった。`runtime.json` の diagnostics にも対象 ID / operation
を含む記録は見つからなかった。

改善案は、秘密を含む raw message を公開・永続化せず、transport timeout を「削除結果不明」と
して区別し、許可された診断コードと operation/attempt/Workdir の相関を残すこと。
さらに、完了まで時間のかかる guarded cleanup を通常の10秒 control request と同じ同期要求で
扱う契約を見直すこと。単にタイムアウトを延長するだけでは結果不明の状態はなくならない。
今回は調査報告のみで、これらの実装変更はしていない。
