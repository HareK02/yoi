<!-- event: create author: "yoi ticket" at: 2026-07-16T03:19:36Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-16T03:19:50Z -->

## Plan

背景:
Memory extract が raw tool/result slice から意味を復元しようとすると、目的・判断・現在地が抜けて断片的な抽出になりやすい。専用 Tool を増やすと tool surface が増え、注意分散と token 重複が増えるため、ユーザーに見える通常 Assistant Message を progress / checkpoint として活用し、人間にも機械にも追いやすい transcript を作る。

要件:
- 長い作業・tool loop・調査/実装 phase の節目で、main Worker が短い Progress message を通常 Assistant Message として出すように prompt / guidance を追加または変更する。
- Progress message は user-facing transcript の一部として成立する内容に限定する。raw reasoning や迷いの垂れ流しではなく、目的、確認済み事実、判断、未解決点、次の作業を簡潔に書く。
- 専用 Tool は追加しない。Progress message はユーザーへの進捗報告と、後段 extract 用 Overview の semantic backbone を兼ねる。
- tool call ごとではなく、意味のある phase boundary / long-running interval / 重要な確認結果の節目で出す。
- Memory extract 側は将来的に user messages + assistant messages を Overview として優先し、tool logs は evidence として参照できる設計に接続する。

受け入れ条件:
- relevant prompt/resource の指示に、turn 中の Progress message を残す方針が明記されている。
- Progress message の内容制約が明記されている: public に見せられる短い作業状態、確認済み事実、判断、未解決点、次の作業。chain-of-thought や冗長な tool detail は含めない。
- 専用 Tool を増やさず、通常 Assistant Message として記録する方針になっている。
- 既存の最終応答と過剰に重複しない頻度・粒度の guidance がある。
- prompt/resource 変更として `nix build .#yoi` で packaging を検証する。


---

<!-- event: intake_summary author: hare at: 2026-07-16T03:19:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-16T03:19:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: hare at: 2026-07-16T16:48:26Z from: ready to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-16T16:48:26Z status: closed -->

## 完了

Progress message guidance was added to `resources/prompts/common/writing.md`.

The prompt now instructs long-running work to emit short ordinary user-visible prose progress updates at meaningful boundaries, without tool calls, hidden notes, chain-of-thought, raw reasoning, secrets, or verbose tool-output details.

Validation: `nix build .#yoi` passed.


---
