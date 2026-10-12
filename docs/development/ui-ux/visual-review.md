# Web UI変更の目視確認

描画結果が変わる変更は、実装者自身が変更前後を目視する。type check、linter、testと別の検証であり、後段Reviewerや利用者に最初の発見を委ねない。画像生成やHTTP成功だけでは確認済みにならない。

## 対象と条件

page、layout、navigation、componentのmarkup・style、色・書体・余白、文言・状態表示、可変幅、keyboard操作、表示に影響するAPI projectionを変えた場合に行う。描画に影響しない文書・型・Backend変更だけなら省略できるが、影響しない理由を検証記録に残す。

変更箇所に関係する条件を選び、都合のよい一画面だけで済ませない。

- 権限やデータが表示を変える場合は、owner、non-owner、anonymous、read-onlyなど該当する利用主体を分ける。
- 幅はdesktop `1440px`、狭い幅`768px`、mobile `390px`を基準とする。`320px`対応やbreakpoint付近が変更対象なら追加し、Sidebarなどを除いた実内容幅も確認する。
- 色・背景・線・focus・状態・code・terminalを変える場合はlightとdarkを確認する。
- 通常、空、読込み中、失敗、権限不足、長文・長い識別子、少数・多数の一覧など、該当するデータ状態を用意する。

## 実施と確認項目

1. route、利用主体、幅、theme、データ状態、撮影点を固定する。既存scenarioを再利用し、不足分は同じ変更で更新する。
2. 変更前を撮影して画像を開き、既存問題と変更対象を把握する。
3. [Design Language](../../principle/design-language.md)と[UX設計](../../principle/ux-design.md)に沿って実装する。撮影専用のCSS・shellや、本番と異なる権限判定で見栄えを整えない。
4. 同条件で変更後を撮影し、実装者自身が画像と`review-context.json`を確認する。Agentも`ViewImage`などで実画像を見る。
5. 発見した新規問題を修正し、再撮影して確認する。既存の無関係な問題は根拠を付けて分離する。

画像では、情報のまとまり、区切り線の役割、余白の大小、重複見出し、文字の強弱、折返し、欠落、overflow、主要操作の位置を確認する。DOM・表示・keyboardの順、focusの維持、色以外の状態表現、必須情報への到達も確認する。画素差の大小だけで合否を決めない。

`review-context.json`では、document status、可視エラー、console・page・requestエラー、accessibility snapshot、撮影点と画像hashを確認する。HTTP 200でも画面が権限エラーなら成功ではない。意図的な障害fixtureは予期したエラーと予期しないエラーを分けて判定する。

## 証拠と合否

引継ぎにはscenario、変更前後のartifact、source、確認した条件、修正した問題、根拠付きの既存問題、最終の`pass | fail`を残す。必要な条件を自分で確認し、新規問題を修正し、意図しない描画回帰がない場合だけ合格とする。自動テストと目視確認のどちらも、他方の代替にはしない。

成果物は`target/web-ux`などの開発artifact領域へ置き、画像を大量にGitやTicketへ保存しない。認証profile、cookie、token、秘密情報、非公開response bodyを含めない。確認支援ツールは[tools/web-ux](../../../tools/web-ux/README.md)が所有し、合否判断は実装者が行う。
