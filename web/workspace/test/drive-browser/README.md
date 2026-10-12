# Drive Browser fixture

production static shellをloopbackのin-memory API fixtureに接続し、描画、操作、遅延応答、draft保持を検証する。Backend／DB／blob storageの認可を証明するテストではない。

- `fixture-api.test.ts`: fixtureとWeb parserの契約一致。
- `drive.browser.test.ts`: production画面の操作と要求の整合。
- `responsive.browser.test.ts` / `capture-visual.ts`: responsive検査と画像・診断の保存。

fixtureは実Workspaceや資格情報を使わず、所有subprocessを片付ける。テストは自動buildを行わない。検証対象のstatic生成物を明示し、画像と診断を確認してから視覚面の合否を判断する。実HTTP／storageの保証は [別harness](../drive-real-api/README.md) で検証する。
