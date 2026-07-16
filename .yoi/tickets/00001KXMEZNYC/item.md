---
title: 'ターン中のProgress messageを残す指示を追加する'
state: 'closed'
created_at: '2026-07-16T03:19:36Z'
updated_at: '2026-07-16T16:48:26Z'
assignee: null
---

## 背景

Memory extract が raw tool/result slice から意味を復元しようとすると、目的・判断・現在地が抜けて断片的な抽出になりやすい。専用 Tool を増やすと tool surface が増え、注意分散と token 重複も増える。

長い作業の節目で、main Worker がユーザーに見える通常 Assistant Message として短い Progress message を残せれば、人間への進捗報告と後段 extract 用 Overview の semantic backbone を兼ねられる。

## 要件

- 長い作業、tool loop、調査/実装 phase の節目で、main Worker が短い Progress message を通常 Assistant Message として出すように prompt / guidance を追加または変更する。
- Progress message は user-facing transcript の一部として成立する内容に限定する。
- raw reasoning や迷いの垂れ流しではなく、目的、確認済み事実、判断、未解決点、次の作業を簡潔に書く。
- 専用 Tool は追加しない。
- tool call ごとではなく、意味のある phase boundary / long-running interval / 重要な確認結果の節目で出す。
- Memory extract 側は将来的に user messages + assistant messages を Overview として優先し、tool logs は evidence として参照できる設計に接続する。

## 受け入れ条件

- relevant prompt/resource の指示に、turn 中の Progress message を残す方針が明記されている。
- Progress message の内容制約が明記されている: public に見せられる短い作業状態、確認済み事実、判断、未解決点、次の作業。chain-of-thought や冗長な tool detail は含めない。
- 専用 Tool を増やさず、通常 Assistant Message として記録する方針になっている。
- 既存の最終応答と過剰に重複しない頻度・粒度の guidance がある。
- prompt/resource 変更として `nix build .#yoi` で packaging を検証する。
