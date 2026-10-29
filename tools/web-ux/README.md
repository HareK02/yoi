# Web UI検証支援

`tools/web-ux`は、実際のWeb画面とブラウザの診断を同じ条件で採取する開発用ツールである。製品のSkill・FlowやRuntime権限は追加しない。合否は[目視確認の規則](../../docs/development/ui-ux/visual-review.md)に従って実装者が判断する。

入口は[`cli.ts`](cli.ts)、採取条件は[`scenarios/`](scenarios/)、保存証拠の構造は[`src/types.ts`](src/types.ts)に置く。撮影・認証fixture・プロセス管理の理由は対応する`src/`のコメントで管理する。

## 維持規則

- scenarioは利用者の具体的な判断とデータ状態を定める。権限差・幅・themeは変更の契約を確認する範囲で選ぶ。
- readinessは画面または応答で確認し、固定sleepで推測しない。本番と異なるCSSやshellを撮影用に作らない。
- 認証状態と成果物を分離し、scenario・引数・成果物に秘密の値を含めない。
- PlaywrightとNixで固定するbrowserの対応を、toolchain変更として同時に検証する。
- 変更時は`deno.json`のcheck・testと必要なbrowser testを選ぶ。scenario変更では実際の画像と`review-context.json`を確認する。browser testの成功を目視の代わりにしない。

Subject権限の操作確認には[`subject_permissions_review.ts`](browser-tests/subject_permissions_review.ts)、変更前の撮影には[`subject_permissions_before.ts`](browser-tests/subject_permissions_before.ts)を使う。実Webのビルドを合成HTTP fixtureで動かすため、採取結果は実Backendの認証・保存・認可・Worker制御の証拠ではない。画像と診断の目視確認も別途必要である。
