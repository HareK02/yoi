---
title: "ネイティブGUIアプリケーション"
state: "active"
created_at: "2026-06-10T07:41:18Z"
updated_at: "2026-06-10T07:41:18Z"
linked_tickets: []
---

## Goal

Yoi の Pod / Ticket / Orchestrator / workflow 操作を、TUI だけでなくネイティブ GUI から扱えるようにする。

最初の到達点は、既存の runtime / Ticket backend / Pod protocol / Profile / workflow authority を再実装せずに、workspace の状態を視覚的に把握し、選択した Pod・Ticket・role action に対して安全に操作できる desktop GUI client を持つこと。GUI は core authority ではなく client surface とし、既存 CLI/TUI と同じ durable state・同じ protocol・同じ permission/prompt/workflow 境界を使う。

## Motivation / background

現在の TUI / Panel は dogfooding 可能な状態まで進んでいるが、複数 Pod・複数 Ticket・Orchestrator 状態・session output・role launch・review/merge dossier を同時に扱うには、terminal UI の表示密度・視覚的状態表現・スクロール/選択/比較操作に限界が出ている。

ネイティブ GUI があると、以下をより自然に扱える。

- live / stored Pod、Ticket lane、Orchestrator progress、Companion/Intake 状態の同時表示。
- Ticket body/thread/artifacts、Pod output、validation evidence、diff/report の並列閲覧。
- composer target、role action、queue/routing/attach/restore の明確な affordance。
- long-running orchestration の通知、状態変化、失敗診断の視覚化。
- 将来的な review / merge-ready dossier / plan board / settings editor の専用 UI。

一方で、GUI を理由に runtime authority を分散させたり、Ticket/Pod state を独自 DB として二重管理したり、prompt/workflow 文字列を GUI code に直書きしたりしてはいけない。

## Strategy / design direction

- GUI は Yoi core の上に乗る client として作る。
  - Pod lifecycle、session/history、Ticket storage、Profile resolution、workflow/prompt authority は既存 core を正とする。
  - GUI 固有 state は selection、layout、local UI preference などに限定する。
- 最初に toolkit / architecture の小さな spike を置く。
  - 評価軸は Rust code reuse、async/runtime 統合、native packaging、Linux dogfooding しやすさ、testability、accessibility、long-running log/output 表示、将来の cross-platform 余地。
  - toolkit 選定は Objective では固定しない。候補比較と採用理由を Ticket に残す。
- 実装は段階的に進める。
  1. read-only workspace dashboard: Pod / Ticket / Orchestrator 状態を既存 backend から表示する。
  2. attach / restore / open など、既存 protocol に乗る低リスク操作を追加する。
  3. composer と role action: Companion / Intake / Orchestrator / coder / reviewer launch を既存 launcher 経由で扱う。
  4. Ticket body/thread/artifacts、Pod output、validation evidence、review report を閲覧しやすくする。
  5. 必要に応じて settings/profile/config editor や merge-ready dossier UI を追加する。
- TUI は廃止前提にしない。
  - GUI 導入後も CLI/TUI は fallback / automation / terminal-first workflow として維持する。
  - GUI で見つかった state model の改善は、TUI と共有できる pure data model / client API に寄せる。
- Prompt / workflow / role guidance は GUI code に直書きしない。
  - LLM-facing prompt は `resources/prompts` または `.yoi/workflow` / configured resources を正とする。
  - GUI は prompt 文言を所有せず、選択・起動・runtime context の入力面を担当する。
- Security / privacy / authority boundary を保つ。
  - secret-like data は UI diagnostics / logs / model context に漏らさない。
  - permission / scope / profile の authority は既存 resolver/policy に従う。
  - GUI convenience action は durable Ticket/Pod state transition と対応付け、暗黙の side effect を避ける。

## Success criteria / exit conditions

- 明示コマンドまたは binary で native GUI を起動できる。
- GUI は既存 workspace config、Profile、Ticket backend、Pod registry/protocol を使い、独自の authority store を持たない。
- 最小 dashboard で live/stored Pod、Ticket lane/state、Orchestrator/role session の概況を確認できる。
- GUI から少なくとも attach/restore/open 相当の安全な Pod 操作ができる。
- GUI から Ticket Intake または既存 role launcher を使った role action を実行でき、既存の prompt/workflow/resource 境界を壊さない。
- Pod output / Ticket body/thread/artifacts を、TUI より見通しよく閲覧できる最小 UI がある。
- GUI 固有 state と core durable state の境界が文書化されている。
- toolkit / architecture 選定理由、採用しなかった選択肢、packaging 方針が Ticket artifact または design note として残っている。
- GUI で使う state transformation / action eligibility は pure model として test 可能で、主要な selection/action state の unit test がある。
- GUI 実装は CLI/TUI の既存 workflow を破壊せず、必要な targeted validation が定義されている。

## Decision context

- この Objective は中長期の方向性・判断軸を保持する。具体的な toolkit 選定、crate 構成、初期 dashboard 実装、role action 実装、packaging は個別 Ticket に分ける。
- GUI は TUI の単純な置換ではなく、複数 Pod / Ticket / Orchestrator を扱う workspace cockpit として設計する。
- authority は既存 core に残す。GUI は client/view/controller surface であり、Pod/Ticket/workflow/prompt の正本を所有しない。
- Prompt 直書き禁止方針を守る。GUI 実装中に LLM-facing 文言が必要になった場合は、`resources/prompts` または workflow/resource 側に置く。
- 初期 target は dogfooding しやすい desktop GUI とし、public release / cross-platform polish / installer は後続段階で扱う。
