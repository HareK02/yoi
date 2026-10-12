# 生成されたWeb契約

このディレクトリのAPI型とvalidatorはRust契約からの生成物であり、手で編集しない。生成元とgeneratorは各ファイルのheaderに記録する。

- `repository-api.ts`: canonical OpenAPIから生成するRepository契約。
- `protocol.ts` / `protocol-validator.ts`: `protocol::subscription::SubscriptionFrame`から一緒に生成する型と受信検査。
- `legacy-server-api.ts`: 未分離の契約。新しいdomainを追加するときは、対象を限定した生成契約へ分ける。

契約変更時はRustの生成元と出力を同じ変更で更新し、generatorの検査と互換fixtureで一致を確認する。Browser固有の受信処理は `../workspace/multiplexer.ts` が所有する。
