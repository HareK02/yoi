# Drive実API roundtrip

Rustの `drive_web_roundtrip_tests` が起動する隔離loopback Serverへ、`roundtrip.ts` がproduction Web adapterで接続する。実SQLite、FeatureStorage、LocalFileSystemを使い、認可、保存、receipt、競合、grant取消しを検証する。

認証情報とWorkspace identityは親harnessからstdinで受け取り、argvやログには出さない。実ユーザーデータと稼働中Serverは使わない。Denoプロセスは接続先を指定loopback originへ限定する。

Runtime spawn境界はfixtureであり、Passkey／cookie loginや画面操作のE2Eではない。画面のdraft保持、キャンセル、視覚面は [Browser fixture](../drive-browser/README.md) が担当する。親Rust harnessとproduction adapterの両方を維持し、通信応答をmockへ置き換えない。
